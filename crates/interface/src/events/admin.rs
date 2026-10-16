use {
    codama_macros::CodamaEvent,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{I64, U64},
};

/// `Paused.kind`: the vault-flow mask (`Vault.paused`, `Pause` bits).
pub const PAUSE_KIND_FLOWS: u8 = 0;
/// `Paused.kind`: the share-transfer halt (`Policy.paused_transfers`).
pub const PAUSE_KIND_TRANSFERS: u8 = 1;

// VaultEvent::FeeAccrued = 12.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [12]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct FeeAccrued {
    pub vault: Address,
    /// Curator annual management fee (`Vault.management_fee_bps`), numéraire.
    #[codama(type = number(u64))]
    pub curator_management: U64,
    /// Protocol annual management fee (`PROTOCOL_MANAGEMENT_FEE_BPS`), numéraire.
    #[codama(type = number(u64))]
    pub protocol_management: U64,
    /// Curator HWM performance fee (`Vault.performance_fee_bps`), numéraire.
    #[codama(type = number(u64))]
    pub curator_performance: U64,
    /// Protocol HWM performance fee (`PROTOCOL_PERFORMANCE_FEE_BPS`), numéraire.
    #[codama(type = number(u64))]
    pub protocol_performance: U64,
    /// Shares minted to the curator fee PDA (management + performance).
    #[codama(type = number(u64))]
    pub curator_shares: U64,
    /// Shares minted to the protocol fee PDA (management + performance).
    #[codama(type = number(u64))]
    pub protocol_shares: U64,
    #[codama(type = number(i64))]
    pub timestamp: I64,
}

// VaultEvent::CuratorRotated = 14.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [14]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct CuratorRotated {
    pub vault: Address,
    pub old_curator: Address,
    pub new_curator: Address,
}

// VaultEvent::Paused = 13.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [13]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Paused {
    pub vault: Address,
    /// One of `PAUSE_KIND_*`.
    pub kind: u8,
    /// The new mask/flag value after this write.
    pub value: u8,
}

// VaultEvent::Swept = 15.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [15]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Swept {
    pub vault: Address,
    pub mint: Address,
    /// Residual custody balance swept out, in the mint's raw units.
    #[codama(type = number(u64))]
    pub amount: U64,
}

// VaultEvent::FeesClaimed = 18.
#[derive(SchemaRead, SchemaWrite, CodamaEvent)]
#[codama(discriminator(bytes = [18]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct FeesClaimed {
    pub vault: Address,
    pub claimant: Address,
    pub destination: Address,
    /// Fee shares transferred to the claimant.
    #[codama(type = number(u64))]
    pub shares: U64,
}
