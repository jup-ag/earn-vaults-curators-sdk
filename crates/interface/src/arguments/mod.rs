use crate::encoding::PodOption;
mod initialize_vault;
mod price_opaque;
mod set_protocol_policy;
mod update_share_meta;
mod update_vault;

use {
    crate::constants::{MAX_PARAM_STALENESS_SECONDS, UNBOUNDED_BPS},
    solana_program_error::ProgramError,
    zeropod::{U16, U32, U64},
};
pub use {
    initialize_vault::*, price_opaque::*, set_protocol_policy::*, update_share_meta::*,
    update_vault::*,
};

/// Resolve a curator-set position or sink freshness argument to its stored
/// value. `None` stores `0`, which read sites interpret as the global default.
/// `Some(0)` asks for the widest window and stores
/// [`MAX_PARAM_STALENESS_SECONDS`], not an infinite one: a
/// `REQUIRED_IN_AUM` mark that can never go stale would leave an old price in
/// NAV indefinitely. Explicit values cannot exceed that same ceiling.
/// Keeping the default and widest-window encodings distinct lets curators
/// request either without changing how existing stored zeroes are read.
#[inline]
pub fn window_seconds(arg: PodOption<U32>) -> Result<U64, ProgramError> {
    Ok(match arg.get().map(u32::from).map(u64::from) {
        None => U64::ZERO,
        Some(0) => U64::from(MAX_PARAM_STALENESS_SECONDS),
        Some(n) if n <= MAX_PARAM_STALENESS_SECONDS => U64::from(n),
        Some(_) => return Err(ProgramError::InvalidArgument),
    })
}

/// Resolve a curator-set position drift or sink bootstrap cap argument to its
/// stored value. `None` and `Some(0)` store `0` (the default); `Some(u16::MAX)`
/// stores the waiver sentinel. Explicit caps through 10,000 bps are valid,
/// including values above the 200 bps default. A cap of 10,000 is a real 100% budget, distinct
/// from waiving the budget. Updates to stored values use the same encoding.
#[inline]
pub fn window_bps(arg: PodOption<U16>) -> Result<U16, ProgramError> {
    Ok(match arg.get().map(u16::from) {
        None => U16::ZERO,
        Some(UNBOUNDED_BPS) => U16::from(UNBOUNDED_BPS),
        Some(n) if n <= 10_000 => U16::from(n),
        Some(_) => return Err(ProgramError::InvalidArgument),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_arguments_keep_defaults_waivers_and_explicit_caps_distinct() {
        assert_eq!(window_seconds(PodOption::none()).unwrap().get(), 0);
        assert_eq!(
            window_seconds(PodOption::some(U32::ZERO)).unwrap().get(),
            MAX_PARAM_STALENESS_SECONDS
        );
        assert_eq!(
            window_seconds(PodOption::some(U32::from(3_600)))
                .unwrap()
                .get(),
            3_600
        );
        assert!(
            window_seconds(PodOption::some(U32::from(
                MAX_PARAM_STALENESS_SECONDS as u32 + 1
            )))
            .is_err()
        );

        assert_eq!(window_bps(PodOption::none()).unwrap().get(), 0);
        assert_eq!(window_bps(PodOption::some(U16::ZERO)).unwrap().get(), 0);
        assert_eq!(
            window_bps(PodOption::some(U16::from(u16::MAX)))
                .unwrap()
                .get(),
            UNBOUNDED_BPS
        );
        assert!(crate::constants::is_waived_bps(
            window_bps(PodOption::some(U16::from(u16::MAX)))
                .unwrap()
                .get()
        ));
        assert_eq!(
            window_bps(PodOption::some(U16::from(500))).unwrap().get(),
            500
        );
        assert_eq!(
            window_bps(PodOption::some(U16::from(10_000)))
                .unwrap()
                .get(),
            10_000
        );
        assert!(window_bps(PodOption::some(U16::from(10_001))).is_err());
    }
}
