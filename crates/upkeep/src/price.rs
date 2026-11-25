//! Row prices, gated and struck the way the vault program reads them.
//!
//! Valuations price every holding off the reading `RefreshPrices` cached on the
//! vault's asset row, through the same gate `ValidateAum` runs: positive price,
//! inside the row's staleness window and future-skew allowance, confidence
//! inside `max_confidence_bps`. A keeper that refreshes in the same transaction
//! values off the pre-refresh reading; the drift budget absorbs the gap.

use {
    crate::{Error, PriceFault, Result},
    earn_vaults_sdk::{
        constants::{
            DEFAULT_ASSET_STALENESS_SECONDS, DEFAULT_MAX_CONFIDENCE_BPS, MAX_STALENESS_SECONDS,
            NUMERAIRE_DECIMALS,
        },
        state::{AssetConfig, Vault},
    },
    solana_address::Address,
};

/// Which side of the confidence band a holding is struck at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// Something the vault holds or is owed: price − confidence, rounded down.
    Asset,
    /// Something the vault owes: price + confidence, rounded up.
    Liability,
}

/// The cached reading a valuation used, as logged by the keeper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowPrice {
    pub row: u8,
    pub mint: Address,
    pub decimals: u8,
    pub price: i64,
    pub conf: u64,
    pub exponent: i32,
    pub publish_time: i64,
    /// The last second this reading still passes the gate.
    pub valid_until: i64,
}

/// The configured row for `mint`.
pub fn row<'a>(vault: &'a Vault, mint: &Address) -> Option<(u8, &'a AssetConfig)> {
    vault
        .assets
        .iter()
        .enumerate()
        .find(|(_, a)| a.configured() && a.mint == *mint)
        .map(|(i, a)| (i as u8, a))
}

/// Read row `index` through the program's freshness and confidence gate at `now`.
pub fn gate(index: u8, asset: &AssetConfig, now: i64) -> Result<RowPrice> {
    let fault = |reason| Error::Price {
        row: index,
        mint: asset.mint,
        reason,
    };
    let price = asset.last_price.get();
    let conf = asset.last_conf.get();
    let publish_time = asset.last_publish_time.get();
    let skew = i64::from(asset.max_future_skew_seconds.get());
    if asset.composed()
        && asset
            .base_publish_time
            .get()
            .max(asset.quote_publish_time.get())
            .saturating_sub(now)
            > skew
    {
        return Err(fault(PriceFault::Future));
    }
    if price <= 0 || price as u128 + conf as u128 > i64::MAX as u128 {
        return Err(fault(PriceFault::NonPositive));
    }
    let window = match asset.max_staleness_seconds.get() {
        0 => DEFAULT_ASSET_STALENESS_SECONDS,
        seconds => i64::from(seconds),
    };
    let window = if window <= 0 {
        i64::from(MAX_STALENESS_SECONDS)
    } else {
        window
    };
    if now.saturating_sub(publish_time) > window {
        return Err(fault(PriceFault::Stale));
    }
    if publish_time.saturating_sub(now) > skew {
        return Err(fault(PriceFault::Future));
    }
    let cap = match asset.max_confidence_bps.get() {
        0 => DEFAULT_MAX_CONFIDENCE_BPS,
        bps => bps,
    };
    if u128::from(conf).saturating_mul(10_000) > (price as u128).saturating_mul(u128::from(cap)) {
        return Err(fault(PriceFault::Confidence));
    }
    Ok(RowPrice {
        row: index,
        mint: asset.mint,
        decimals: asset.decimals,
        price,
        conf,
        exponent: asset.last_price_expo.get(),
        publish_time,
        valid_until: publish_time.saturating_add(window),
    })
}

/// Largest `n` with `10^n <= u128::MAX`.
const MAX_POW10: i64 = 38;

impl RowPrice {
    /// `amount` raw units in numéraire micro-units, struck and rounded for `side`.
    pub fn value(&self, amount: u128, side: Side) -> Result<u64> {
        if amount == 0 {
            return Ok(0);
        }
        let struck = match side {
            Side::Asset => self
                .price
                .saturating_sub(i64::try_from(self.conf).unwrap_or(i64::MAX)),
            Side::Liability => self
                .price
                .checked_add(i64::try_from(self.conf).map_err(|_| Error::Overflow("price band"))?)
                .ok_or(Error::Overflow("price band"))?,
        };
        if struck <= 0 {
            return Err(Error::Price {
                row: self.row,
                mint: self.mint,
                reason: PriceFault::NoFloor,
            });
        }
        let mantissa = amount
            .checked_mul(struck as u128)
            .ok_or(Error::Overflow("holding value"))?;
        let net =
            i64::from(self.exponent) + i64::from(NUMERAIRE_DECIMALS) - i64::from(self.decimals);
        let value = if net >= 0 {
            if net > MAX_POW10 {
                return Err(Error::Overflow("holding value"));
            }
            mantissa
                .checked_mul(10u128.pow(net as u32))
                .ok_or(Error::Overflow("holding value"))?
        } else if -net > MAX_POW10 {
            // Smaller than one micro-unit per 10^38 raw units.
            match side {
                Side::Asset => 0,
                Side::Liability => 1,
            }
        } else {
            let scale = 10u128.pow((-net) as u32);
            match side {
                Side::Asset => mantissa / scale,
                Side::Liability => mantissa.div_ceil(scale),
            }
        };
        u64::try_from(value).map_err(|_| Error::Overflow("holding value"))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use {
        super::*,
        earn_vaults_sdk::zeropod::{I32, I64, U32, U64},
    };

    pub(crate) fn asset(mint: Address, price: i64, conf: u64, expo: i32, at: i64) -> AssetConfig {
        let mut a = AssetConfig::zero();
        a.mint = mint;
        a.decimals = 6;
        a.flags = earn_vaults_sdk::state::ASSET_FLAG_PRICE_IN_AUM;
        a.last_price = I64::from(price);
        a.last_conf = U64::from(conf);
        a.last_price_expo = I32::from(expo);
        a.last_publish_time = I64::from(at);
        a
    }

    fn usdc() -> AssetConfig {
        asset(
            Address::new_from_array([1; 32]),
            99_990_000,
            20_000,
            -8,
            1_000,
        )
    }

    #[test]
    fn assets_round_down_at_the_floor_and_liabilities_round_up_at_the_ceiling() {
        let p = gate(0, &usdc(), 1_000).unwrap();
        // 1.5 USDC at $0.9997 floor and $1.0001 ceiling.
        assert_eq!(p.value(1_500_000, Side::Asset), Ok(1_499_550));
        assert_eq!(p.value(1_500_000, Side::Liability), Ok(1_500_150));
        // One raw unit of a 9-decimal token: 0.99970 vs 1.00010 nano-dollars.
        let nine = RowPrice { decimals: 9, ..p };
        assert_eq!(nine.value(1, Side::Asset), Ok(0));
        assert_eq!(nine.value(1, Side::Liability), Ok(1));
        assert_eq!(nine.value(1_000, Side::Asset), Ok(0));
        assert_eq!(nine.value(1_000, Side::Liability), Ok(2));
        assert_eq!(p.value(0, Side::Liability), Ok(0));
    }

    #[test]
    fn overflow_is_an_error_never_a_wrap() {
        let p = gate(0, &usdc(), 1_000).unwrap();
        // A product just past u128::MAX would wrap to under one dollar.
        let wraps = (1u128 << 127) / 99_970_000 * 2 + 1;
        assert!(wraps.checked_mul(99_970_000).is_none());
        assert_eq!(
            p.value(wraps, Side::Asset),
            Err(Error::Overflow("holding value"))
        );
        // $9.997 per token: u64::MAX raw units is worth more than u64::MAX.
        let ten = RowPrice { exponent: -7, ..p };
        assert_eq!(
            ten.value(u128::from(u64::MAX), Side::Asset),
            Err(Error::Overflow("holding value"))
        );
        assert_eq!(
            p.value(u128::MAX, Side::Asset),
            Err(Error::Overflow("holding value"))
        );
        let huge = RowPrice { exponent: 40, ..p };
        assert!(huge.value(1, Side::Asset).is_err());
    }

    #[test]
    fn the_gate_matches_the_program() {
        let row = usdc();
        // Default window: 15 s.
        assert!(gate(0, &row, 1_015).is_ok());
        let stale = gate(0, &row, 1_016).unwrap_err();
        assert!(matches!(
            stale,
            Error::Price {
                reason: PriceFault::Stale,
                ..
            }
        ));
        // A reading further ahead of the clock than the skew allowance.
        let mut ahead = row.clone();
        ahead.max_future_skew_seconds = U32::from(0u32);
        assert!(gate(0, &row, 985).is_ok(), "default skew is 15 s");
        assert!(matches!(
            gate(0, &ahead, 999),
            Err(Error::Price {
                reason: PriceFault::Future,
                ..
            })
        ));
        let mut wide = row.clone();
        // Default cap 100 bps: 1% of 0.9999 is 999_900.
        wide.last_conf = U64::from(999_901u64);
        assert!(matches!(
            gate(0, &wide, 1_000),
            Err(Error::Price {
                reason: PriceFault::Confidence,
                ..
            })
        ));
        wide.last_conf = U64::from(999_900u64);
        assert!(gate(0, &wide, 1_000).is_ok());
        let mut zero = row.clone();
        zero.last_price = I64::from(0);
        assert!(matches!(
            gate(0, &zero, 1_000),
            Err(Error::Price {
                reason: PriceFault::NonPositive,
                ..
            })
        ));
        let mut week = row;
        week.max_staleness_seconds = U32::from(7 * 86_400u32);
        assert!(gate(0, &week, 1_000 + 7 * 86_400).is_ok());
        assert_eq!(
            gate(0, &week, 1_000).unwrap().valid_until,
            1_000 + 7 * 86_400
        );
    }
}
