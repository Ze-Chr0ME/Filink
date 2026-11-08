//! Bounds pre-authentication work and closes slow handshakes at an absolute deadline.
use super::*;
use std::sync::{atomic::AtomicUsize, mpsc};

pub(super) const DEADLINE: Duration = Duration::from_secs(10);
const LIMIT: usize = 8;

pub(super) struct Permit(Arc<AtomicUsize>);
impl Permit {
    pub(super) fn acquire(count: &Arc<AtomicUsize>) -> Option<Self> {
        count
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < LIMIT).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(count.clone()))
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) struct Handshake {
    stop: Option<mpsc::Sender<()>>,
    watchdog: Option<thread::JoinHandle<()>>,
    deadline: Instant,
    permit: Option<Permit>,
}
impl Handshake {
    pub(super) fn new(socket: &TcpStream, permit: Permit, timeout: Duration) -> Result<Self> {
        let deadline = Instant::now() + timeout;
        let socket = socket.try_clone()?;
        let (stop, receiver) = mpsc::channel();
        let watchdog = thread::Builder::new()
            .name("handshake deadline".into())
            .spawn(move || {
                if matches!(
                    receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let _ = socket.shutdown(Shutdown::Both);
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            watchdog: Some(watchdog),
            deadline,
            permit: Some(permit),
        })
    }
    // PAIR_REQ stops the network deadline, but retains its slot throughout the user wait.
    pub(super) fn stop_deadline(&mut self) -> Result<()> {
        self.stop_watchdog();
        ensure!(
            Instant::now() < self.deadline,
            "handshake deadline exceeded"
        );
        Ok(())
    }
    pub(super) fn authenticated(&mut self) -> Result<()> {
        self.stop_deadline()?;
        self.permit.take();
        Ok(())
    }
    fn stop_watchdog(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(watchdog) = self.watchdog.take() {
            let _ = watchdog.join();
        }
    }
}
impl Drop for Handshake {
    fn drop(&mut self) {
        self.stop_watchdog();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn bounds_handlers_and_releases_slots() {
        let count = Arc::new(AtomicUsize::new(0));
        let permits: Vec<_> = (0..LIMIT)
            .map(|_| Permit::acquire(&count).unwrap())
            .collect();
        assert!(Permit::acquire(&count).is_none());
        drop(permits);
        assert!(Permit::acquire(&count).is_some());
    }

    #[test]
    fn deadline_closes_trickling_connection() -> Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let mut client = TcpStream::connect(listener.local_addr()?)?;
        let (mut server, _) = listener.accept()?;
        prepare(&server)?;
        let permit = Permit::acquire(&Arc::new(AtomicUsize::new(0))).unwrap();
        let timeout = Duration::from_millis(250);
        let guard = Handshake::new(&server, permit, timeout)?;
        let start = Instant::now();
        let trickle = thread::spawn(move || {
            // A valid header with a long, incomplete payload defeats per-read timeouts.
            if client.write_all(&[HELLO, 0, 0, 4, 0]).is_err() {
                return;
            }
            loop {
                if client.write_all(b" ").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        assert!(read_frame(&mut server).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        drop(guard);
        trickle.join().unwrap();
        Ok(())
    }

    #[test]
    fn pairing_stops_deadline_but_retains_slot() -> Result<()> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let mut client = TcpStream::connect(listener.local_addr()?)?;
        let (mut server, _) = listener.accept()?;
        server.set_read_timeout(Some(Duration::from_secs(1)))?;
        let count = Arc::new(AtomicUsize::new(0));
        let mut guard = Handshake::new(
            &server,
            Permit::acquire(&count).unwrap(),
            Duration::from_millis(100),
        )?;
        guard.stop_deadline()?;
        thread::sleep(Duration::from_millis(150));
        client.write_all(b"x")?;
        let mut byte = [0];
        server.read_exact(&mut byte)?;
        assert_eq!(byte, *b"x");
        assert_eq!(count.load(Ordering::Acquire), 1);
        drop(guard);
        assert_eq!(count.load(Ordering::Acquire), 0);
        Ok(())
    }
}
