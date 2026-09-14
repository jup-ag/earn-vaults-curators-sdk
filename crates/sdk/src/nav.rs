//! Off-chain NAV / APY math — the read-side mirror of the program's
//! `pricing::nav`. Clients (frontend, indexers) read `Vault.aum` and the
//! share-mint `supply`, then call these instead of re-deriving the formula.
//!
//! `aum` comes from `Vault.aum` (written by `ValidateAum`); `supply` is the
//! Token-2022 share-mint supply (bytes 36..44 of the mint account).

pub use earn_vaults_interface::nav::*;

/// Per-share NAV in `NAV_SCALE` (1e18) fixed point. Thin read-side wrapper over
/// the on-chain [`earn_vaults_interface::nav::nav_per_share`] so the formula
/// lives in exactly one place; an AUM that overflows the fixed-point numerator
/// (unreachable for real vaults) saturates to 0 here instead of erroring.
pub fn nav_per_share_or_zero(supply: u64, aum: u64) -> u128 {
    earn_vaults_interface::nav::nav_per_share(supply, aum).unwrap_or(0)
}

/// Annualized return (APY) between two per-share NAV snapshots taken
/// `elapsed_secs` apart, as a fraction (`0.10` == 10%). Compounded:
///
/// `apy = (nav_end / nav_start) ^ (SECONDS_PER_YEAR / elapsed_secs) − 1`
///
/// Returns `0.0` for a non-positive window or a zero start NAV.
pub fn apy(nav_start: u128, nav_end: u128, elapsed_secs: i64) -> f64 {
    if elapsed_secs <= 0 || nav_start == 0 {
        return 0.0;
    }
    let ratio = nav_end as f64 / nav_start as f64;
    let periods = earn_vaults_interface::constants::SECONDS_PER_YEAR as f64 / elapsed_secs as f64;
    ratio.powf(periods) - 1.0
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::constants::{NAV_SCALE, VIRTUAL_ASSETS, VIRTUAL_SHARES},
    };

    #[test]
    fn nav_and_apy() {
        // Bootstrap NAV is the unit point.
        assert_eq!(
            nav_per_share_or_zero(0, 0),
            NAV_SCALE * VIRTUAL_ASSETS / VIRTUAL_SHARES
        );

        // 1 USDC deposited at bootstrap: supply = deposit · VS/VA (= deposit).
        let supply = 1_000_000u64 * (VIRTUAL_SHARES / VIRTUAL_ASSETS) as u64;
        let nav0 = nav_per_share_or_zero(supply, 1_000_000);

        // AUM grows 10% over exactly one year, supply unchanged → ~10% APY.
        let nav1 = nav_per_share_or_zero(supply, 1_100_000);
        let a = apy(
            nav0,
            nav1,
            earn_vaults_interface::constants::SECONDS_PER_YEAR as i64,
        );
        assert!((a - 0.10).abs() < 1e-3, "apy={a}");

        // Same 10% gain over 30 days annualizes far higher (compounded).
        let monthly = apy(nav0, nav1, 30 * 86_400);
        assert!(monthly > 2.0, "30-day annualized apy={monthly}");

        // Degenerate windows are 0, not NaN/inf.
        assert_eq!(apy(nav0, nav1, 0), 0.0);
    }
}
