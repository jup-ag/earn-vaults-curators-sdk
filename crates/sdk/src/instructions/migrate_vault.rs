use {
    earn_vaults_interface::VaultInstruction,
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Migrate a deployed mainnet vault to inline oracle settings. The curator signs
/// and pays any additional rent. Repeating a completed migration is a no-op.
pub struct MigrateVault {
    pub curator: Address,
    pub vault: Address,
}

impl MigrateVault {
    pub fn into_instruction(self) -> Instruction {
        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.curator, true),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(super::SYSTEM_PROGRAM_ID, false),
            ],
            data: super::encode(&VaultInstruction::MigrateVault),
        }
    }
}
