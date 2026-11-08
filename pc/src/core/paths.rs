use anyhow::{ensure, Result};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};
pub fn sanitize(path: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for s in path.split(['/', '\\']) {
        if s.is_empty() || s == "." {
            continue;
        }
        ensure!(s != ".." && !s.contains([':', '\0']), "unsafe path");
        let s: String = s
            .chars()
            .map(|c| {
                if c.is_control() || "<>\"|?*".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect(); // Windows trims trailing dots/spaces and recognizes device basenames.
        ensure!(!s.ends_with(['.', ' ']), "unsafe trailing path character");
        let base = s.split('.').next().unwrap_or("").to_ascii_uppercase();
        ensure!(
            ![
                "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
                "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8",
                "LPT9"
            ]
            .contains(&base.as_str()),
            "reserved path"
        );
        out.push(s);
    }
    ensure!(!out.as_os_str().is_empty(), "empty path");
    Ok(out)
}
pub fn create(root: &Path, relative: &Path) -> Result<(PathBuf, File)> {
    std::fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let mut parent = root.clone();
    let parts: Vec<_> = relative.components().collect();
    for p in &parts[..parts.len() - 1] {
        parent.push(p.as_os_str());
        if !parent.exists() {
            std::fs::create_dir(&parent)?;
        }
        ensure!(
            !std::fs::symlink_metadata(&parent)?.file_type().is_symlink(),
            "symlink destination"
        );
        ensure!(
            parent.canonicalize()?.starts_with(&root),
            "path outside download directory"
        );
    }
    let name = Path::new(parts.last().unwrap().as_os_str());
    let stem = name.file_stem().unwrap().to_string_lossy();
    let ext = name
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for i in 0u64.. {
        let p = parent.join(if i == 0 {
            name.to_string_lossy().into_owned()
        } else {
            format!("{stem} ({i}){ext}")
        });
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    unreachable!()
}
pub fn collect(
    paths: Vec<PathBuf>,
    check_cancel: impl Fn() -> Result<()>,
) -> Result<Vec<(PathBuf, String, u64, u64)>> {
    fn visit(
        p: PathBuf,
        rel: String,
        out: &mut Vec<(PathBuf, String, u64, u64)>,
        check_cancel: &impl Fn() -> Result<()>,
    ) -> Result<()> {
        check_cancel()?;
        let m =
            std::fs::symlink_metadata(&p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
        check_cancel()?;
        if m.file_type().is_symlink() {
            return Ok(());
        }
        if m.is_dir() {
            let entries =
                std::fs::read_dir(&p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
            let mut entries = entries;
            loop {
                check_cancel()?;
                let Some(entry) = entries.next() else { break };
                let entry = entry.map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                visit(
                    entry.path(),
                    format!("{rel}/{}", entry.file_name().to_string_lossy()),
                    out,
                    check_cancel,
                )?;
            }
        } else if m.is_file() {
            let mt = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            out.push((p, rel, m.len(), mt));
        }
        Ok(())
    }
    let mut out = vec![];
    for p in paths {
        let name = p
            .file_name()
            .unwrap_or(p.as_os_str())
            .to_string_lossy()
            .into_owned();
        visit(p, name, &mut out, &check_cancel)?;
    }
    check_cancel()?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_interrupts_walk_between_entries() -> Result<()> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("walk-cancel-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root)?;
        for n in 0..10 {
            std::fs::write(root.join(n.to_string()), b"data")?;
        }
        let checks = std::cell::Cell::new(0);
        let result = collect(vec![root.clone()], || {
            checks.set(checks.get() + 1);
            ensure!(checks.get() < 6, "cancelled");
            Ok(())
        });
        assert_eq!(result.unwrap_err().to_string(), "cancelled");
        assert_eq!(checks.get(), 6);
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn windows_paths() {
        assert_eq!(
            sanitize("/folder//./sub\\a?.txt").unwrap(),
            PathBuf::from("folder/sub/a_.txt")
        );
        for p in ["../x", "a/../x", "C:/x", "a\0b", "", ".", "CON.txt", "a/b."] {
            assert!(sanitize(p).is_err(), "{p}");
        }
    }
}
