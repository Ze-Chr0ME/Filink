use super::*;
use std::io::{BufReader, BufWriter, Read, Write};
// Short OS waits let Windows cancellation interrupt a pending read. Each IO
// operation still has the protocol's full 30-second inactivity deadline.
pub(super) struct TransferSocket {
    pub(super) socket: TcpStream,
    core: Core,
    id: u64,
}
impl TransferSocket {
    pub(super) fn new(socket: TcpStream, core: &Core, id: u64) -> Result<Self> {
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        socket.set_write_timeout(Some(Duration::from_millis(100)))?;
        Ok(Self {
            socket,
            core: core.clone(),
            id,
        })
    }
    fn io<T>(
        &mut self,
        mut op: impl FnMut(&mut TcpStream) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let start = Instant::now();
        loop {
            if !self
                .core
                .inner
                .state
                .lock()
                .unwrap()
                .transfers
                .iter()
                .any(|t| t.id == self.id && t.state == TransferState::Running)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionAborted,
                    "cancelled",
                ));
            }
            match op(&mut self.socket) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) && start.elapsed() < Duration::from_secs(30) =>
                {
                    continue
                }
                result => return result,
            }
        }
    }
}
impl Read for TransferSocket {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.io(|s| s.read(b))
    }
}
impl Write for TransferSocket {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.io(|s| s.write(b))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Core {
    pub(super) fn new_transfer(
        &self,
        name: String,
        direction: Direction,
        count: u32,
        total: u64,
    ) -> u64 {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner.state.lock().unwrap().transfers.insert(
            0,
            TransferView {
                id,
                peer_name: name,
                direction,
                current_file: String::new(),
                files_done: 0,
                files_total: count,
                bytes_done: 0,
                bytes_total: total,
                speed_bps: 0.,
                state: TransferState::Running,
                dest: None,
            },
        );
        self.changed();
        id
    }
    fn update(&self, id: u64, f: impl FnOnce(&mut TransferView)) {
        if let Some(t) = self
            .inner
            .state
            .lock()
            .unwrap()
            .transfers
            .iter_mut()
            .find(|t| t.id == id)
        {
            f(t);
        }
    }
    pub(super) fn register(&self, id: u64, socket: &TcpStream) -> Result<()> {
        let mut s = self.inner.state.lock().unwrap();
        ensure!(
            s.transfers
                .iter()
                .any(|t| t.id == id && t.state == TransferState::Running),
            "cancelled"
        );
        s.sockets.insert(id, socket.try_clone()?);
        Ok(())
    }
    pub(super) fn finish(&self, id: u64, result: Result<()>) {
        let mut s = self.inner.state.lock().unwrap();
        s.sockets.remove(&id);
        if let Some(t) = s.transfers.iter_mut().find(|t| t.id == id) {
            if t.state == TransferState::Running {
                t.state = match result {
                    Ok(()) => TransferState::Done,
                    Err(e) => TransferState::Failed(e.to_string()),
                };
            }
            t.speed_bps = 0.;
        }
        drop(s);
        self.changed();
    }
    fn progress(&self, id: u64, n: u64, last: &mut Instant, pending: &mut u64) {
        *pending += n;
        self.update(id, |t| t.bytes_done += n);
        let dt = last.elapsed().as_secs_f64();
        if dt >= 0.1 {
            self.update(id, |t| {
                let rate = *pending as f64 / dt;
                t.speed_bps = if t.speed_bps == 0. {
                    rate
                } else {
                    0.25 * rate + 0.75 * t.speed_bps
                };
            });
            *pending = 0;
            *last = Instant::now();
            self.changed();
        }
    }
    fn check_cancel(&self, id: u64) -> Result<()> {
        ensure!(
            self.inner
                .state
                .lock()
                .unwrap()
                .transfers
                .iter()
                .any(|t| t.id == id && t.state == TransferState::Running),
            "cancelled"
        );
        Ok(())
    }
    pub(super) fn send_files(&self, id: u64, peer: &str, paths: Vec<PathBuf>) -> Result<()> {
        let files = paths::collect(paths, || self.check_cancel(id))?;
        ensure!(files.len() <= u32::MAX as usize, "too many files");
        let total = files.iter().map(|f| f.2).sum::<u64>();
        self.update(id, |t| {
            t.files_total = files.len() as u32;
            t.bytes_total = total;
        });
        self.check_cancel(id)?;
        let (socket, _) = self.connect(peer, Some(id))?;
        let mut writer =
            BufWriter::with_capacity(CHUNK, TransferSocket::new(socket.try_clone()?, self, id)?);
        let mut reader = BufReader::with_capacity(CHUNK, TransferSocket::new(socket, self, id)?);
        send(
            &mut writer,
            BATCH_START,
            json!({"count":files.len(),"totalBytes":total}),
        )?;
        let mut buf = vec![0; CHUNK];
        let mut last = Instant::now();
        let mut pending = 0;
        for (path, relative, size, mtime) in files {
            self.check_cancel(id)?;
            paths::sanitize(&relative)?;
            self.update(id, |t| t.current_file = relative.clone());
            self.changed();
            send(
                &mut writer,
                FILE_START,
                json!({"path":relative,"size":size,"mtime":mtime}),
            )?;
            self.check_cancel(id)?;
            let mut file = std::fs::File::open(&path)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            let mut bytes = 0;
            loop {
                self.check_cancel(id)?;
                let n = file
                    .read(&mut buf)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                self.check_cancel(id)?;
                if n == 0 {
                    break;
                }
                write_frame(&mut writer, DATA, &buf[..n])?;
                bytes += n as u64;
                self.progress(id, n as u64, &mut last, &mut pending);
            }
            ensure!(bytes == size, "source file changed during transfer");
            send(&mut writer, FILE_END, json!({}))?;
            self.update(id, |t| t.files_done += 1);
        }
        send(&mut writer, BATCH_END, json!({}))?;
        let ack = recv(&mut reader, BATCH_ACK)?;
        ensure!(
            ack["ok"] == true
                && ack["saved"].as_u64()
                    == Some(
                        self.inner
                            .state
                            .lock()
                            .unwrap()
                            .transfers
                            .iter()
                            .find(|t| t.id == id)
                            .map(|t| t.files_total as u64)
                            .unwrap_or(0)
                    ),
            "invalid batch acknowledgement"
        );
        Ok(())
    }
    pub(super) fn receive_files(
        &self,
        id: u64,
        socket: &mut TcpStream,
        start: serde_json::Value,
    ) -> Result<()> {
        let count = start["count"]
            .as_u64()
            .filter(|n| *n <= u32::MAX as u64)
            .ok_or_else(|| anyhow::anyhow!("invalid count"))? as u32;
        let total = start["totalBytes"]
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("invalid totalBytes"))?;
        let root = self.inner.state.lock().unwrap().config.download_dir.clone();
        self.update(id, |t| {
            t.files_total = count;
            t.bytes_total = total;
            t.dest = Some(root.clone());
        });
        let mut reader =
            BufReader::with_capacity(CHUNK, TransferSocket::new(socket.try_clone()?, self, id)?);
        let mut last = Instant::now();
        let mut pending = 0;
        for _ in 0..count {
            let header = recv(&mut reader, FILE_START)?;
            let rel = paths::sanitize(
                header["path"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("missing path"))?,
            )?;
            let size = header["size"]
                .as_i64()
                .filter(|s| *s >= -1)
                .ok_or_else(|| anyhow::anyhow!("invalid size"))?;
            let mtime = header["mtime"]
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("invalid mtime"))?;
            let (dest, mut file) = paths::create(&root, &rel)?;
            self.update(id, |t| {
                t.current_file = rel.to_string_lossy().into_owned();
                if count == 1 {
                    t.dest = Some(dest.clone());
                }
            });
            self.changed();
            let result = (|| -> Result<()> {
                let mut bytes = 0u64;
                loop {
                    let (kind, data) = read_frame(&mut reader)?;
                    match kind {
                        DATA => {
                            ensure!(!data.is_empty() && data.len() <= CHUNK, "invalid DATA size");
                            bytes = bytes
                                .checked_add(data.len() as u64)
                                .ok_or_else(|| anyhow::anyhow!("file too large"))?;
                            ensure!(size < 0 || bytes <= size as u64, "file size mismatch");
                            file.write_all(&data)?;
                            self.progress(id, data.len() as u64, &mut last, &mut pending);
                        }
                        FILE_END => {
                            let v: serde_json::Value = serde_json::from_slice(&data)?;
                            ensure!(v.is_object(), "invalid FILE_END");
                            break;
                        }
                        _ => anyhow::bail!("unexpected frame in file"),
                    }
                }
                ensure!(size < 0 || bytes == size as u64, "file size mismatch");
                file.flush()?;
                if mtime > 0 {
                    let time = std::time::UNIX_EPOCH
                        .checked_add(Duration::from_millis(mtime))
                        .ok_or_else(|| anyhow::anyhow!("invalid mtime"))?;
                    file.set_modified(time)?;
                }
                Ok(())
            })();
            drop(file);
            if let Err(e) = result {
                let _ = std::fs::remove_file(&dest);
                return Err(e);
            }
            self.update(id, |t| t.files_done += 1);
            self.changed();
        }
        recv(&mut reader, BATCH_END)?;
        send(socket, BATCH_ACK, json!({"ok":true,"saved":count}))?;
        Ok(())
    }
}
