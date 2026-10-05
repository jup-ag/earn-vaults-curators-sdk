use offerbook_client::{
    BorshConfig, deserialize_exact, serialize,
    state::{Asset, Offer},
};
use wincode::{SchemaRead, SchemaWrite, TypeMeta};

#[test]
fn unit_asset_keeps_the_fixed_payload_and_offer_fields_aligned() {
    let fixed = TypeMeta::Static {
        size: 265,
        zero_copy: false,
    };
    assert_eq!(<Asset as SchemaWrite<BorshConfig>>::TYPE_META, fixed);
    assert_eq!(
        <Asset as SchemaRead<'static, BorshConfig>>::TYPE_META,
        fixed
    );

    let mut none = vec![0xa5u8; 265];
    none[0] = 0;
    assert_eq!(serialize(&Asset::None).unwrap(), vec![0; 265]);
    assert_eq!(deserialize_exact::<Asset>(&none).unwrap(), Asset::None);

    let mut offer = Offer::DISCRIMINATOR.to_vec();
    offer.extend_from_slice(&[0; 32 + 1 + 1 + 6]);
    let mut token = vec![0u8; 265];
    token[0] = 1; // principal: Token
    offer.extend_from_slice(&token);
    offer.extend_from_slice(&[0; 7]);
    offer.extend_from_slice(&none); // collateral: None
    offer.extend_from_slice(&[0; 7]);
    offer.extend_from_slice(&[0; 265]); // filter: NoFilter
    offer.extend_from_slice(&[0; 7]);
    offer.extend_from_slice(&42u64.to_le_bytes()); // principal_amount
    offer.extend_from_slice(&[0; 352]);

    assert_eq!(offer.len(), 1224);
    let decoded = Offer::from_account_data(&offer).unwrap();
    assert_eq!(decoded.collateral, Asset::None);
    assert_eq!(decoded.principal_amount, 42);
    assert_eq!(serialize(&decoded).unwrap().len(), offer.len() - 8);

    offer.extend_from_slice(&[0xa5; 16]);
    assert_eq!(Offer::from_account_data(&offer), Some(decoded));
}
