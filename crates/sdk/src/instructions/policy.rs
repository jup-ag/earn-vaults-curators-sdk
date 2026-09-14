//! Curator-side policy ixs + raw transfer-hook `Execute` builder.

use {
    crate::{
        instructions::SYSTEM_PROGRAM_ID,
        pda::{derive_extra_account_metas, derive_policy, derive_share_mint},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{
        VaultInstruction,
        constants::{EVENT_PDA, HOOK_ID, HOOK_IX_EXECUTE, HOOK_IX_INIT_EXTRA_ACCOUNT_META_LIST},
    },
};

pub struct SetPausedTransfers {
    pub curator: Address,
    pub vault: Address,
    pub paused: bool,
}

impl SetPausedTransfers {
    pub fn into_instruction(self) -> Instruction {
        let policy = derive_policy(&self.vault);
        let paused = self.paused as u8;
        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new_readonly(self.curator, true),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new(policy, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data: super::encode(&VaultInstruction::SetPausedTransfers { paused: &paused }),
        }
    }
}

/// Installs the transfer-hook `ExtraAccountMetaList` PDA for the vault's share
/// mint. Dispatched via the SPL `InitializeExtraAccountMetaList` discriminator
/// on the **hook** program (`HOOK_ID`), which owns the PDA. Call once per vault
/// (after `InitializeVault`, which creates the policy PDA); shares can't move
/// until it exists.
pub struct InitializeExtraAccountMetas {
    pub payer: Address,
    pub vault: Address,
}

impl InitializeExtraAccountMetas {
    pub fn into_instruction(self) -> Instruction {
        let mint = derive_share_mint(&self.vault);
        let meta_list = derive_extra_account_metas(&mint);
        Instruction {
            program_id: HOOK_ID,
            accounts: vec![
                AccountMeta::new(self.payer, true),
                AccountMeta::new(meta_list, false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data: HOOK_IX_INIT_EXTRA_ACCOUNT_META_LIST.to_vec(),
        }
    }
}

/// Raw `Execute` builder against the hook program — simulates a
/// Token-2022-driven hook invocation. Used in tests today; in production
/// Token-2022 builds this CPI itself once the share mint has the TransferHook
/// extension wired up.
pub struct TransferHookExecute {
    pub source_token_account: Address,
    pub mint: Address,
    pub destination_token_account: Address,
    pub owner: Address,
    pub extra_account_meta_list: Address,
    pub policy: Address,
    /// Amount transferred. Hook doesn't use it today; included for wire
    /// compatibility with the SPL interface.
    pub amount: u64,
}

impl TransferHookExecute {
    pub fn into_instruction(self) -> Instruction {
        // Data = [HOOK_IX_EXECUTE(8), amount(8)].
        let mut data = Vec::with_capacity(16);
        data.extend_from_slice(&HOOK_IX_EXECUTE);
        data.extend_from_slice(&self.amount.to_le_bytes());

        Instruction {
            program_id: HOOK_ID,
            accounts: vec![
                AccountMeta::new_readonly(self.source_token_account, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(self.destination_token_account, false),
                AccountMeta::new_readonly(self.owner, false),
                AccountMeta::new_readonly(self.extra_account_meta_list, false),
                AccountMeta::new_readonly(self.policy, false),
            ],
            data,
        }
    }
}
