use {
    crate::pda::{
        derive_associated_token_account, derive_protocol_delegate, derive_vault_authority,
    },
    earn_vaults_interface::{VaultInstruction, constants::NEST_OFT_ID},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Temporary pre-instruction for a Nest redeem while LayerZero's send stack
/// cannot fit below another CPI. Place the actual OFT `send` immediately after
/// this instruction in the same transaction.
pub struct PrepareNestRedeem {
    pub signer: Address,
    pub vault: Address,
    pub share_mint: Address,
    pub token_program: Address,
}

impl PrepareNestRedeem {
    pub fn into_instruction(self) -> Instruction {
        let authority = derive_vault_authority(&self.vault);
        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new_readonly(self.signer, true),
                AccountMeta::new_readonly(
                    derive_protocol_delegate(&self.vault, &self.signer, &NEST_OFT_ID),
                    false,
                ),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(authority, false),
                AccountMeta::new_readonly(self.share_mint, false),
                AccountMeta::new(
                    derive_associated_token_account(
                        &authority,
                        &self.share_mint,
                        &self.token_program,
                    ),
                    false,
                ),
                AccountMeta::new(
                    derive_associated_token_account(
                        &self.signer,
                        &self.share_mint,
                        &self.token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(self.token_program, false),
                AccountMeta::new_readonly(
                    Address::from_str_const("Sysvar1nstructions1111111111111111111111111"),
                    false,
                ),
            ],
            data: super::encode(&VaultInstruction::PrepareNestRedeem),
        }
    }
}
