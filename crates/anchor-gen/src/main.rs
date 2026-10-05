//! anchor-gen: generate a Solana client crate from an Anchor IDL.
//!
//! Usage: anchor-gen <idl.json> [-o <out-dir>] [--name <crate-name>] [--program-id <base58>]

use {
    askama::Template,
    std::{env, fs, path::PathBuf, process},
};

mod codegen;
mod idl;

use codegen::{CrateMeta, InstructionVm, TypeVm};

#[derive(Template)]
#[template(path = "lib.askama", escape = "none")]
struct LibTemplate {
    crate_name: String,
    program_name: String,
    program_id: String,
}

#[derive(Template)]
#[template(path = "instructions.askama", escape = "none")]
struct InstructionsTemplate {
    program_name: String,
    instructions: Vec<InstructionVm>,
}

#[derive(Template)]
#[template(path = "state.askama", escape = "none")]
struct StateTemplate {
    program_name: String,
    types: Vec<TypeVm>,
}

#[derive(Template)]
#[template(path = "Cargo.toml.askama", escape = "none")]
struct CargoTomlTemplate {
    crate_name: String,
    program_name: String,
}

fn usage() -> ! {
    eprintln!(
        "anchor-gen: generate a wincode-based client crate from an Anchor IDL\n\nusage: \
         anchor-gen <idl.json> [options]\n\noptions:\n\x20 -o, --out <dir>          output \
         directory (default: ./<crate-name>)\n\x20     --name <crate-name>  override the \
         generated crate name\n\x20     --program-id <id>    override the program ID from the \
         IDL\n\x20 -h, --help               show this help"
    );
    process::exit(2)
}

fn fatal(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    process::exit(1)
}

fn write_file(path: &std::path::Path, contents: String) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|e| fatal(format!("cannot create {}: {e}", parent.display())));
    }
    fs::write(path, contents)
        .unwrap_or_else(|e| fatal(format!("cannot write {}: {e}", path.display())));
}

fn render(t: &impl Template, what: &str) -> String {
    t.render()
        .unwrap_or_else(|e| fatal(format!("failed to render {what}: {e}")))
}

fn main() {
    let mut idl_path: Option<String> = None;
    let mut out_dir: Option<String> = None;
    let mut name: Option<String> = None;
    let mut program_id: Option<String> = None;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => usage(),
            "-o" | "--out" => out_dir = Some(args.next().unwrap_or_else(|| usage())),
            "--name" => name = Some(args.next().unwrap_or_else(|| usage())),
            "--program-id" => program_id = Some(args.next().unwrap_or_else(|| usage())),
            _ if !arg.starts_with('-') && idl_path.is_none() => idl_path = Some(arg),
            _ => usage(),
        }
    }

    let idl_path = idl_path.unwrap_or_else(|| usage());
    let raw = fs::read_to_string(&idl_path)
        .unwrap_or_else(|e| fatal(format!("cannot read {idl_path}: {e}")));
    let idl: idl::Idl = serde_json::from_str(&raw)
        .unwrap_or_else(|e| fatal(format!("cannot parse {idl_path} as an Anchor IDL: {e}")));

    let meta: CrateMeta = codegen::crate_meta(&idl, name, program_id);
    let out = out_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&meta.crate_name));
    let src = out.join("src");

    let instructions = codegen::collect_instructions(&idl);
    let types = codegen::collect_types(&idl);

    let manifest = out.join("Cargo.toml");
    if !manifest.exists() {
        write_file(
            &manifest,
            render(
                &CargoTomlTemplate {
                    crate_name: meta.crate_name.clone(),
                    program_name: meta.program_name.clone(),
                },
                "Cargo.toml",
            ),
        );
    }
    write_file(
        &src.join("lib.rs"),
        render(
            &LibTemplate {
                crate_name: meta.crate_name.clone(),
                program_name: meta.program_name.clone(),
                program_id: meta.program_id.clone(),
            },
            "lib.rs",
        ),
    );
    write_file(
        &src.join("instructions.rs"),
        render(
            &InstructionsTemplate {
                program_name: meta.program_name.clone(),
                instructions,
            },
            "instructions.rs",
        ),
    );
    write_file(
        &src.join("state.rs"),
        render(
            &StateTemplate {
                program_name: meta.program_name.clone(),
                types,
            },
            "state.rs",
        ),
    );

    println!(
        "generated client crate `{}` for program `{}` in {}",
        meta.crate_name,
        meta.program_id,
        out.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_wrapper_name_does_not_shadow_state_args() {
        let idl: idl::Idl = serde_json::from_str(
            r#"{"name":"fixture","instructions":[{"name":"create_offer","discriminator":[1],"accounts":[],"args":[{"name":"args","type":{"defined":{"name":"CreateOfferArgs"}}}]}]}"#,
        )
        .unwrap();
        let generated = InstructionsTemplate {
            program_name: "fixture".into(),
            instructions: codegen::collect_instructions(&idl),
        }
        .render()
        .unwrap();
        assert!(generated.contains("pub struct CreateOfferInstructionArgs {"));
        assert!(generated.contains("pub args: crate::state::CreateOfferArgs,"));
    }

    #[test]
    fn repr_c_field_names_do_not_shadow_io_variables() {
        let idl: idl::Idl = serde_json::from_str(
            r#"{"name":"fixture","types":[{"name":"Shadow","repr":{"kind":"c"},"type":{"kind":"enum","variants":[{"name":"Fields","fields":[{"name":"writer","type":"u64"},{"name":"reader","type":"u16"}]}]}}]}"#,
        )
        .unwrap();
        let generated = StateTemplate {
            program_name: "fixture".into(),
            types: codegen::collect_types(&idl),
        }
        .render()
        .unwrap();
        assert!(generated.contains("Self::Fields { writer: v0, reader: v1,"));
        assert!(generated.contains("let v0 = <u64 as SchemaRead<'de, C>>::get(reader.by_ref())?;"));
    }
}
