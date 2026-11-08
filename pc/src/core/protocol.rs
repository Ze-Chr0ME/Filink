//! Byte-exact DroidTransfer v1 framing and authentication.
use anyhow::{bail, ensure, Result};
use hmac::{Hmac, Mac};
use rand::{rngs::OsRng, RngCore};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
pub const HELLO: u8 = 1;
pub const HELLO_ACK: u8 = 2;
pub const AUTH: u8 = 3;
pub const AUTH_OK: u8 = 4;
pub const ERROR: u8 = 5;
pub const PAIR_REQ: u8 = 6;
pub const PAIR_OK: u8 = 7;
pub const TEXT: u8 = 0x10;
pub const TEXT_ACK: u8 = 0x11;
pub const BATCH_START: u8 = 0x20;
pub const FILE_START: u8 = 0x21;
pub const DATA: u8 = 0x22;
pub const FILE_END: u8 = 0x23;
pub const BATCH_END: u8 = 0x24;
pub const BATCH_ACK: u8 = 0x25;
pub const CHUNK: usize = 1024 * 1024;
pub fn read_frame(r: &mut impl Read) -> Result<(u8, Vec<u8>)> {
    let mut h = [0; 5];
    r.read_exact(&mut h)?;
    let n = u32::from_be_bytes(h[1..].try_into()?) as usize;
    ensure!(n <= 16 * CHUNK, "frame too large");
    let mut p = vec![0; n];
    r.read_exact(&mut p)?;
    Ok((h[0], p))
}
pub fn write_frame(w: &mut impl Write, t: u8, p: &[u8]) -> Result<()> {
    ensure!(p.len() <= 16 * CHUNK, "frame too large");
    w.write_all(&[t])?;
    w.write_all(&(p.len() as u32).to_be_bytes())?;
    w.write_all(p)?;
    w.flush()?;
    Ok(())
}
pub fn send(w: &mut impl Write, t: u8, v: Value) -> Result<()> {
    write_frame(w, t, &serde_json::to_vec(&v)?)
}
pub fn recv(r: &mut impl Read, t: u8) -> Result<Value> {
    let (k, p) = read_frame(r)?;
    if k == ERROR {
        bail!(
            "{}",
            serde_json::from_slice::<Value>(&p)?["msg"]
                .as_str()
                .unwrap_or("peer error")
        );
    }
    ensure!(k == t, "unexpected frame {k:#x}, expected {t:#x}");
    let v: Value = serde_json::from_slice(&p)?;
    ensure!(v.is_object(), "expected JSON object");
    Ok(v)
}
pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0; N];
    OsRng.fill_bytes(&mut b);
    b
}
pub fn nonce(v: &Value) -> Result<[u8; 16]> {
    let s = v["nonce"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing nonce"))?;
    ensure!(
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid nonce"
    );
    hex::decode(s)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid nonce"))
}
pub fn mac(key: &[u8], role: &str, c: &[u8; 16], s: &[u8; 16]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC key");
    m.update(role.as_bytes());
    m.update(c);
    m.update(s);
    m.finalize().into_bytes().to_vec()
}
pub fn verify(key: &[u8], role: &str, c: &[u8; 16], s: &[u8; 16], value: &Value) -> Result<()> {
    let bytes = hex::decode(value["mac"].as_str().unwrap_or(""))?;
    let mut m = Hmac::<Sha256>::new_from_slice(key)?;
    m.update(role.as_bytes());
    m.update(c);
    m.update(s);
    m.verify_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("auth failed"))
}
pub fn code(c: &[u8; 16], s: &[u8; 16]) -> String {
    let mut h = Sha256::new();
    h.update(c);
    h.update(s);
    format!(
        "{:06}",
        u32::from_be_bytes(h.finalize()[..4].try_into().unwrap()) % 1_000_000
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fixed_vectors() {
        // Independently computed using .NET SHA256/HMACSHA256; key=00..1f,
        // cNonce=00..0f, sNonce=10..1f; message=ASCII("DT1-C") || cNonce || sNonce.
        let c = std::array::from_fn(|i| i as u8);
        let s = std::array::from_fn(|i| (i + 16) as u8);
        let key: Vec<_> = (0..32).collect();
        assert_eq!(code(&c, &s), "848873");
        assert_eq!(
            hex::encode(mac(&key, "DT1-C", &c, &s)),
            "f6fb1513c4b3b89e2540086ed31e4134e732e7e55acade3d7d0ff8b728f6f5e2"
        );
    }
    #[test]
    fn oversized_frame() {
        assert!(read_frame(&mut &[1, 1, 0, 0, 1][..]).is_err());
    }
}
