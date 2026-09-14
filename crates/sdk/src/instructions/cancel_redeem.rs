use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{
            derive_escrow, derive_escrow_token_account, derive_share_mint, derive_vault_authority,
        },
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
};

/// Cancel a queued redeem. Without `partial`, cancels a still-pending redeem:
/// re-mints the burned shares back to the user (mint authority = vault-
/// authority PDA) and closes the escrow. With `partial`, cancels a partially
/// filled redeem: sweeps the escrowed proceeds to the user and re-mints the
/// numéraire remainder as shares at current NAV.
pub struct CancelRedeem {
    pub user: Address,
    pub vault: Address,
    pub user_share_account: Address,
    /// Nonce the escrow was opened with (`Redeem.nonce`).
    pub nonce: u64,
    pub partial: Option<CancelPartialRedeem>,
}

pub struct CancelPartialRedeem {
    pub user_token_account: Address,
    pub base_mint: Address,
    pub token_program: Address,
}

impl CancelRedeem {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let vault_authority = derive_vault_authority(&self.vault);

        let data = super::encode(&VaultInstruction::CancelRedeem);

        let mut accounts = vec![
            AccountMeta::new(self.user, true),
            AccountMeta::new(self.vault, false),
            AccountMeta::new(escrow, false),
            AccountMeta::new(share_mint, false),
            AccountMeta::new(self.user_share_account, false),
            AccountMeta::new_readonly(vault_authority, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(EVENT_PDA.0, false),
            AccountMeta::new_readonly(crate::ID, false),
        ];
        if let Some(partial) = self.partial {
            let escrow_ta = derive_escrow_token_account(&self.vault, &self.user, self.nonce);
            accounts.extend([
                AccountMeta::new(escrow_ta, false),
                AccountMeta::new(partial.user_token_account, false),
                AccountMeta::new_readonly(partial.base_mint, false),
                AccountMeta::new_readonly(partial.token_program, false),
            ]);
        }

        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}
