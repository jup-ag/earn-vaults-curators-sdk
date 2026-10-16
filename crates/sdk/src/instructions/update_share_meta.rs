use {
    crate::{
        SdkErrors,
        instructions::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID},
        pda::{derive_share_mint, derive_vault_authority},
    },
    earn_vaults_interface::{
        VaultInstruction,
        arguments::UpdateShareMetaArgs,
        encoding::{PodOption, PodString},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Curator-only update of the share mint's Token-2022 metadata. `None` fields
/// are left untouched. Limits: name 25, symbol 5, uri 200 bytes.
pub struct UpdateShareMeta<'a> {
    pub curator: Address,
    pub vault: Address,
    pub name: Option<&'a str>,
    pub symbol: Option<&'a str>,
    pub uri: Option<&'a str>,
}

impl UpdateShareMeta<'_> {
    pub fn into_instruction(self) -> Result<Instruction, SdkErrors> {
        fn opt<const N: usize>(s: Option<&str>) -> Result<PodOption<PodString<N>>, SdkErrors> {
            Ok(s.map(PodString::try_from)
                .transpose()?
                .map_or_else(PodOption::none, PodOption::some))
        }
        let args = UpdateShareMetaArgs {
            metadata_name: opt(self.name)?,
            metadata_symbol: opt(self.symbol)?,
            metadata_uri: opt(self.uri)?,
        };
        let data = super::encode(&VaultInstruction::UpdateShareMeta { args: &args });

        Ok(Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.curator, true),
                AccountMeta::new_readonly(self.vault, false),
                AccountMeta::new_readonly(derive_vault_authority(&self.vault), false),
                AccountMeta::new(derive_share_mint(&self.vault), false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_instruction() {
        let ix = UpdateShareMeta {
            curator: Address::new_from_array([1; 32]),
            vault: Address::new_from_array([2; 32]),
            name: Some("Fund"),
            symbol: None,
            uri: None,
        }
        .into_instruction()
        .unwrap();
        assert_eq!(ix.data[0], 21); // UpdateShareMeta discriminant
        assert_eq!(ix.accounts.len(), 6);
        assert!(ix.accounts[0].is_signer);
        assert!(ix.accounts[0].is_writable); // curator funds the realloc rent top-up
        assert!(ix.accounts[3].is_writable); // share mint takes the metadata write

        let too_long = UpdateShareMeta {
            curator: Address::new_from_array([1; 32]),
            vault: Address::new_from_array([2; 32]),
            name: Some("this name is way past the 25-byte cap"),
            symbol: None,
            uri: None,
        }
        .into_instruction();
        assert!(too_long.is_err());
    }
}
