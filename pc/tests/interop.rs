use anyhow::Result;
use droidtransfer::core::{protocol::*, *};
use serde_json::{json, Value};
use std::{
    net::{TcpListener, TcpStream, UdpSocket},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn wait(mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(start.elapsed() < Duration::from_secs(8), "timed out");
        thread::sleep(Duration::from_millis(20));
    }
}
fn hello() -> Result<(TcpStream, Value, [u8; 16])> {
    let mut s = TcpStream::connect("127.0.0.1:47821")?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    let c = [7; 16];
    send(
        &mut s,
        HELLO,
        json!({"v":1,"id":"test-phone","name":"Fake phone","kind":"phone","nonce":hex::encode(c)}),
    )?;
    let ack = recv(&mut s, HELLO_ACK)?;
    Ok((s, ack, c))
}
fn auth(secret: &[u8]) -> Result<TcpStream> {
    let (mut s, ack, c) = hello()?;
    let sn = nonce(&ack)?;
    send(
        &mut s,
        AUTH,
        json!({"mac":hex::encode(mac(secret,"DT1-C",&c,&sn))}),
    )?;
    let ok = recv(&mut s, AUTH_OK)?;
    verify(secret, "DT1-S", &c, &sn, &ok)?;
    Ok(s)
}
fn file(s: &mut TcpStream, path: &str, size: i64, data: &[u8]) -> Result<()> {
    send(
        s,
        FILE_START,
        json!({"path":path,"size":size,"mtime":1700000000000u64}),
    )?;
    write_frame(s, DATA, data)?;
    send(s, FILE_END, json!({}))
}
#[test]
fn fake_phone_interop() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("interop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root)?;
    std::env::set_var("DROIDTRANSFER_CONFIG_DIR", root.join("config"));
    let core = Core::start(Box::new(|| {}))?;
    assert!(
        core.snapshot().server_error.is_none(),
        "{:?}",
        core.snapshot().server_error
    );
    core.set_download_dir(root.join("downloads"));
    let udp = UdpSocket::bind("127.0.0.1:0")?;
    udp.send_to(&serde_json::to_vec(&json!({"t":"disc","v":1,"id":"test-phone","name":"Fake phone","kind":"phone","port":47822}))?,"127.0.0.1:47820")?;
    wait(|| core.snapshot().peers.iter().any(|p| p.id == "test-phone"));
    let (mut s, ack, c) = hello()?;
    assert_eq!(ack["paired"], false);
    send(&mut s, PAIR_REQ, json!({}))?;
    wait(|| !core.snapshot().pair_requests.is_empty());
    let req = core.snapshot().pair_requests[0].clone();
    assert_eq!(req.code, code(&c, &nonce(&ack)?));
    core.respond_pair(req.id, true);
    let paired = recv(&mut s, PAIR_OK)?;
    let secret = hex::decode(paired["secret"].as_str().unwrap())?;
    drop(s);
    let mut s = auth(&secret)?;
    send(&mut s, TEXT, json!({"text":"Hello phone"}))?;
    recv(&mut s, TEXT_ACK)?;
    wait(|| {
        core.snapshot()
            .texts
            .first()
            .is_some_and(|t| t.text == "Hello phone")
    });
    let mut s = auth(&secret)?;
    send(&mut s, BATCH_START, json!({"count":2,"totalBytes":3}))?;
    file(&mut s, "nested/deep/a.txt", 3, b"abc")?;
    file(&mut s, "unknown.bin", -1, b"unknown")?;
    send(&mut s, BATCH_END, json!({}))?;
    assert_eq!(recv(&mut s, BATCH_ACK)?["saved"], 2);
    assert_eq!(
        std::fs::read(root.join("downloads/nested/deep/a.txt"))?,
        b"abc"
    );
    assert_eq!(
        std::fs::read(root.join("downloads/unknown.bin"))?,
        b"unknown"
    );
    let mut s = auth(&secret)?;
    send(&mut s, BATCH_START, json!({"count":1,"totalBytes":3}))?;
    file(&mut s, "nested/deep/a.txt", 3, b"def")?;
    send(&mut s, BATCH_END, json!({}))?;
    recv(&mut s, BATCH_ACK)?;
    assert_eq!(
        std::fs::read(root.join("downloads/nested/deep/a (1).txt"))?,
        b"def"
    );
    let mut s = auth(&secret)?;
    send(&mut s, BATCH_START, json!({"count":1,"totalBytes":0}))?;
    send(
        &mut s,
        FILE_START,
        json!({"path":"../x","size":0,"mtime":0}),
    )?;
    assert_eq!(read_frame(&mut s)?.0, ERROR);
    assert!(!root.join("x").exists());
    let (mut s, _, _) = hello()?;
    send(&mut s, AUTH, json!({"mac":"00".repeat(32)}))?;
    let (t, p) = read_frame(&mut s)?;
    assert_eq!(t, ERROR);
    assert_eq!(serde_json::from_slice::<Value>(&p)?["msg"], "auth failed");
    let mut s = auth(&secret)?;
    send(&mut s, BATCH_START, json!({"count":1,"totalBytes":9}))?;
    file(&mut s, "bad.bin", 9, b"short")?;
    assert_eq!(read_frame(&mut s)?.0, ERROR);
    assert!(!root.join("downloads/bad.bin").exists());
    let mut s = auth(&secret)?;
    send(&mut s, BATCH_START, json!({"count":1,"totalBytes":0}))?;
    send(
        &mut s,
        FILE_START,
        json!({"path":"partial.bin","size":-1,"mtime":0}),
    )?;
    write_frame(&mut s, DATA, b"partial")?;
    wait(|| root.join("downloads/partial.bin").exists());
    let tid = core
        .snapshot()
        .transfers
        .iter()
        .find(|t| t.current_file == "partial.bin")
        .unwrap()
        .id;
    core.cancel_transfer(tid);
    wait(|| !root.join("downloads/partial.bin").exists());
    assert!(core
        .snapshot()
        .transfers
        .iter()
        .any(|t| t.id == tid && t.state == TransferState::Cancelled));
    drop(s);
    let listener = TcpListener::bind("127.0.0.1:47822")?;
    let key = secret.clone();
    let pc_id = core.snapshot().my_id;
    let phone = thread::spawn(move || -> Result<()> {
        for operation in 0..2 {
            let (mut s, _) = listener.accept()?;
            s.set_read_timeout(Some(Duration::from_secs(5)))?;
            let h = recv(&mut s, HELLO)?;
            assert_eq!(h["id"], pc_id);
            let c = nonce(&h)?;
            let sn = [8; 16];
            send(
                &mut s,
                HELLO_ACK,
                json!({"v":1,"id":"test-phone","name":"Fake phone","kind":"phone","nonce":hex::encode(sn),"paired":true}),
            )?;
            let a = recv(&mut s, AUTH)?;
            verify(&key, "DT1-C", &c, &sn, &a)?;
            send(
                &mut s,
                AUTH_OK,
                json!({"mac":hex::encode(mac(&key,"DT1-S",&c,&sn))}),
            )?;
            if operation == 0 {
                assert_eq!(recv(&mut s, BATCH_START)?["count"], 2);
                let mut names = vec![];
                for _ in 0..2 {
                    let h = recv(&mut s, FILE_START)?;
                    names.push(h["path"].as_str().unwrap().to_owned());
                    let mut bytes = vec![];
                    loop {
                        let (t, p) = read_frame(&mut s)?;
                        if t == FILE_END {
                            break;
                        }
                        assert_eq!(t, DATA);
                        bytes.extend(p);
                    }
                    assert_eq!(bytes.len() as i64, h["size"].as_i64().unwrap());
                }
                assert!(names.contains(&"folder/sub/a.txt".to_owned()));
                assert!(names.contains(&"single.txt".to_owned()));
                recv(&mut s, BATCH_END)?;
                send(&mut s, BATCH_ACK, json!({"ok":true,"saved":2}))?;
            } else {
                assert_eq!(recv(&mut s, TEXT)?["text"], "outgoing");
                send(&mut s, TEXT_ACK, json!({}))?;
            }
        }
        Ok(())
    });
    std::fs::create_dir_all(root.join("folder/sub"))?;
    std::fs::write(root.join("folder/sub/a.txt"), b"folder content")?;
    std::fs::write(root.join("single.txt"), b"single content")?;
    let id = core.send_paths(
        "test-phone",
        vec![root.join("folder"), root.join("single.txt")],
    );
    wait(|| {
        core.snapshot()
            .transfers
            .iter()
            .any(|t| t.id == id && t.state != TransferState::Running)
    });
    assert!(core
        .snapshot()
        .transfers
        .iter()
        .any(|t| t.id == id && t.state == TransferState::Done));
    core.send_text("test-phone", "outgoing".into());
    wait(|| {
        core.snapshot()
            .texts
            .iter()
            .any(|t| !t.incoming && t.text == "outgoing")
    });
    phone.join().unwrap()?;
    // Cancelling an outgoing connection also closes it while AUTH is pending.
    let listener = TcpListener::bind("127.0.0.1:47822")?;
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let stalled = thread::spawn(move || -> Result<()> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(3)))?;
        recv(&mut socket, HELLO)?;
        ready_tx.send(())?;
        assert!(read_frame(&mut socket).is_err());
        Ok(())
    });
    let cancelled = core.send_paths("test-phone", vec![root.join("single.txt")]);
    ready_rx.recv_timeout(Duration::from_secs(5))?;
    core.cancel_transfer(cancelled);
    stalled.join().unwrap()?;
    assert!(core
        .snapshot()
        .transfers
        .iter()
        .any(|t| t.id == cancelled && t.state == TransferState::Cancelled));
    // Cancel a batch after authentication and after payload streaming has begun.
    let large = root.join("large.bin");
    std::fs::File::create(&large)?.set_len(32 * CHUNK as u64)?;
    let listener = TcpListener::bind("127.0.0.1:47822")?;
    let key = secret.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let streaming_phone = thread::spawn(move || -> Result<()> {
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(5)))?;
        let hello = recv(&mut socket, HELLO)?;
        let c = nonce(&hello)?;
        let sn = [9; 16];
        send(
            &mut socket,
            HELLO_ACK,
            json!({"v":1,"id":"test-phone","name":"Fake phone","kind":"phone","nonce":hex::encode(sn),"paired":true}),
        )?;
        let auth = recv(&mut socket, AUTH)?;
        verify(&key, "DT1-C", &c, &sn, &auth)?;
        send(
            &mut socket,
            AUTH_OK,
            json!({"mac":hex::encode(mac(&key,"DT1-S",&c,&sn))}),
        )?;
        recv(&mut socket, BATCH_START)?;
        recv(&mut socket, FILE_START)?;
        ready_tx.send(())?;
        release_rx.recv_timeout(Duration::from_secs(5))?;
        while let Ok((kind, _)) = read_frame(&mut socket) {
            assert_eq!(kind, DATA, "cancelled batch must not finish");
        }
        Ok(())
    });
    let streaming = core.send_paths("test-phone", vec![large]);
    ready_rx.recv_timeout(Duration::from_secs(5))?;
    wait(|| {
        core.snapshot()
            .transfers
            .iter()
            .any(|t| t.id == streaming && t.bytes_done > 0)
    });
    core.cancel_transfer(streaming);
    assert!(core
        .snapshot()
        .transfers
        .iter()
        .any(|t| t.id == streaming && t.state == TransferState::Cancelled));
    release_tx.send(())?;
    streaming_phone.join().unwrap()?;
    assert!(core
        .snapshot()
        .transfers
        .iter()
        .any(|t| t.id == streaming && t.state == TransferState::Cancelled));
    // Enumeration failures must surface before attempting to connect.
    let missing = root.join("missing-file.txt");
    let failed = core.send_paths("test-phone", vec![root.join("single.txt"), missing.clone()]);
    wait(|| {
        core.snapshot()
            .transfers
            .iter()
            .any(|t| t.id == failed && t.state != TransferState::Running)
    });
    let snapshot = core.snapshot();
    let failure = snapshot.transfers.iter().find(|t| t.id == failed).unwrap();
    assert!(matches!(&failure.state, TransferState::Failed(message)
        if message.starts_with(&format!("{}: ", missing.display()))));
    core.clear_finished();
    assert!(core.snapshot().transfers.is_empty());
    core.set_name("Test PC".into());
    let cfg: Value = serde_json::from_slice(&std::fs::read(root.join("config/config.json"))?)?;
    assert_eq!(cfg["name"], "Test PC");
    core.unpair("test-phone");
    assert!(!core.snapshot().peers.iter().any(|p| p.paired));
    drop(core);
    thread::sleep(Duration::from_millis(700));
    std::fs::remove_dir_all(root)?;
    Ok(())
}
