use {
    codama_macros::CodamaEvent,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::U64,
};

// VaultEvent tags: Deposited = 1.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [1]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Deposited {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub amount: U64,
    #[codama(type = number(u64))]
    pub shares_minted: U64,
}

// VaultEvent::SubscribeRequested = 2.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [2]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct SubscribeRequested {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub amount: U64,
}

// VaultEvent::SubscribeFulfilled = 3.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [3]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct SubscribeFulfilled {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub amount: U64,
    #[codama(type = number(u64))]
    pub shares_minted: U64,
}

// VaultEvent::SubscribeCancelled = 4.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [4]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct SubscribeCancelled {
    pub owner: Address,
    #[codama(type = number(u64))]
    pub refund_amount: U64,
}
