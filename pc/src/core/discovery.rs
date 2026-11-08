use super::*;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{SocketAddr, UdpSocket};
pub(super) fn start(core: &Core) -> Result<()> {
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.set_reuse_address(true)?;
    sock.set_broadcast(true)?;
    sock.bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 47820)).into())?;
    let udp: UdpSocket = sock.into();
    udp.set_read_timeout(Some(Duration::from_millis(500)))?;
    let weak = Arc::downgrade(&core.inner);
    thread::spawn(move || {
        let mut last = Instant::now() - Duration::from_secs(5);
        let mut buf = [0; 1025];
        loop {
            let Some(inner) = weak.upgrade() else { break };
            let core = Core { inner };
            let cfg = core.inner.state.lock().unwrap().config.clone();
            let packet = |t: &str| {
                serde_json::to_vec(
                    &json!({"t":t,"v":1,"id":cfg.id,"name":cfg.name,"kind":"pc","port":47821}),
                )
                .unwrap()
            };
            if last.elapsed() >= Duration::from_secs(5) {
                let interfaces = if_addrs::get_if_addrs().unwrap_or_default();
                let mut ips = vec![];
                let mut targets = vec![Ipv4Addr::BROADCAST];
                for interface in interfaces {
                    if interface.is_loopback() {
                        continue;
                    }
                    if let if_addrs::IfAddr::V4(a) = interface.addr {
                        ips.push(a.ip);
                        if let Some(b) = a.broadcast {
                            if !targets.contains(&b) {
                                targets.push(b);
                            }
                        }
                    }
                }
                core.inner.state.lock().unwrap().local_ips = ips;
                let p = packet("disc");
                if p.len() <= 1024 {
                    for target in &targets {
                        let _ = udp.send_to(&p, (*target, 47820));
                    }
                }
                last = Instant::now();
            }
            if let Ok((n, addr)) = udp.recv_from(&mut buf) {
                if n <= 1024 {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..n]) {
                        if v["v"] == 1
                            && v["kind"] == "phone"
                            && (v["t"] == "disc" || v["t"] == "here")
                        {
                            if let (Some(id), Some(name)) = (v["id"].as_str(), v["name"].as_str()) {
                                if id != cfg.id {
                                    core.seen(id, name, addr.ip());
                                    if v["t"] == "disc" {
                                        let p = packet("here");
                                        if p.len() <= 1024 {
                                            let _ = udp.send_to(&p, addr);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            core.changed();
        }
    });
    Ok(())
}
