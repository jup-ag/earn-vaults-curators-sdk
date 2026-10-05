use std::{
    fs,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[test]
fn regeneration_preserves_manifest_and_requires_program_id() {
    let root = std::env::temp_dir().join(format!(
        "anchor-gen-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let out = root.join("client");
    fs::create_dir_all(&out).unwrap();
    let idl = root.join("idl.json");
    fs::write(
        &idl,
        r#"{"name":"fixture","address":"11111111111111111111111111111111"}"#,
    )
    .unwrap();
    let manifest = out.join("Cargo.toml");
    let original = "[package]\nname = \"keep-me\"\n";
    fs::write(&manifest, original).unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_anchor-gen"))
        .arg(&idl)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(fs::read_to_string(&manifest).unwrap(), original);
    assert!(out.join("src/state.rs").exists());

    fs::write(&idl, r#"{"name":"fixture"}"#).unwrap();
    let missing = Command::new(env!("CARGO_BIN_EXE_anchor-gen"))
        .arg(&idl)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("--program-id"));

    let override_id = "offerbkFMvVfpQhL8ZQ5iromnjct5rz3r52B9ewu3ie";
    let overridden = Command::new(env!("CARGO_BIN_EXE_anchor-gen"))
        .arg(&idl)
        .arg("-o")
        .arg(&out)
        .arg("--program-id")
        .arg(override_id)
        .output()
        .unwrap();
    assert!(overridden.status.success());
    assert!(
        fs::read_to_string(out.join("src/lib.rs"))
            .unwrap()
            .contains(override_id)
    );
    fs::remove_dir_all(root).unwrap();
}
