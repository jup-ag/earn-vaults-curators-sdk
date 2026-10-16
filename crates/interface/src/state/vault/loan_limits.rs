use {
    crate::encoding,
    codama_macros::CodamaType,
    encoding::{ZcElem, ZcValidate, ZeroPodError},
    solana_address::Address,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::U32,
};

/// Curator-approved terms for new loans on either side. The all-zero value
/// disables new loans. APY is a ceiling when borrowing and a floor when
/// lending. Per-mint raw token caps live in the collateral policy.
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct LoanLimits {
    /// The rate line, nominal annual bps. As borrower a maximum; together with
    /// `max_duration_seconds` it also bounds any interest-derived protocol fee
    /// deducted from proceeds. As lender a minimum. Zero disables new loans: it
    /// would be no floor at all on the lending side.
    #[codama(type = number(u32))]
    pub max_apy_bps: U32,
    #[codama(type = number(u32))]
    pub min_duration_seconds: U32,
    #[codama(type = number(u32))]
    pub max_duration_seconds: U32,
    /// Maximum lifetime of a new standing (principal) offer.
    #[codama(type = number(u32))]
    pub max_offer_expiry_seconds: U32,
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
            max_apy_bps: U32::ZERO,
            min_duration_seconds: U32::ZERO,
            max_duration_seconds: U32::ZERO,
            max_offer_expiry_seconds: U32::ZERO,
            lender: Address::new_from_array([0; 32]),
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.max_apy_bps.get() != 0
    }

    pub fn validate(&self) -> Result<(), ProgramError> {
        if !self.enabled() {
            return Ok(());
        }
        if self.min_duration_seconds.get() == 0
            || self.min_duration_seconds.get() > self.max_duration_seconds.get()
            || self.max_offer_expiry_seconds.get() == 0
        {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(())
    }
}

const _: () = assert!(core::mem::size_of::<LoanLimits>() == 48);
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
            max_apy_bps: 1.into(),
            min_duration_seconds: 60.into(),
            max_duration_seconds: 86_400.into(),
            max_offer_expiry_seconds: 300.into(),
            lender: Address::new_from_array([7; 32]),
        };
        assert!(limits.enabled());
        assert!(limits.validate().is_ok());
        for invalid in [
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
        ] {
            assert_eq!(invalid.validate(), Err(ProgramError::InvalidArgument));
        }
    }
}
