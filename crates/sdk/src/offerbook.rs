//! Offerbook (`offerbkFM…`) routes whose account lists the SDK derives, each
//! wrapped in a [`ProtocolCpi`] signed by the vault's Offerbook delegate.
//!
//! ABI: jup-ag/offerbook-program@736d809 (audit-2026-11). The vault authority
//! owns the vault's Offerbook `User` (`["user", vault_authority]`).

use {
    crate::{
        instructions::ProtocolCpi,
        pda::{
            derive_associated_token_account, derive_protocol_policy, derive_vault_authority,
            protocol_policy_meta,
        },
        state::Protocol,
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

pub const PROGRAM_ID: Address =
    Address::from_str_const("offerbkFMvVfpQhL8ZQ5iromnjct5rz3r52B9ewu3ie");

pub const IX_SET_YIELD_OPERATOR: [u8; 8] = [191, 2, 217, 79, 34, 85, 48, 82];
pub const IX_EXTEND_LOAN: [u8; 8] = [2, 208, 222, 190, 109, 148, 247, 117];

/// Token/token `Loan` field offsets (discriminator included).
pub const LOAN_LENDER_OFFSET: usize = 8;
pub const LOAN_PRINCIPAL_MINT_OFFSET: usize = 153;
pub const LOAN_PRINCIPAL_TOKEN_PROGRAM_OFFSET: usize = 185;
pub const LOAN_COLLATERAL_MINT_OFFSET: usize = 425;
pub const LOAN_COLLATERAL_TOKEN_PROGRAM_OFFSET: usize = 457;
/// `Config.fee_authority`, owner of the protocol fee ATAs.
pub const CONFIG_FEE_AUTHORITY_OFFSET: usize = 40;

/// `["user", owner]`: an owner's Offerbook `User`.
pub fn derive_user(owner: &Address) -> Address {
    Address::find_program_address(&[b"user", owner.as_array()], &PROGRAM_ID).0
}

/// `["config"]`: Offerbook's global `Config`.
pub fn derive_config() -> Address {
    Address::find_program_address(&[b"config"], &PROGRAM_ID).0
}

/// `["__event_authority"]`: Anchor's `emit_cpi!` authority.
pub fn derive_event_authority() -> Address {
    Address::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

/// `["loan_vault", loan]`: the token account holding a loan's collateral.
pub fn derive_loan_vault(loan: &Address) -> Address {
    Address::find_program_address(&[b"loan_vault", loan.as_array()], &PROGRAM_ID).0
}

/// `set_yield_operator(Option<Pubkey>)` for the vault's own Offerbook `User`:
/// name the key that may top up (`escrow_yield_withdraw`) and compound
/// (`escrow_yield_deposit`) its escrow yield alongside the vault authority and
/// Offerbook's operator, or clear it with `None`. Naming a key needs
/// `permissions::offerbook::YIELD`; clearing needs `ESCROW`.
pub struct SetYieldOperator {
    /// The Offerbook delegate signing the outer `ProtocolCpi`.
    pub signer: Address,
    pub vault: Address,
    pub operator: Option<Address>,
}

impl SetYieldOperator {
    pub fn cpi_data(&self) -> Vec<u8> {
        let mut data = IX_SET_YIELD_OPERATOR.to_vec();
        match self.operator {
            None => data.push(0),
            Some(key) => {
                data.push(1);
                data.extend_from_slice(key.as_array());
            }
        }
        data
    }

    pub fn protocol_cpi(self) -> ProtocolCpi {
        let authority = derive_vault_authority(&self.vault);
        ProtocolCpi {
            signer: self.signer,
            vault: self.vault,
            target_program: PROGRAM_ID,
            cpi_data: self.cpi_data(),
            forwarded: vec![
                AccountMeta::new_readonly(authority, false),
                AccountMeta::new(derive_user(&authority), false),
                AccountMeta::new_readonly(derive_event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        }
    }

    pub fn into_instruction(self) -> Instruction {
        self.protocol_cpi().into_instruction()
    }
}

/// `extend_loan` as borrower on a token/token loan, in the 18-account layout
/// of offerbook-program@736d809, followed by the vault's loan-limits policy
/// and the collateral mint's policy. Needs `permissions::offerbook::BORROW`.
///
/// The four optional collateral accounts are always passed: Offerbook needs
/// them only when the loan was paid down this period (`repaid_principal > 0`),
/// to release the surplus collateral into the vault's escrow, and ignores them
/// otherwise. `core_asset` is passed absent: the vault borrows against tokens
/// only. Read the loan's fields at the `LOAN_*_OFFSET`s and the config's fee
/// authority at [`CONFIG_FEE_AUTHORITY_OFFSET`].
pub struct ExtendLoan {
    /// The Offerbook delegate signing the outer `ProtocolCpi`.
    pub signer: Address,
    pub vault: Address,
    pub loan: Address,
    pub lender: Address,
    pub principal_mint: Address,
    pub principal_token_program: Address,
    pub collateral_mint: Address,
    pub collateral_token_program: Address,
    /// `Config.fee_authority`.
    pub fee_authority: Address,
}

impl ExtendLoan {
    pub fn protocol_cpi(self) -> ProtocolCpi {
        let authority = derive_vault_authority(&self.vault);
        let user = derive_user(&authority);
        let lender_user = derive_user(&self.lender);
        let principal_ata = |owner: &Address| {
            derive_associated_token_account(
                owner,
                &self.principal_mint,
                &self.principal_token_program,
            )
        };
        ProtocolCpi {
            signer: self.signer,
            vault: self.vault,
            target_program: PROGRAM_ID,
            cpi_data: IX_EXTEND_LOAN.to_vec(),
            forwarded: vec![
                AccountMeta::new(authority, false),
                AccountMeta::new_readonly(user, false),
                AccountMeta::new_readonly(self.lender, false),
                AccountMeta::new_readonly(lender_user, false),
                AccountMeta::new(self.loan, false),
                AccountMeta::new_readonly(derive_config(), false),
                AccountMeta::new_readonly(self.principal_mint, false),
                AccountMeta::new(principal_ata(&authority), false),
                AccountMeta::new(principal_ata(&lender_user), false),
                AccountMeta::new(principal_ata(&self.fee_authority), false),
                AccountMeta::new_readonly(self.principal_token_program, false),
                AccountMeta::new_readonly(self.collateral_mint, false),
                AccountMeta::new(derive_loan_vault(&self.loan), false),
                AccountMeta::new(
                    derive_associated_token_account(
                        &user,
                        &self.collateral_mint,
                        &self.collateral_token_program,
                    ),
                    false,
                ),
                AccountMeta::new_readonly(self.collateral_token_program, false),
                // `core_asset`: absent.
                AccountMeta::new_readonly(PROGRAM_ID, false),
                AccountMeta::new_readonly(derive_event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
                protocol_policy_meta(&self.vault, Protocol::Offerbook),
                AccountMeta::new_readonly(
                    derive_protocol_policy(&self.vault, Protocol::Offerbook, &self.collateral_mint),
                    false,
                ),
            ],
        }
    }

    pub fn into_instruction(self) -> Instruction {
        self.protocol_cpi().into_instruction()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_yield_operator_encodes_borsh_option() {
        let vault = Address::new_from_array([1; 32]);
        let signer = Address::new_from_array([2; 32]);
        let operator = Address::new_from_array([3; 32]);
        let some = SetYieldOperator {
            signer,
            vault,
            operator: Some(operator),
        };
        let data = some.cpi_data();
        assert_eq!(data.len(), 41);
        assert_eq!(&data[..8], &IX_SET_YIELD_OPERATOR);
        assert_eq!(data[8], 1);
        assert_eq!(&data[9..], operator.as_array());
        let cpi = some.protocol_cpi();
        assert_eq!(cpi.forwarded.len(), 4);
        assert_eq!(cpi.forwarded[0].pubkey, derive_vault_authority(&vault));
        assert!(cpi.forwarded[1].is_writable);
        let none = SetYieldOperator {
            signer,
            vault,
            operator: None,
        };
        assert_eq!(
            none.cpi_data(),
            [IX_SET_YIELD_OPERATOR.as_slice(), &[0]].concat()
        );
    }

    #[test]
    fn extend_loan_uses_the_eighteen_account_layout() {
        let vault = Address::new_from_array([1; 32]);
        let loan = Address::new_from_array([4; 32]);
        let collateral_mint = Address::new_from_array([7; 32]);
        let cpi = ExtendLoan {
            signer: Address::new_from_array([2; 32]),
            vault,
            loan,
            lender: Address::new_from_array([3; 32]),
            principal_mint: Address::new_from_array([5; 32]),
            principal_token_program: Address::new_from_array([6; 32]),
            collateral_mint,
            collateral_token_program: Address::new_from_array([8; 32]),
            fee_authority: Address::new_from_array([9; 32]),
        }
        .protocol_cpi();
        assert_eq!(cpi.cpi_data, IX_EXTEND_LOAN);
        assert_eq!(cpi.forwarded.len(), 20);
        assert_eq!(cpi.forwarded[11].pubkey, collateral_mint);
        assert_eq!(cpi.forwarded[12].pubkey, derive_loan_vault(&loan));
        assert!(cpi.forwarded[12].is_writable && cpi.forwarded[13].is_writable);
        assert_eq!(cpi.forwarded[15].pubkey, PROGRAM_ID);
        assert_eq!(cpi.forwarded[17].pubkey, PROGRAM_ID);
        assert_eq!(
            cpi.forwarded[19].pubkey,
            derive_protocol_policy(&vault, Protocol::Offerbook, &collateral_mint)
        );
    }
}
