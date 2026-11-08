use offerbook_client::{
    ID,
    instructions::{
        ExtendLoan, ExtendLoanAccounts, SetYieldOperator, SetYieldOperatorInstructionArgs,
    },
    serialize,
    solana_address::Address,
    state::User,
};

#[test]
fn renewal_keeps_optional_collateral_before_event_accounts() {
    let collateral = Address::new_from_array([1; 32]);
    let loan_vault = Address::new_from_array([2; 32]);
    let escrow = Address::new_from_array([3; 32]);
    let accounts = ExtendLoanAccounts {
        signer: ID,
        signer_user: ID,
        lender: ID,
        lender_user: ID,
        loan: ID,
        config: ID,
        principal_mint: ID,
        signer_principal_token_account: ID,
        lender_principal_escrow: ID,
        protocol_fee_token_account: ID,
        principal_token_program: ID,
        collateral_mint: Some(collateral),
        loan_vault: Some(loan_vault),
        borrower_collateral_escrow: Some(escrow),
        collateral_token_program: Some(ID),
        core_asset: None,
        event_authority: Address::new_from_array([4; 32]),
        program: ID,
    };
    let ix = ExtendLoan::instruction(&accounts);
    assert_eq!(ix.data, [2, 208, 222, 190, 109, 148, 247, 117]);
    assert_eq!(ix.accounts.len(), 18);
    assert_eq!(ix.accounts[11].pubkey, collateral);
    assert_eq!(ix.accounts[12].pubkey, loan_vault);
    assert_eq!(ix.accounts[13].pubkey, escrow);
    assert!(ix.accounts[12].is_writable && ix.accounts[13].is_writable);
    assert_eq!(ix.accounts[15].pubkey, ID);
    assert!(!ix.accounts[15].is_writable && !ix.accounts[15].is_signer);
    assert_eq!(ix.accounts[16].pubkey, accounts.event_authority);
    assert_eq!(ix.accounts[17].pubkey, ID);
}

#[test]
fn user_reserved_bytes_decode_operator_and_fee_profile_without_growing() {
    let mut bytes = vec![0; 208];
    bytes[..8].copy_from_slice(&User::DISCRIMINATOR);
    let legacy = User::from_account_data(&bytes).unwrap();
    assert_eq!(legacy.yield_operator, Address::default());
    assert_eq!(legacy.origination_fee_bps, 0);
    assert_eq!(serialize(&legacy).unwrap().len(), 200);

    bytes[137] = 3; // Yield enabled and custom fees.
    bytes[144..176].fill(7);
    for (offset, rate) in [(176, 100u16), (178, 200), (180, 300)] {
        bytes[offset..offset + 2].copy_from_slice(&rate.to_le_bytes());
    }
    let user = User::from_account_data(&bytes).unwrap();
    assert_eq!(user.flags, 3);
    assert_eq!(user.yield_operator, Address::new_from_array([7; 32]));
    assert_eq!(user.origination_fee_bps, 100);
    assert_eq!(user.repay_fee_bps, 200);
    assert_eq!(user.liquidation_fee_bps, 300);
    assert_eq!(serialize(&user).unwrap(), bytes[8..]);

    for operator in [None, Some(user.yield_operator)] {
        let data = SetYieldOperator::data(&SetYieldOperatorInstructionArgs { operator });
        assert_eq!(&data[..8], &[191, 2, 217, 79, 34, 85, 48, 82]);
        assert_eq!(data[8], u8::from(operator.is_some()));
        assert_eq!(data.len(), if operator.is_some() { 41 } else { 9 });
    }
}
