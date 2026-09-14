use {
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Recompute `Vault.aum` from live custody balances and the last posted position
/// marks.
///
/// Takes **no price accounts**: every priced asset values off the reading
/// [`RefreshPrices`] cached on its row, and `validate_aum` re-runs the same
/// freshness/confidence gate — so a row nobody refreshed inside its window fails
/// the whole AUM closed. Bundle `RefreshPrices` ahead of this in the same
/// transaction to price AUM off readings taken this slot.
///
/// `custody_token_accounts` is one vault-owned token account per `PRICE_IN_AUM`
/// asset row, **in row order** — each is pinned to the row's canonical custody
/// ATA on-chain, so a wrong or reordered account is rejected rather than
/// mispriced. Decimals come from `AssetConfig.decimals`, so no mint is passed.
///
/// [`RefreshPrices`]: super::RefreshPrices
pub struct ValidateAum {
    pub vault: Address,
    pub share_mint: Address,
    pub custody_token_accounts: Vec<Address>,
}

impl ValidateAum {
    pub fn into_instruction(self) -> Instruction {
        let data = super::encode(&VaultInstruction::ValidateAum);
        let mut accounts = vec![
            AccountMeta::new(self.vault, false),
            AccountMeta::new_readonly(self.share_mint, false),
            AccountMeta::new_readonly(EVENT_PDA.0, false),
            AccountMeta::new_readonly(crate::ID, false),
        ];
        for token_account in &self.custody_token_accounts {
            accounts.push(AccountMeta::new_readonly(*token_account, false));
        }
        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}
