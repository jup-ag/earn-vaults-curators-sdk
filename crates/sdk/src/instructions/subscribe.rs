use {
    crate::{
        instructions::SYSTEM_PROGRAM_ID,
        pda::{derive_escrow, derive_escrow_token_account},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::pod::PodU64},
};

pub struct Subscribe {
    pub user: Address,
    pub user_token_account: Address,
    pub vault: Address,
    pub base_mint: Address,
    pub token_program: Address,
    pub amount: u64,
    pub nonce: u64,
}

impl Subscribe {
    pub fn into_instruction(self) -> Instruction {
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let escrow_token_account = derive_escrow_token_account(&self.vault, &self.user, self.nonce);

        let amount = PodU64::from(self.amount);
        let nonce = PodU64::from(self.nonce);
        let data = super::encode(&VaultInstruction::Subscribe {
            amount: &amount,
            nonce: &nonce,
        });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new(self.user_token_account, false),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(escrow_token_account, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
