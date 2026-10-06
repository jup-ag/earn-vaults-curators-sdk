pub mod offerbook {
    /// Create the vault's offerbook user and move listed tokens in/out of its
    /// escrow. Withdrawing an unlisted mint also requires [`CLAIM`].
    pub const ESCROW: u64 = 1 << 0;
    /// Create a principal (lender-side) offer within curator loan limits, and
    /// cancel any offer.
    pub const OFFER: u64 = 1 << 1;
    /// Claim a filled loan's repayment/default collateral; paired with
    /// [`ESCROW`] to withdraw unlisted seized collateral into custody.
    pub const CLAIM: u64 = 1 << 2;
    /// Fill a principal offer as borrower, subject to curator loan limits.
    pub const BORROW: u64 = 1 << 3;
    /// Repay a borrowed loan to release its collateral.
    pub const REPAY: u64 = 1 << 4;
    /// Cancel offers. Creating a standing collateral offer is refused (H-03):
    /// its fill never passes through the vault, so it cannot be priced.
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

pub mod ccip {
    /// Route `ccip_send` as the vault authority: bridge one listed asset to
    /// the curator-pinned [`CcipLane`](crate::state::CcipLane), paying the
    /// CCIP fee from the authority's LINK/WSOL ATA or its lamports.
    pub const SEND: u64 = 1 << 0;
}

pub mod nest {
    /// Route CCTP v2 `deposit_for_burn_with_hook` as the vault authority: burn
    /// listed USDC toward the curator-pinned
    /// [`NestRoute`](crate::state::NestRoute) relayer with a hook that mints
    /// Nest shares back to the vault authority. Granted on the CCTP program id.
    pub const MINT: u64 = 1 << 0;
    /// Redeem curator-pinned Nest shares through the Nest OFT program.
    pub const REDEEM: u64 = 1 << 1;
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
