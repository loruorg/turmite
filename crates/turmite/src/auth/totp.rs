//! TOTP (RFC 6238): HMAC-SHA1, 30-second steps, 6 digits.

use hmac::{Hmac, KeyInit, Mac};
use rand::RngExt;
use sha1::Sha1;

const STEP: u64 = 30;
const DIGITS: u32 = 6;

pub fn new_secret() -> Vec<u8> {
    let mut secret = vec![0u8; 20];
    rand::rng().fill(&mut secret[..]);
    secret
}

fn code_at(secret: &[u8], step: u64) -> u32 {
    let mut mac = <Hmac<Sha1>>::new_from_slice(secret).expect("any key length");
    mac.update(&step.to_be_bytes());
    let hash = mac.finalize().into_bytes();
    let offset = (hash[hash.len() - 1] & 0x0f) as usize;
    let bin = u32::from_be_bytes([hash[offset] & 0x7f, hash[offset + 1], hash[offset + 2], hash[offset + 3]]);
    bin % 10u32.pow(DIGITS)
}

/// The 6-digit code for a time (zero-padded).
pub fn code(secret: &[u8], unix_secs: u64) -> String {
    format!("{:06}", code_at(secret, unix_secs / STEP))
}

pub fn current_step(unix_secs: u64) -> u64 {
    unix_secs / STEP
}

/// Returns the matched step if `code` is valid within one step of `now` and newer than
/// `last_step` (a code works once).
pub fn verify(secret: &[u8], code: &str, unix_secs: u64, last_step: u64) -> Option<u64> {
    let code = code.trim().replace(' ', "");
    if code.len() != DIGITS as usize || !code.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let want: u32 = code.parse().ok()?;
    let now = current_step(unix_secs);
    [now.saturating_sub(1), now, now + 1]
        .into_iter()
        .filter(|s| *s > last_step)
        .find(|s| crate::crypto::ct_eq(&code_at(secret, *s).to_be_bytes(), &want.to_be_bytes()))
}

pub fn base32(data: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new();
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in data {
        buffer = (buffer << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((buffer >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// `otpauth://` URI for authenticator apps.
pub fn otpauth_uri(secret: &[u8], issuer: &str, account: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20");
    format!(
        "otpauth://totp/{}:{}?secret={}&issuer={}&algorithm=SHA1&digits=6&period=30",
        enc(issuer),
        enc(account),
        base32(secret),
        enc(issuer)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc6238_vectors() {
        // RFC 6238 appendix B, SHA1 secret "12345678901234567890", 8 digits truncated to 6.
        let secret = b"12345678901234567890";
        assert_eq!(code_at(secret, 59 / 30), 287082);
        assert_eq!(code_at(secret, 1111111109 / 30), 81804);
        assert_eq!(code_at(secret, 1234567890 / 30), 5924);
    }

    #[test]
    fn verify_once_only() {
        let secret = b"12345678901234567890";
        let t = 1234567890;
        let step = verify(secret, "005924", t, 0).unwrap();
        assert!(verify(secret, "005924", t, step).is_none());
        assert!(verify(secret, "123456", t, 0).is_none());
    }

    #[test]
    fn base32_known() {
        assert_eq!(base32(b"foobar"), "MZXW6YTBOI");
    }
}
