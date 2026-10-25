use tokio::sync::broadcast;

use crate::domain::PaymentStatus;

/// In-process domain event bus.
/// Chain watchers publish; the reducer/IPN/sweep workers each subscribe and
/// process serially on their own receiver.
#[derive(Clone, Debug)]
pub enum DomainEvent {
    PaymentCreated {
        payment_id: i64,
    },
    /// A chain watcher saw funds; recompute the payment/invoice state.
    PaymentDetected {
        payment_id: i64,
        confirmations: i32,
    },
    PaymentStatusChanged {
        payment_id: i64,
        from: PaymentStatus,
        to: PaymentStatus,
    },
    PaymentFinished {
        payment_id: i64,
    },
    InvoiceExpired {
        invoice_id: i64,
    },
    SweepReady {
        payment_id: i64,
    },
    SweepBroadcast {
        sweep_id: i64,
        txid: String,
    },
    SweepConfirmed {
        sweep_id: i64,
    },
    /// Generic "this payment needs the reducer to run".
    NeedsRecompute {
        payment_id: i64,
    },
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<DomainEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(2048);
        Self { tx }
    }

    pub fn publish(&self, event: DomainEvent) {
        // Err only when there are no subscribers; that is fine.
        let _ = self.tx.send(event);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DomainEvent> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}
