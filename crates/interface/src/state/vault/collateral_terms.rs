use {
    codama_macros::CodamaType,
    solana_address::Address,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{ZcElem, ZcValidate, error::ZeroPodError, pod::PodU64},
};

/// Policy key of Offerbook's general collateral approval: [`CollateralTerms`]
/// that let the vault **lend** against any collateral mint. Borrowing always
/// needs the mint's own policy, since its collateral cap is in that mint's raw
/// units and bounds vault value leaving custody. Approvals are a union: the
/// delegate may pass either policy, so a mint policy under an open general one
/// can only widen that mint's caps, and excluding a mint means closing this.
/// The vault program's own address: an executable account, never a mint.
pub const GENERAL_COLLATERAL: Address = crate::ID;

/// Offerbook's [`ProtocolPolicy`](crate::state::ProtocolPolicy) payload, keyed by collateral
/// mint (or [`GENERAL_COLLATERAL`]): the curator's approval for lending or
/// borrowing with that mint. The manager chooses loan terms; the policy caps
/// raw token amounts per loan.
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct CollateralTerms {
    /// Only this listed principal mint may be offered against the policy key.
    pub principal_mint: Address,
    /// Maximum gross principal in the mint's raw token units, per offer.
    #[codama(type = number(u64))]
    pub max_principal_amount: PodU64,
    /// Maximum raw collateral tokens pledged or promised in one loan.
    #[codama(type = number(u64))]
    pub max_collateral_amount: PodU64,
}

impl CollateralTerms {
    pub fn validate(&self) -> Result<(), ProgramError> {
        if self.principal_mint == Address::default()
            || self.max_principal_amount.is_zero()
            || self.max_collateral_amount.is_zero()
        {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(())
    }
}

impl ZcValidate for CollateralTerms {
    fn validate_ref(_: &Self) -> Result<(), ZeroPodError> {
        Ok(())
    }
}

// SAFETY: repr(C) over alignment-one POD fields with no padding; every bit
// pattern is valid. `SetProtocolPolicy` validates the economic bounds separately.
unsafe impl ZcElem for CollateralTerms {}

const _: () = assert!(core::mem::size_of::<CollateralTerms>() == 48);
