use {
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
};

/// Completes a curator rotation proposed via
/// [`crate::instructions::UpdateVault`]: `new_curator` must equal
/// `Vault.pending_curator` and signs; it becomes `Vault.curator`.
pub struct AcceptCurator {
    pub new_curator: Address,
    pub vault: Address,
}

impl AcceptCurator {
    pub fn into_instruction(self) -> Instruction {
        let data = super::encode(&VaultInstruction::AcceptCurator);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new_readonly(self.new_curator, true),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
