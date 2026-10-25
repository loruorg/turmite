//! Dashboard authentication: passwords, TOTP, rate limits.

pub mod jwt;
pub mod limits;
pub mod totp;

use std::sync::{Arc, LazyLock};

use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use tokio::sync::Semaphore;

use crate::error::{Error, Result};

/// At most four hashes at once, so a login flood cannot exhaust memory.
static HASH_SLOTS: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(4)));

fn argon2() -> Argon2<'static> {
    // 64 MiB, 3 passes, 1 lane.
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::new(64 * 1024, 3, 1, None).expect("valid params"))
}

pub const MIN_PASSWORD_LEN: usize = 12;

pub fn check_password_policy(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(Error::BadRequest(format!("Use a password of at least {MIN_PASSWORD_LEN} characters.")));
    }
    if password.len() > 1024 {
        return Err(Error::BadRequest("Password is too long.".into()));
    }
    Ok(())
}

pub async fn hash_password(password: &str) -> Result<String> {
    let _slot = HASH_SLOTS.clone().acquire_owned().await.map_err(|e| Error::Internal(e.to_string()))?;
    let password = password.to_owned();
    tokio::task::spawn_blocking(move || {
        argon2().hash_password(password.as_bytes()).map(|h| h.to_string()).map_err(|e| Error::Internal(format!("hash password: {e}")))
    })
    .await
    .map_err(|e| Error::Internal(e.to_string()))?
}

pub async fn verify_password(password: &str, hash: &str) -> bool {
    let Ok(_slot) = HASH_SLOTS.clone().acquire_owned().await else {
        return false;
    };
    let password = password.to_owned();
    let hash = hash.to_owned();
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash).is_ok_and(|parsed| argon2().verify_password(password.as_bytes(), &parsed).is_ok())
    })
    .await
    .unwrap_or(false)
}

/// A hash to verify against when the email is unknown, so timing does not reveal accounts.
pub static DUMMY_HASH: LazyLock<String> =
    LazyLock::new(|| argon2().hash_password(b"turmite-dummy-password").map(|h| h.to_string()).unwrap_or_default());

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn password_round_trip() {
        let h = hash_password("a long enough password").await.unwrap();
        assert!(verify_password("a long enough password", &h).await);
        assert!(!verify_password("wrong password here", &h).await);
        assert!(check_password_policy("short").is_err());
    }
}
