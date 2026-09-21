//! Curator policy for one routed protocol, stored as opaque bytes tagged with
//! the [`Protocol`] they belong to.
//!
//! Every adapter that needs curator configuration — Offerbook's loan limits,
//! CCIP's lane, Nest's route — owns a small POD struct and decodes it out of the vault's
//! [`Vault::policies`](crate::state::Vault::policies) table by tag. The vault
//! core never learns the shape, so a new protocol adds a struct and a
//! [`ProtocolPolicy::validate`] arm, never a `Vault` field or a layout
//! migration. `MAX_POLICIES` slots are shared by every protocol; a slot is
//! free while its tag is `SpotTokens` (0), which never carries policy.

use {
    super::{CcipLane, LoanLimits, NestRoute},
    crate::state::Protocol,
    codama_macros::CodamaType,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{ZcElem, ZcValidate, error::ZeroPodError},
};

/// Concurrent protocol policies a vault can hold — one per position slot,
/// since a policy without a position has nothing to govern.
pub const MAX_POLICIES: usize = 8;
/// Payload width; every adapter policy struct must fit.
pub const POLICY_BYTES: usize = 63;

#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct ProtocolPolicy {
    /// [`Protocol`] discriminant; `SpotTokens` (0) marks a free slot.
    pub protocol: u8,
    /// The adapter's POD policy struct, zero-padded. Decode with
    /// [`ProtocolPolicy::decode`]. (`POLICY_BYTES` spelled out: Codama needs
    /// the literal.)
    pub bytes: [u8; 63],
}

impl ZcValidate for ProtocolPolicy {
    fn validate_ref(_: &Self) -> Result<(), ZeroPodError> {
        Ok(())
    }
}

// SAFETY: repr(C) over `u8`s; align 1, no padding, every bit pattern valid.
unsafe impl ZcElem for ProtocolPolicy {}

impl ProtocolPolicy {
    pub const fn zero() -> Self {
        Self {
            protocol: Protocol::SpotTokens as u8,
            bytes: [0; POLICY_BYTES],
        }
    }

    #[inline]
    pub fn is_free(&self) -> bool {
        self.protocol == Protocol::SpotTokens as u8
    }

    /// Whether the adapter reads this payload as "off". Storing a disabled
    /// policy frees the slot, so a half-filled disabled struct never lingers
    /// (Offerbook: `min_ltv_bps == 0` closes loans whatever the other lines
    /// say). Unknown tags are disabled by definition.
    pub fn is_disabled(&self) -> bool {
        match Protocol::from_u8(self.protocol) {
            Some(Protocol::Offerbook) => !self.decode::<LoanLimits>().enabled(),
            Some(Protocol::Ccip) => !self.decode::<CcipLane>().enabled(),
            Some(Protocol::Nest) => !self.decode::<NestRoute>().enabled(),
            _ => true,
        }
    }

    /// Wrap an adapter policy struct. `ZcElem` is the whole soundness
    /// argument: align 1, no padding, so its bytes are its value.
    pub fn encode<T: ZcElem>(protocol: Protocol, value: &T) -> Self {
        const { assert!(core::mem::size_of::<T>() <= POLICY_BYTES) }
        let mut bytes = [0u8; POLICY_BYTES];
        // SAFETY: `T: ZcElem` — alignment 1, no padding, every byte initialized.
        let src = unsafe {
            core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
        };
        bytes[..core::mem::size_of::<T>()].copy_from_slice(src);
        Self {
            protocol: protocol as u8,
            bytes,
        }
    }

    /// Read the payload as `T`. The caller picks `T` by the slot's tag; a
    /// mismatched read is a caller bug, not UB (`ZcElem`: any bit pattern is
    /// a valid `T`).
    pub fn decode<T: ZcElem>(&self) -> T {
        const { assert!(core::mem::size_of::<T>() <= POLICY_BYTES) }
        // SAFETY: `T: ZcElem` — every bit pattern valid, align 1, and the
        // const assertion keeps the read inside `bytes`.
        unsafe { core::ptr::read_unaligned(self.bytes.as_ptr().cast::<T>()) }
    }

    /// Per-protocol validation of a curator-supplied policy. The one place a
    /// new adapter registers its struct.
    pub fn validate(&self) -> Result<(), ProgramError> {
        match Protocol::from_u8(self.protocol) {
            Some(Protocol::Offerbook) => self.decode::<LoanLimits>().validate(),
            Some(Protocol::Ccip) => self.decode::<CcipLane>().validate(),
            Some(Protocol::Nest) => self.decode::<NestRoute>().validate(),
            _ => Err(ProgramError::InvalidArgument),
        }
    }
}

impl From<LoanLimits> for ProtocolPolicy {
    fn from(limits: LoanLimits) -> Self {
        Self::encode(Protocol::Offerbook, &limits)
    }
}

impl From<CcipLane> for ProtocolPolicy {
    fn from(lane: CcipLane) -> Self {
        Self::encode(Protocol::Ccip, &lane)
    }
}

impl From<NestRoute> for ProtocolPolicy {
    fn from(route: NestRoute) -> Self {
        Self::encode(Protocol::Nest, &route)
    }
}

const _: () = assert!(core::mem::size_of::<ProtocolPolicy>() == 1 + POLICY_BYTES);
const _: () = assert!(core::mem::align_of::<ProtocolPolicy>() == 1);

#[cfg(test)]
mod tests {
    use {super::*, solana_address::Address};

    #[test]
    fn round_trips_adapter_structs_and_gates_by_tag() {
        let limits = LoanLimits {
            min_ltv_bps: 5_000.into(),
            max_apy_bps: 1.into(),
            min_duration_seconds: 60.into(),
            max_duration_seconds: 86_400.into(),
            max_offer_expiry_seconds: 300.into(),
            max_principal_value: 1_000_000.into(),
            lender: Address::new_from_array([7; 32]),
        };
        let p: ProtocolPolicy = limits.into();
        assert_eq!(p.protocol, Protocol::Offerbook as u8);
        assert!(!p.is_free() && !p.is_disabled());
        assert!(p.validate().is_ok());
        let back = p.decode::<LoanLimits>();
        assert_eq!(back.max_principal_value.get(), 1_000_000);
        assert_eq!(back.lender, limits.lender);

        let lane: ProtocolPolicy = CcipLane {
            dest_chain_selector: 9.into(),
            receiver: Address::new_from_array([1; 32]),
        }
        .into();
        assert!(lane.validate().is_ok());
        assert!(lane.decode::<CcipLane>().permits(9, &[1u8; 32]));

        // A disabled struct frees the slot when stored, even when other lines
        // are set — the adapter's own `enabled()` decides.
        assert!(ProtocolPolicy::from(LoanLimits::zero()).is_disabled());
        assert!(
            ProtocolPolicy::from(LoanLimits {
                min_ltv_bps: 0.into(),
                ..limits
            })
            .is_disabled()
        );
        // Unknown or policy-less protocols are refused.
        let spot = ProtocolPolicy {
            protocol: Protocol::SpotTokens as u8,
            ..p
        };
        assert!(spot.validate().is_err());
        let unknown = ProtocolPolicy { protocol: 200, ..p };
        assert!(unknown.validate().is_err());
    }
}
