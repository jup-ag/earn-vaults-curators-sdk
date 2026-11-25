//! Position valuations and upkeep instruction lists for earn vault keepers.
//!
//! Pure: no RPC, no async, no clock of its own. A keeper names the accounts a
//! valuation needs ([`earn::EarnPlan::keys`], [`offerbook::OfferbookPlan::keys`],
//! [`fluid::FluidPlan::keys`]),
//! fetches them with one `getMultipleAccounts` at one slot, wraps the result in a
//! [`Snapshot`] (which must include the clock sysvar), and gets back the value
//! `PriceOpaqueArgs.value` takes: numéraire micro-units, `u64`, with every input
//! it used. A JupiterLend valuation also takes the vault authority's token
//! listing ([`listing`]) and, for Fluid positions, the simulated exchange
//! prices ([`fluid`]), each read at the snapshot's slot.
//!
//! Rules every valuation keeps:
//! - **Fail closed.** A key the snapshot never fetched, a missing account that
//!   must exist, a wrong owner or layout, a stale or out-of-band price, an
//!   overdue Offerbook loan, a Fluid position in liquidation, or a holding the
//!   listing shows but nothing values is an [`Error`], never a zero.
//! - **Round against the vault.** Assets are struck at price − confidence and
//!   rounded down; liabilities at price + confidence and rounded up. This is
//!   the vault program's own band (`Floor` for holdings, `Ceil` for payouts).
//! - **Checked arithmetic.** An overflow is [`Error::Overflow`], never a wrap.
//! - **No first zero mark.** [`mark::MarkDecision`] refuses to post a zero
//!   bootstrap mark: from zero, the drift guard can reach nothing but zero.
//!
//! [`jobs`] turns marks into instruction lists in the order program 0.2.0
//! requires. Lists, never transactions: every signer mode wraps the same list.

pub mod earn;
pub mod fluid;
pub mod jobs;
pub mod listing;
pub mod mark;
pub mod offerbook;
pub mod price;
pub mod snapshot;

pub use snapshot::{Account, Snapshot};
use solana_address::Address;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The valuation needs this key but the snapshot never fetched it.
    #[error("account {0} is not in the snapshot")]
    NotFetched(Address),
    /// The snapshot says this account does not exist, and it must.
    #[error("{what} {address} does not exist")]
    Missing {
        what: &'static str,
        address: Address,
    },
    #[error("{what} {address} is owned by {owner}")]
    WrongOwner {
        what: &'static str,
        address: Address,
        owner: Address,
    },
    #[error("{what} {address} has an unexpected layout: {reason}")]
    Layout {
        what: &'static str,
        address: Address,
        reason: &'static str,
    },
    /// The same key appears twice in a snapshot with different contents.
    #[error("account {0} appears twice in the snapshot with different contents")]
    Conflicting(Address),
    #[error("the clock sysvar is at slot {clock}, the snapshot at {snapshot}")]
    ClockSlot { clock: u64, snapshot: u64 },
    /// The vault changed in a way the plan was built against; build it again.
    #[error("the vault changed since the plan was built: {0}")]
    PlanStale(&'static str),
    #[error("vault has no {0} position this valuation supports: {1}")]
    Position(&'static str, &'static str),
    /// The cached price on asset row `row` fails the program's gate at `now`.
    #[error("asset row {row} ({mint}) price is unusable: {reason}")]
    Price {
        row: u8,
        mint: Address,
        reason: PriceFault,
    },
    #[error("{mint} is not a configured asset row, so it has no price")]
    Unpriced { mint: Address },
    #[error("Offerbook loan {loan} cannot be valued: {reason}")]
    Loan { loan: Address, reason: &'static str },
    /// The Offerbook `User` counts loans the plan does not know about.
    #[error("Offerbook loans changed since discovery; discover them again")]
    LoansChanged,
    #[error("liabilities exceed assets by {shortfall} micro-units; no unsigned mark exists")]
    Negative { shortfall: u64 },
    /// A read that must come from the snapshot's slot came from another.
    #[error("{what} was read at slot {read}, the snapshot at {snapshot}")]
    ReadSlot {
        what: &'static str,
        read: u64,
        snapshot: u64,
    },
    /// The vault authority's token listing is incomplete or malformed.
    #[error("token listing is unusable: {0}")]
    Listing(&'static str),
    /// A token account the vault authority holds that this valuation cannot
    /// attribute to a priced holding.
    #[error("vault authority's token account {account} ({mint}) is unattributed: {reason}")]
    Unattributed {
        account: Address,
        mint: Address,
        reason: &'static str,
    },
    /// A Fluid position, vault or exchange price this crate does not value.
    #[error("Fluid account {account} cannot be valued: {reason}")]
    Fluid {
        account: Address,
        reason: &'static str,
    },
    #[error("arithmetic overflow while valuing {0}")]
    Overflow(&'static str),
    #[error("invalid job: {0}")]
    Job(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceFault {
    NonPositive,
    /// Older than the row's staleness window.
    Stale,
    /// Published further in the future than the row's skew allowance.
    Future,
    /// Confidence wider than the row's `max_confidence_bps`.
    Confidence,
    /// `price − confidence` is not positive.
    NoFloor,
}

impl core::fmt::Display for PriceFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NonPositive => "price is not positive",
            Self::Stale => "price is stale",
            Self::Future => "price is published in the future",
            Self::Confidence => "confidence band is too wide",
            Self::NoFloor => "price minus confidence is not positive",
        })
    }
}

pub type Result<T, E = Error> = core::result::Result<T, E>;
