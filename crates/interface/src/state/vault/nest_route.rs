//! The one Nest vault a `nest::MINT` / `nest::REDEEM` delegate may route to. Nest
//! settles on Plume: a mint is a CCTP v2 USDC burn whose
//! `mint_recipient` / `destination_caller` is Nest's relayer contract and
//! whose hook names the vault's `NestVaultComposer`. Neither contract is an
//! account this program can pin, so the curator names both here. The remaining
//! bytes bind redeem to one OFTStore + share mint. All-zero disables Nest.

use {
    crate::encoding,
    codama_macros::CodamaType,
    encoding::{ZcElem, ZcValidate, ZeroPodError},
    solana_address::Address,
    solana_program_error::ProgramError,
    solana_sha256_hasher::hashv,
    wincode::{SchemaRead, SchemaWrite},
};

#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct NestRoute {
    /// `NestVaultComposer` of the target Nest vault on Plume (EVM address,
    /// e.g. nOPAL/USDC `0x9053e541c4bf3f44a8dbb7e25625639b6d0b62c9`); all-zero
    /// disables Nest.
    pub composer: [u8; 20],
    /// Nest's CCTP relayer on Plume — the burn's `mint_recipient` and
    /// `destination_caller`, the contract that receives the USDC and executes
    /// the hook into `composer` (e.g. `0x7de01896d36bea9cf072ac64e41685418941d8be`).
    pub relayer: [u8; 20],
    /// First 23 bytes of SHA-256(domain || OFTStore || share mint). This fills
    /// the policy payload while binding redeem to one Nest product.
    pub oft_commitment: [u8; 23],
}

impl ZcValidate for NestRoute {
    fn validate_ref(_: &Self) -> Result<(), ZeroPodError> {
        Ok(())
    }
}

// SAFETY: repr(C) over `u8` arrays; align 1, no padding, every bit pattern
// valid. `validate` enforces the economic invariant separately.
unsafe impl ZcElem for NestRoute {}

impl NestRoute {
    pub const fn zero() -> Self {
        Self {
            composer: [0; 20],
            relayer: [0; 20],
            oft_commitment: [0; 23],
        }
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.composer != [0u8; 20]
    }

    /// An enabled route must name a relayer: a zero `mint_recipient` is
    /// refused by CCTP, a zero `destination_caller` would let anyone relay the
    /// burn without running the hook.
    pub fn validate(&self) -> Result<(), ProgramError> {
        if self.enabled() && (self.relayer == [0u8; 20] || self.oft_commitment == [0u8; 23]) {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(())
    }

    /// `composer` as the 32-byte word the CCTP hook carries (left-padded with
    /// 12 zero bytes).
    #[inline]
    pub fn composer_word(&self) -> [u8; 32] {
        pad(&self.composer)
    }

    /// `relayer` as the 32-byte CCTP `mint_recipient` / `destination_caller`.
    #[inline]
    pub fn relayer_word(&self) -> [u8; 32] {
        pad(&self.relayer)
    }

    pub fn commitment(oft_store: &Address, share_mint: &Address) -> [u8; 23] {
        const DOMAIN: &[u8] = b"yield-store:nest-oft:v1";
        let hash = hashv(&[DOMAIN, oft_store.as_array(), share_mint.as_array()]);
        let mut out = [0u8; 23];
        out.copy_from_slice(&hash.as_ref()[..23]);
        out
    }

    #[inline]
    pub fn permits_oft(&self, oft_store: &Address, share_mint: &Address) -> bool {
        self.oft_commitment == Self::commitment(oft_store, share_mint)
    }
}

#[inline]
fn pad(evm: &[u8; 20]) -> [u8; 32] {
    let mut w = [0u8; 32];
    w[12..].copy_from_slice(evm);
    w
}

const _: () = assert!(core::mem::size_of::<NestRoute>() == 63);
const _: () = assert!(core::mem::align_of::<NestRoute>() == 1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_gates_on_composer_and_needs_a_relayer() {
        let zero = NestRoute::zero();
        assert!(!zero.enabled());
        assert!(zero.validate().is_ok());

        let route = NestRoute {
            composer: [9; 20],
            relayer: [7; 20],
            oft_commitment: NestRoute::commitment(
                &Address::new_from_array([1; 32]),
                &Address::new_from_array([2; 32]),
            ),
        };
        assert!(route.enabled());
        assert!(route.validate().is_ok());
        let mut want = [0u8; 32];
        want[12..].fill(9);
        assert_eq!(route.composer_word(), want);
        want[12..].fill(7);
        assert_eq!(route.relayer_word(), want);

        let no_relayer = NestRoute {
            composer: [9; 20],
            relayer: [0; 20],
            oft_commitment: [1; 23],
        };
        assert!(no_relayer.validate().is_err());
        let no_oft = NestRoute {
            composer: [9; 20],
            relayer: [7; 20],
            oft_commitment: [0; 23],
        };
        assert!(no_oft.validate().is_err());
    }
}
