//! Position valuations and upkeep instruction lists for earn vault keepers.
//!
//! Pure: no RPC, no async, no clock of its own. A keeper names the accounts a
//! valuation needs ([`earn::EarnPlan::keys`], [`offerbook::OfferbookPlan::keys`]),
//! fetches them with one `getMultipleAccounts` at one slot, wraps the result in a
//! [`Snapshot`] (which must include the clock sysvar), and gets back the value
//! `PriceOpaqueArgs.value` takes: numéraire micro-units, `u64`, with every input
//! it used. When the same transaction refreshes prices, value against
//! [`Snapshot::with_refresh`], so the mark uses the prices the refresh installs.
//!
//! Rules every valuation keeps:
//! - **Fail closed.** A key the snapshot never fetched, a missing account that
//!   must exist, a wrong owner or layout, a stale or out-of-band price, an
//!   overdue Offerbook loan the vault lent, or a holding with no priced row is an [`Error`],
//!   never a zero.
//! - **Round against the vault.** Assets are struck at price − confidence and
//!   rounded down; liabilities at price + confidence and rounded up. This is
//!   the vault program's own band (`Floor` for holdings, `Ceil` for payouts).
//! - **Checked arithmetic.** An overflow is [`Error::Overflow`], never a wrap.
//! - **No mark the drift guard refuses.** [`mark::SlotState::decide`] builds
//!   no later mark outside the slot's drift budget, and no zero first mark
//!   while the vault is in bootstrap. Its docs list what it does not model.
//!
//! [`jobs`] turns marks into instruction lists in the order program 0.2.0
//! requires. Lists, never transactions: every signer mode wraps the same list.

pub mod earn;
pub mod jobs;
pub mod mark;
pub mod offerbook;
pub mod price;
pub mod rows;
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
    Loan { loan: Address, reason: LoanFault },
    /// The Offerbook `User` counts loans the plan does not know about.
    #[error("Offerbook loans changed since discovery; discover them again")]
    LoansChanged,
    #[error("liabilities exceed assets by {shortfall} micro-units; no unsigned mark exists")]
    Negative { shortfall: u64 },
    #[error("arithmetic overflow while valuing {0}")]
    Overflow(&'static str),
    /// The caller's reads came in an order that cannot prove the result.
    #[error("reads out of order: {0}")]
    ReadOrder(&'static str),
    /// The caller's input to a plan or job is inconsistent.
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

/// Why an Offerbook loan cannot be valued. [`Self::PastDue`] is the one a
/// keeper acts on: claim the loan the vault lent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoanFault {
    /// A loan the vault lent, at or past `expired_at`: Offerbook allows a
    /// claim and still takes full settlement, so the receivable is unknown.
    /// A borrowed loan past due is valued instead; see
    /// [`crate::offerbook::OfferbookValuation::overdue_borrowed`].
    PastDue,
    NotClassic,
    ZeroDuration,
    /// The vault is not the lender (or borrower) the plan says it is.
    WrongSide,
    OverRepaid,
    ExpiryOutOfRange,
    PrincipalNotToken,
    CollateralNotToken,
    /// The loan vault holds less collateral than the loan locks.
    CollateralShort,
}

impl core::fmt::Display for LoanFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::PastDue => "past due",
            Self::NotClassic => "only classic loans are supported",
            Self::ZeroDuration => "zero duration",
            Self::WrongSide => "the vault is not the loan's planned side",
            Self::OverRepaid => "repaid principal exceeds principal",
            Self::ExpiryOutOfRange => "expiry out of range",
            Self::PrincipalNotToken => "principal is not a token",
            Self::CollateralNotToken => "collateral is not a token",
            Self::CollateralShort => "loan vault holds less than the locked collateral",
        })
    }
}

pub type Result<T, E = Error> = core::result::Result<T, E>;
