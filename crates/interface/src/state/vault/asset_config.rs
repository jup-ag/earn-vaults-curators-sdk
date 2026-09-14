use {
    crate::constants::MAX_STALENESS_SECONDS_U32,
    codama_macros::CodamaType,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI32, PodI64, PodU16, PodU32, PodU64},
};

pub const ASSET_FLAG_DEPOSIT_ENABLED: u8 = 1 << 0;
pub const ASSET_FLAG_PRICE_IN_AUM: u8 = 1 << 1;
pub const ASSET_FLAG_CHAINLINK: u8 = 1 << 2;
pub const ASSET_FLAG_MANUAL_PRICE: u8 = 1 << 3;
pub const ASSET_FLAG_CURATOR_MASK: u8 = ASSET_FLAG_CHAINLINK;
pub const ASSET_FLAG_UPDATE_MASK: u8 = ASSET_FLAG_CHAINLINK | ASSET_FLAG_DEPOSIT_ENABLED;

#[derive(SchemaRead, SchemaWrite, Clone, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AssetConfig {
    pub mint: Address,
    pub oracle: Address,
    pub flags: u8,
    pub decimals: u8,
    pub ata_bump: u8,
    /// Curator-set freshness window in seconds; `0` means
    /// [`DEFAULT_ASSET_STALENESS_SECONDS`]. Stored as `u32` because the write
    /// path caps it at one week.
    ///
    /// [`DEFAULT_ASSET_STALENESS_SECONDS`]: crate::constants::DEFAULT_ASSET_STALENESS_SECONDS
    #[codama(type = number(u32))]
    pub max_staleness_seconds: PodU32,
    /// The decimal scale this row's price source quotes its raw mantissa at,
    /// capped at `MAX_ORACLE_REPORT_DECIMALS`. `0` means the source states its
    /// own exponent or its documented default applies.
    ///
    /// Some sources state no exponent at all: a Chainlink Data Streams v3 report
    /// is 8 or 18 decimals *depending on the stream*, and nothing in the body
    /// says which. For those this is the scale, and getting it wrong by `1e10`
    /// prices the asset low enough to redeem the whole holding for dust. For a
    /// source that does state its exponent (a Pyth `PriceUpdateV2`) a non-zero
    /// value is the *expected* scale instead, and a reading that disagrees is
    /// rejected — a feed that silently re-scales stops the vault rather than
    /// re-pricing it.
    pub report_decimals: u8,
    #[codama(type = number(u16))]
    pub max_confidence_bps: PodU16,
    #[codama(type = number(u64))]
    pub deployed: PodU64,
    #[codama(type = number(u64))]
    pub max_deployed: PodU64,
    #[codama(type = number(u64))]
    pub max_deposit: PodU64,
    #[codama(type = number(i64))]
    pub last_price: PodI64,
    #[codama(type = number(u64))]
    pub last_conf: PodU64,
    #[codama(type = number(i32))]
    pub last_price_expo: PodI32,
    #[codama(type = number(i64))]
    pub last_publish_time: PodI64,
}

impl AssetConfig {
    #[inline]
    pub const fn zero() -> Self {
        Self {
            mint: Address::new_from_array([0; 32]),
            oracle: Address::new_from_array([0; 32]),
            flags: 0,
            decimals: 0,
            ata_bump: 255,
            max_staleness_seconds: MAX_STALENESS_SECONDS_U32,
            report_decimals: 0,
            max_confidence_bps: PodU16::ZERO,
            deployed: PodU64::ZERO,
            max_deployed: PodU64::ZERO,
            max_deposit: PodU64::ZERO,
            last_price: PodI64::ZERO,
            last_conf: PodU64::ZERO,
            last_price_expo: PodI32::ZERO,
            last_publish_time: PodI64::ZERO,
        }
    }

    /// Whether this row is configured at all. An unwritten row is all-zero, and
    /// an all-zero `mint` is the system program's address — a *real* address, so
    /// a lookup by mint must exclude these rows explicitly or
    /// `find_asset(&Address::default())` answers with one.
    #[inline]
    pub fn configured(&self) -> bool {
        !solana_address::address_eq(&self.mint, &Address::new_from_array([0; 32]))
    }

    #[inline]
    pub const fn deposit_enabled(&self) -> bool {
        self.flags & ASSET_FLAG_DEPOSIT_ENABLED != 0
    }

    #[inline]
    pub const fn price_in_aum(&self) -> bool {
        self.flags & ASSET_FLAG_PRICE_IN_AUM != 0
    }

    /// `true` when `oracle` is a Chainlink Data Streams feed id — refreshed by
    /// `refresh_prices` with a DON report, not with a `PriceUpdateV2` account.
    #[inline]
    pub const fn chainlink(&self) -> bool {
        self.flags & ASSET_FLAG_CHAINLINK != 0
    }

    /// `true` when the row carries **no feed** and is therefore priced by hand.
    #[inline]
    pub const fn manual_price(&self) -> bool {
        self.flags & ASSET_FLAG_MANUAL_PRICE != 0
    }
}
