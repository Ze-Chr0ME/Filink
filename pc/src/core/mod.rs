mod config;
mod discovery;
mod handshake;
mod paths;
pub mod protocol;
mod transfer;
use anyhow::{ensure, Result};
use config::{Config, Peer};
use protocol::*;
use serde_json::json;
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
#[derive(Clone)]
pub struct Core {
    inner: Arc<Inner>,
}
struct Inner {
    state: Mutex<State>,
    repaint: Box<dyn Fn() + Send + Sync>,
    next: AtomicU64,
    config_path: PathBuf,
}
struct State {
    config: Config,
    seen: HashMap<String, (String, IpAddr, Instant)>,
    pending: HashMap<u64, Arc<Pending>>,
    transfers: Vec<TransferView>,
    sockets: HashMap<u64, TcpStream>,
    texts: Vec<TextItem>,
    server_error: Option<String>,
    local_ips: Vec<Ipv4Addr>,
}
struct Pending {
    view: PairRequest,
    created: Instant,
    answer: Mutex<Option<bool>>,
    wake: Condvar,
}
#[derive(Clone)]
pub struct Snapshot {
    pub my_name: String,
    pub my_id: String,
    pub download_dir: PathBuf,
    pub local_ips: Vec<Ipv4Addr>,
    pub server_error: Option<String>,
    pub peers: Vec<PeerView>,
    pub pair_requests: Vec<PairRequest>,
    pub transfers: Vec<TransferView>,
    pub texts: Vec<TextItem>,
}
#[derive(Clone)]
pub struct PeerView {
    pub id: String,
    pub name: String,
    pub ip: Option<IpAddr>,
    pub online: bool,
    pub paired: bool,
    pub last_seen_secs: Option<u64>,
}
#[derive(Clone)]
pub struct PairRequest {
    pub id: u64,
    pub peer_id: String,
    pub peer_name: String,
    pub ip: IpAddr,
    pub code: String,
    pub expires_in_secs: u64,
}
#[derive(Clone, PartialEq)]
pub enum Direction {
    Send,
    Receive,
}
#[derive(Clone, PartialEq)]
pub enum TransferState {
    Running,
    Done,
    Failed(String),
    Cancelled,
}
#[derive(Clone)]
pub struct TransferView {
    pub id: u64,
    pub peer_name: String,
    pub direction: Direction,
    pub current_file: String,
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub speed_bps: f64,
    pub state: TransferState,
    pub dest: Option<PathBuf>,
}
#[derive(Clone)]
pub struct TextItem {
    pub peer_name: String,
    pub text: String,
    pub incoming: bool,
    pub at: SystemTime,
}
impl Core {
    pub fn start(repaint: Box<dyn Fn() + Send + Sync>) -> Result<Self> {
        let (config, config_path) = Config::load()?;
        let core = Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    config,
                    seen: HashMap::new(),
                    pending: HashMap::new(),
                    transfers: vec![],
                    sockets: HashMap::new(),
                    texts: vec![],
                    server_error: None,
                    local_ips: vec![],
                }),
                repaint,
                next: AtomicU64::new(1),
                config_path,
            }),
        };
        match TcpListener::bind((Ipv4Addr::UNSPECIFIED, 47821)) {
            Ok(listener) => {
                listener.set_nonblocking(true)?;
                let unauthenticated = Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let weak = Arc::downgrade(&core.inner);
                thread::spawn(move || loop {
                    let Some(inner) = weak.upgrade() else { break };
                    drop(inner);
                    match listener.accept() {
                        Ok((mut socket, addr)) => {
                            let Some(permit) = handshake::Permit::acquire(&unauthenticated) else {
                                let _ = socket.shutdown(Shutdown::Both);
                                continue;
                            };
                            let Ok(mut handshake) =
                                handshake::Handshake::new(&socket, permit, handshake::DEADLINE)
                            else {
                                continue;
                            };
                            if let Some(inner) = weak.upgrade() {
                                thread::spawn(move || {
                                    let core = Core { inner };
                                    let result = prepare(&socket).and_then(|_| {
                                        core.serve(&mut socket, addr.ip(), &mut handshake)
                                    });
                                    if let Err(e) = result {
                                        let _ =
                                            send(&mut socket, ERROR, json!({"msg":e.to_string()}));
                                    }
                                });
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(50))
                        }
                        Err(_) => break,
                    }
                });
            }
            Err(e) => core.inner.state.lock().unwrap().server_error = Some(e.to_string()),
        };
        if let Err(e) = discovery::start(&core) {
            core.inner
                .state
                .lock()
                .unwrap()
                .server_error
                .get_or_insert_with(|| format!("discovery: {e}"));
        }
        Ok(core)
    }
    pub fn snapshot(&self) -> Snapshot {
        let s = self.inner.state.lock().unwrap();
        let mut peers = vec![];
        for p in &s.config.peers {
            let seen = s.seen.get(&p.id);
            peers.push(PeerView {
                id: p.id.clone(),
                name: seen.map(|x| x.0.clone()).unwrap_or_else(|| p.name.clone()),
                ip: seen.map(|x| x.1).or(p.last_ip),
                online: seen.is_some_and(|x| x.2.elapsed() < Duration::from_secs(15)),
                paired: true,
                last_seen_secs: seen.map(|x| x.2.elapsed().as_secs()),
            });
        }
        let mut unpaired: Vec<_> = s
            .seen
            .iter()
            .filter(|(id, _)| !s.config.peers.iter().any(|p| &p.id == *id))
            .map(|(id, (name, ip, t))| PeerView {
                id: id.clone(),
                name: name.clone(),
                ip: Some(*ip),
                online: t.elapsed() < Duration::from_secs(15),
                paired: false,
                last_seen_secs: Some(t.elapsed().as_secs()),
            })
            .collect();
        unpaired.sort_by(|a, b| a.name.cmp(&b.name));
        peers.extend(unpaired);
        Snapshot {
            my_name: s.config.name.clone(),
            my_id: s.config.id.clone(),
            download_dir: s.config.download_dir.clone(),
            local_ips: s.local_ips.clone(),
            server_error: s.server_error.clone(),
            peers,
            pair_requests: s
                .pending
                .values()
                .map(|p| {
                    let mut v = p.view.clone();
                    v.expires_in_secs = 60u64.saturating_sub(p.created.elapsed().as_secs());
                    v
                })
                .collect(),
            transfers: s.transfers.clone(),
            texts: s.texts.clone(),
        }
    }
    pub fn respond_pair(&self, request_id: u64, accept: bool) {
        if let Some(p) = self.inner.state.lock().unwrap().pending.get(&request_id) {
            *p.answer.lock().unwrap() = Some(accept);
            p.wake.notify_one();
        }
    }
    pub fn send_paths(&self, peer_id: &str, paths: Vec<PathBuf>) -> u64 {
        let name = self
            .snapshot()
            .peers
            .iter()
            .find(|p| p.id == peer_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| peer_id.into());
        let id = self.new_transfer(name, Direction::Send, 0, 0);
        let core = self.clone();
        let peer = peer_id.to_owned();
        thread::spawn(move || {
            let result = core.send_files(id, &peer, paths);
            core.finish(id, result);
        });
        id
    }
    pub fn send_text(&self, peer_id: &str, text: String) {
        let core = self.clone();
        let peer = peer_id.to_owned();
        thread::spawn(move || {
            let result = (|| -> Result<()> {
                let (mut stream, name) = core.connect(&peer, None)?;
                send(&mut stream, TEXT, json!({"text":text}))?;
                recv(&mut stream, TEXT_ACK)?;
                core.record_text(name, text, false);
                Ok(())
            })();
            if let Err(e) = result {
                let id = core.new_transfer(peer, Direction::Send, 0, 0);
                core.finish(id, Err(e));
            }
        });
    }
    pub fn cancel_transfer(&self, id: u64) {
        let mut s = self.inner.state.lock().unwrap();
        if let Some(t) = s
            .transfers
            .iter_mut()
            .find(|t| t.id == id && t.state == TransferState::Running)
        {
            t.state = TransferState::Cancelled;
            if let Some(socket) = s.sockets.get(&id) {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        drop(s);
        self.changed();
    }
    pub fn clear_finished(&self) {
        self.inner
            .state
            .lock()
            .unwrap()
            .transfers
            .retain(|t| t.state == TransferState::Running);
        self.changed();
    }
    pub fn unpair(&self, id: &str) {
        let mut s = self.inner.state.lock().unwrap();
        s.config.peers.retain(|p| p.id != id);
        self.save(&mut s);
        drop(s);
        self.changed();
    }
    pub fn set_download_dir(&self, dir: PathBuf) {
        let mut s = self.inner.state.lock().unwrap();
        s.config.download_dir = dir;
        self.save(&mut s);
        drop(s);
        self.changed();
    }
    pub fn set_name(&self, name: String) {
        let mut s = self.inner.state.lock().unwrap();
        s.config.name = name;
        self.save(&mut s);
        drop(s);
        self.changed();
    }
    fn changed(&self) {
        (self.inner.repaint)();
    }
    fn save(&self, s: &mut State) {
        if let Err(e) = s.config.save(&self.inner.config_path) {
            s.server_error = Some(format!("save config: {e}"));
        }
    }
    fn record_text(&self, name: String, text: String, incoming: bool) {
        if incoming {
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(text.clone());
            }
        }
        let mut s = self.inner.state.lock().unwrap();
        s.texts.insert(
            0,
            TextItem {
                peer_name: name,
                text,
                incoming,
                at: SystemTime::now(),
            },
        );
        s.texts.truncate(50);
        drop(s);
        self.changed();
    }
    fn seen(&self, id: &str, name: &str, ip: IpAddr) {
        let mut s = self.inner.state.lock().unwrap();
        s.seen.insert(id.into(), (name.into(), ip, Instant::now()));
        if let Some(p) = s.config.peers.iter_mut().find(|p| p.id == id) {
            if p.last_ip != Some(ip) || p.name != name {
                p.last_ip = Some(ip);
                p.name = name.into();
                self.save(&mut s);
            }
        }
        drop(s);
        self.changed();
    }
    fn serve(
        &self,
        socket: &mut TcpStream,
        ip: IpAddr,
        handshake: &mut handshake::Handshake,
    ) -> Result<()> {
        let hello = recv(socket, HELLO)?;
        ensure!(hello["v"] == 1 && hello["kind"] == "phone", "invalid hello");
        let id = hello["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing id"))?
            .to_owned();
        let name = hello["name"].as_str().unwrap_or("Phone").to_owned();
        let c = nonce(&hello)?;
        self.seen(&id, &name, ip);
        let sn = random::<16>();
        let cfg = self.inner.state.lock().unwrap().config.clone();
        let peer = cfg.peers.iter().find(|p| p.id == id);
        send(
            socket,
            HELLO_ACK,
            json!({"v":1,"id":cfg.id,"name":cfg.name,"kind":"pc","nonce":hex::encode(sn),"paired":peer.is_some()}),
        )?;
        if let Some(peer) = peer {
            let key = hex::decode(&peer.secret_hex)?;
            let auth = recv(socket, AUTH)?;
            verify(&key, "DT1-C", &c, &sn, &auth).map_err(|_| anyhow::anyhow!("auth failed"))?;
            send(
                socket,
                AUTH_OK,
                json!({"mac":hex::encode(mac(&key,"DT1-S",&c,&sn))}),
            )?;
            handshake.authenticated()?;
            self.seen(&id, &name, ip);
            let (kind, payload) = read_frame(socket)?;
            match kind {
                TEXT => {
                    let v: serde_json::Value = serde_json::from_slice(&payload)?;
                    let text = v["text"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing text"))?
                        .to_owned();
                    self.record_text(name, text, true);
                    send(socket, TEXT_ACK, json!({}))?;
                }
                BATCH_START => {
                    let v = serde_json::from_slice(&payload)?;
                    let tid = self.new_transfer(name, Direction::Receive, 0, 0);
                    let result = self
                        .register(tid, socket)
                        .and_then(|_| self.receive_files(tid, socket, v));
                    let msg = result.as_ref().err().map(ToString::to_string);
                    self.finish(tid, result);
                    if let Some(msg) = msg {
                        anyhow::bail!(msg);
                    }
                }
                _ => anyhow::bail!("unexpected operation"),
            };
        } else {
            recv(socket, PAIR_REQ)?;
            handshake.stop_deadline()?;
            let request_id = self.inner.next.fetch_add(1, Ordering::Relaxed);
            let pending = Arc::new(Pending {
                view: PairRequest {
                    id: request_id,
                    peer_id: id.clone(),
                    peer_name: name.clone(),
                    ip,
                    code: code(&c, &sn),
                    expires_in_secs: 60,
                },
                created: Instant::now(),
                answer: Mutex::new(None),
                wake: Condvar::new(),
            });
            {
                let mut state = self.inner.state.lock().unwrap();
                ensure!(
                    !state.pending.values().any(|p| p.view.peer_id == id),
                    "pairing already pending"
                );
                state.pending.insert(request_id, pending.clone());
            }
            self.changed();
            let answer = pending.answer.lock().unwrap();
            let (answer, _) = pending
                .wake
                .wait_timeout_while(answer, Duration::from_secs(60), |a| a.is_none())
                .unwrap();
            let accepted = *answer;
            drop(answer);
            self.inner.state.lock().unwrap().pending.remove(&request_id);
            self.changed();
            ensure!(accepted.is_some(), "timeout");
            ensure!(accepted == Some(true), "rejected");
            let secret = hex::encode(random::<32>());
            {
                let mut state = self.inner.state.lock().unwrap();
                state.config.peers.retain(|p| p.id != id);
                state.config.peers.push(Peer {
                    id: id.clone(),
                    name: name.clone(),
                    secret_hex: secret.clone(),
                    last_ip: Some(ip),
                });
                state.config.save(&self.inner.config_path)?;
            }
            self.seen(&id, &name, ip);
            send(socket, PAIR_OK, json!({"secret":secret}))?;
        }
        Ok(())
    }
    fn connect(&self, id: &str, tid: Option<u64>) -> Result<(TcpStream, String)> {
        let cfg = self.inner.state.lock().unwrap().config.clone();
        let peer = cfg
            .peers
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| anyhow::anyhow!("device is not paired"))?;
        let ip = self
            .inner
            .state
            .lock()
            .unwrap()
            .seen
            .get(id)
            .map(|p| p.1)
            .or(peer.last_ip)
            .ok_or_else(|| {
                anyhow::anyhow!("phone not reachable — open DroidTransfer on the phone")
            })?;
        let mut socket = TcpStream::connect_timeout(
            &std::net::SocketAddr::new(ip, 47822),
            Duration::from_secs(3),
        )
        .map_err(|_| anyhow::anyhow!("phone not reachable — open DroidTransfer on the phone"))?;
        prepare(&socket)?;
        if let Some(id) = tid {
            self.register(id, &socket)?;
        }
        if let Some(tid) = tid {
            let mut wire = transfer::TransferSocket::new(socket, self, tid)?;
            client_auth(&mut wire, &cfg, peer)?;
            socket = wire.socket;
        } else {
            client_auth(&mut socket, &cfg, peer)?;
        }
        self.seen(id, &peer.name, ip);
        Ok((socket, peer.name.clone()))
    }
}
fn prepare(s: &TcpStream) -> Result<()> {
    // Accepted Windows sockets inherit the listener's nonblocking mode.
    s.set_nonblocking(false)?;
    s.set_nodelay(true)?;
    s.set_read_timeout(Some(Duration::from_secs(30)))?;
    s.set_write_timeout(Some(Duration::from_secs(30)))?;
    let sock = socket2::SockRef::from(s);
    let _ = sock.set_send_buffer_size(4 * CHUNK);
    let _ = sock.set_recv_buffer_size(4 * CHUNK);
    Ok(())
}

fn client_auth(
    socket: &mut (impl std::io::Read + std::io::Write),
    cfg: &Config,
    peer: &Peer,
) -> Result<()> {
    let c = random::<16>();
    send(
        socket,
        HELLO,
        json!({"v":1,"id":cfg.id,"name":cfg.name,"kind":"pc","nonce":hex::encode(c)}),
    )?;
    let ack = recv(socket, HELLO_ACK)?;
    ensure!(
        ack["v"] == 1 && ack["kind"] == "phone" && ack["id"] == peer.id && ack["paired"] == true,
        "phone identity/pairing mismatch"
    );
    let sn = nonce(&ack)?;
    let key = hex::decode(&peer.secret_hex)?;
    send(
        socket,
        AUTH,
        json!({"mac":hex::encode(mac(&key,"DT1-C",&c,&sn))}),
    )?;
    let auth = recv(socket, AUTH_OK)?;
    verify(&key, "DT1-S", &c, &sn, &auth)?;
    Ok(())
}
