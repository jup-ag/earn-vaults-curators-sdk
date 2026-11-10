use {
    crate::{
        instructions::SYSTEM_PROGRAM_ID,
        pda::{
            derive_escrow, derive_escrow_token_account, derive_protocol_delegate,
            derive_share_mint, derive_vault_authority,
        },
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Settles a queued redeem: transfers base asset out of the vault into the
/// per-user proceeds escrow. Shares were burned at `Redeem` time, so this
/// step pays the net shares and leaves the fee's backing in the vault for
/// remaining holders. The current fee rate applies at settlement, except when
/// the request represents all remaining live and pending shares.
pub struct FulfillRedeem {
    pub authority: Address,
    pub vault: Address,
    pub user: Address,
    /// Nonce the escrow was opened with (`Redeem.nonce`).
    pub nonce: u64,
    pub base_mint: Address,
    pub vault_token_account: Address,
    pub token_program: Address,
}

impl FulfillRedeem {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);
        let escrow_token_account = derive_escrow_token_account(&self.vault, &self.user, self.nonce);
        let vault_authority = derive_vault_authority(&self.vault);
        let delegate = derive_protocol_delegate(&self.vault, &self.authority, &crate::ID);

        let data = super::encode(&VaultInstruction::FulfillRedeem);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.authority, true),
                AccountMeta::new_readonly(delegate, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(share_mint, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new(escrow_token_account, false),
                AccountMeta::new(self.vault_token_account, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fulfillment_reads_share_mint_and_ends_before_transfer_hooks() {
        let vault = Address::new_from_array([2; 32]);
        let ix = FulfillRedeem {
            authority: Address::new_from_array([1; 32]),
            vault,
            user: Address::new_from_array([3; 32]),
            nonce: 42,
            base_mint: Address::new_from_array([4; 32]),
            vault_token_account: Address::new_from_array([5; 32]),
            token_program: Address::new_from_array([6; 32]),
        }
        .into_instruction();
        assert_eq!(ix.data, [12]);
        assert_eq!(ix.accounts.len(), 13);
        assert_eq!(
            ix.accounts[3],
            AccountMeta::new_readonly(derive_share_mint(&vault), false)
        );
        assert_eq!(ix.accounts[12], AccountMeta::new_readonly(crate::ID, false));
    }
}
