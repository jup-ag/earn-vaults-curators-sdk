use {
    codama_macros::CodamaEvent,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::U64,
};

// VaultEvent::RedeemRequested = 5.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [5]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct RedeemRequested {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub shares_burned: U64,
}

// VaultEvent::RedeemFulfilled = 6.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [6]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct RedeemFulfilled {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub amount_paid: U64,
    #[codama(type = number(u64))]
    pub remaining: U64,
    /// Settled shares retained for the curator instead of paid out.
    #[codama(type = number(u64))]
    pub fee_shares: U64,
}

// VaultEvent::Redeemed = 7.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [7]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Redeemed {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub amount: U64,
}

// VaultEvent::RedeemedInstant = 10.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [10]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct RedeemedInstant {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub shares_burned: U64,
    #[codama(type = number(u64))]
    pub amount_paid: U64,
    #[codama(type = number(u64))]
    pub fee_shares: U64,
}

// VaultEvent::RedeemCancelled = 8.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [8]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct RedeemCancelled {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub shares_reminted: U64,
}
