//! What the signer needs from each chain family.

use async_trait::async_trait;
use rust_decimal::Decimal;
use sqlx::PgConnection;

use crate::{
    app::App,
    db::{AttemptRow, NewAttempt, PayoutRow},
    domain::AssetNetwork,
    error::Result,
    hot::HotKey,
    payouts::rpc::Nodes,
};

/// Why a transaction is signed.
pub enum SignMode<'a> {
    /// The first transaction of the payout.
    New,
    /// EVM: the same nonce, recipient and amount with higher fees.
    Bump(&'a AttemptRow),
    /// The previous transaction expired without landing: sign it again with a new validity.
    Renew,
}

/// Everything a chain needs to sign one payout.
pub struct SignCtx<'a> {
    pub app: &'a App,
    pub nodes: &'a Nodes,
    pub an: &'a AssetNetwork,
    pub payout: &'a PayoutRow,
    pub key: &'a HotKey,
}

pub enum Broadcast {
    /// A node took the transaction (or already had it).
    Accepted,
    /// Every node that answered refused it.
    Refused(String),
    /// No node answered.
    Unreachable(String),
}

#[derive(Debug, Clone)]
pub enum TxState {
    /// No node knows the transaction.
    Unknown,
    /// A node has it, not yet in a block.
    Pending,
    /// In a block. `success` false: the chain ran it and it failed (the fee is still spent).
    Included { confirmations: i64, success: bool, fee: Option<Decimal>, error: Option<String> },
}

/// Can a transaction that no node knows still land?
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// Still valid: broadcast it again.
    CanLand,
    /// Expired: it can never land. Safe to sign a new one.
    Expired,
    /// Its nonce or seqno is used, but not by a transaction we know: the outcome is unknown.
    NonceUsed,
}

#[async_trait]
pub trait PayoutChain: Send + Sync {
    /// Sign a transaction. Inside the signer's transaction (and the nonce lock where the chain has nonces).
    /// `Error::BadRequest` means the payout cannot be sent (balance, recipient); other errors are retried.
    async fn sign(&self, ctx: &SignCtx<'_>, conn: &mut PgConnection, mode: SignMode<'_>) -> Result<NewAttempt>;
    async fn broadcast(&self, nodes: &Nodes, a: &AttemptRow) -> Broadcast;
    async fn state(&self, nodes: &Nodes, a: &AttemptRow) -> Result<TxState>;
    async fn landing(&self, nodes: &Nodes, a: &AttemptRow) -> Result<Landing>;
    /// Network fee of a payout of `an`, in the native coin.
    async fn estimate_fee(&self, nodes: &Nodes, an: &AssetNetwork, from: Option<&str>) -> Result<Decimal>;
    async fn native_balance(&self, nodes: &Nodes, address: &str) -> Result<Decimal>;
    /// Checks before a payout is accepted (recipient type, rent). Only fails on facts the chain reports.
    async fn precheck(&self, _nodes: &Nodes, _an: &AssetNetwork, _to: &str, _amount: Decimal) -> Result<()> {
        Ok(())
    }
    /// EVM: sign again with higher fees when a transaction waits in the mempool this many seconds.
    fn bump_after_secs(&self) -> Option<i64> {
        None
    }
    /// Session lock key that serialises signing and broadcasting per wallet, with the SQL to take it inside
    /// the signing transaction too.
    fn lock_key(&self, _chain: &str, _address: &str) -> Option<String> {
        None
    }
}

/// The payout implementation of a family.
pub fn for_family(family: &str) -> Option<&'static dyn PayoutChain> {
    match family {
        #[cfg(feature = "evm")]
        "evm" => Some(&crate::payouts::evm::Evm),
        #[cfg(feature = "sol")]
        "solana" => Some(&crate::payouts::sol::Sol),
        #[cfg(feature = "tron")]
        "tron" => Some(&crate::payouts::tron::Tron),
        #[cfg(feature = "ton")]
        "ton" => Some(&crate::payouts::ton::Ton),
        _ => None,
    }
}
