use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{derive_escrow, derive_share_mint},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::pod::PodU64},
};

/// Burns the user's shares (atomic, matching GLAM's burn-on-enqueue model)
/// and stamps an `Escrow` record the manager later fulfills against.
/// `nonce` discriminates the escrow PDA — a user may hold several concurrent
/// requests with distinct nonces.
pub struct Redeem {
    pub user: Address,
    pub user_share_account: Address,
    pub vault: Address,
    pub shares: u64,
    pub nonce: u64,
}

impl Redeem {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);

        let shares = PodU64::from(self.shares);
        let nonce = PodU64::from(self.nonce);
        let data = super::encode(&VaultInstruction::Redeem {
            shares: &shares,
            nonce: &nonce,
        });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new(self.user_share_account, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(share_mint, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new_readonly(crate::instructions::SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
