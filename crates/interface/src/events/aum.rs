use {
    codama_macros::CodamaEvent,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI64, PodU64},
};

// VaultEvent::AumValidated = 9.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [9]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AumValidated {
    pub vault: Address,
    #[codama(type = number(u64))]
    pub aum: PodU64,
    #[codama(type = number(u64))]
    pub eff_aum: PodU64,
    #[codama(type = number(u64))]
    pub eff_supply: PodU64,
    #[codama(type = number(i64))]
    pub timestamp: PodI64,
}

// VaultEvent::Priced = 11.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [11]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Priced {
    pub vault: Address,
    /// Index into `Vault.positions`.
    pub slot: u8,
    /// The mark this write replaced (`0` on the bootstrap write).
    #[codama(type = number(u64))]
    pub prior_value: PodU64,
    #[codama(type = number(u64))]
    pub value: PodU64,
    #[codama(type = number(i64))]
    pub timestamp: PodI64,
}

// VaultEvent::AssetPriced = 16.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [16]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct AssetPriced {
    pub vault: Address,
    /// Index into `Vault.assets`.
    pub asset: u8,
    /// The price this write replaced, in numéraire micro-units per whole token.
    #[codama(type = number(u64))]
    pub prior_price: PodU64,
    #[codama(type = number(u64))]
    pub price: PodU64,
    #[codama(type = number(i64))]
    pub timestamp: PodI64,
}
