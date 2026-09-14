use {
    crate::pda::{derive_escrow, derive_escrow_token_account},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
};

pub struct Cancel {
    pub user: Address,
    pub vault: Address,
    pub user_token_account: Address,
    /// Nonce the escrow was opened with (`Subscribe.nonce`).
    pub nonce: u64,
    pub base_mint: Address,
    /// Token program that owns `base_mint` (classic SPL or Token-2022).
    pub token_program: Address,
}

impl Cancel {
    pub fn into_instruction(self) -> Instruction {
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let escrow_token_account = derive_escrow_token_account(&self.vault, &self.user, self.nonce);

        let data = super::encode(&VaultInstruction::Cancel);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(escrow_token_account, false),
                AccountMeta::new(self.user_token_account, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
