use {
    crate::pda::{derive_associated_token_account, derive_share_mint, derive_vault_authority},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Curator sweeps a listed asset's residual custody balance out of a vault
/// with no user claims (supply and pending redeems all zero) — the teardown
/// path for donated dust blocking `CloseVault`.
pub struct SweepIdle {
    pub curator: Address,
    pub vault: Address,
    pub mint: Address,
    pub token_program: Address,
    /// Curator-chosen destination; must hold `mint`.
    pub destination_token_account: Address,
}

impl SweepIdle {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let vault_authority = derive_vault_authority(&self.vault);
        let custody =
            derive_associated_token_account(&vault_authority, &self.mint, &self.token_program);

        let data = super::encode(&VaultInstruction::SweepIdle);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new_readonly(self.curator, true),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(share_mint, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new(custody, false),
                AccountMeta::new(self.destination_token_account, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
