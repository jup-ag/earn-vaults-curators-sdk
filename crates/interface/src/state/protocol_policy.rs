use {
    crate::{ID, check_account_layout, errors::VaultError, state::Protocol},
    codama_macros::CodamaAccount,
    solana_account_view::AccountView,
    solana_address::{Address, address_eq},
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite, ZeroCopy},
    zeropod::ZcElem,
};

/// `["protocol_policy", vault, protocol, key]`: one curator policy for a routed
/// protocol, outside the vault and unlimited in number. This header is
/// followed by a payload only the owning adapter decodes — what `key` means
/// and how long the payload is are the adapter's business (Offerbook: zero key
/// for [`LoanLimits`](crate::state::LoanLimits), collateral mint key for
/// [`CollateralTerms`](crate::state::CollateralTerms)).
#[derive(SchemaRead, SchemaWrite, CodamaAccount)]
#[codama(discriminator(bytes = [5]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct ProtocolPolicy {
    pub vault: Address,
    pub key: Address,
    /// [`Protocol`](crate::state::Protocol) discriminant.
    pub protocol: u8,
    pub bump: u8,
}

impl ProtocolPolicy {
    /// Header length; the payload starts here.
    pub const LEN: usize = 1 + core::mem::size_of::<Self>();
    pub const DISCRIMINATOR: u8 = 5;

    /// Header and payload of a program-owned policy.
    ///
    /// # Safety
    ///
    /// No mutable borrow of `account`'s data may be alive while the returned
    /// references are.
    pub unsafe fn load(account: &AccountView) -> Result<(&Self, &[u8]), ProgramError> {
        if !address_eq(account.owner(), &ID) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        let bytes = unsafe { account.borrow_unchecked() };
        if bytes.len() < Self::LEN || bytes[0] != Self::DISCRIMINATOR {
            return Err(ProgramError::InvalidAccountData);
        }
        let header = <Self as ZeroCopy>::from_bytes(&bytes[1..Self::LEN])
            .map_err(|_| ProgramError::InvalidAccountData)?;
        Ok((header, &bytes[Self::LEN..]))
    }

    /// `vault`'s `protocol` policy under `key`, read as the adapter's `T`. The
    /// one check every policy read goes through: a program-owned
    /// `ProtocolPolicy` whose stored vault, protocol and key are exactly these,
    /// with a payload exactly `T`-sized.
    ///
    /// # Safety
    ///
    /// No mutable borrow of `account`'s data may be alive while the returned
    /// reference is.
    pub unsafe fn load_as<'a, T: ZcElem>(
        account: &'a AccountView,
        vault: &Address,
        protocol: Protocol,
        key: &Address,
    ) -> Result<&'a T, ProgramError> {
        let (header, payload) = unsafe { Self::load(account) }?;
        if !address_eq(&header.vault, vault)
            || header.protocol != protocol as u8
            || !address_eq(&header.key, key)
        {
            return Err(VaultError::PolicyMismatch.into());
        }
        Self::decode(payload)
    }

    /// Read a payload as the adapter's `T`, which must fill it exactly.
    pub fn decode<T: ZcElem>(payload: &[u8]) -> Result<&T, ProgramError> {
        const { assert!(core::mem::align_of::<T>() == 1) }
        if payload.len() != core::mem::size_of::<T>() {
            return Err(ProgramError::InvalidAccountData);
        }
        // SAFETY: `T: ZcElem` — align 1 and every bit pattern valid; the
        // length check above keeps the view inside `payload`.
        Ok(unsafe { &*payload.as_ptr().cast::<T>() })
    }
}

check_account_layout!(ProtocolPolicy);
