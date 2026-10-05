use offerbook_client::{instructions::ClaimNonFungibleLoanAccounts, solana_address::Address, ID};

#[test]
fn missing_optional_account_is_readonly_and_present_account_keeps_flags() {
    let mut accounts = ClaimNonFungibleLoanAccounts {
        signer: ID,
        signer_user: ID,
        borrower: ID,
        borrower_user: ID,
        loan: ID,
        loan_vault: None,
        config: ID,
        event_authority: ID,
        program: ID,
    };
    let absent = &accounts.to_account_metas()[5];
    assert_eq!(absent.pubkey, ID);
    assert!(!absent.is_writable);
    assert!(!absent.is_signer);

    let vault = Address::from_str_const("11111111111111111111111111111111");
    accounts.loan_vault = Some(vault);
    let present = &accounts.to_account_metas()[5];
    assert_eq!(present.pubkey, vault);
    assert!(present.is_writable);
    assert!(!present.is_signer);
}
