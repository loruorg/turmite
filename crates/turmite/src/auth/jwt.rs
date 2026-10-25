//! Short-lived bearer tokens for the payout API (`POST /v1/auth`).

use chrono::{Duration, Utc};
use hkdf::Hkdf;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::error::{Error, Result};

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: i64,
    iat: i64,
}

fn key(secret: &[u8; 32]) -> [u8; 32] {
    let mut k = [0u8; 32];
    Hkdf::<Sha256>::new(Some(b"turmite-jwt-v1"), secret).expand(b"jwt", &mut k).expect("valid length");
    k
}

pub fn issue(secret: &[u8; 32], user_id: i64, ttl: Duration) -> Result<String> {
    let now = Utc::now();
    let claims = Claims { sub: user_id.to_string(), iat: now.timestamp(), exp: (now + ttl).timestamp() };
    encode(&Header::new(Algorithm::HS256), &claims, &EncodingKey::from_secret(&key(secret)))
        .map_err(|e| Error::Internal(format!("jwt: {e}")))
}

/// The user id in a valid, unexpired token.
pub fn verify(secret: &[u8; 32], token: &str) -> Result<i64> {
    let mut v = Validation::new(Algorithm::HS256);
    v.leeway = 5;
    decode::<Claims>(token.trim(), &DecodingKey::from_secret(&key(secret)), &v)
        .ok()
        .and_then(|d| d.claims.sub.parse().ok())
        .ok_or_else(|| Error::Unauthorized("Invalid or expired token. Get a new one from POST /v1/auth.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = [3u8; 32];
        let t = issue(&s, 42, Duration::minutes(5)).unwrap();
        assert_eq!(verify(&s, &t).unwrap(), 42);
        assert!(verify(&[4u8; 32], &t).is_err());
    }
}
