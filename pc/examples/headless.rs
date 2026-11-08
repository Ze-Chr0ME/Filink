use droidtransfer::core::*;
use std::{thread, time::Duration};
fn main() -> anyhow::Result<()> {
    let core = Core::start(Box::new(|| {}))?;
    let mut previous = String::new();
    loop {
        let s = core.snapshot();
        for p in &s.pair_requests {
            println!("Pair {}: code {} (auto-accept)", p.peer_name, p.code);
            core.respond_pair(p.id, true);
        }
        let status = format!(
            "peers: {:?}\ntransfers: {:?}\ntexts: {:?}\nserver error: {:?}",
            s.peers
                .iter()
                .map(|p| (&p.name, p.online, p.paired))
                .collect::<Vec<_>>(),
            s.transfers
                .iter()
                .map(|t| (
                    &t.current_file,
                    t.bytes_done,
                    t.files_done,
                    match &t.state {
                        TransferState::Running => "Running".to_owned(),
                        TransferState::Done => "Done".to_owned(),
                        TransferState::Cancelled => "Cancelled".to_owned(),
                        TransferState::Failed(e) => format!("Failed: {e}"),
                    }
                ))
                .collect::<Vec<_>>(),
            s.texts
                .iter()
                .map(|t| (&t.peer_name, &t.text, t.incoming))
                .collect::<Vec<_>>(),
            s.server_error
        );
        if status != previous {
            println!("{status}");
            previous = status;
        }
        thread::sleep(Duration::from_millis(100));
    }
}
