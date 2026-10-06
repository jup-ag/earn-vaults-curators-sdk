use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{
            derive_curator_fee_account, derive_extra_account_metas, derive_policy,
            derive_protocol_fee_account, derive_share_mint, derive_vault_authority,
        },
    },
    earn_vaults_interface::{
        VaultInstruction,
        constants::{EVENT_PDA, HOOK_ID, PROTOCOL_TREASURY},
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
        let fee_account = if self.signer == PROTOCOL_TREASURY {
            derive_protocol_fee_account(&self.vault)
        } else {
            derive_curator_fee_account(&self.vault)
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> Address {
        Address::new_from_array([byte; 32])
    }

    #[test]
    fn signer_role_selects_and_binds_the_fee_pda() {
        let vault = address(2);
        let destination = address(3);
        let curator = address(1);
        let curator_ix = ClaimFees {
            signer: curator,
            vault,
            destination,
        }
        .into_instruction();

        assert_eq!(curator_ix.data, vec![26]);
        assert_eq!(curator_ix.accounts.len(), 12);
        assert_eq!(curator_ix.accounts[0].pubkey, curator);
        assert!(curator_ix.accounts[0].is_signer);
        assert!(curator_ix.accounts[0].is_writable);
        assert_eq!(curator_ix.accounts[1].pubkey, vault);
        assert_eq!(curator_ix.accounts[2].pubkey, derive_share_mint(&vault));
        assert_eq!(
            curator_ix.accounts[3].pubkey,
            derive_curator_fee_account(&vault)
        );
        assert_eq!(curator_ix.accounts[4].pubkey, destination);
        assert!(curator_ix.accounts[3].is_writable);
        assert!(curator_ix.accounts[4].is_writable);
        assert_eq!(
            curator_ix.accounts[5].pubkey,
            derive_vault_authority(&vault)
        );
        assert_eq!(
            curator_ix.accounts[6].pubkey,
            derive_extra_account_metas(&derive_share_mint(&vault))
        );
        assert_eq!(curator_ix.accounts[7].pubkey, derive_policy(&vault));
        assert_eq!(curator_ix.accounts[8].pubkey, HOOK_ID);
        assert_eq!(curator_ix.accounts[9].pubkey, TOKEN_2022_PROGRAM_ID);
        assert_eq!(curator_ix.accounts[10].pubkey, EVENT_PDA.0);
        assert_eq!(curator_ix.accounts[11].pubkey, crate::ID);

        let protocol_ix = ClaimFees {
            signer: PROTOCOL_TREASURY,
            vault,
            destination,
        }
        .into_instruction();
        assert_eq!(
            protocol_ix.accounts[3].pubkey,
            derive_protocol_fee_account(&vault)
        );
    }
}
