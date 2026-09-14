use {
    crate::{check_account_layout, state::Account},
    codama_macros::CodamaAccount,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI64, PodU64},
};

pub const FULFILLED_PENDING: u8 = 0;
pub const FULFILLED_DONE: u8 = 1;
pub const FULFILLED_PARTIAL: u8 = 2;

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
}

impl Account for Escrow {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 3;
}

check_account_layout!(Escrow);
