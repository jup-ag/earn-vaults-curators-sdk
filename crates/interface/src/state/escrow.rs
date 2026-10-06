use {
    crate::{check_account_layout, state::Account},
    codama_macros::CodamaAccount,
    solana_address::Address,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite, ZeroCopy},
    zeropod::pod::{PodI64, PodU64},
};

pub const FULFILLED_PENDING: u8 = 0;
pub const FULFILLED_DONE: u8 = 1;
pub const FULFILLED_PARTIAL: u8 = 2;

/// Pre-upgrade header, retained only for migration and hook compatibility.
#[derive(SchemaRead, SchemaWrite)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct LegacyEscrow {
    pub amount: PodU64,
    pub created_at: PodI64,
    pub owner: Address,
    pub vault: Address,
    pub request_type: u8,
    pub fulfilled: u8,
    pub bump: u8,
    pub ta_bump: u8,
    pub share_ta_bump: u8,
    pub unlock_at: PodI64,
    pub nonce: PodU64,
}

impl Account for LegacyEscrow {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 3;
}
check_account_layout!(LegacyEscrow);

#[derive(SchemaRead, SchemaWrite, CodamaAccount)]
#[codama(discriminator(bytes = [3]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Escrow {
    #[codama(type = number(u64))]
    pub amount: PodU64,
    #[codama(type = number(i64))]
    pub created_at: PodI64,
    pub owner: Address,
    pub vault: Address,
    pub request_type: u8,
    pub fulfilled: u8,
    pub bump: u8,
    pub ta_bump: u8,
    pub share_ta_bump: u8,
    #[codama(type = number(i64))]
    pub unlock_at: PodI64,
    #[codama(type = number(u64))]
    pub nonce: PodU64,
    /// Minimum shares for a deposit; minimum tokens for the original redeem.
    #[codama(type = number(u64))]
    pub min_out: PodU64,
    /// Fixed denominator for redeem partial fills; zero for deposits and migrated requests.
    #[codama(type = number(u64))]
    pub original_shares: PodU64,
    /// User-selected redemption token; zero for deposits and migrated requests.
    pub payout_mint: Address,
}

impl Account for Escrow {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 3;
}

check_account_layout!(Escrow);

impl Escrow {
    /// Read either complete layout without accepting a truncated protection tail.
    pub fn read_header(data: &[u8]) -> Result<&LegacyEscrow, ProgramError> {
        if data.first() != Some(&Self::DISCRIMINATOR)
            || (data.len() != LegacyEscrow::LEN && data.len() != Self::LEN)
        {
            return Err(ProgramError::InvalidAccountData);
        }
        LegacyEscrow::from_bytes(&data[1..LegacyEscrow::LEN])
            .map_err(|_| ProgramError::InvalidAccountData)
    }
}

const _: () =
    assert!(core::mem::offset_of!(Escrow, min_out) == core::mem::size_of::<LegacyEscrow>());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escrow_layout_rejects_truncated_limits() {
        assert_eq!(LegacyEscrow::LEN, 102);
        assert_eq!(Escrow::LEN, 150);
        let mut data = [0; Escrow::LEN + 1];
        data[0] = Escrow::DISCRIMINATOR;
        for len in 0..=data.len() {
            assert_eq!(
                <Escrow as Account>::read(&data[..len]).is_ok(),
                len == Escrow::LEN
            );
            assert_eq!(
                Escrow::read_header(&data[..len]).is_ok(),
                len == LegacyEscrow::LEN || len == Escrow::LEN
            );
        }
        data[0] = 5;
        assert!(Escrow::read_header(&data[..Escrow::LEN]).is_err());
    }
}
