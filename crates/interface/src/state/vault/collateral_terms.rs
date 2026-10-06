use {
    crate::constants::MAX_ASSET_STALENESS_SECONDS,
    codama_macros::CodamaType,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{
        ZcElem, ZcValidate,
        error::ZeroPodError,
        pod::{PodU16, PodU32},
    },
};

/// Offerbook's [`ProtocolPolicy`](crate::state::ProtocolPolicy) payload, keyed by collateral
/// mint: the curator's terms for lending against a mint the vault need not
/// list. A standing principal offer that names the policy prices its collateral
/// from a Pyth update for `feed_id` instead of the vault's cached prices.
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct CollateralTerms {
    /// The Pyth feed id every price update for this collateral must carry.
    pub feed_id: [u8; 32],
    /// The vault's LTV ceiling as lender: principal value at its ceiling over
    /// collateral value at its floor, in basis points.
    #[codama(type = number(u16))]
    pub max_ltv_bps: PodU16,
    /// Confidence band cap in basis points of price (`0` = protocol default).
    #[codama(type = number(u16))]
    pub max_confidence_bps: PodU16,
    /// Price age cap in seconds (`0` = protocol default).
    #[codama(type = number(u32))]
    pub max_staleness_seconds: PodU32,
}

impl CollateralTerms {
    /// A named feed and an LTV line, with freshness and band held to the same
    /// write-time bounds as an asset row's.
    pub fn validate(&self) -> Result<(), ProgramError> {
        let ltv = self.max_ltv_bps.get();
        if ltv == 0
            || ltv > 10_000
            || self.feed_id == [0; 32]
            || i64::from(self.max_staleness_seconds.get()) > MAX_ASSET_STALENESS_SECONDS
            || self.max_confidence_bps.get() > 10_000
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
