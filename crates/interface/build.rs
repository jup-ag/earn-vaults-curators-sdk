//! Codama IDL build script.

use {
    codama::Codama,
    std::{env, fs, path::Path},
};

fn main() {
    println!("cargo:rerun-if-changed=src/");

    if let Err(e) = generate_idl() {
        println!("cargo:warning=Failed to generate IDL: {e}")
    }
}

fn generate_idl() -> Result<(), Box<dyn std::error::Error>> {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let crate_path = Path::new(&manifest_dir);
    let codama = Codama::load(crate_path)?;
    let idl_json = codama.get_json_idl()?;

    let mut parsed: serde_json::Value = serde_json::from_str(&idl_json)?;
    parsed["program"]["name"] = serde_json::Value::String("earnVaults".into());
    let formatted_json = serde_json::to_string_pretty(&parsed)?;

    let idl_path = crate_path
        .ancestors()
        .nth(2)
        .ok_or("no workspace root")?
        .join("idl.json");
    if fs::read_to_string(&idl_path).is_ok_and(|current| current == formatted_json) {
        return Ok(());
    }
    fs::write(&idl_path, formatted_json)?;

    Ok(())
}
