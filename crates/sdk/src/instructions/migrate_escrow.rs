use {
    earn_vaults_interface::VaultInstruction,
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Extend a legacy escrow in place. Safe to retry; existing limits are preserved.
pub struct MigrateEscrow {
    pub payer: Address,
    pub escrow: Address,
}

impl MigrateEscrow {
    pub fn into_instruction(self) -> Instruction {
        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.payer, true),
                AccountMeta::new(self.escrow, false),
                AccountMeta::new_readonly(super::SYSTEM_PROGRAM_ID, false),
            ],
            data: super::encode(&VaultInstruction::MigrateEscrow),
        }
    }
}
