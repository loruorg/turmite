//! Keeps a pool of derived deposit addresses for chains whose keys come from the
//! deposit seed, so the API process can hand them out without holding the seed.

use sqlx::Row;

use crate::{app::App, chains::PaymentDriver, config::Role, domain::Network, error::Result};

/// Free addresses kept per network.
pub const POOL_TARGET: i64 = 25;
/// Interval of the refill loop.
pub const REFILL_SECONDS: u64 = 30;
/// Families whose deposit keys come from the install deposit seed.
pub const SEED_FAMILIES: &[&str] = &["solana", "tron", "ton"];

/// Top up the pool of every seed-derived network. Worker role only: in one process
/// the API derives directly.
pub async fn refill(app: &App) {
    if app.role != Role::Worker {
        return;
    }
    let Ok(networks) = app.store.list_networks().await else { return };
    let drivers = app.drivers();
    for n in networks.iter().filter(|n| n.is_enabled && SEED_FAMILIES.contains(&n.family.as_str())) {
        let Some(driver) = drivers.get(&n.chain) else { continue };
        if !driver.can_derive() {
            tracing::warn!(network = %n.code, "the worker cannot read the deposit seed: no addresses for the pool");
            continue;
        }
        match refill_network(app, n, driver.as_ref()).await {
            Ok(0) => {}
            Ok(added) => tracing::info!(network = %n.code, added, "address pool refilled"),
            Err(e) => tracing::warn!(network = %n.code, error = %e, "address pool refill failed"),
        }
    }
}

/// Returns the number of addresses added.
pub async fn refill_network(app: &App, n: &Network, driver: &dyn PaymentDriver) -> Result<i64> {
    // The counter never goes below the highest index in use (a restored database or a
    // lost counter row would otherwise derive used addresses again).
    sqlx::query(
        "INSERT INTO network_address_counter (network_id, next_index) \
         SELECT $1, COALESCE(MAX(derivation_index), 0) + 1 FROM deposit_addresses WHERE network_id = $1 \
         ON CONFLICT (network_id) DO UPDATE \
           SET next_index = GREATEST(network_address_counter.next_index, EXCLUDED.next_index)",
    )
    .bind(n.network_id)
    .execute(&app.store.pool)
    .await?;

    // Free addresses that this seed does not derive (another seed or derivation) are
    // useless: the worker could not sweep them. Remove them before the API hands them out.
    let free = sqlx::query(
        "SELECT deposit_address_id, derivation_index, address FROM deposit_addresses \
         WHERE network_id = $1 AND project_id IS NULL AND payment_id IS NULL",
    )
    .bind(n.network_id)
    .fetch_all(&app.store.pool)
    .await?;
    for row in &free {
        let index: i64 = row.get("derivation_index");
        let address: String = row.get("address");
        if driver.derive_deposit(&Default::default(), index as u64).await?.address != address {
            tracing::error!(network = %n.code, index, %address, "pool address does not match the deposit seed; removed");
            sqlx::query("DELETE FROM deposit_addresses WHERE deposit_address_id = $1 AND project_id IS NULL AND payment_id IS NULL")
                .bind(row.get::<i64, _>("deposit_address_id"))
                .execute(&app.store.pool)
                .await?;
        }
    }

    let mut added = 0;
    for _ in app.store.pool_size(n.network_id).await?..POOL_TARGET {
        let index = app.store.next_network_index(n.network_id).await?;
        let d = driver.derive_deposit(&Default::default(), index as u64).await?;
        app.store.insert_deposit_address(n.network_id, None, index, &d.address, d.payin_extra_id.as_deref()).await?;
        added += 1;
    }
    Ok(added)
}
