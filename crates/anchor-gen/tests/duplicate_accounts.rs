use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn flattened_account_collision_names_the_instruction() {
    let root = std::env::temp_dir().join(format!(
        "anchor-gen-duplicate-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&root).unwrap();
    let idl = root.join("idl.json");
    fs::write(
        &idl,
        r#"{"name":"fixture","address":"11111111111111111111111111111111","instructions":[{"name":"placeOffer","discriminator":[1],"accounts":[{"name":"group","accounts":[{"name":"a","writable":false,"signer":false}]},{"name":"group_a","writable":false,"signer":false}],"args":[]}]}"#,
    )
    .unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_anchor-gen"))
        .arg(&idl)
        .arg("-o")
        .arg(root.join("client"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("placeOffer"), "{stderr}");
    assert!(stderr.contains("group_a"), "{stderr}");
    fs::remove_dir_all(root).unwrap();
}
