//! The settlement seam — where SpaceDB hands off to whoever pays.
//!
//! SpaceDB *measures*; it does not mint money. A [`UsageClaim`] is handed to a
//! [`Settlement`] implementation, which prices it and records the payout. This is
//! the open-core boundary:
//!
//! - The bundled [`LocalSettlement`] just prices claims against a [`RateCard`] and
//!   keeps a local tally — enough for a self-hoster's own accounting, with no
//!   marketplace.
//! - A host (e.g. MATA) implements [`Settlement`] over its existing pipeline —
//!   `UsageClaim → Maestro counter-signs → EarningRecord → Iron Bank credits
//!   $MATA` — so SpaceDB usage settles through the same Loop-11 path already
//!   shipped, without SpaceDB depending on any of it.

use std::collections::BTreeMap;

use crate::claim::UsageClaim;
use crate::error::MeterError;
use crate::ratecard::RateCard;

/// A priced settlement receipt (the open-core analogue of an `EarningRecord`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settled {
    pub claim_id: String,
    pub settles_to_did: String,
    pub micro_mata: u64,
}

/// Consumes usage claims and settles them. A host plugs its money plane in here.
pub trait Settlement {
    fn settle(&mut self, claim: &UsageClaim) -> Result<Settled, MeterError>;
}

/// The bundled, marketplace-free settlement: price against a rate card and tally
/// per customer. No `$MATA` is minted — that is a host's concern.
#[derive(Clone, Debug)]
pub struct LocalSettlement {
    rate_card: RateCard,
    /// settles_to_did → total micro-$MATA recorded.
    tally: BTreeMap<String, u64>,
    settled: Vec<Settled>,
}

impl LocalSettlement {
    pub fn new(rate_card: RateCard) -> Self {
        Self {
            rate_card,
            tally: BTreeMap::new(),
            settled: Vec::new(),
        }
    }

    /// Total micro-`$MATA` tallied for a customer.
    pub fn tallied(&self, settles_to_did: &str) -> u64 {
        self.tally.get(settles_to_did).copied().unwrap_or(0)
    }

    /// All receipts recorded so far.
    pub fn receipts(&self) -> &[Settled] {
        &self.settled
    }
}

impl Settlement for LocalSettlement {
    fn settle(&mut self, claim: &UsageClaim) -> Result<Settled, MeterError> {
        let micro_mata = self.rate_card.price(&claim.usage);
        let receipt = Settled {
            claim_id: claim.claim_id.clone(),
            settles_to_did: claim.settles_to_did.clone(),
            micro_mata,
        };
        *self.tally.entry(claim.settles_to_did.clone()).or_default() += micro_mata;
        self.settled.push(receipt.clone());
        Ok(receipt)
    }
}
