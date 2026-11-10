use {
    crate::encoding,
    codama_macros::CodamaType,
    encoding::{PodOption, ZcElem, ZcValidate, ZeroPodError},
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{I64, U16, U32, U64},
};

#[derive(SchemaRead, SchemaWrite, CodamaType, Clone, Copy)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AssetUpdate {
    pub index: u8,
    /// New feed id — the rotation path for a deprecated feed. A zero feed id
    /// makes the row hand-priced instead (`refresh_prices` skips it, the price
    /// comes from `price_opaque`, initially signed by the curator).
    /// Supplying a feed resets its scale to zero unless `report_decimals` is
    /// also supplied. The cache is retired only if the effective config changes.
    #[codama(type = option(public_key, fixed))]
    pub oracle: PodOption<Address>,
    /// Curator-settable `ASSET_FLAG_*` bits (`ASSET_FLAG_CURATOR_MASK`, today
    /// just `CHAINLINK`) — the provider switch. Bits outside the mask are
    /// rejected; switching retires the cached reading like a rotation does.
    #[codama(type = option(number(u8), fixed))]
    pub flags: PodOption<u8>,
    /// New [`AssetConfig::report_decimals`] for this row; `Some(0)` hands the
    /// scale back to the source. A changed scale retires the cached reading;
    /// an identical restatement preserves the cache and replay watermark.
    ///
    /// [`AssetConfig::report_decimals`]: crate::state::AssetConfig::report_decimals
    #[codama(type = option(number(u8), fixed))]
    pub report_decimals: PodOption<u8>,
    /// Maximum observation age for valuation and settlement, 0..=u32::MAX.
    /// Zero selects the 15s default. The i64 wire encoding is unchanged.
    #[codama(type = option(number(i64, le), fixed))]
    pub max_staleness_seconds: PodOption<I64>,
    /// Confidence cap in bps of price; `0` means the global default.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_confidence_bps: PodOption<U16>,
    /// Hand-priced row's drift cap, 1..=200 bps; None leaves it unchanged.
    /// Unlike positions, zero and the waiver sentinel are rejected.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_drift_bps: PodOption<U16>,
    /// Per-asset strategy-outflow ceiling; `0` disables deployment, `u64::MAX` is unlimited.
    #[codama(type = option(number(u64, le), fixed))]
    pub max_deployed: PodOption<U64>,
    /// Restate the remaining `deployed` charge across all protocols in raw token
    /// units after verifying a principal return. CCIP OffRamp deliveries and
    /// Offerbook escrow withdrawals leave this counter unchanged. Preserve
    /// unrelated deployment charges; this affects both capacity caps, never AUM.
    /// Listed Offerbook withdrawals already fold the position mark: do not reset
    /// it; reconcile and re-attest the remaining value with `PriceOpaque`.
    #[codama(type = option(number(u64, le), fixed))]
    pub deployed: PodOption<U64>,
    /// Per-asset deposit capacity ceiling; `0` disables deposits, `u64::MAX` is unlimited.
    #[codama(type = option(number(u64, le), fixed))]
    pub max_deposit: PodOption<U64>,
    /// Toggle deposits for this asset (`1` enable / `0` disable). Its own
    /// field, not a `flags` bit, so a provider switch can't silently rewrite
    /// the deposit gate. Among enabled rows, the lowest index is canonical
    /// for direct deposits; later rows are queued-only. Manual pricing requires
    /// instant redemption to remain disabled.
    #[codama(type = option(number(u8), fixed))]
    pub deposit_enabled: PodOption<u8>,
    /// Quote leg feed id of a composed price; the zero address makes the row
    /// single-feed again. Resets its scale unless `quote_report_decimals` is
    /// also supplied; retires the cache only if the effective config changes.
    #[codama(type = option(public_key, fixed))]
    pub quote_oracle: PodOption<Address>,
    /// [`AssetUpdate::report_decimals`] for the quote leg. A changed scale
    /// retires the cached reading.
    #[codama(type = option(number(u8), fixed))]
    pub quote_report_decimals: PodOption<u8>,
}

impl ZcValidate for AssetUpdate {
    fn validate_ref(value: &Self) -> Result<(), ZeroPodError> {
        PodOption::validate_ref(&value.oracle)?;
        PodOption::validate_ref(&value.flags)?;
        PodOption::validate_ref(&value.report_decimals)?;
        PodOption::validate_ref(&value.max_staleness_seconds)?;
        PodOption::validate_ref(&value.max_confidence_bps)?;
        PodOption::validate_ref(&value.max_drift_bps)?;
        PodOption::validate_ref(&value.max_deployed)?;
        PodOption::validate_ref(&value.deployed)?;
        PodOption::validate_ref(&value.max_deposit)?;
        PodOption::validate_ref(&value.deposit_enabled)?;
        PodOption::validate_ref(&value.quote_oracle)?;
        PodOption::validate_ref(&value.quote_report_decimals)
    }
}

// SAFETY: `#[repr(C)]` over `u8` + `PodOption`s, all `ZcElem` (align 1, no
// padding); every initialized bit pattern is a valid `&Self` and the option
// tags are gated by `validate_ref` above.
unsafe impl ZcElem for AssetUpdate {}

#[derive(SchemaRead, SchemaWrite, CodamaType, Clone, Copy)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct PositionUpdate {
    /// Index into `Vault.positions`.
    pub index: u8,
    /// Per-position staleness override; `0` means the global default.
    #[codama(type = option(number(u64, le), fixed))]
    pub max_staleness_seconds: PodOption<U64>,
    /// Per-position drift override; `0` selects the global default,
    /// `u16::MAX` waives the budget, and `None` leaves it unchanged.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_drift_bps: PodOption<U16>,
    /// `Some(1)` resets a non-DEBT row to never-priced (clears `PRICED` /
    /// `INITIALIZED`, zeroes the mark), so the next `price_opaque` bootstraps
    /// again against the last validated AUM. The unbrick path for an asset-like
    /// slot frozen at a dust/wrong first mark. DEBT rows may never reset: omitting
    /// a live liability would overstate NAV. Resetting an initialized
    /// `REQUIRED_IN_AUM` row needs both share flows paused (`Pause::SHARE_FLOWS`),
    /// counting `paused` in the same instruction.
    #[codama(type = option(number(u8), fixed))]
    pub reset: PodOption<u8>,
}

impl ZcValidate for PositionUpdate {
    fn validate_ref(value: &Self) -> Result<(), ZeroPodError> {
        PodOption::validate_ref(&value.max_staleness_seconds)?;
        PodOption::validate_ref(&value.max_drift_bps)?;
        PodOption::validate_ref(&value.reset)
    }
}

// SAFETY: same layout argument as `AssetUpdate` above.
unsafe impl ZcElem for PositionUpdate {}

/// Configure a brand-new asset row in the first free `Vault.assets` slot — the
/// post-init onboarding path. The mint is the trailing account of
/// `UpdateVault`; decimals, the ATA bump, and `PRICE_IN_AUM` / `MANUAL_PRICE`
/// are derived exactly as at init. The cached AUM is staled, so nothing flows
/// until the new row is priced and re-validated.
#[derive(SchemaRead, SchemaWrite, CodamaType, Clone, Copy)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AssetAdd {
    /// Feed id; the zero address makes the row hand-priced.
    pub oracle: Address,
    /// `ASSET_FLAG_CHAINLINK` and/or `ASSET_FLAG_DEPOSIT_ENABLED`.
    pub flags: u8,
    /// The decimal scale the feed quotes its raw mantissa at; `0` leaves the
    /// source's own. See [`AssetUpdate::report_decimals`].
    pub report_decimals: u8,
    /// Strategy-outflow ceiling; `0` disables deployment, `u64::MAX` is unlimited.
    #[codama(type = number(u64))]
    pub max_deployed: U64,
    /// Deposit capacity ceiling; `0` disables deposits, `u64::MAX` is unlimited.
    #[codama(type = number(u64))]
    pub max_deposit: U64,
}

impl ZcValidate for AssetAdd {
    fn validate_ref(_: &Self) -> Result<(), ZeroPodError> {
        Ok(()) // plain pods, every bit pattern is valid
    }
}

// SAFETY: same layout argument as `AssetUpdate` above.
unsafe impl ZcElem for AssetAdd {}

/// Configure a free (`flags == 0`) `Vault.positions` slot — the post-init path
/// for onboarding a new protocol integration. Occupied slots stay immutable
/// (only risk parameters move, via [`PositionUpdate`]); the new slot starts
/// never-priced, so `validate_aum` skips it until its first mark.
#[derive(SchemaRead, SchemaWrite, CodamaType, Clone, Copy)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct PositionAdd {
    /// Index of a free slot in `Vault.positions`.
    pub index: u8,
    /// Protocol this position belongs to (at most one *enabled* slot each).
    pub protocol: u8,
    /// `POSITION_FLAG_CURATOR_MASK` bits; `0` is rejected (nothing to add).
    pub flags: u8,
    /// Freshness window for the new slot's mark — same three-way reading as
    /// [`PositionInit::max_staleness_seconds`]: `None` = the global default,
    /// `Some(0)` = the week-long ceiling, `Some(n)` = `n` seconds.
    ///
    /// [`PositionInit::max_staleness_seconds`]: crate::arguments::PositionInit::max_staleness_seconds
    #[codama(type = option(number(u32, le), fixed))]
    pub max_staleness_seconds: PodOption<U32>,
    /// Drift budget for the new slot: `None` or `Some(0)` uses the global
    /// default; `Some(u16::MAX)` waives it; other `Some(n)` = `n` bps.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_drift_bps: PodOption<U16>,
}

impl ZcValidate for PositionAdd {
    fn validate_ref(value: &Self) -> Result<(), ZeroPodError> {
        PodOption::validate_ref(&value.max_staleness_seconds)?;
        PodOption::validate_ref(&value.max_drift_bps)
    }
}

// SAFETY: same layout argument as `AssetUpdate` above.
unsafe impl ZcElem for PositionAdd {}

#[derive(SchemaRead, SchemaWrite, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct UpdateVaultArgs {
    #[codama(type = option(number(u8), fixed))]
    pub paused: PodOption<u8>,
    #[codama(type = option(number(u64, le), fixed))]
    pub timelock_delay: PodOption<U64>,
    #[codama(type = option(public_key, fixed))]
    pub pending_curator: PodOption<Address>,
    /// Annual curator management fee; the global protocol rate is additive.
    #[codama(type = option(number(u16, le), fixed))]
    pub management_fee_bps: PodOption<U16>,
    /// Curator HWM performance fee in bps, capped at
    /// `MAX_PERFORMANCE_FEE_BPS`; same crystallize-first gate as
    /// `management_fee_bps`. The global protocol rate is additive.
    #[codama(type = option(number(u16, le), fixed))]
    pub performance_fee_bps: PodOption<U16>,
    #[codama(type = option(number(u16, le), fixed))]
    pub instant_redeem_fee_bps: PodOption<U16>,
    /// Asset row (`Vault.assets` index) plus the fields to apply to it.
    #[codama(type = option(link("assetUpdate"), fixed))]
    pub asset: PodOption<AssetUpdate>,
    /// Position row (`Vault.positions` index) plus the risk fields to apply
    /// to it.
    #[codama(type = option(link("positionUpdate"), fixed))]
    pub position: PodOption<PositionUpdate>,
    /// Ceiling on the transient flash liability, in numéraire micro-units.
    /// `Some(0)` disables the path outright.
    #[codama(type = option(number(u64, le), fixed))]
    pub max_flash_liability: PodOption<U64>,
    /// Discount `validate_aum` applies to every sink mark.
    #[codama(type = option(number(u16, le), fixed))]
    pub sink_haircut_bps: PodOption<U16>,
    /// Freshness window every sink mark is held to; `0` = the global default.
    #[codama(type = option(number(u64, le), fixed))]
    pub sink_max_staleness_seconds: PodOption<U64>,
    /// Vault-wide ceiling on aggregate first sink marks, in bps of validated
    /// AUM; `0` selects the global default, `u16::MAX` waives the ceiling,
    /// and `None` leaves it unchanged. Later marks are held to `MAX_DRIFT_BPS`
    /// per window.
    #[codama(type = option(number(u16, le), fixed))]
    pub sink_bootstrap_cap_bps: PodOption<U16>,
    /// How many sink slots may be claimed, up to `MAX_SINKS`. `Some(0)` stops
    /// new claims; slots already occupied keep their marks.
    #[codama(type = option(number(u8), fixed))]
    pub sink_capacity: PodOption<u8>,
    /// Write a sink's mark off to zero, keeping its mint bound so the balance
    /// stays sellable.
    #[codama(type = option(number(u8), fixed))]
    pub sink_release: PodOption<u8>,
    /// Onboard a new asset into the first free `Vault.assets` row; the mint is
    /// the instruction's trailing account. See [`AssetAdd`].
    #[codama(type = option(link("assetAdd"), fixed))]
    pub add_asset: PodOption<AssetAdd>,
    /// Onboard a new position into a free `Vault.positions` slot. See
    /// [`PositionAdd`].
    #[codama(type = option(link("positionAdd"), fixed))]
    pub add_position: PodOption<PositionAdd>,
    /// Retire an asset row (index into `Vault.assets`) and free its slot. Only
    /// an emptied row goes: deposits disabled, `deployed == 0`, and the
    /// canonical custody ATA — the trailing account, after `add_asset`'s mint
    /// when both are given — holds zero. Tokens donated to that ATA afterwards
    /// are invisible to AUM until the mint is re-onboarded.
    #[codama(type = option(number(u8), fixed))]
    pub remove_asset: PodOption<u8>,
    /// Retire a position row (index into `Vault.positions`) and free its slot.
    /// Only a slot with no live mark goes (never-priced, reset, or marked to
    /// zero) — a live mark is AUM, and removal must never move AUM.
    #[codama(type = option(number(u8), fixed))]
    pub remove_position: PodOption<u8>,
    /// Curator attestation that an explicit DEBT position (index into
    /// `Vault.positions`) is fully repaid. Records a fresh, initialized zero
    /// mark without changing drift limits, then invalidates cached AUM. The
    /// curator must verify repayment off-chain: opaque positions carry no
    /// protocol account from which the program can prove it. May be combined
    /// with `remove_position` for the same slot.
    #[codama(type = option(number(u8), fixed))]
    pub retire_debt: PodOption<u8>,
    /// Future timestamp tolerance for the row selected by `asset.index`.
    /// None leaves it unchanged. Some(0) rejects any future timestamp;
    /// every positive u32 value applies independently of the age limit.
    /// Appended so the pre-upgrade UpdateVault encoding remains a prefix.
    #[codama(type = option(number(u32, le), fixed))]
    pub asset_max_future_skew_seconds: PodOption<U32>,
    /// Validated AUM age limit; zero selects 60s, positive u32 values apply exactly.
    #[codama(type = option(number(u32, le), fixed))]
    pub max_aum_staleness_seconds: PodOption<U32>,
    /// Vault-retained fee at queued settlement; Some(0) disables, capped at 1,000 bps.
    #[codama(type = option(number(u16, le), fixed))]
    pub queued_redeem_fee_bps: PodOption<U16>,
}

impl ZcValidate for UpdateVaultArgs {
    fn validate_ref(value: &Self) -> Result<(), zeropod::Error> {
        ZcValidate::validate_ref(&value.paused)?;
        ZcValidate::validate_ref(&value.timelock_delay)?;
        ZcValidate::validate_ref(&value.pending_curator)?;
        ZcValidate::validate_ref(&value.management_fee_bps)?;
        ZcValidate::validate_ref(&value.performance_fee_bps)?;
        ZcValidate::validate_ref(&value.instant_redeem_fee_bps)?;
        ZcValidate::validate_ref(&value.asset)?;
        ZcValidate::validate_ref(&value.position)?;
        ZcValidate::validate_ref(&value.max_flash_liability)?;
        ZcValidate::validate_ref(&value.sink_haircut_bps)?;
        ZcValidate::validate_ref(&value.sink_max_staleness_seconds)?;
        ZcValidate::validate_ref(&value.sink_bootstrap_cap_bps)?;
        ZcValidate::validate_ref(&value.sink_capacity)?;
        ZcValidate::validate_ref(&value.sink_release)?;
        ZcValidate::validate_ref(&value.add_asset)?;
        ZcValidate::validate_ref(&value.add_position)?;
        ZcValidate::validate_ref(&value.remove_asset)?;
        ZcValidate::validate_ref(&value.remove_position)?;
        ZcValidate::validate_ref(&value.retire_debt)?;
        ZcValidate::validate_ref(&value.asset_max_future_skew_seconds)?;
        ZcValidate::validate_ref(&value.max_aum_staleness_seconds)?;
        ZcValidate::validate_ref(&value.queued_redeem_fee_bps)?;
        Ok(())
    }
}
