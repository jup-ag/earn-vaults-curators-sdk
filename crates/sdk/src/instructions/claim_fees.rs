use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{
            derive_curator_fee_account, derive_extra_account_metas, derive_policy,
            derive_share_mint, derive_vault_authority,
        },
    },
    earn_vaults_interface::{
        VaultInstruction,
        constants::{EVENT_PDA, HOOK_ID},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Claim the caller's already-minted fee shares into a caller-owned token
/// account. The protocol treasury selects the protocol fee PDA; every other
/// signer selects the curator PDA and is validated against the vault on-chain.
pub struct ClaimFees {
    pub signer: Address,
    pub vault: Address,
    pub destination: Address,
}

impl ClaimFees {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let fee_account = derive_curator_fee_account(&self.vault);
        let vault_authority = derive_vault_authority(&self.vault);
        let meta_list = derive_extra_account_metas(&share_mint);
        let policy = derive_policy(&self.vault);

        let data = super::encode(&VaultInstruction::ClaimFees);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.signer, true),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new_readonly(share_mint, false),
                AccountMeta::new(fee_account, false),
                AccountMeta::new(self.destination, false),
                AccountMeta::new_readonly(vault_authority, false),
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
