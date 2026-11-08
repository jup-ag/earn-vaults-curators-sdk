use {
    crate::encoding,
    codama_macros::CodamaType,
    encoding::{PodOption, PodString, ZcValidate},
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{I64, U16, U32, U64},
};

/// One opaque position, laid out at its own slot in
/// `InitializeVaultArgs::positions` (index = slot). `protocol` names the
/// integration; at most one *enabled* slot per protocol.
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct PositionInit {
    pub protocol: u8,
    pub flags: u8,
    /// Freshness window this slot's mark is held to. `None` = the global
    /// default ([`MAX_STALENESS_SECONDS`]), `Some(0)` = the widest window the
    /// program allows ([`MAX_PARAM_STALENESS_SECONDS`], a week — never
    /// unbounded: a `REQUIRED_IN_AUM` mark that cannot go stale is the edge the
    /// ceiling bounds), `Some(n)` = `n` seconds, capped at the same. `u32`
    /// because that cap is a week either way, and `InitializeVault` is a few
    /// bytes a slot from overrunning one transaction packet.
    ///
    /// [`MAX_STALENESS_SECONDS`]: crate::constants::MAX_STALENESS_SECONDS
    /// [`MAX_PARAM_STALENESS_SECONDS`]: crate::constants::MAX_PARAM_STALENESS_SECONDS
    #[codama(type = option(number(u32, le), fixed))]
    pub max_staleness_seconds: PodOption<U32>,
    /// Drift budget per window for this slot's mark. `None` or `Some(0)` uses
    /// the global default ([`MAX_DRIFT_BPS`]). `Some(u16::MAX)` waives the
    /// budget, including bootstrap; other `Some(n)` sets a budget of `n` bps.
    ///
    /// [`MAX_DRIFT_BPS`]: crate::constants::MAX_DRIFT_BPS
    #[codama(type = option(number(u16, le), fixed))]
    pub max_drift_bps: PodOption<U16>,
}

#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AssetInit {
    pub oracle: Address,
    /// Curator-settable `ASSET_FLAG_*` bits — `ASSET_FLAG_CHAINLINK` to read
    /// `oracle` as a Chainlink Data Streams feed id instead of a Pyth one.
    /// Bits outside `ASSET_FLAG_CURATOR_MASK` are rejected; `DEPOSIT_ENABLED`
    /// and `PRICE_IN_AUM` are derived from `based_assets` / the mint list.
    pub flags: u8,
    /// The decimal scale this feed quotes its raw mantissa at; `0` leaves the
    /// source's own (a Pyth exponent, a Data Streams stream's documented 18).
    /// Required for a Data Streams stream that quotes at anything else — the v3
    /// body cannot state it. See [`AssetConfig::report_decimals`].
    ///
    /// [`AssetConfig::report_decimals`]: crate::state::AssetConfig::report_decimals
    pub report_decimals: u8,
    /// Strategy-outflow ceiling; `0` disables deployment, `u64::MAX` is unlimited.
    #[codama(type = number(u64))]
    pub max_deployed: U64,
    /// Deposit capacity ceiling; `0` disables deposits, `u64::MAX` is unlimited.
    #[codama(type = number(u64))]
    pub max_deposit: U64,
}

#[derive(SchemaRead, SchemaWrite, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct InitializeVaultArgs {
    /// Bitmask of deposit-enabled asset rows. The lowest-index enabled row is
    /// the canonical asset for `SubscribeDirect` and `RedeemDirect`; additional
    /// enabled rows may settle only through the curator-gated queued paths.
    pub based_assets: u8,
    pub assets: [AssetInit; 8],
    // Literal: codama cannot resolve a const as a fixed-array length.
    pub positions: [PositionInit; 8],
    #[codama(type = number(i64))]
    pub created_at: I64,
    /// Seconds between `Redeem` and `ClaimRedeem`. `None` = the baseline
    /// posture ([`DEFAULT_TIMELOCK_DELAY`], 1 day), `Some(0)` = no cool-down at
    /// all, `Some(n)` = `n` seconds, capped at [`MAX_TIMELOCK_DELAY`].
    ///
    /// [`DEFAULT_TIMELOCK_DELAY`]: crate::constants::DEFAULT_TIMELOCK_DELAY
    /// [`MAX_TIMELOCK_DELAY`]: crate::constants::MAX_TIMELOCK_DELAY
    #[codama(type = option(number(u64, le), fixed))]
    pub timelock_delay: PodOption<U64>,
    /// Annual curator management fee on AUM, in bps; capped at
    /// `MAX_MANAGEMENT_FEE_BPS`. The global protocol management fee is
    /// additive and is not configured per vault.
    #[codama(type = number(u16))]
    pub management_fee_bps: U16,
    /// Curator HWM performance fee on NAV gains, in bps; capped at
    /// `MAX_PERFORMANCE_FEE_BPS`. `0` disables the curator component. The
    /// global protocol performance fee remains additive.
    #[codama(type = number(u16))]
    pub performance_fee_bps: U16,
    /// Exit fee on the instant (`RedeemDirect`) path. `None` leaves the path
    /// disabled; `Some(n)` enables it at `n` bps, which must sit inside
    /// `MIN_INSTANT_REDEEM_FEE_BPS..=MAX_INSTANT_REDEEM_FEE_BPS` — the floor is
    /// what keeps an instant exit from front-running a pending mark.
    #[codama(type = option(number(u16, le), fixed))]
    pub instant_redeem_fee_bps: PodOption<U16>,
    /// Maximum transient flash liability in numéraire micro-units. `0` leaves
    /// the path disabled.
    #[codama(type = number(u64))]
    pub max_flash_liability: U64,
    /// `Vault.sink_haircut_bps` — the discount `validate_aum` applies to every
    /// sink mark.
    #[codama(type = number(u16))]
    pub sink_haircut_bps: U16,
    /// `Vault.sink_max_staleness_seconds`. `None` = the global default,
    /// `Some(0)` = the week-long ceiling, `Some(n)` = `n` seconds (capped at
    /// that week, hence `u32` — see [`PositionInit::max_staleness_seconds`]).
    #[codama(type = option(number(u32, le), fixed))]
    pub sink_max_staleness_seconds: PodOption<U32>,
    /// `Vault.sink_bootstrap_cap_bps` — vault-wide ceiling on aggregate first
    /// sink marks, in bps of validated AUM. `None` or `Some(0)` uses the global
    /// default; `Some(u16::MAX)` waives the ceiling; other `Some(n)` = `n` bps.
    #[codama(type = option(number(u16, le), fixed))]
    pub sink_bootstrap_cap_bps: PodOption<U16>,
    /// `Vault.sink_capacity` — how many sink slots may be claimed, up to
    /// `MAX_SINKS`. `0` leaves seized-collateral custody disabled.
    pub sink_capacity: u8,
    #[codama(type = struct(field("len", number(u8)), field("value", fixed_size(string, 50))))]
    pub metadata_name: PodString<50>,
    #[codama(type = struct(field("len", number(u8)), field("value", fixed_size(string, 5))))]
    pub metadata_symbol: PodString<5>,
    #[codama(type = struct(field("len", number(u8)), field("value", fixed_size(string, 200))))]
    pub metadata_uri: PodString<200>,
    /// Curator fee on settled queued shares; 0 disables, capped at 1,000 bps.
    #[codama(type = number(u16))]
    pub queued_redeem_fee_bps: U16,
}

impl ZcValidate for InitializeVaultArgs {
    fn validate_ref(value: &Self) -> Result<(), zeropod::Error> {
        ZcValidate::validate_ref(&value.timelock_delay)?;
        ZcValidate::validate_ref(&value.instant_redeem_fee_bps)?;
        ZcValidate::validate_ref(&value.sink_max_staleness_seconds)?;
        ZcValidate::validate_ref(&value.sink_bootstrap_cap_bps)?;
        ZcValidate::validate_ref(&value.metadata_name)?;
        ZcValidate::validate_ref(&value.metadata_symbol)?;
        ZcValidate::validate_ref(&value.metadata_uri)?;
        for position in &value.positions {
            ZcValidate::validate_ref(&position.max_staleness_seconds)?;
            ZcValidate::validate_ref(&position.max_drift_bps)?;
        }
        Ok(())
    }
}
