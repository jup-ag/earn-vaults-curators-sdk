use {
    earn_vaults_interface::{
        VaultInstruction,
        arguments::{UpdateShareMetaArgs, UpdateVaultArgs},
        encoding::{PodOption, PodString},
        state::{Account, Delegate, Escrow, Vault},
        zeropod::U64,
    },
    solana_address::Address,
    wincode::Deserialize,
};

#[test]
fn unaligned_accounts_preserve_pinned_field_offsets() {
    assert_eq!(Vault::LEN, 2318);
    assert_eq!(Delegate::LEN, 106);
    assert_eq!(Escrow::LEN, 150);
    assert_eq!(core::mem::align_of::<Vault>(), 1);
    #[repr(align(16))]
    struct Buffer([u8; 2319]);
    let mut bytes = Buffer([0; 2319]);
    bytes.0[1] = Vault::DISCRIMINATOR;
    let vault = Vault::read_mut(&mut bytes.0[1..]).unwrap();
    vault.aum = U64::new(123);
    vault.assets[7].deployed = U64::new(u64::MAX);
    for (i, sink) in vault.sinks.iter_mut().enumerate() {
        sink.balance = U64::new(456 + i as u64);
    }
    vault.nest_receipt_mint = Address::new_from_array([7; 32]);
    vault.queued_redeem_fee_bps = 200u16.into();
    assert_eq!(&bytes.0[2 + 136..2 + 144], &123u64.to_le_bytes());
    let offset = 2 + 256 + 7 * 179 + 74;
    assert_eq!(&bytes.0[offset..offset + 8], &u64::MAX.to_le_bytes());
    for i in 0..4 {
        let offset = 2 + 1912 + i * 57 + 40;
        assert_eq!(
            &bytes.0[offset..offset + 8],
            &(456 + i as u64).to_le_bytes()
        );
    }
    assert_eq!(&bytes.0[2 + 2155..2 + 2187], &[7; 32]);
    assert_eq!(&bytes.0[2 + 2187..2 + 2189], &200u16.to_le_bytes());
    assert_eq!(&bytes.0[2 + 2189..], &[0; 128]);
    let vault = Vault::read(&bytes.0[1..]).unwrap();
    assert_eq!(vault.aum.get(), 123);
    assert_eq!(wincode::serialize(vault).unwrap(), bytes.0[2..]);
    assert!(Vault::read(&bytes.0[1..2318]).is_err());
    assert!(Vault::read(&bytes.0[1..2317]).is_err());
    assert!(Vault::read(&bytes.0[1..2189]).is_err());
    // The old account size must fail closed until the separate migration lands.
    assert!(Vault::read(&bytes.0[1..2185]).is_err());
    assert!(Vault::read_mut(&mut bytes.0[1..2185]).is_err());
    assert!(Vault::read(&bytes.0).is_err());
    bytes.0[1] = 0;
    assert!(Vault::read(&bytes.0[1..]).is_err());
}

#[test]
fn fixed_options_and_strings_preserve_main_bytes() {
    assert_eq!(
        wincode::serialize(&PodOption::<U64>::none()).unwrap(),
        [0; 9]
    );
    let some = wincode::serialize(&PodOption::some(U64::new(42))).unwrap();
    assert_eq!(some, [1, 42, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(
        wincode::deserialize::<PodOption<U64>>(&some)
            .unwrap()
            .get()
            .unwrap()
            .get(),
        42
    );
    assert_eq!(
        wincode::serialize(&PodString::<5>::try_from("Fund").unwrap()).unwrap(),
        b"\x04Fund\0"
    );
    assert!(wincode::deserialize::<PodOption<U64>>(&[2; 9]).is_err());
    assert!(wincode::deserialize::<PodString<5>>(&[6; 6]).is_err());
    assert!(wincode::deserialize::<PodString<5>>(&[1, 0xff, 0, 0, 0, 0]).is_err());
}

#[test]
fn borrowed_instructions_validate_active_fixed_fields() {
    let args = UpdateShareMetaArgs {
        metadata_name: PodOption::some(PodString::try_from("Fund").unwrap()),
        metadata_symbol: PodOption::none(),
        metadata_uri: PodOption::none(),
    };
    let mut bytes = wincode::serialize(&VaultInstruction::UpdateShareMeta { args: &args }).unwrap();
    assert_eq!(bytes.len(), 262);
    assert_eq!(&bytes[..7], b"\x15\x01\x04Fund");
    bytes[10] = 0xff; // Unused padding remains accepted, as in main.
    assert!(VaultInstruction::deserialize(&bytes).is_ok());
    bytes[1] = 2;
    assert!(VaultInstruction::deserialize(&bytes).is_err());
    bytes[1] = 1;
    bytes[10] = 0; // Isolate the length bound from UTF-8 validation.
    bytes[2] = 51;
    assert!(VaultInstruction::deserialize(&bytes).is_err());
    bytes[2] = 4;
    bytes[3] = 0xff;
    assert!(VaultInstruction::deserialize(&bytes).is_err());
    bytes[1] = 0; // None does not validate inactive string storage.
    assert!(VaultInstruction::deserialize(&bytes).is_ok());

    let mut update = [0u8; 1 + size_of::<UpdateVaultArgs>()];
    update[0] = 1;
    let asset = 1 + core::mem::offset_of!(UpdateVaultArgs, asset);
    update[asset] = 1;
    update[asset + 2] = 2; // Invalid nested oracle option tag.
    assert!(VaultInstruction::deserialize(&update).is_err());
    update[asset] = 0;
    assert!(VaultInstruction::deserialize(&update).is_ok());
    for offset in [
        core::mem::offset_of!(UpdateVaultArgs, asset_max_future_skew_seconds),
        core::mem::offset_of!(UpdateVaultArgs, max_aum_staleness_seconds),
        core::mem::offset_of!(UpdateVaultArgs, queued_redeem_fee_bps),
    ] {
        update[1 + offset] = 2;
        assert!(VaultInstruction::deserialize(&update).is_err());
        update[1 + offset] = 0;
    }
}
