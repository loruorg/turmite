//! Hot wallets: keys that pay network fees for sweeps and send payouts.
//! Private keys are age-encrypted to the install recipient; only the worker decrypts them.

use sqlx::Row;
use zeroize::Zeroizing;

use crate::{
    app::App,
    crypto,
    error::{Error, Result},
};

/// An unlocked hot key. `secret` is the 32-byte private key (secp256k1 for EVM and Tron,
/// ed25519 seed for Solana and TON).
pub struct HotKey {
    pub hot_wallet_id: i64,
    pub family: String,
    pub address: String,
    pub secret: Zeroizing<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct HotWalletRow {
    pub hot_wallet_id: i64,
    pub family: String,
    pub label: String,
    pub address: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Store a hot key, encrypted to the install recipient. Works in the API process.
pub async fn store(app: &App, family: &str, label: &str, address: &str, secret: &[u8], created_by: Option<i64>) -> Result<i64> {
    let recipient = app.hot_recipient().ok_or_else(|| Error::BadRequest("TURMITE_HOT_RECIPIENT is not set. Run `turmite init`.".into()))?;
    let ct = crypto::hot_encrypt(&recipient, secret)?;
    let res = sqlx::query_scalar(
        "INSERT INTO hot_wallets (family, label, address, key_age, created_by) VALUES ($1,$2,$3,$4,$5) RETURNING hot_wallet_id",
    )
    .bind(family)
    .bind(label)
    .bind(address)
    .bind(ct)
    .bind(created_by)
    .fetch_one(&app.store.pool)
    .await;
    match res {
        Ok(id) => Ok(id),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => Err(Error::Conflict("This hot wallet exists.".into())),
        Err(e) => Err(e.into()),
    }
}

pub async fn list(app: &App) -> Result<Vec<HotWalletRow>> {
    let rows = sqlx::query("SELECT hot_wallet_id, family, label, address, created_at FROM hot_wallets ORDER BY family, hot_wallet_id")
        .fetch_all(&app.store.pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| HotWalletRow {
            hot_wallet_id: r.get("hot_wallet_id"),
            family: r.get("family"),
            label: r.get("label"),
            address: r.get("address"),
            created_at: r.get("created_at"),
        })
        .collect())
}

pub async fn assign(app: &App, project_id: i64, family: &str, hot_wallet_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO project_hot_wallets (project_id, family, hot_wallet_id) VALUES ($1,$2,$3) \
         ON CONFLICT (project_id, family) DO UPDATE SET hot_wallet_id = EXCLUDED.hot_wallet_id",
    )
    .bind(project_id)
    .bind(family)
    .bind(hot_wallet_id)
    .execute(&app.store.pool)
    .await?;
    Ok(())
}

/// Address of the hot wallet a project uses for a family, if any.
pub async fn address_for_project(app: &App, project_id: i64, family: &str) -> Result<Option<(i64, String)>> {
    let row = sqlx::query(
        "SELECT h.hot_wallet_id, h.address FROM project_hot_wallets p JOIN hot_wallets h ON h.hot_wallet_id = p.hot_wallet_id \
         WHERE p.project_id = $1 AND p.family = $2",
    )
    .bind(project_id)
    .bind(family)
    .fetch_optional(&app.store.pool)
    .await?;
    Ok(row.map(|r| (r.get("hot_wallet_id"), r.get("address"))))
}

/// Remove a project's hot wallet for a family.
pub async fn unassign(app: &App, project_id: i64, family: &str) -> Result<()> {
    sqlx::query("DELETE FROM project_hot_wallets WHERE project_id = $1 AND family = $2")
        .bind(project_id)
        .bind(family)
        .execute(&app.store.pool)
        .await?;
    Ok(())
}

/// (project_id, family, hot_wallet_id) of every assignment.
pub async fn assignments(app: &App) -> Result<Vec<(i64, String, i64)>> {
    let rows = sqlx::query("SELECT project_id, family, hot_wallet_id FROM project_hot_wallets ORDER BY project_id, family")
        .fetch_all(&app.store.pool)
        .await?;
    Ok(rows.iter().map(|r| (r.get("project_id"), r.get("family"), r.get("hot_wallet_id"))).collect())
}

pub async fn get(app: &App, hot_wallet_id: i64) -> Result<HotWalletRow> {
    list(app).await?.into_iter().find(|w| w.hot_wallet_id == hot_wallet_id).ok_or(Error::NotFound)
}

/// Address of a 32-byte secret: EIP-55 (EVM), base58check T-address (Tron), base58 (Solana),
/// non-bounceable wallet v4r2 address (TON).
pub fn derive_address(family: &str, secret: &[u8], testnet: bool) -> Result<String> {
    let _ = testnet;
    match family {
        #[cfg(feature = "evm")]
        "evm" => crate::payouts::evm::address_from_secret(secret),
        #[cfg(feature = "tron")]
        "tron" => crate::payouts::tron::address_from_secret(secret),
        #[cfg(feature = "sol")]
        "solana" => crate::payouts::sol::address_from_secret(secret),
        #[cfg(feature = "ton")]
        "ton" => crate::payouts::ton::address_from_secret(secret, testnet),
        other => Err(Error::BadRequest(format!("This build has no hot wallets for `{other}`."))),
    }
}

/// A new random secret for a family. For secp256k1 the bytes must be a valid scalar.
pub fn generate_secret(family: &str) -> Result<Zeroizing<Vec<u8>>> {
    for _ in 0..8 {
        let bytes = Zeroizing::new(hex::decode(crypto::random_hex(32)).map_err(|e| Error::Internal(e.to_string()))?);
        if derive_address(family, &bytes, true).is_ok() {
            return Ok(bytes);
        }
    }
    Err(Error::Internal("could not make a key".into()))
}

/// Parse an imported key. EVM and Tron: 64 hex characters. Solana: a base58 keypair (64 bytes), a JSON byte
/// array (keypair file) or a 32-byte hex seed. TON: a 24-word mnemonic or a 32-byte hex seed.
pub fn parse_secret(family: &str, input: &str) -> Result<Zeroizing<Vec<u8>>> {
    let input = Zeroizing::new(input.trim().to_string());
    let hex32 = |s: &str| -> Option<Zeroizing<Vec<u8>>> {
        let s = s.strip_prefix("0x").unwrap_or(s);
        (s.len() == 64).then(|| hex::decode(s).ok().map(Zeroizing::new)).flatten()
    };
    let bad = || Error::BadRequest("The key is not in a format Turmite reads. See the hint below the field.".into());
    let secret = match family {
        "evm" | "tron" => hex32(&input).ok_or_else(bad)?,
        #[cfg(feature = "sol")]
        "solana" => {
            if let Some(s) = hex32(&input) {
                s
            } else if input.starts_with('[') {
                let bytes: Zeroizing<Vec<u8>> = Zeroizing::new(serde_json::from_str(&input).map_err(|_| bad())?);
                keypair_seed(&bytes).ok_or_else(bad)?
            } else {
                let bytes = Zeroizing::new(bs58::decode(input.as_str()).into_vec().map_err(|_| bad())?);
                keypair_seed(&bytes).ok_or_else(bad)?
            }
        }
        #[cfg(feature = "ton")]
        "ton" => match hex32(&input) {
            Some(s) => s,
            None => Zeroizing::new(crate::payouts::ton::seed_from_mnemonic(&input)?.to_vec()),
        },
        _ => return Err(bad()),
    };
    derive_address(family, &secret, true)?;
    Ok(secret)
}

/// The seed of a 64-byte Solana keypair (seed ++ public key), checked against the public key.
#[cfg(feature = "sol")]
fn keypair_seed(bytes: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if bytes.len() != 64 {
        return None;
    }
    let seed = Zeroizing::new(bytes[..32].to_vec());
    let addr = crate::payouts::sol::address_from_secret(&seed).ok()?;
    if bs58::decode(addr).into_vec().ok()? != bytes[32..] {
        return None;
    }
    Some(seed)
}

/// Unlock a hot key by id. Worker only.
pub async fn key_by_id(app: &App, hot_wallet_id: i64) -> Result<HotKey> {
    let r = sqlx::query("SELECT hot_wallet_id, family, address, key_age FROM hot_wallets WHERE hot_wallet_id = $1")
        .bind(hot_wallet_id)
        .fetch_optional(&app.store.pool)
        .await?
        .ok_or(Error::NotFound)?;
    let identity = app.hot_identity().ok_or_else(|| Error::Internal("hot keys can only be unlocked by the worker".into()))?;
    let ct: Vec<u8> = r.get("key_age");
    Ok(HotKey { hot_wallet_id: r.get("hot_wallet_id"), family: r.get("family"), address: r.get("address"), secret: identity.decrypt(&ct)? })
}

/// Unlock the hot key a project uses for a family. Worker only.
pub async fn key_for_project(app: &App, project_id: i64, family: &str) -> Result<Option<HotKey>> {
    let row = sqlx::query(
        "SELECT h.hot_wallet_id, h.family, h.address, h.key_age FROM project_hot_wallets p \
         JOIN hot_wallets h ON h.hot_wallet_id = p.hot_wallet_id WHERE p.project_id = $1 AND p.family = $2",
    )
    .bind(project_id)
    .bind(family)
    .fetch_optional(&app.store.pool)
    .await?;
    let Some(r) = row else { return Ok(None) };
    let identity = app.hot_identity().ok_or_else(|| Error::Internal("hot keys can only be unlocked by the worker".into()))?;
    let ct: Vec<u8> = r.get("key_age");
    Ok(Some(HotKey {
        hot_wallet_id: r.get("hot_wallet_id"),
        family: r.get("family"),
        address: r.get("address"),
        secret: identity.decrypt(&ct)?,
    }))
}

#[cfg(all(test, feature = "all-chains"))]
mod tests {
    use super::*;

    const ANVIL_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

    #[test]
    fn secp256k1_keys() {
        let a = parse_secret("evm", ANVIL_KEY).unwrap();
        let b = parse_secret("evm", &format!("  0x{ANVIL_KEY}\n")).unwrap();
        assert_eq!(*a, *b);
        assert_eq!(derive_address("evm", &a, false).unwrap(), "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
        assert!(derive_address("tron", &a, false).unwrap().starts_with('T'));
        assert!(parse_secret("evm", "abc").is_err());
        assert!(parse_secret("evm", &"0".repeat(64)).is_err(), "zero is not a valid key");
    }

    #[test]
    fn solana_key_formats() {
        let seed = hex::decode("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60").unwrap();
        let address = derive_address("solana", &seed, false).unwrap();
        let mut keypair = seed.clone();
        keypair.extend(bs58::decode(&address).into_vec().unwrap());
        let from_b58 = parse_secret("solana", &bs58::encode(&keypair).into_string()).unwrap();
        let from_json = parse_secret("solana", &serde_json::to_string(&keypair).unwrap()).unwrap();
        let from_hex = parse_secret("solana", &hex::encode(&seed)).unwrap();
        assert_eq!(*from_b58, seed);
        assert_eq!(*from_json, seed);
        assert_eq!(*from_hex, seed);
        // A keypair whose public half does not match is refused.
        keypair[40] ^= 1;
        assert!(parse_secret("solana", &bs58::encode(&keypair).into_string()).is_err());
    }

    #[test]
    fn ton_mnemonic() {
        let words = "fancy carpet hello mandate penalty trial consider property top vicious exit rebuild tragic profit urban major total month holiday sudden rib gather media vicious";
        let seed = parse_secret("ton", words).unwrap();
        assert_eq!(seed.len(), 32);
        let a = derive_address("ton", &seed, true).unwrap();
        assert!(a.starts_with("0Q"), "{a}");
        assert!(parse_secret("ton", "not a mnemonic").is_err());
    }

    #[test]
    fn generated_keys_have_addresses() {
        for f in ["evm", "tron", "solana", "ton"] {
            let s = generate_secret(f).unwrap();
            assert_eq!(s.len(), 32);
            assert!(derive_address(f, &s, true).is_ok());
        }
        assert!(generate_secret("bitcoin").is_err());
    }
}
