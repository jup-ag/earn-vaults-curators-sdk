use {
    super::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID},
    crate::{
        instructions::TOKEN_PROGRAM_ID,
        pda::{derive_policy, derive_share_mint, derive_vault_authority},
    },
    earn_vaults_interface::VaultInstruction,
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Curator closes a wound-down vault: the vault state, the share mint and any
/// lamports parked on the authority PDA all refund to `recipient`, plus the
/// `Policy` PDA when it exists, plus every configured asset's empty custody
/// ATA in vault asset order. Requires zero share supply, no in-flight redeems,
/// zero AUM, no sinks, non-lender-only positions or deployments. A lender-only
/// position may remain if its mark is zero. For any absent custody ATA, call
/// the associated-token program's `CreateIdempotent` with the vault authority,
/// asset mint and mint's token program. Run `SweepIdle` for idle assets or
/// written-off sinks, then `ValidateAum` before closing.
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
