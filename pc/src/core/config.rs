use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{net::IpAddr, path::PathBuf};
#[derive(Clone, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub secret_hex: String,
    pub last_ip: Option<IpAddr>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub id: String,
    pub name: String,
    pub download_dir: PathBuf,
    pub peers: Vec<Peer>,
}
pub fn directory() -> PathBuf {
    std::env::var_os("DROIDTRANSFER_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("DroidTransfer")
        })
}
impl Config {
    pub fn load() -> Result<(Self, PathBuf)> {
        Self::load_from(directory())
    }
    fn load_from(dir: PathBuf) -> Result<(Self, PathBuf)> {
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("config.json");
        let loaded = if path.exists() {
            match serde_json::from_slice(&std::fs::read(&path)?) {
                Ok(config) => Some(config),
                Err(e) => {
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)?
                        .as_secs();
                    let backup = dir.join(format!("config.json.bad-{ts}"));
                    std::fs::rename(&path, &backup)?;
                    eprintln!(
                        "Invalid config {}: {e}; moved to {}; using fresh defaults",
                        path.display(),
                        backup.display()
                    );
                    None
                }
            }
        } else {
            None
        };
        let mut config = loaded.unwrap_or_else(|| Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into()),
            download_dir: dirs::download_dir()
                .or_else(|| dirs::home_dir().map(|p| p.join("Downloads")))
                .unwrap_or_else(|| PathBuf::from("Downloads"))
                .join("DroidTransfer"),
            peers: vec![],
        });
        // A damaged pairing only costs that one phone a re-pair; never block startup over it.
        config.peers.retain(|p| {
            let ok = hex::decode(&p.secret_hex).is_ok_and(|s| s.len() == 32);
            if !ok {
                eprintln!("Dropping pairing for {} ({}): invalid stored secret", p.name, p.id);
            }
            ok
        });
        config.save(&path)?;
        Ok((config, path))
    }
    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let tmp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&serde_json::to_vec_pretty(self)?)?;
            f.sync_all()?;
        } // Windows rename does not replace an existing destination.
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn MoveFileExW(a: *const u16, b: *const u16, flags: u32) -> i32;
            }
            let a: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
            let b: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe { MoveFileExW(a.as_ptr(), b.as_ptr(), 0x1 | 0x8) } == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(windows))]
        std::fs::rename(tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn corrupt_config_is_preserved_and_replaced() -> Result<()> {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("config-recovery-{}", uuid::Uuid::new_v4()));
        let (original, path) = Config::load_from(dir.clone())?;
        let bad = b"{broken config";
        std::fs::write(&path, bad)?;
        let (fresh, _) = Config::load_from(dir.clone())?;
        assert_ne!(fresh.id, original.id);
        assert!(fresh.peers.is_empty());
        let backup = std::fs::read_dir(&dir)?
            .map(|e| e.unwrap().path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("config.json.bad-")
            })
            .expect("corrupt config backup");
        assert_eq!(std::fs::read(backup)?, bad);
        assert_eq!(Config::load_from(dir.clone())?.0.id, fresh.id);
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
