//! Deposit address allocation.

use crate::{
    app::App,
    chains::driver::{IndexScope, WalletContext},
    domain::AssetNetwork,
    error::{Error, Result},
};

/// The project's wallet on a network, with sealed fields opened.
pub async fn wallet_context(app: &App, project_id: i64, network_id: i64) -> Result<WalletContext> {
    let Some(w) = app.store.project_wallet(project_id, network_id).await? else {
        return Ok(WalletContext { project_id, ..Default::default() });
    };
    let view_key = match app.store.project_wallet_view_key(project_id, network_id).await? {
        Some(sealed) => Some(app.sealer.open_str("view_key", &sealed)?),
        None => None,
    };
    Ok(WalletContext {
        project_id,
        address: w.address,
        address_extra_id: w.address_extra_id,
        xpub: w.xpub,
        view_key,
        restore_height: w.restore_height,
        created_at: Some(w.created_at),
        factory_address: w.factory_address,
        hot_secret: None,
        hot_address: None,
    })
}

/// A deposit address reserved for a new payment.
#[derive(Clone, Debug)]
pub struct Allocation {
    pub deposit_address_id: Option<i64>,
    pub address: String,
    pub payin_extra_id: Option<String>,
    /// True when it came from the shared pool, false when it was derived for the project.
    from_pool: bool,
}

impl Allocation {
    /// Give the address back when no payment took it: a pool address goes back to the pool,
    /// a derived one is deleted (its index stays used). Nobody saw the address, so this is safe.
    pub async fn release(&self, app: &App) -> Result<()> {
        let Some(id) = self.deposit_address_id else { return Ok(()) };
        app.store.release_deposit_address(id, self.from_pool).await
    }
}

/// A deposit address for a new payment.
///
/// The driver derives it when it can in this process (node wallets, watch-only wallets,
/// deposit contracts, and seed-derived chains in the worker). Otherwise it comes from
/// the pool the worker fills.
pub async fn allocate(app: &App, project_id: i64, an: &AssetNetwork) -> Result<Allocation> {
    let network = &an.network;
    let driver = app.drivers().get(&network.chain);
    if let Some(driver) = driver.as_ref().filter(|d| d.can_derive()) {
        let w = wallet_context(app, project_id, network.network_id).await?;
        let index = match driver.index_scope() {
            IndexScope::Network => app.store.next_network_index(network.network_id).await?,
            IndexScope::Wallet => app.store.next_wallet_index(project_id, network.network_id).await?,
            IndexScope::Family => app.store.next_family_index(&network.family).await?,
        };
        let derived = driver.derive_deposit(&w, index as u64).await?;
        // A driver may assign its own index (Monero subaddresses).
        let index = derived.derivation_index as i64;
        let id = app
            .store
            .insert_deposit_address(network.network_id, Some(project_id), index, &derived.address, derived.payin_extra_id.as_deref())
            .await?;
        return Ok(Allocation {
            deposit_address_id: Some(id),
            address: derived.address,
            payin_extra_id: derived.payin_extra_id,
            from_pool: false,
        });
    }
    if let Some((id, address, payin_extra_id)) = app.store.take_pool_address(network.network_id, project_id).await? {
        return Ok(Allocation { deposit_address_id: Some(id), address, payin_extra_id, from_pool: true });
    }
    let seed_chain = crate::worker::pool::SEED_FAMILIES.contains(&network.family.as_str());
    if driver.is_some() || (seed_chain && app.store.list_nodes(Some(network.network_id)).await?.iter().any(|n| n.is_enabled)) {
        return Err(Error::Unavailable(format!(
            "No free {} deposit address right now. The worker adds addresses every {} seconds. Try again in a minute.",
            network.name,
            crate::worker::pool::REFILL_SECONDS
        )));
    }
    Err(Error::Unavailable(format!("{} is not connected. Add a node under Networks in the dashboard.", network.name)))
}

/// Check an address for a network: offline by format first (checksums, network prefixes, EIP-55),
/// then through the connected driver.
pub async fn validate(app: &App, network: &crate::domain::Network, address: &str) -> Result<bool> {
    if address.is_empty() || address.len() > 200 || address.chars().any(char::is_whitespace) {
        return Ok(false);
    }
    if !offline_format_ok(&network.family, !network.is_testnet, address) {
        return Ok(false);
    }
    if let Some(driver) = app.drivers().get(&network.chain)
        && !driver.is_mock()
    {
        return driver.validate_address(address).await;
    }
    Ok(true)
}

/// Format check that needs no node. `mainnet`: refuse test network addresses where the
/// format shows the network.
pub fn offline_format_ok(family: &str, mainnet: bool, a: &str) -> bool {
    match family {
        #[cfg(feature = "evm")]
        "evm" => crate::chains::evm::parse_address(a).is_some(),
        #[cfg(not(feature = "evm"))]
        "evm" => a.len() == 42 && a.starts_with("0x") && a[2..].chars().all(|c| c.is_ascii_hexdigit()),
        "tron" => a.len() == 34 && base58check(a).is_some_and(|p| p.len() == 21 && p[0] == 0x41),
        "solana" => (32..=44).contains(&a.len()) && base58_decode(a).is_some_and(|b| b.len() == 32),
        "monero" => crate::chains::xmr::address_ok(a, mainnet),
        "bitcoin" => {
            let (hrps, versions): (&[&str], [u8; 2]) = if mainnet { (&["bc"], [0x00, 0x05]) } else { (&["tb", "bcrt"], [0x6f, 0xc4]) };
            segwit_ok(a, hrps) || base58check(a).is_some_and(|p| p.len() == 21 && versions.contains(&p[0]))
        }
        "ton" => a.len() == 48 || (a.contains(':') && a.len() >= 66),
        _ => !a.is_empty(),
    }
}

const B58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Base58 (Bitcoin alphabet) to bytes. Each leading `1` is a zero byte.
fn base58_decode(s: &str) -> Option<Vec<u8>> {
    let mut num: Vec<u8> = Vec::new();
    for c in s.bytes() {
        let mut carry = B58.iter().position(|&b| b == c)? as u32;
        for byte in num.iter_mut().rev() {
            carry += *byte as u32 * 58;
            *byte = carry as u8;
            carry >>= 8;
        }
        while carry > 0 {
            num.insert(0, carry as u8);
            carry >>= 8;
        }
    }
    let mut out = vec![0u8; s.bytes().take_while(|&b| b == b'1').count()];
    out.extend(num);
    Some(out)
}

/// The payload of a base58check string (version byte first) when its checksum is correct.
fn base58check(s: &str) -> Option<Vec<u8>> {
    use sha2::{Digest, Sha256};
    let raw = base58_decode(s)?;
    if raw.len() < 5 {
        return None;
    }
    let (payload, sum) = raw.split_at(raw.len() - 4);
    (Sha256::digest(Sha256::digest(payload))[..4] == *sum).then(|| payload.to_vec())
}

/// A segwit address (BIP 173, BIP 350) with one of `hrps`, checksum included.
fn segwit_ok(a: &str, hrps: &[&str]) -> bool {
    const CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    const GEN: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];
    if a.len() > 90 || (a != a.to_ascii_lowercase() && a != a.to_ascii_uppercase()) {
        return false;
    }
    let a = a.to_ascii_lowercase();
    let Some((hrp, data)) = a.rsplit_once('1') else { return false };
    if !hrps.contains(&hrp) || data.len() < 7 {
        return false;
    }
    let Some(values) = data.bytes().map(|c| CHARSET.iter().position(|&x| x == c).map(|p| p as u8)).collect::<Option<Vec<u8>>>() else {
        return false;
    };
    let mut chk: u32 = 1;
    let expanded = hrp.bytes().map(|b| b >> 5).chain([0]).chain(hrp.bytes().map(|b| b & 31)).chain(values.iter().copied());
    for v in expanded {
        let top = chk >> 25;
        chk = ((chk & 0x01ff_ffff) << 5) ^ v as u32;
        for (i, g) in GEN.iter().enumerate() {
            if (top >> i) & 1 == 1 {
                chk ^= g;
            }
        }
    }
    let version = values[0];
    // Version 0 uses bech32, later versions bech32m.
    if version > 16 || chk != if version == 0 { 1 } else { 0x2bc8_30a3 } {
        return false;
    }
    let (mut acc, mut bits, mut program) = (0u32, 0u32, Vec::new());
    for &v in &values[1..values.len() - 6] {
        acc = ((acc << 5) | v as u32) & 0xfff;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            program.push((acc >> bits) as u8);
        }
    }
    if bits >= 5 || acc & ((1 << bits) - 1) != 0 {
        return false;
    }
    (2..=40).contains(&program.len()) && (version != 0 || matches!(program.len(), 20 | 32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitcoin_addresses() {
        for a in [
            "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
            "3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy",
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
            "BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
            "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0",
        ] {
            assert!(offline_format_ok("bitcoin", true, a), "{a}");
            assert!(!offline_format_ok("bitcoin", false, a), "a mainnet address on a test install: {a}");
        }
        for a in ["tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx", "mipcBbFg9gMiCh81Kj8tqqdgoZub1ZJRfn"] {
            assert!(offline_format_ok("bitcoin", false, a), "{a}");
            assert!(!offline_format_ok("bitcoin", true, a), "{a}");
        }
        for bad in [
            "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNb",
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t5",
            "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kV8f3t4",
            // A version 1 program with a bech32 (not bech32m) checksum.
            "bc1pw508d6qejxtdg4y5r3zarvary0c5xw7kw508d6qejxtdg4y5r3zarvary0c5xw7k7grplx",
            "bc1qxyz",
            "",
        ] {
            assert!(!offline_format_ok("bitcoin", true, bad), "{bad}");
        }
    }

    #[test]
    fn account_chain_addresses() {
        assert!(offline_format_ok("tron", true, "TBMspfKq6FS8hXjXbxmFrpiN8xXmreBSmP"));
        assert!(!offline_format_ok("tron", true, "TBMspfKq6FS8hXjXbxmFrpiN8xXmreBSmQ"), "checksum");
        assert!(!offline_format_ok("tron", true, "Tnotarealaddressxxxxxxxxxxxxxxxxxx"));
        assert!(offline_format_ok("solana", true, "So11111111111111111111111111111111111111112"));
        assert!(offline_format_ok("solana", true, "11111111111111111111111111111111"));
        assert!(!offline_format_ok("solana", true, "So1111111111111111111111111111111111111111"), "31 bytes");
        assert!(!offline_format_ok("solana", true, "0o11111111111111111111111111111111111111112"));
    }

    #[cfg(feature = "evm")]
    #[test]
    fn evm_addresses_need_a_correct_checksum() {
        assert!(offline_format_ok("evm", true, "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
        assert!(offline_format_ok("evm", true, "0x70997970c51812dc3a010c7d01b50e0d17dc79c8"));
        assert!(!offline_format_ok("evm", true, "0x70997970C51812dc3A010C7d01b50e0d17dc79c8"), "EIP-55");
        assert!(!offline_format_ok("evm", true, "0x12"));
    }
}
