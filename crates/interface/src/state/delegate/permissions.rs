pub mod offerbook {
    /// Create the vault's offerbook user and move listed tokens in/out of its
    /// escrow. Withdrawing an unlisted mint also requires [`CLAIM`].
    pub const ESCROW: u64 = 1 << 0;
    /// Create a principal (lender-side) offer, and cancel any offer.
    pub const OFFER: u64 = 1 << 1;
    /// Claim a filled loan's repayment/default collateral; paired with
    /// [`ESCROW`] to withdraw unlisted seized collateral into custody.
    pub const CLAIM: u64 = 1 << 2;
    /// Fill a principal offer as borrower, subject to curator loan limits.
    pub const BORROW: u64 = 1 << 3;
    /// Repay a borrowed loan to release its collateral.
    pub const REPAY: u64 = 1 << 4;
    /// Create a collateral offer within curator loan limits, and cancel offers.
    pub const COLLATERAL_OFFER: u64 = 1 << 5;
}

pub mod jupiter {
    /// Swap between allow-listed vault positions only.
    pub const SWAP: u64 = 1 << 0;
    /// Swap out of an unlisted position, as needed to unwind a liquidation.
    pub const LIQUIDATE: u64 = 1 << 1;
}

pub mod jupiter_lend {
    /// Open/close a lend position and change its collateral leg.
    pub const LEND: u64 = 1 << 0;
    /// Move the debt leg of a lend position in either direction (borrow/repay).
    pub const BORROW: u64 = 1 << 1;
    /// Deposit into and withdraw from a Jupiter Earn vault.
    pub const EARN: u64 = 1 << 3;
}

pub mod vault {
    /// Post a mark for an opaque position slot, or a price for a hand-priced
    /// asset row (`price_opaque`, `ASSET_TARGET`) — both drift-guarded.
    pub const PRICE: u64 = 1 << 0;
    /// Fulfill pending subscribe and redeem requests.
    pub const FULFILL: u64 = 1 << 1;
    /// Set pause flags to halt subscribe/redeem/settlement flows.
    pub const PAUSE: u64 = 1 << 3;
    /// Lend the vault an external flashloan for one transaction
    /// (`flash_deposit` / `flash_withdraw`).
    pub const FLASH: u64 = 1 << 4;
}
