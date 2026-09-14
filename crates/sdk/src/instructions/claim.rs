use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{
            derive_escrow, derive_escrow_share_account, derive_extra_account_metas, derive_policy,
            derive_share_mint,
        },
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{
        VaultInstruction,
        constants::{EVENT_PDA, HOOK_ID},
    },
};

pub struct Claim {
    pub user: Address,
    pub vault: Address,
    pub user_share_account: Address,
    /// Nonce the escrow was opened with (`Subscribe.nonce`).
    pub nonce: u64,
}

impl Claim {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let escrow_share_account = derive_escrow_share_account(&self.vault, &self.user, self.nonce);
        let meta_list = derive_extra_account_metas(&share_mint);
        let policy = derive_policy(&self.vault);

        let data = super::encode(&VaultInstruction::Claim);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(escrow_share_account, false),
                AccountMeta::new(self.user_share_account, false),
                AccountMeta::new_readonly(share_mint, false),
                AccountMeta::new_readonly(meta_list, false),
                AccountMeta::new_readonly(policy, false),
                AccountMeta::new_readonly(HOOK_ID, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
