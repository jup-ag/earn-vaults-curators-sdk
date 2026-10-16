//! The one cross-chain destination a `ccip::SEND` delegate may bridge custody
//! to. Unlike every other adapter, the far end of a CCIP transfer is not an
//! account this program can pin to the vault authority — it is a receiver on
//! another chain — so the curator names it here and the dispatcher refuses any
//! `ccip_send` whose `(dest_chain_selector, receiver)` differs.
//! All-zero disables bridging.
//!
//! One lane per vault. Add a `[CcipLane; N]` (or a lane PDA keyed by
//! selector) when a vault needs to fan out to several chains.

use {
    crate::encoding,
    codama_macros::CodamaType,
    encoding::{ZcElem, ZcValidate, ZeroPodError},
    solana_address::Address,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::U64,
};

#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct CcipLane {
    /// CCIP chain selector of the destination (e.g. Ethereum mainnet
    /// `5009297550715157269`); `0` disables bridging.
    #[codama(type = number(u64))]
    pub dest_chain_selector: U64,
    /// The exact 32-byte `SVM2AnyMessage.receiver`. An EVM address is
    /// left-padded with 12 zero bytes, as CCIP encodes it.
    pub receiver: Address,
}

impl ZcValidate for CcipLane {
    fn validate_ref(_: &Self) -> Result<(), ZeroPodError> {
        Ok(())
    }
}

// SAFETY: repr(C) over alignment-one POD fields with no padding; every bit
// pattern is valid. `validate` enforces the economic invariant separately.
unsafe impl ZcElem for CcipLane {}

impl CcipLane {
    pub const fn zero() -> Self {
        Self {
            dest_chain_selector: U64::ZERO,
            receiver: Address::new_from_array([0; 32]),
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.dest_chain_selector.get() != 0
    }

    /// An enabled lane must name a receiver: the zero address is a burn on
    /// every CCIP destination.
    pub fn validate(&self) -> Result<(), ProgramError> {
        if self.enabled() && *self.receiver.as_array() == [0u8; 32] {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(())
    }

    /// Whether `ccip_send(dest_chain_selector, receiver)` targets this lane.
    #[inline]
    pub fn permits(&self, dest_chain_selector: u64, receiver: &[u8]) -> bool {
        self.enabled()
            && self.dest_chain_selector.get() == dest_chain_selector
            && receiver == self.receiver.as_array()
    }
}

const _: () = assert!(core::mem::size_of::<CcipLane>() == 40);
const _: () = assert!(core::mem::align_of::<CcipLane>() == 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lane_gates_selector_and_receiver() {
        let zero = CcipLane::zero();
        assert!(!zero.enabled());
        assert!(zero.validate().is_ok());
        assert!(
            !zero.permits(0, &[0u8; 32]),
            "a disabled lane permits nothing"
        );

        let lane = CcipLane {
            dest_chain_selector: 7.into(),
            receiver: Address::new_from_array([9; 32]),
        };
        assert!(lane.validate().is_ok());
        assert!(lane.permits(7, &[9u8; 32]));
        assert!(!lane.permits(8, &[9u8; 32]));
        assert!(!lane.permits(7, &[8u8; 32]));
        assert!(!lane.permits(7, &[9u8; 20]), "length must match too");

        let burn = CcipLane {
            dest_chain_selector: 7.into(),
            receiver: Address::new_from_array([0; 32]),
        };
        assert!(burn.validate().is_err());
    }
}
