//! PostgreSQL store. Each file adds one `impl PgStore` block for one area.

mod api_keys;
mod audit;
mod catalog;
mod checkout;
mod evm;
mod nodes;
mod ops;
mod payments;
mod payouts;
mod projects;
mod settings;
mod users;
mod utxo;
mod wallets;
mod webhooks;

pub use api_keys::ApiKeyRow;
pub use audit::AuditRow;
pub use checkout::{BrandingRow, BrandingUpdate, ExchangeCoin, ExchangeSettings, NewTrade, TradeRow};
pub use evm::{EvmDepositRow, EvmScanRow, EvmSweepCandidate, EvmSweepRow, EvmTxRow};
pub use nodes::NodeRow;
pub use payments::{InvoiceRow, NewPayment, PaymentFilter, PaymentRow, StuckSweep, SweepRow, TxRow};
pub use payouts::{
    AddressBookRow, ApprovalCodeRow, AttemptRow, NewAttempt, NewPayout, PAYOUT_COLS, PayoutFilter, PayoutRow, payout_from_row,
};
pub use projects::{MemberRow, ProjectRow};
pub use users::{SessionUser, UserRow};
pub use webhooks::{DeliveryRow, DueDelivery, EndpointRow};

use sqlx::{PgPool, postgres::PgPoolOptions};

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct PgStore {
    pub pool: PgPool,
}

impl PgStore {
    /// Connect and run migrations.
    pub async fn connect(database_url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new().max_connections(16).connect(database_url).await?;
        let store = Self { pool };
        store.migrate().await?;
        Ok(store)
    }

    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await.map_err(|e| Error::Internal(format!("migration failed: {e}")))
    }
}

/// `LIMIT`/`OFFSET` page of a list.
#[derive(Clone, Copy, Debug)]
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl Page {
    pub fn new(limit: i64, page: i64) -> Self {
        let limit = limit.clamp(1, 500);
        Self { limit, offset: page.max(0) * limit }
    }
}
