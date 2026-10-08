use {
    codama_macros::CodamaErrors,
    solana_program_error::{ProgramError, ToStr},
};

#[derive(CodamaErrors, Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum VaultError {
    MissingPermission = 200,
    AumOverflow,
    StalePrice,
    PriceDrift,
    InvalidDelegate,
    InvalidAccountLayout,
    InvalidMintForVault,
    InvalidPriceFeedMint,
    OracleStale,
    PricedSlotStale,
    PricedSlotMissing,
    DisallowedAsset,
    TimelockNotElapsed,
    InsufficientIdleLiquidity,
    CpiSettlementInvalid,
    InvalidEventAuthority,
    InvalidOraclePrice,
    InstantRedeemDisabled,
    VaultPaused,
    InsufficientBacking,
    VaultNotEmpty,
    SwapSlippageBudgetExceeded,
    FeeAccrualPending,
    DepositCapExceeded,
    DeployedCeilingExceeded,
    TransferPaused,
    MarkUnderflow,
    DebtSlotNotFoldable,
    LenderOnlyVault,
    DuplicateAccount,
    InvalidOracleReport,
    UnpricedBacking,
    FlashLiabilityOutstanding,
    FlashPaybackMissing,
    FlashLiabilityExceeded,
    FlashAmountInvalid,
    FlashCpiForbidden,
    SinkCapacityExhausted,
    OracleExponentMismatch,
    /// A positive recipient fee is too small to mint one share at current NAV.
    FeeAmountTooSmall,
    /// Borrowing is unconfigured or the proposed loan exceeds policy.
    LoanLimitExceeded,
    /// `ccip_send` targets a chain or receiver other than the vault's CCIP lane policy.
    CcipLaneMismatch,
    /// A Nest mint/redeem differs from the vault's Nest route policy.
    NestRouteMismatch,
    /// The temporary Nest redeem handoff is not followed by the exact OFT send it authorized.
    NestRedeemSequenceInvalid,
    /// The redeem payout is below the caller's `min_out`.
    RedeemOutputBelowMinimum,
    /// The deposit mints fewer shares than the caller's `min_shares_out`.
    SubscribeOutputBelowMinimum,
    /// A queued redeem must settle in the user's requested token.
    RedeemPayoutMintMismatch,
    /// A `ProtocolPolicy` account names another vault, protocol or key.
    PolicyMismatch,
}

impl From<VaultError> for ProgramError {
    fn from(e: VaultError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

impl ToStr for VaultError {
    fn to_str(&self) -> &'static str {
        match self {
            Self::LoanLimitExceeded => "Loan violates curator-configured limits",
            Self::MissingPermission => "Signer lacks the required delegate permission",
            Self::AumOverflow => "AUM computation overflowed u64",
            Self::StalePrice => "AUM snapshot is too stale for settlement",
            Self::PriceDrift => "AUM snapshot drifts too far from the oracle reference",
            Self::InvalidDelegate => "Delegate account does not match the signer",
            Self::InvalidAccountLayout => "Account data is shorter than the expected layout",

            Self::InvalidMintForVault => "Mint PDA does not belong to the supplied vault",
            Self::InvalidPriceFeedMint => "PriceFeed mint does not match the token account mint",
            Self::OracleStale => "PriceFeed timestamp exceeds MAX_ORACLE_STALENESS_SECONDS",
            Self::PricedSlotStale => "Priced-protocol slot timestamp exceeds MAX_STALENESS_SECONDS",
            Self::PricedSlotMissing => "A required priced-protocol slot has never been priced",
            Self::DisallowedAsset => "Mint is not listed in Vault.allowed_assets",
            Self::TimelockNotElapsed => {
                "Redeem timelock has not elapsed — try ClaimRedeem again later"
            }
            Self::InsufficientIdleLiquidity => {
                "Vault has no idle base asset to fulfill this redeem right now"
            }
            Self::CpiSettlementInvalid => {
                "Routed CPI did not move the vault's settlement balance as expected"
            }
            Self::InvalidEventAuthority => {
                "Event CPI account is not the event-authority PDA signer"
            }
            Self::InvalidOraclePrice => "Oracle reported a non-positive price",
            Self::InstantRedeemDisabled => {
                "Instant redeem is not enabled on this vault — use the queued Redeem"
            }
            Self::VaultPaused => "This operation is paused on the vault (see Vault.paused)",
            Self::InsufficientBacking => {
                "Net backing (AUM less redeem and fee liabilities) is zero — deposit would over-mint"
            }
            Self::UnpricedBacking => {
                "Custody holds base the recorded AUM has never priced — run validate_aum first"
            }
            Self::VaultNotEmpty => {
                "Vault still has outstanding shares, in-flight redeems, or delegates — cannot be closed"
            }
            Self::SwapSlippageBudgetExceeded => {
                "Routed swap would exceed the vault's cumulative slippage bucket"
            }
            Self::FeeAccrualPending => {
                "Uncrystallized fee accrual — run crystallize_fees before changing management or performance fees"
            }
            Self::DepositCapExceeded => {
                "Deposit would exceed the asset's capacity ceiling (max_deposit)"
            }
            Self::DeployedCeilingExceeded => {
                "Strategy outflow would exceed the asset's deployed ceiling (max_deployed)"
            }
            Self::TransferPaused => {
                "Share transfers are halted by the vault's policy (see Policy.paused_transfers)"
            }
            Self::MarkUnderflow => {
                "Settlement would fold more value out of the position than its mark holds — \
                 re-mark the slot with price_opaque first"
            }
            Self::DebtSlotNotFoldable => {
                "Settlement folds an asset-side mark and cannot target a DEBT position — \
                 track opaque liabilities on a SpotTokens slot"
            }
            Self::LenderOnlyVault => {
                "This vault's position slot is LENDER_ONLY — the protocol's borrower \
                 side (Offerbook fills/repays, any Fluid Vaults call) can be neither \
                 granted nor routed, and the slot cannot be removed"
            }
            Self::OracleExponentMismatch => {
                "Price source reported a different exponent than the asset row's \
                 declared `report_decimals` — a re-scaled feed stops the vault \
                 rather than re-pricing it"
            }
            Self::FeeAmountTooSmall => {
                "Accrued fees are too small to mint a share — wait for more fees to accrue"
            }
            Self::InvalidOracleReport => {
                "Chainlink Data Streams report is malformed, expired, or its feed id \
                 matches no Chainlink asset row on this vault"
            }
            Self::DuplicateAccount => {
                "Two accounts that must differ were passed as the same address — \
                 a token transfer between them would be a no-op the vault still books"
            }
            Self::FlashLiabilityOutstanding => {
                "Custody holds a transient flash loan (Vault.flash_liability != 0) — \
                 no share mint/burn or AUM attestation may price against it"
            }
            Self::FlashPaybackMissing => {
                "A flash_deposit needs exactly one matching flash_withdraw later in the \
                 same transaction, for the same vault and mint"
            }
            Self::FlashLiabilityExceeded => {
                "Flash deposit would exceed Vault.max_flash_liability (0 = the feature \
                 is disabled on this vault)"
            }
            Self::FlashAmountInvalid => {
                "Flash payback must equal the outstanding principal exactly, or there is no \
                 liability to settle"
            }
            Self::SinkCapacityExhausted => {
                "No sink slot is available for this collateral mint — either \
                 Vault.sink_capacity is 0 (seized-collateral custody is off) or \
                 every slot within it already holds a different mint; raise the \
                 capacity or sell one down before seizing another"
            }
            Self::FlashCpiForbidden => {
                "flash_deposit / flash_withdraw must be top-level instructions — the \
                 introspection that bounds the liability to one transaction cannot see \
                 a CPI"
            }
            Self::CcipLaneMismatch => {
                "ccip_send must target exactly the chain selector and receiver of the \
                 vault's CCIP lane policy (curator-set via set_protocol_policy; none = bridging \
                 disabled)"
            }
            Self::NestRouteMismatch => {
                "the Nest burn / OFT send must target exactly the composer and relayer of \
                 the vault's Nest route policy (curator-set via set_protocol_policy; none = Nest \
                 disabled)"
            }
            Self::NestRedeemSequenceInvalid => {
                "prepare_nest_redeem must be top-level and immediately followed by the exact \
                 curator-pinned Nest OFT send"
            }
            Self::RedeemOutputBelowMinimum => "Redeem payout is below the caller's minimum",
            Self::SubscribeOutputBelowMinimum => "Deposit shares are below the caller's minimum",
            Self::RedeemPayoutMintMismatch => "Redeem payout mint differs from the user's request",
            Self::PolicyMismatch => {
                "the ProtocolPolicy account does not belong to this vault, protocol and key"
            }
        }
    }
}
