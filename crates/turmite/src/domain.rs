use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::error::Error;

/// How addresses on a network behave.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkKind {
    Utxo,
    Evm,
    Account,
    AccountMemo,
}

impl NetworkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NetworkKind::Utxo => "utxo",
            NetworkKind::Evm => "evm",
            NetworkKind::Account => "account",
            NetworkKind::AccountMemo => "account_memo",
        }
    }
}

impl std::str::FromStr for NetworkKind {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "utxo" => Ok(NetworkKind::Utxo),
            "evm" => Ok(NetworkKind::Evm),
            "account" => Ok(NetworkKind::Account),
            "account_memo" => Ok(NetworkKind::AccountMemo),
            other => Err(Error::Internal(format!("unknown network kind `{other}`"))),
        }
    }
}

/// Payment status. The strings are part of the public API.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Waiting,
    Confirming,
    Confirmed,
    Sending,
    Finished,
    PartiallyPaid,
    Failed,
    Expired,
    Refunded,
}

impl PaymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PaymentStatus::Waiting => "waiting",
            PaymentStatus::Confirming => "confirming",
            PaymentStatus::Confirmed => "confirmed",
            PaymentStatus::Sending => "sending",
            PaymentStatus::Finished => "finished",
            PaymentStatus::PartiallyPaid => "partially_paid",
            PaymentStatus::Failed => "failed",
            PaymentStatus::Expired => "expired",
            PaymentStatus::Refunded => "refunded",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, PaymentStatus::Finished | PaymentStatus::Failed | PaymentStatus::Expired | PaymentStatus::Refunded)
    }

    /// A refund can be recorded. `PgStore::mark_refunded` has the same rule in SQL.
    pub fn can_refund(self, actually_paid: Decimal) -> bool {
        match self {
            PaymentStatus::Finished | PaymentStatus::PartiallyPaid | PaymentStatus::Failed => true,
            PaymentStatus::Expired => actually_paid > Decimal::ZERO,
            _ => false,
        }
    }

    pub const ALL: [PaymentStatus; 9] = [
        PaymentStatus::Waiting,
        PaymentStatus::Confirming,
        PaymentStatus::Confirmed,
        PaymentStatus::Sending,
        PaymentStatus::Finished,
        PaymentStatus::PartiallyPaid,
        PaymentStatus::Failed,
        PaymentStatus::Expired,
        PaymentStatus::Refunded,
    ];
}

impl std::str::FromStr for PaymentStatus {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PaymentStatus::ALL.into_iter().find(|st| st.as_str() == s).ok_or_else(|| Error::Internal(format!("unknown payment status `{s}`")))
    }
}

/// Exception axis next to the status: why an amount differs or came late.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settlement {
    None,
    Underpaid,
    Overpaid,
    PaidLate,
    MarkedPaid,
    Refunded,
}

impl Settlement {
    pub fn as_str(self) -> &'static str {
        match self {
            Settlement::None => "none",
            Settlement::Underpaid => "underpaid",
            Settlement::Overpaid => "overpaid",
            Settlement::PaidLate => "paid_late",
            Settlement::MarkedPaid => "marked_paid",
            Settlement::Refunded => "refunded",
        }
    }
}

impl std::str::FromStr for Settlement {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "none" => Settlement::None,
            "underpaid" => Settlement::Underpaid,
            "overpaid" => Settlement::Overpaid,
            "paid_late" => Settlement::PaidLate,
            "marked_paid" => Settlement::MarkedPaid,
            "refunded" => Settlement::Refunded,
            other => return Err(Error::Internal(format!("unknown settlement `{other}`"))),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Asset {
    pub asset_id: i64,
    pub symbol: String,
    pub name: String,
    pub coingecko_id: Option<String>,
    pub is_stable: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Network {
    pub network_id: i64,
    /// Upper-case code, e.g. `ETH`.
    pub code: String,
    /// Driver key, e.g. `ethereum`.
    pub chain: String,
    pub family: String,
    pub name: String,
    pub chain_id: Option<i64>,
    pub kind: NetworkKind,
    pub native_asset_id: i64,
    pub confirmations_required: i32,
    pub explorer_tx_url: Option<String>,
    pub explorer_address_url: Option<String>,
    pub is_testnet: bool,
    pub is_enabled: bool,
    pub last_block: Option<i64>,
    pub last_block_at: Option<DateTime<Utc>>,
}

impl Network {
    pub fn tx_url(&self, txid: &str) -> Option<String> {
        self.explorer_tx_url.as_deref().filter(|u| !u.is_empty()).map(|u| u.replace("{txid}", txid))
    }
    pub fn address_url(&self, address: &str) -> Option<String> {
        self.explorer_address_url.as_deref().filter(|u| !u.is_empty()).map(|u| u.replace("{address}", address))
    }
    /// Lower-case network code used in API responses.
    pub fn api_name(&self) -> String {
        self.code.to_ascii_lowercase()
    }
}

/// An asset on a network, with its asset and network rows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssetNetwork {
    pub asset_network_id: i64,
    pub asset: Asset,
    pub network: Network,
    pub ticker: String,
    pub decimals: i32,
    pub contract_address: Option<String>,
    pub min_payment_amount: Option<Decimal>,
    pub is_enabled: bool,
}

impl AssetNetwork {
    pub fn is_native(&self) -> bool {
        self.contract_address.is_none()
    }
    pub fn ticker(&self) -> String {
        self.ticker.clone()
    }
}

/// Where a project's funds go on one network.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectWallet {
    pub project_wallet_id: i64,
    pub project_id: i64,
    pub network_id: i64,
    pub address: Option<String>,
    pub address_extra_id: Option<String>,
    pub xpub: Option<String>,
    pub restore_height: Option<i64>,
    pub factory_address: Option<String>,
    pub is_enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
