use core_fixture::{
    deserialize_exact,
    solana_address::Address,
    state::{BaseAssetV1, Key, UpdateAuthority},
};

#[test]
fn core_asset_discriminator_is_its_key_field() {
    // Core's key is part of the Borsh struct, not an extra Anchor prefix.
    let mut data = vec![1];
    data.extend_from_slice(&[42; 32]); // owner
    data.push(2); // UpdateAuthority::Collection
    data.extend_from_slice(&[43; 32]);
    for text in ["Core asset", "https://example.com/asset.json"] {
        data.extend_from_slice(&(text.len() as u32).to_le_bytes());
        data.extend_from_slice(text.as_bytes());
    }
    data.push(1); // Some(seq)
    data.extend_from_slice(&123u64.to_le_bytes());
    let expected = BaseAssetV1 {
        key: Key::AssetV1,
        owner: Address::new_from_array([42; 32]),
        update_authority: UpdateAuthority::Collection(Address::new_from_array([43; 32])),
        name: "Core asset".into(),
        uri: "https://example.com/asset.json".into(),
        seq: Some(123),
    };
    assert_eq!(deserialize_exact::<BaseAssetV1>(&data).unwrap(), expected);
    assert_eq!(
        BaseAssetV1::from_account_data(&data),
        Some(expected.clone())
    );
    for len in 0..data.len() {
        assert_eq!(BaseAssetV1::from_account_data(&data[..len]), None);
    }
    data.extend_from_slice(&[0xa5; 16]); // trailing plugin data
    assert_eq!(BaseAssetV1::from_account_data(&data), Some(expected));
    for key in [0, 2, 3, 4, 5, 255] {
        data[0] = key;
        assert_eq!(BaseAssetV1::from_account_data(&data), None);
    }
}
