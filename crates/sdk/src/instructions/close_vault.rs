use {
    super::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID},
    crate::{
        instructions::TOKEN_PROGRAM_ID,
        pda::{derive_policy, derive_share_mint, derive_vault_authority},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::VaultInstruction,
};

/// Curator closes a wound-down vault: the vault state, the share mint and any
/// lamports parked on the authority PDA all refund to `recipient`, plus the
/// `Policy` PDA when it exists, plus any `custody_token_accounts` passed (the
/// vault's per-asset ATAs, already empty). Requires
/// zero share supply, no in-flight redeems and zero AUM — run `SweepIdle` +
/// `ValidateAum` first if donated dust is still counted.
pub struct CloseVault {
    pub curator: Address,
    pub vault: Address,
    pub recipient: Address,
    pub custody_token_accounts: Vec<Address>,
}

impl CloseVault {
    pub fn into_instruction(self) -> Instruction {
        let data = super::encode(&VaultInstruction::CloseVault);

        let mut accounts = vec![
            AccountMeta::new_readonly(self.curator, true),
            AccountMeta::new(self.vault, false),
            AccountMeta::new(derive_share_mint(&self.vault), false),
            AccountMeta::new(self.recipient, false),
            AccountMeta::new(derive_vault_authority(&self.vault), false),
            AccountMeta::new(derive_policy(&self.vault), false),
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ];
        accounts.extend(
            self.custody_token_accounts
                .into_iter()
                .map(|a| AccountMeta::new(a, false)),
        );

        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}
