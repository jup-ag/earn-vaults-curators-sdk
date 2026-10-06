use {
    codama_macros::CodamaType,
    solana_address::Address,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{
        ZcElem, ZcValidate,
        error::ZeroPodError,
        pod::{PodU16, PodU32, PodU64},
    },
};

/// Curator-approved terms for new loans on either side. The all-zero value
/// disables new loans. The LTV and APY lines are read from the vault's side:
/// borrowing must sit at or above `min_ltv_bps` and at or below `max_apy_bps`,
/// lending at or below `min_ltv_bps` and at or above `max_apy_bps`. Each
/// adapter enforces its counterparty and standing-offer restrictions.
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct LoanLimits {
    /// The LTV line, principal / collateral oracle value in basis points. As
    /// borrower this floor is the whole economic protection: it caps a
    /// deliberate default's loss at `(1 - min_ltv) x collateral`. There is
    /// deliberately no maximum — borrowing *more* against the same collateral
    /// only ever favors the vault, and leverage is already bounded by
    /// `max_deployed` and `max_principal_value`. As lender the same line is a
    /// ceiling: a standing principal offer may not lend above it.
    #[codama(type = number(u16))]
    pub min_ltv_bps: PodU16,
    /// The rate line, nominal annual bps. As borrower a maximum; together with
    /// `max_duration_seconds` it also bounds any interest-derived protocol fee
    /// deducted from proceeds. As lender a minimum.
    #[codama(type = number(u32))]
    pub max_apy_bps: PodU32,
    #[codama(type = number(u32))]
    pub min_duration_seconds: PodU32,
    #[codama(type = number(u32))]
    pub max_duration_seconds: PodU32,
    /// Maximum lifetime of a new standing (principal) offer.
    #[codama(type = number(u32))]
    pub max_offer_expiry_seconds: PodU32,
    /// Maximum principal oracle value per loan, in USD micro-units.
    #[codama(type = number(u64))]
    pub max_principal_value: PodU64,
    /// Zero permits any lender. Adapters must reject operations that cannot
    /// enforce a nonzero lender.
    pub lender: Address,
}

impl ZcValidate for LoanLimits {
    fn validate_ref(_: &Self) -> Result<(), ZeroPodError> {
        Ok(())
    }
}

// SAFETY: repr(C) over alignment-one POD fields with no padding; every bit
// pattern is valid. Curator handlers validate economic bounds separately.
unsafe impl ZcElem for LoanLimits {}

impl LoanLimits {
    pub const fn zero() -> Self {
        Self {
            min_ltv_bps: PodU16::ZERO,
            max_apy_bps: PodU32::ZERO,
            min_duration_seconds: PodU32::ZERO,
            max_duration_seconds: PodU32::ZERO,
            max_offer_expiry_seconds: PodU32::ZERO,
            max_principal_value: PodU64::ZERO,
            lender: Address::new_from_array([0; 32]),
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.min_ltv_bps.get() != 0
    }

    pub fn validate(&self) -> Result<(), ProgramError> {
        if !self.enabled() {
            return Ok(());
        }
        // A zero rate line would be no floor at all on the lending side.
        if self.min_ltv_bps.get() > 10_000
            || self.max_apy_bps.is_zero()
            || self.min_duration_seconds.is_zero()
            || self.min_duration_seconds.get() > self.max_duration_seconds.get()
            || self.max_offer_expiry_seconds.is_zero()
            || self.max_principal_value.is_zero()
        {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(())
    }
}

const _: () = assert!(core::mem::size_of::<LoanLimits>() == 58);
const _: () = assert!(core::mem::align_of::<LoanLimits>() == 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_complete_limits_or_disabled() {
        let zero = LoanLimits::zero();
        assert!(!zero.enabled());
        assert!(zero.validate().is_ok());
        let limits = LoanLimits {
            min_ltv_bps: 5_000.into(),
            max_apy_bps: 1.into(),
            min_duration_seconds: 60.into(),
            max_duration_seconds: 86_400.into(),
            max_offer_expiry_seconds: 300.into(),
            max_principal_value: 1_000_000.into(),
            lender: Address::new_from_array([7; 32]),
        };
        assert!(limits.enabled());
        assert!(limits.validate().is_ok());
        for invalid in [
            LoanLimits {
                min_ltv_bps: 10_001.into(),
                ..limits
            },
            LoanLimits {
                max_apy_bps: 0.into(),
                ..limits
            },
            LoanLimits {
                min_duration_seconds: 0.into(),
                ..limits
            },
            LoanLimits {
                max_duration_seconds: 59.into(),
                ..limits
            },
            LoanLimits {
                max_offer_expiry_seconds: 0.into(),
                ..limits
            },
            LoanLimits {
                max_principal_value: 0.into(),
                ..limits
            },
        ] {
            assert_eq!(invalid.validate(), Err(ProgramError::InvalidArgument));
        }
    }
}
