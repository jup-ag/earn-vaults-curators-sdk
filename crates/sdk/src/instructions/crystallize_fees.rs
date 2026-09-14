use {
    crate::{
        instructions::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID},
        pda::{
            derive_curator_fee_account, derive_protocol_fee_account, derive_share_mint,
            derive_vault_authority,
        },
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
};

/// Crystallize accrued fees as newly minted shares. Permissionless — `signer`
/// can be anyone and pays rent when either program-owned fee account must be
/// created. The program pins both destinations to their per-vault PDAs.
pub struct CrystallizeFees {
    pub signer: Address,
    pub vault: Address,
}

impl CrystallizeFees {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let curator_fee_account = derive_curator_fee_account(&self.vault);
        let protocol_fee_account = derive_protocol_fee_account(&self.vault);
        let vault_authority = derive_vault_authority(&self.vault);

        let data = super::encode(&VaultInstruction::CrystallizeFees);

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.signer, true),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(share_mint, false),
                AccountMeta::new(curator_fee_account, false),
                AccountMeta::new(protocol_fee_account, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
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
    fn derives_both_fee_destinations_in_program_order() {
        let signer = Address::new_from_array([1; 32]);
        let vault = Address::new_from_array([2; 32]);
        let ix = CrystallizeFees { signer, vault }.into_instruction();

        assert_eq!(ix.data, vec![17]);
        assert_eq!(ix.accounts.len(), 10);
        assert_eq!(ix.accounts[0].pubkey, signer);
        assert!(ix.accounts[0].is_signer);
        assert!(ix.accounts[0].is_writable);
        assert_eq!(ix.accounts[1].pubkey, vault);
        assert_eq!(ix.accounts[2].pubkey, derive_share_mint(&vault));
        assert_eq!(ix.accounts[3].pubkey, derive_curator_fee_account(&vault));
        assert_eq!(ix.accounts[4].pubkey, derive_protocol_fee_account(&vault));
        assert_eq!(ix.accounts[5].pubkey, derive_vault_authority(&vault));
        assert_eq!(ix.accounts[6].pubkey, TOKEN_2022_PROGRAM_ID);
        assert_eq!(ix.accounts[7].pubkey, SYSTEM_PROGRAM_ID);
        assert_eq!(ix.accounts[8].pubkey, EVENT_PDA.0);
        assert_eq!(ix.accounts[9].pubkey, crate::ID);
    }
}
