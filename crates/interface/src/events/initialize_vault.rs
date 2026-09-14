use {
    crate::state::AssetConfig,
    codama_macros::CodamaEvent,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI64, PodU16, PodU64},
};

// Discriminator = the `VaultEvent` tag byte (InitializeVault = 0), matching the
// `Event<T>` = [tag][payload] self-CPI wire format.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [0]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct InitializeVaultEvent {
    pub vault: Address,
    pub curator: Address,
    pub share_mint: Address,
    /// Configured asset rows (priced + deposit-only), empty slots zeroed.
    pub assets: [AssetConfig; 8],
    /// Annual curator management fee; the global protocol rate is additive.
    #[codama(type = number(u16))]
    pub management_fee_bps: PodU16,
    #[codama(type = number(u16))]
    pub instant_redeem_fee_bps: PodU16,
    #[codama(type = number(u64))]
    pub timelock_delay: PodU64,
    #[codama(type = number(i64))]
    pub created_at: PodI64,
}
