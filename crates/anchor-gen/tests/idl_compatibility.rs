use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn test_generated_client(name: &str, idl: &str, tests: &str) {
    let root = std::env::temp_dir().join(format!(
        "anchor-gen-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let idl_path = root.join("idl.json");
    fs::write(&idl_path, idl).unwrap();
    let client = root.join("client");
    let generated = Command::new(env!("CARGO_BIN_EXE_anchor-gen"))
        .arg(idl_path)
        .arg("-o")
        .arg(&client)
        .args([
            "--name",
            name,
            "--program-id",
            "11111111111111111111111111111111",
        ])
        .output()
        .unwrap();
    assert!(
        generated.status.success(),
        "{}",
        String::from_utf8_lossy(&generated.stderr)
    );
    fs::create_dir_all(client.join("tests")).unwrap();
    fs::write(client.join("tests/decode.rs"), tests).unwrap();
    // Reuse dependencies across fixtures without contending with the parent Cargo build.
    let target = std::path::Path::new(env!("CARGO_BIN_EXE_anchor-gen"))
        .parent()
        .unwrap()
        .join("idl-compatibility-target");
    let result = Command::new(env!("CARGO"))
        .arg("test")
        .arg("--manifest-path")
        .arg(client.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .output()
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn shank_accounts_decode_without_a_discriminator() {
    test_generated_client(
        "shank-fixture",
        r#"{
        "metadata": {"origin": "shank"},
        "accounts": [
            {"name": "Vault", "type": {"kind": "struct", "fields": [
                {"name": "amount", "type": "u64"}
            ]}},
            {"name": "Counter"}
        ],
        "types": [{"name": "Counter", "type": {"kind": "struct", "fields": [
            {"name": "value", "type": "u16"}
        ]}}]
    }"#,
        r#"
        use shank_fixture::{state::{Vault, Counter}, serialize};
        #[test]
        fn raw_account_data() {
            let bytes = 42u64.to_le_bytes();
            let vault = Vault::from_account_data(&bytes).expect("raw Shank account");
            assert_eq!(vault.amount, 42);
            assert_eq!(serialize(&vault).unwrap(), bytes);
            assert!(Vault::from_account_data(&bytes[..7]).is_none());
            let counter = Counter::from_account_data(&513u16.to_le_bytes()).unwrap();
            assert_eq!(counter.value, 513);
            assert!(Counter::from_account_data(&[1]).is_none());
        }
    "#,
    );
}

#[test]
fn core_asset_key_is_not_stripped() {
    test_generated_client(
        "core-fixture",
        include_str!("../../offerbook-client/idl.json"),
        &include_str!("../../offerbook-client/tests/core_asset.rs")
            .replace("offerbook_client", "core_fixture"),
    );
}

#[test]
fn legacy_events_keep_payload_and_anchor_discriminators() {
    test_generated_client(
        "anchor-fixture",
        r#"{
        "accounts": [
            {"name": "Vault", "type": {"kind": "struct", "fields": [
                {"name": "amount", "type": "u64"}
            ]}},
            {"name": "Counter", "discriminator": [9, 8]},
            {"name": "Tagged", "discriminator": [1]}
        ],
        "events": [
            {"name": "Deposited", "fields": [
                {"name": "amount", "type": "u64", "index": false},
                {"name": "memo", "type": "string", "index": false}
            ]},
            {"name": "ModernEvent", "discriminator": [1, 2, 3]},
            {"name": "EmptyEvent", "fields": []}
        ],
        "types": [
            {"name": "Key", "type": {"kind": "enum", "variants": [
                {"name": "Uninitialized"}, {"name": "AssetV1"}
            ]}},
            {"name": "Tagged", "type": {"kind": "struct", "fields": [
                {"name": "key", "type": {"defined": {"name": "Key"}}},
                {"name": "value", "type": "u16"}
            ]}},
            {"name": "Counter", "type": {"kind": "struct", "fields": [
                {"name": "value", "type": "u16"}
            ]}},
            {"name": "ModernEvent", "type": {"kind": "struct", "fields": [
                {"name": "value", "type": "u16"}
            ]}}
        ]
    }"#,
        r#"
        use anchor_fixture::{state::*, serialize, deserialize_exact};
        #[test]
        fn legacy_event_payload() {
            let mut payload = 42u64.to_le_bytes().to_vec();
            payload.extend_from_slice(&3u32.to_le_bytes());
            payload.extend_from_slice(b"abc");
            let event: Deposited = deserialize_exact(&payload).unwrap();
            assert_eq!(event.amount, 42);
            assert_eq!(event.memo, "abc");
            assert_eq!(serialize(&event).unwrap(), payload);
            assert!(deserialize_exact::<Deposited>(&payload[..payload.len() - 1]).is_err());
            assert_eq!(Deposited::DISCRIMINATOR.len(), 8);
            let _: EmptyEvent = deserialize_exact(&[]).unwrap();
            assert_eq!(ModernEvent::DISCRIMINATOR, [1, 2, 3]);
            let modern: ModernEvent = deserialize_exact(&513u16.to_le_bytes()).unwrap();
            assert_eq!(modern.value, 513);
        }
        #[test]
        fn anchor_account_prefixes() {
            assert_eq!(Vault::DISCRIMINATOR, [211, 8, 232, 43, 2, 152, 117, 119]);
            let mut data = Vault::DISCRIMINATOR.to_vec();
            data.extend_from_slice(&42u64.to_le_bytes());
            assert_eq!(Vault::from_account_data(&data).unwrap().amount, 42);
            data[0] ^= 1;
            assert!(Vault::from_account_data(&data).is_none());
            assert_eq!(Counter::DISCRIMINATOR, [9, 8]);
            assert_eq!(Counter::from_account_data(&[9, 8, 1, 2]).unwrap().value, 513);
            assert!(Counter::from_account_data(&[9, 7, 1, 2]).is_none());
            // A one-byte Anchor prefix is still separate, even with a key field.
            let tagged = Tagged::from_account_data(&[1, 1, 1, 2]).unwrap();
            assert_eq!(tagged.key, Key::AssetV1);
            assert_eq!(tagged.value, 513);
            assert!(Tagged::from_account_data(&[1, 1, 2]).is_none());
            assert!(Tagged::from_account_data(&[0, 1, 1, 2]).is_none());
        }
    "#,
    );
}

#[test]
fn special_keyword_fields_and_accounts_compile() {
    test_generated_client(
        "keywords-fixture",
        r#"{
        "instructions": [
            {"name": "useKeywords", "discriminator": [1], "accounts": [
                {"name": "self"}, {"name": "super"},
                {"name": "crate"}, {"name": "type"}
            ], "args": []},
            {"name": "useSelf", "discriminator": [2], "accounts": [
                {"name": "Self"}
            ], "args": []}
        ],
        "types": [
            {"name": "KeywordFields", "type": {"kind": "struct", "fields": [
                {"name": "self", "type": "u8"},
                {"name": "super", "type": "u8"},
                {"name": "crate", "type": "u8"},
                {"name": "type", "type": "u8"}
            ]}},
            {"name": "UpperField", "type": {"kind": "struct", "fields": [
                {"name": "Self", "type": "u8"}
            ]}}
        ]
    }"#,
        r#"
        use keywords_fixture::state::{KeywordFields, UpperField};
        #[test]
        fn fields_are_accessible() {
            let fields = KeywordFields { self_: 1, super_: 2, crate_: 3, r#type: 4 };
            assert_eq!(fields.self_ + fields.super_ + fields.crate_ + fields.r#type, 10);
            assert_eq!(UpperField { self_: 5 }.self_, 5);
        }
    "#,
    );
}

#[test]
fn unsupported_serialization_fails_before_writing_a_client() {
    let root = std::env::temp_dir().join(format!(
        "anchor-gen-serialization-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let idl = root.join("idl.json");

    for (serialization, label, supported) in [
        (serde_json::json!("bytemuck"), "bytemuck", true),
        (serde_json::json!("bytemuckunsafe"), "bytemuckunsafe", false),
        (serde_json::json!({ "custom": "manual" }), "custom", false),
    ] {
        let b_type = if supported { "u8" } else { "u64" };
        fs::write(
            &idl,
            serde_json::json!({
                "name": "fixture",
                "address": "11111111111111111111111111111111",
                "types": [{
                    "name": "Pool",
                    "serialization": serialization,
                    "type": { "kind": "struct", "fields": [
                        { "name": "a", "type": "u8" },
                        { "name": "b", "type": b_type }
                    ] }
                }]
            })
            .to_string(),
        )
        .unwrap();
        let out = root.join(label);
        let result = Command::new(env!("CARGO_BIN_EXE_anchor-gen"))
            .arg(&idl)
            .arg("-o")
            .arg(&out)
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.success(), supported, "{label}: {stderr}");
        if supported {
            assert!(out.join("src/state.rs").exists());
        } else {
            assert!(stderr.contains("Pool"), "{stderr}");
            assert!(stderr.contains(label), "{stderr}");
            assert!(!out.exists());
        }
    }
    fs::remove_dir_all(root).unwrap();
}
