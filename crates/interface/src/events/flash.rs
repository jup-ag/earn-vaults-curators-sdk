use {
    codama_macros::CodamaEvent,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::U64,
};

pub const FLASH_DIRECTION_DEPOSIT: u8 = 0;
pub const FLASH_DIRECTION_WITHDRAW: u8 = 1;

// VaultEvent::FlashMoved = 17.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [17]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct FlashMoved {
    pub vault: Address,
    pub operator: Address,
    pub mint: Address,
    #[codama(type = number(u64))]
    pub amount: U64,
    pub direction: u8,
}
