use {
    crate::state::ProtocolPolicy,
    codama_macros::CodamaType,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{
        ZcElem, ZcValidate,
        error::ZeroPodError,
        pod::{PodI64, PodOption, PodU16, PodU32, PodU64},
    },
};

#[derive(SchemaRead, SchemaWrite, CodamaType, Clone, Copy)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AssetUpdate {
    pub index: u8,
    /// New feed id — the rotation path for a deprecated feed. A zero feed id
    /// makes the row hand-priced instead (`refresh_prices` skips it, the price
    /// comes from `price_opaque`); a deposit-enabled row may not go feedless.
    /// Either way the cached reading is retired: a new source, a new price.
    #[codama(type = option(public_key, fixed))]
    pub oracle: PodOption<Address>,
    /// Curator-settable `ASSET_FLAG_*` bits (`ASSET_FLAG_CURATOR_MASK`, today
    /// just `CHAINLINK`) — the provider switch. Bits outside the mask are
    /// rejected; switching retires the cached reading like a rotation does.
    #[codama(type = option(number(u8), fixed))]
    pub flags: PodOption<u8>,
    /// New [`AssetConfig::report_decimals`] for this row; `Some(0)` hands the
    /// scale back to the source. Retires the cached reading like a rotation does
    /// — the cache holds one exponent and nothing downstream can tell it was
    /// struck under the old scale.
    ///
    /// [`AssetConfig::report_decimals`]: crate::state::AssetConfig::report_decimals
    #[codama(type = option(number(u8), fixed))]
    pub report_decimals: PodOption<u8>,
    /// Per-asset staleness override; `0` means the global default.
    #[codama(type = option(number(i64, le), fixed))]
    pub max_staleness_seconds: PodOption<PodI64>,
    /// Confidence cap in bps of price; `0` means the global default.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_confidence_bps: PodOption<PodU16>,
    /// Per-asset strategy-outflow ceiling; `0` = unlimited.
    #[codama(type = option(number(u64, le), fixed))]
    pub max_deployed: PodOption<PodU64>,
    /// Restate the row's `deployed` counter outright (raw token units). Only
    /// routed settlements move it, so custody that returns outside the program
    /// — a CCIP OffRamp delivery into the custody ATA — leaves it overstated
    /// until the curator corrects it here. Telemetry and the `max_deployed`
    /// basis, never AUM.
    #[codama(type = option(number(u64, le), fixed))]
    pub deployed: PodOption<PodU64>,
    /// Per-asset deposit capacity ceiling; `0` = unlimited.
    #[codama(type = option(number(u64, le), fixed))]
    pub max_deposit: PodOption<PodU64>,
    /// Toggle deposits for this asset (`1` enable / `0` disable). Its own
    /// field, not a `flags` bit, so a provider switch can't silently rewrite
    /// the deposit gate. A hand-priced row may not be deposit-enabled. Among
    /// enabled rows, the lowest index is canonical for instant settlement;
    /// later rows are queued-only.
    #[codama(type = option(number(u8), fixed))]
    pub deposit_enabled: PodOption<u8>,
}

impl ZcValidate for AssetUpdate {
    fn validate_ref(value: &Self) -> Result<(), ZeroPodError> {
        PodOption::validate_ref(&value.oracle)?;
        PodOption::validate_ref(&value.flags)?;
        PodOption::validate_ref(&value.report_decimals)?;
        PodOption::validate_ref(&value.max_staleness_seconds)?;
        PodOption::validate_ref(&value.max_confidence_bps)?;
        PodOption::validate_ref(&value.max_deployed)?;
        PodOption::validate_ref(&value.deployed)?;
        PodOption::validate_ref(&value.max_deposit)?;
        PodOption::validate_ref(&value.deposit_enabled)
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
    pub max_staleness_seconds: PodOption<PodU64>,
    /// Per-position drift override; `0` means the global default.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_drift_bps: PodOption<PodU16>,
    /// `Some(1)` resets a non-DEBT row to never-priced (clears `PRICED` /
    /// `INITIALIZED`, zeroes the mark), so the next `price_opaque` bootstraps
    /// again against the last validated AUM. The unbrick path for an asset-like
    /// slot frozen at a dust/wrong first mark. DEBT rows may never reset: omitting
    /// a live liability would overstate NAV.
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
    /// `ASSET_FLAG_CHAINLINK` and/or `ASSET_FLAG_DEPOSIT_ENABLED` (a
    /// hand-priced row may not be deposit-enabled).
    pub flags: u8,
    /// The decimal scale the feed quotes its raw mantissa at; `0` leaves the
    /// source's own. See [`AssetUpdate::report_decimals`].
    pub report_decimals: u8,
    /// Strategy-outflow ceiling; `0` = unlimited.
    #[codama(type = number(u64))]
    pub max_deployed: PodU64,
    /// Deposit capacity ceiling; `0` = unlimited.
    #[codama(type = number(u64))]
    pub max_deposit: PodU64,
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
    pub max_staleness_seconds: PodOption<PodU32>,
    /// Drift budget for the new slot — `None` = the global default, `Some(0)`
    /// waives it, `Some(n)` = `n` bps.
    #[codama(type = option(number(u16, le), fixed))]
    pub max_drift_bps: PodOption<PodU16>,
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
    /// Set one protocol's curator policy (Offerbook `LoanLimits`, CCIP
    /// `CcipLane`, Nest `NestRoute`, …) by tag; an all-zero payload disables that protocol and
    /// frees its slot. See [`ProtocolPolicy`].
    #[codama(type = option(link("protocolPolicy"), fixed))]
    pub policy: PodOption<ProtocolPolicy>,
    #[codama(type = option(number(u8), fixed))]
    pub paused: PodOption<u8>,
    #[codama(type = option(number(u64, le), fixed))]
    pub timelock_delay: PodOption<PodU64>,
    #[codama(type = option(public_key, fixed))]
    pub pending_curator: PodOption<Address>,
    /// Annual curator management fee; the global protocol rate is additive.
    #[codama(type = option(number(u16, le), fixed))]
    pub management_fee_bps: PodOption<PodU16>,
    /// Curator HWM performance fee in bps, capped at
    /// `MAX_PERFORMANCE_FEE_BPS`; same crystallize-first gate as
    /// `management_fee_bps`. The global protocol rate is additive.
    #[codama(type = option(number(u16, le), fixed))]
    pub performance_fee_bps: PodOption<PodU16>,
    #[codama(type = option(number(u16, le), fixed))]
    pub instant_redeem_fee_bps: PodOption<PodU16>,
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
    pub max_flash_liability: PodOption<PodU64>,
    /// Discount `validate_aum` applies to every sink mark.
    #[codama(type = option(number(u16, le), fixed))]
    pub sink_haircut_bps: PodOption<PodU16>,
    /// Freshness window every sink mark is held to; `0` = the global default.
    #[codama(type = option(number(u64, le), fixed))]
    pub sink_max_staleness_seconds: PodOption<PodU64>,
    /// Vault-wide ceiling on aggregate first sink marks, in bps of validated
    /// AUM; `0` = the global default. Not the walk rate — later marks are held
    /// to `MAX_DRIFT_BPS` per window.
    #[codama(type = option(number(u16, le), fixed))]
    pub sink_bootstrap_cap_bps: PodOption<PodU16>,
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
}
