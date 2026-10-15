//! What an update's signature covers, and checking it. Shared word for word
//! with the signing tool (`examples/strata-sign.rs` includes this file), so
//! the app and the tool can't disagree about the signed message.
//!
//! Each platform's download in `latest.json` carries an ed25519 signature
//! over a short message naming the platform, the version, the SHA-256 of the
//! file and its size. Signing the hash rather than the file keeps the
//! message small; binding the version and platform means a genuine older
//! download can't be passed off as a newer version, or a Windows zip as a
//! Mac one.

use sha2::{Digest, Sha256};
use std::io::Read;

/// First line of every signed message; a new format gets a new line.
pub const DOMAIN: &str = "strata-update-v1";

/// The bytes signed for one download.
pub fn message(platform: &str, version: &str, sha256_hex: &str, size: u64) -> Vec<u8> {
    format!("{DOMAIN}\n{platform}\n{version}\n{sha256_hex}\n{size}\n").into_bytes()
}

/// Check `signature_b64` (64 bytes, base64) over `msg` with `public_key`.
/// Strict verification: no malleable or small-order signatures.
pub fn verify(public_key: &[u8; 32], msg: &[u8], signature_b64: &str) -> Result<(), String> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(public_key).map_err(|_| "the built-in update key is invalid".to_owned())?;
    let raw = b64_decode(signature_b64.trim()).ok_or("the signature isn't valid base64")?;
    let raw: [u8; 64] = raw.try_into().map_err(|_| "the signature isn't 64 bytes".to_owned())?;
    let sig = ed25519_dalek::Signature::from_bytes(&raw);
    key.verify_strict(msg, &sig).map_err(|_| "the signature doesn't match".to_owned())
}

/// SHA-256 (lowercase hex) and length of everything `r` yields.
pub fn sha256_hex(mut r: impl Read) -> std::io::Result<(String, u64)> {
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut len = 0u64;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        len += n as u64;
    }
    Ok((hex(&h.finalize()), len))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Standard base64; padding required, nothing else allowed (not even
/// whitespace inside). None for anything malformed.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.as_bytes();
    if s.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    for (ci, chunk) in s.chunks(4).enumerate() {
        let last = ci == s.len() / 4 - 1;
        let pad = chunk.iter().rev().take_while(|&&c| c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            let v = if i >= 4 - pad {
                0
            } else {
                match c {
                    b'A'..=b'Z' => c - b'A',
                    b'a'..=b'z' => c - b'a' + 26,
                    b'0'..=b'9' => c - b'0' + 52,
                    b'+' => 62,
                    b'/' => 63,
                    _ => return None,
                }
            };
            n = n << 6 | v as u32;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        // Bits under the padding must be zero (one encoding per value).
        if (pad == 1 && bytes[2] != 0) || (pad == 2 && bytes[1] != 0) {
            return None;
        }
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn keypair() -> (SigningKey, [u8; 32]) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key().to_bytes();
        (sk, pk)
    }

    #[test]
    fn base64_round_trips_and_rejects_junk() {
        for data in [&b""[..], b"f", b"fo", b"foo", b"foob", b"fooba", b"foobar", &[0, 255, 128, 7, 9]] {
            let e = b64_encode(data);
            assert_eq!(b64_decode(&e).as_deref(), Some(data), "{e}");
        }
        assert_eq!(b64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(b64_encode(b"fo"), "Zm8=");
        assert_eq!(b64_decode("Zm8="), Some(b"fo".to_vec()));
        for bad in ["Zm8", "Zm9=v", "Zm===", "Zm8=Zm8=", "Z m8=", "Zm9-", "Zm9=", "Zg=A"] {
            assert_eq!(b64_decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn sha256_of_known_input() {
        let (h, n) = sha256_hex(&b"abc"[..]).unwrap();
        assert_eq!(h, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(n, 3);
    }

    #[test]
    fn a_signature_covers_platform_version_hash_and_size() {
        let (sk, pk) = keypair();
        let hash = "ab".repeat(32);
        let msg = message("macos-universal", "1.2.0", &hash, 1000);
        let sig = b64_encode(&sk.sign(&msg).to_bytes());
        assert_eq!(verify(&pk, &msg, &sig), Ok(()));
        // Anything else signed with the same signature fails.
        for other in [
            message("macos-arm64", "1.2.0", &hash, 1000),
            message("macos-universal", "1.3.0", &hash, 1000),
            message("macos-universal", "1.2.0", &"cd".repeat(32), 1000),
            message("macos-universal", "1.2.0", &hash, 1001),
        ] {
            assert!(verify(&pk, &other, &sig).is_err());
        }
        // A tampered signature, the wrong key, junk.
        let mut raw = b64_decode(&sig).unwrap();
        raw[10] ^= 1;
        assert!(verify(&pk, &msg, &b64_encode(&raw)).is_err());
        let other_key = SigningKey::from_bytes(&[9u8; 32]).verifying_key().to_bytes();
        assert!(verify(&other_key, &msg, &sig).is_err());
        assert!(verify(&pk, &msg, "not base64!").is_err());
        assert!(verify(&pk, &msg, &b64_encode(&[0u8; 63])).is_err());
    }
}
