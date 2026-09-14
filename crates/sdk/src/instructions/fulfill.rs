use {
    crate::{
        instructions::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID},
        pda::{
            derive_escrow, derive_escrow_token_account, derive_protocol_delegate,
            derive_share_mint, derive_vault_authority,
        },
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

pub struct Fulfill {
    pub authority: Address,
    pub vault: Address,
    pub user: Address,
    /// Nonce the escrow was opened with (`Subscribe.nonce`).
    pub nonce: u64,
    pub base_mint: Address,
    pub vault_token_account: Address,
    pub escrow_share_account: Address,
    pub base_token_program: Address,
}

impl Fulfill {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let escrow_token_account = derive_escrow_token_account(&self.vault, &self.user, self.nonce);
        let vault_authority = derive_vault_authority(&self.vault);
        let delegate = derive_protocol_delegate(&self.vault, &self.authority, &crate::ID);

        let data = super::encode(&VaultInstruction::Fulfill);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.authority, true),
                AccountMeta::new_readonly(delegate, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(share_mint, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(escrow_token_account, false),
                AccountMeta::new(self.vault_token_account, false),
                AccountMeta::new(self.escrow_share_account, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.base_token_program, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
