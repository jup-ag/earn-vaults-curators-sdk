use {
    crate::{
        instructions::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID},
        pda::{ASSOCIATED_TOKEN_PROGRAM_ID, derive_share_mint, derive_vault_authority},
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::pod::PodU64},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Un-queued deposit: transfers base into the vault and mints shares straight
/// to the user in one transaction. Reverts if the vault has supply but its AUM
/// snapshot is stale (`StalePrice`) — fall back to [`crate::Subscribe`] then.
/// `min_shares_out` is the least number of share base units the user accepts;
/// fewer shares reverts with `SubscribeOutputBelowMinimum`. `0` disables the bound.
///
/// `base_mint` must be the vault's canonical base asset (the first configured,
/// deposit-enabled row). Later deposit-enabled rows use the queued path. The
/// deposit is valued off the canonical row's cached price, so a
/// [`crate::ValidateAum`] must have run inside
/// `DEFAULT_ASSET_STALENESS_SECONDS` (`OracleStale` otherwise) — bootstrap
/// included, where the NAV itself needs no AUM.
///
/// `vault_token_account` must be a vault-PDA-owned token account of
/// `base_mint`; `user_share_account` is the user's account for the share mint.
pub struct SubscribeDirect {
    pub user: Address,
    pub user_token_account: Address,
    pub vault: Address,
    pub base_mint: Address,
    pub vault_token_account: Address,
    pub user_share_account: Address,
    pub base_token_program: Address,
    pub amount: u64,
    pub min_shares_out: u64,
}

impl SubscribeDirect {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let vault_authority = derive_vault_authority(&self.vault);

        let amount = PodU64::from(self.amount);
        let min_shares_out = PodU64::from(self.min_shares_out);
        let data = super::encode(&VaultInstruction::SubscribeDirect {
            amount: &amount,
            min_shares_out: &min_shares_out,
        });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new(self.user_token_account, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(share_mint, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new(self.vault_token_account, false),
                AccountMeta::new(self.user_share_account, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.base_token_program, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(ASSOCIATED_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
