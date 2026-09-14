use {
    crate::pda::{
        derive_associated_token_account, derive_protocol_delegate, derive_vault_authority,
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::pod::PodU64},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Instructions sysvar used to validate the flash pair.
pub const INSTRUCTIONS_SYSVAR_ID: Address =
    Address::from_str_const("Sysvar1nstructions1111111111111111111111111");

/// Lend a listed asset to vault custody for one transaction.
///
/// Requires exactly one later [`FlashWithdraw`] for the same vault, mint, and
/// principal:
///
/// ```text
/// flashloan_borrow(P)            (the external program, operator signs)
/// FlashDeposit  { amount: P }
/// ProtocolCpi   { ... }          (any number of ordinary routed legs)
/// FlashWithdraw { amount: P }
/// flashloan_payback(P + fee)     (the external program, operator signs)
/// ```
///
/// Must be invoked at the transaction level by a `FLASH` delegate.
pub struct FlashDeposit {
    /// Operator with the `FLASH` permission.
    pub signer: Address,
    pub vault: Address,
    /// A listed `Vault.assets` mint.
    pub mint: Address,
    /// Source account; defaults to the operator's ATA.
    pub signer_token_account: Option<Address>,
    /// Program that owns `mint`.
    pub token_program: Address,
    pub amount: u64,
}

/// Return the principal and clear the transient liability.
pub struct FlashWithdraw {
    pub signer: Address,
    pub vault: Address,
    pub mint: Address,
    pub signer_token_account: Option<Address>,
    pub token_program: Address,
    /// Must equal `flash_liability` exactly.
    pub amount: u64,
}

/// Shared account prefix. The on-chain scanner relies on this order.
fn common_metas(
    signer: Address,
    vault: Address,
    mint: Address,
    signer_token_account: Option<Address>,
    token_program: Address,
) -> Vec<AccountMeta> {
    let authority = derive_vault_authority(&vault);
    let signer_ta = signer_token_account
        .unwrap_or_else(|| derive_associated_token_account(&signer, &mint, &token_program));
    vec![
        AccountMeta::new_readonly(signer, true),
        AccountMeta::new_readonly(derive_protocol_delegate(&vault, &signer, &crate::ID), false),
        AccountMeta::new(vault, false),
        AccountMeta::new_readonly(mint, false),
        AccountMeta::new(signer_ta, false),
        AccountMeta::new(
            derive_associated_token_account(&authority, &mint, &token_program),
            false,
        ),
    ]
}

impl FlashDeposit {
    pub fn into_instruction(self) -> Instruction {
        let amount = PodU64::from(self.amount);
        let data = super::encode(&VaultInstruction::FlashDeposit { amount: &amount });

        let mut accounts = common_metas(
            self.signer,
            self.vault,
            self.mint,
            self.signer_token_account,
            self.token_program,
        );
        accounts.push(AccountMeta::new_readonly(self.token_program, false));
        accounts.push(AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false));
        accounts.push(AccountMeta::new_readonly(EVENT_PDA.0, false));
        accounts.push(AccountMeta::new_readonly(crate::ID, false));

        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}

impl FlashWithdraw {
    pub fn into_instruction(self) -> Instruction {
        let amount = PodU64::from(self.amount);
        let data = super::encode(&VaultInstruction::FlashWithdraw { amount: &amount });

        let mut accounts = common_metas(
            self.signer,
            self.vault,
            self.mint,
            self.signer_token_account,
            self.token_program,
        );
        // The custody transfer signs with the vault authority PDA.
        accounts.push(AccountMeta::new_readonly(
            derive_vault_authority(&self.vault),
            false,
        ));
        accounts.push(AccountMeta::new_readonly(self.token_program, false));
        accounts.push(AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false));
        accounts.push(AccountMeta::new_readonly(EVENT_PDA.0, false));
        accounts.push(AccountMeta::new_readonly(crate::ID, false));

        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}
