use {
    crate::constants::{DEFAULT_ASSET_STALENESS_SECONDS, MAX_DRIFT_BPS},
    codama_macros::CodamaType,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{I32, I64, U16, U32, U64},
};

pub const ASSET_FLAG_DEPOSIT_ENABLED: u8 = 1 << 0;
pub const ASSET_FLAG_PRICE_IN_AUM: u8 = 1 << 1;
pub const ASSET_FLAG_CHAINLINK: u8 = 1 << 2;
pub const ASSET_FLAG_MANUAL_PRICE: u8 = 1 << 3;
/// Validation-owned: this priced row had no custody balance in the AUM snapshot.
pub const ASSET_FLAG_ZERO_AT_PRICING: u8 = 1 << 4;
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
    /// Maximum observation age for valuation and settlement, for every source.
    /// Zero selects the 15s default; positive values apply without a ceiling.
    #[codama(type = number(u32))]
    pub max_staleness_seconds: U32,
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
    pub max_confidence_bps: U16,
    /// Hand-priced row's delegate drift cap per window, 1..=200 bps.
    /// Defaults to 200; also bounds the manual-price entry reserve.
    #[codama(type = number(u16))]
    pub max_drift_bps: U16,
    #[codama(type = number(u64))]
    pub deployed: U64,
    /// Strategy-outflow ceiling; `0` disables deployment, `u64::MAX` is unlimited.
    #[codama(type = number(u64))]
    pub max_deployed: U64,
    /// Deposit capacity ceiling; `0` disables deposits, `u64::MAX` is unlimited.
    #[codama(type = number(u64))]
    pub max_deposit: U64,
    #[codama(type = number(i64))]
    pub last_price: I64,
    #[codama(type = number(u64))]
    pub last_conf: U64,
    #[codama(type = number(i32))]
    pub last_price_expo: I32,
    #[codama(type = number(i64))]
    pub last_publish_time: I64,
    /// Quote leg of a composed price: the row's price is `oracle` × this feed
    /// (sUSDai/USD = sUSDai/USDai × USDai/USD), and the `last_*` reading above
    /// is the composed one. Zero = a single-feed row.
    pub quote_oracle: Address,
    /// [`Self::report_decimals`] for the quote leg.
    pub quote_report_decimals: u8,
    /// Publish time of each leg last composed: the per-feed replay guard.
    /// `last_publish_time` is the older of the two.
    #[codama(type = number(i64))]
    pub base_publish_time: I64,
    #[codama(type = number(i64))]
    pub quote_publish_time: I64,
    /// Maximum seconds an observation may lead the on-chain clock, for every source.
    /// Zero rejects any future timestamp; positive values apply exactly.
    #[codama(type = number(u32))]
    pub max_future_skew_seconds: U32,
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
            max_staleness_seconds: U32::from_bytes(
                (DEFAULT_ASSET_STALENESS_SECONDS as u32).to_le_bytes(),
            ),
            report_decimals: 0,
            max_confidence_bps: U16::ZERO,
            max_drift_bps: U16::from_bytes((MAX_DRIFT_BPS as u16).to_le_bytes()),
            deployed: U64::ZERO,
            max_deployed: U64::ZERO,
            max_deposit: U64::ZERO,
            last_price: I64::ZERO,
            last_conf: U64::ZERO,
            last_price_expo: I32::ZERO,
            last_publish_time: I64::ZERO,
            quote_oracle: Address::new_from_array([0; 32]),
            quote_report_decimals: 0,
            base_publish_time: I64::ZERO,
            quote_publish_time: I64::ZERO,
            max_future_skew_seconds: U32::from_bytes(
                (DEFAULT_ASSET_STALENESS_SECONDS as u32).to_le_bytes(),
            ),
        }
    }

    /// Whether the row's price is composed from two feeds.
    #[inline]
    pub fn composed(&self) -> bool {
        !solana_address::address_eq(&self.quote_oracle, &Address::new_from_array([0; 32]))
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

    #[inline]
    pub const fn zero_at_pricing(&self) -> bool {
        self.flags & ASSET_FLAG_ZERO_AT_PRICING != 0
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
