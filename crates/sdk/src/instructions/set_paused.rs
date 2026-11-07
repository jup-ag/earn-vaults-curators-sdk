use {
    crate::pda::derive_protocol_delegate,
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// `set_paused` — granular pause over `Vault.paused` (`Pause::Subscribe` /
/// `Pause::Redeem` / `Pause::Strategy`). A PAUSE delegate or the program upgrade
/// authority can OR bits in — never clear them. Clearing is curator-only
/// via [`crate::instructions::UpdateVault`].
pub struct SetPaused {
    /// PAUSE delegate or program upgrade authority; must sign (including via CPI).
    pub signer: Address,
    pub vault: Address,
    /// `Pause` bitmask.
    pub flags: u8,
}

impl SetPaused {
    /// Use the current program upgrade authority (e.g. a Squads vault PDA),
    /// without a per-vault delegate grant. The existing account order is preserved.
    pub fn into_upgrade_authority_instruction(self) -> Instruction {
        self.instruction_with_authorization(earn_vaults_interface::constants::PROGRAM_DATA_PDA.0)
    }

    /// Use a per-vault delegate with the `vault::PAUSE` permission.
    pub fn into_instruction(self) -> Instruction {
        // protocol_id == vault program id for vault-internal permissions.
        let delegate = derive_protocol_delegate(&self.vault, &self.signer, &crate::ID);
        self.instruction_with_authorization(delegate)
    }

    fn instruction_with_authorization(self, authorization: Address) -> Instruction {
        let data = super::encode(&VaultInstruction::SetPaused { flags: &self.flags });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new_readonly(self.signer, true),
                AccountMeta::new_readonly(authorization, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        earn_vaults_interface::constants::{PROGRAM_DATA_PDA, UPGRADEABLE_LOADER_ID},
    };

    #[test]
    fn authorization_paths_preserve_pause_abi() {
        let signer = Address::new_from_array([7; 32]);
        let vault = Address::new_from_array([8; 32]);
        let program_data =
            Address::find_program_address(&[crate::ID.as_ref()], &UPGRADEABLE_LOADER_ID);
        assert_eq!(PROGRAM_DATA_PDA, program_data);

        for flags in 0..=7 {
            let delegate = SetPaused {
                signer,
                vault,
                flags,
            }
            .into_instruction();
            let upgrade = SetPaused {
                signer,
                vault,
                flags,
            }
            .into_upgrade_authority_instruction();
            assert_eq!(delegate.program_id, crate::ID);
            assert_eq!(upgrade.program_id, crate::ID);
            assert_eq!(delegate.data, [19, flags]);
            assert_eq!(upgrade.data, delegate.data);
            let mut accounts = vec![
                AccountMeta::new_readonly(signer, true),
                AccountMeta::new_readonly(
                    derive_protocol_delegate(&vault, &signer, &crate::ID),
                    false,
                ),
                AccountMeta::new(vault, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ];
            assert_eq!(delegate.accounts, accounts);
            accounts[1] = AccountMeta::new_readonly(program_data.0, false);
            assert_eq!(upgrade.accounts, accounts);
        }
    }
}
