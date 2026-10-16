use {
    crate::{
        instructions::SYSTEM_PROGRAM_ID,
        pda::{derive_escrow, derive_escrow_token_account},
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::U64},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Queue a deposit with a user-signed minimum share output. The minimum is
/// persisted in a `Escrow` and checked by `Fulfill`; zero opts out.
/// A request that cannot meet its minimum can be cancelled for a refund.
pub struct Subscribe {
    pub user: Address,
    pub user_token_account: Address,
    pub vault: Address,
    pub base_mint: Address,
    pub token_program: Address,
    pub amount: u64,
    pub nonce: u64,
    /// Minimum raw share units to mint at fulfillment, not at enqueue time.
    pub min_shares_out: u64,
}

impl Subscribe {
    pub fn into_instruction(self) -> Instruction {
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let escrow_token_account = derive_escrow_token_account(&self.vault, &self.user, self.nonce);

        let amount = U64::from(self.amount);
        let nonce = U64::from(self.nonce);
        let min_shares_out = U64::from(self.min_shares_out);
        let data = super::encode(&VaultInstruction::Subscribe {
            amount: &amount,
            nonce: &nonce,
            min_shares_out: &min_shares_out,
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
