use {
    earn_vaults_interface::{
        VaultInstruction,
        arguments::{InitializeVaultArgs, UpdateShareMetaArgs, UpdateVaultArgs},
        encoding::{PodOption, PodString},
        state::{Account, Delegate, Escrow, Vault},
        zeropod::U64,
    },
    solana_address::Address,
    wincode::Deserialize,
};

#[test]
fn unaligned_accounts_preserve_pinned_field_offsets() {
    assert_eq!(Vault::LEN, 2334);
    assert_eq!(Delegate::LEN, 106);
    assert_eq!(Escrow::LEN, 150);
    assert_eq!(core::mem::align_of::<Vault>(), 1);
    #[repr(align(16))]
    struct Buffer([u8; 2335]);
    let mut bytes = Buffer([0; 2335]);
    bytes.0[1] = Vault::DISCRIMINATOR;
    let vault = Vault::read_mut(&mut bytes.0[1..]).unwrap();
    vault.aum = U64::new(123);
    vault.assets[7].max_drift_bps = 25u16.into();
    vault.assets[7].deployed = U64::new(u64::MAX);
    for (i, sink) in vault.sinks.iter_mut().enumerate() {
        sink.balance = U64::new(456 + i as u64);
    }
    vault.nest_receipt_mint = Address::new_from_array([7; 32]);
    vault.queued_redeem_fee_bps = 200u16.into();
    vault.prices_changed_at = 789i64.into();
    assert_eq!(&bytes.0[2 + 136..2 + 144], &123u64.to_le_bytes());
    let drift_offset = 2 + 256 + 7 * 181 + 74;
    assert_eq!(
        &bytes.0[drift_offset..drift_offset + 2],
        &25u16.to_le_bytes()
    );
    let offset = 2 + 256 + 7 * 181 + 76;
    assert_eq!(&bytes.0[offset..offset + 8], &u64::MAX.to_le_bytes());
    for i in 0..4 {
        let offset = 2 + 1928 + i * 57 + 40;
        assert_eq!(
            &bytes.0[offset..offset + 8],
            &(456 + i as u64).to_le_bytes()
        );
    }
    assert_eq!(&bytes.0[2 + 2171..2 + 2203], &[7; 32]);
    assert_eq!(&bytes.0[2 + 2203..2 + 2205], &200u16.to_le_bytes());
    assert_eq!(&bytes.0[2 + 2205..2 + 2213], &789i64.to_le_bytes());
    assert_eq!(&bytes.0[2 + 2213..], &[0; 120]);
    let vault = Vault::read(&bytes.0[1..]).unwrap();
    assert_eq!(vault.aum.get(), 123);
    assert_eq!(wincode::serialize(vault).unwrap(), bytes.0[2..]);
    assert!(Vault::read(&bytes.0[1..2334]).is_err());
    assert!(Vault::read(&bytes.0[1..2319]).is_err()); // Previous 2,318-byte layout.
    assert!(Vault::read_mut(&mut bytes.0[1..2319]).is_err());
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

/// `max_aum` is carved from the front of the old 56-byte reserved tail, so an
/// account written before the field existed reads it as zero (no cap).
#[test]
fn pre_max_aum_accounts_decode_as_uncapped() {
    const OLD_RESERVED: usize = 1 + 2_277;
    // Every byte the old layout owned is set; its reserved tail was zeroed.
    let mut bytes = [0xaau8; Vault::LEN];
    bytes[0] = Vault::DISCRIMINATOR;
    bytes[OLD_RESERVED..].fill(0);
    let old = bytes;

    let vault = Vault::read_mut(&mut bytes).unwrap();
    assert_eq!(vault.max_aum.get(), 0);
    assert_eq!(vault.require_aum_headroom(u64::MAX), Ok(()));
    assert_eq!(vault.queued_redeem_fee_bps.get(), 0xaaaa);

    // Writing it touches only the first eight formerly reserved bytes.
    vault.max_aum = U64::new(u64::MAX);
    assert_eq!(bytes[..OLD_RESERVED], old[..OLD_RESERVED]);
    assert_eq!(bytes[OLD_RESERVED..OLD_RESERVED + 8], [0xff; 8]);
    assert_eq!(bytes[OLD_RESERVED + 8..], [0; 48]);
}

/// Instruction args are fixed-size borrows: an encoding without the appended
/// `max_aum` is rejected, so InitializeVault / UpdateVault clients must be
/// rebuilt with the program. Trailing bytes are tolerated, so those clients
/// can switch first.
#[test]
fn max_aum_args_are_appended_and_required() {
    let init = [0u8; 1 + size_of::<InitializeVaultArgs>() + 8];
    let len = init.len() - 8;
    assert_eq!(
        core::mem::offset_of!(InitializeVaultArgs, max_aum),
        size_of::<InitializeVaultArgs>() - 8
    );
    assert!(VaultInstruction::deserialize(&init[..len]).is_ok());
    assert!(VaultInstruction::deserialize(&init[..len - 8]).is_err());
    assert!(VaultInstruction::deserialize(&init).is_ok());

    let mut update = [0u8; 1 + size_of::<UpdateVaultArgs>() + 9];
    update[0] = 1;
    let len = update.len() - 9;
    assert_eq!(
        core::mem::offset_of!(UpdateVaultArgs, max_aum),
        size_of::<UpdateVaultArgs>() - 9
    );
    assert!(VaultInstruction::deserialize(&update[..len]).is_ok());
    assert!(VaultInstruction::deserialize(&update[..len - 9]).is_err());
    assert!(VaultInstruction::deserialize(&update).is_ok());
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
    // The pre-drift AssetUpdate encoding is three bytes shorter.
    assert!(VaultInstruction::deserialize(&update[..update.len() - 3]).is_err());
    let asset = 1 + core::mem::offset_of!(UpdateVaultArgs, asset);
    update[asset] = 1;
    update[asset + 2] = 2; // Invalid nested oracle option tag.
    assert!(VaultInstruction::deserialize(&update).is_err());
    update[asset + 2] = 0;
    let drift = asset
        + 1
        + core::mem::offset_of!(earn_vaults_interface::arguments::AssetUpdate, max_drift_bps);
    update[drift] = 2;
    assert!(VaultInstruction::deserialize(&update).is_err());
    update[drift] = 0;
    assert!(VaultInstruction::deserialize(&update).is_ok());
    update[asset] = 0;
    assert!(VaultInstruction::deserialize(&update).is_ok());
    for offset in [
        core::mem::offset_of!(UpdateVaultArgs, asset_max_future_skew_seconds),
        core::mem::offset_of!(UpdateVaultArgs, max_aum_staleness_seconds),
        core::mem::offset_of!(UpdateVaultArgs, queued_redeem_fee_bps),
        core::mem::offset_of!(UpdateVaultArgs, max_aum),
    ] {
        update[1 + offset] = 2;
        assert!(VaultInstruction::deserialize(&update).is_err());
        update[1 + offset] = 0;
    }
}
