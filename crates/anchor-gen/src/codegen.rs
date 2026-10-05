//! View models and helpers used to render the client crate templates.

use crate::idl::{
    ArrayLen, DefinedFields, Idl, IdlDefined, IdlGenericArg, Instruction, InstructionAccountItem,
    Repr, Serialization, Type, TypeDef, TypeDefGeneric, TypeDefTy,
};

// ---------------------------------------------------------------------------
// Name helpers
// ---------------------------------------------------------------------------

pub fn to_pascal_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut upper = true;
    for ch in s.chars() {
        if ch == '_' || ch == '-' || ch == ' ' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    // Identifiers cannot start with a digit.
    if out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

pub fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let mut prev_lower_or_digit = false;
    let mut prev_upper = false;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '_' {
            out.push('_');
            prev_lower_or_digit = false;
        } else if ch == '-' || ch == ' ' {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            prev_lower_or_digit = false;
        } else if ch.is_uppercase() {
            if prev_lower_or_digit
                || (prev_upper && chars.peek().is_some_and(|next| next.is_lowercase()))
            {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
            prev_lower_or_digit = false;
        } else {
            out.push(ch);
            prev_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
        prev_upper = ch.is_uppercase();
    }
    out
}

/// Escape Rust keywords with a raw identifier prefix.
pub fn escape_keyword(s: &str) -> String {
    const KEYWORDS: &[&str] = &[
        "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref",
        "return", "self", "Self", "static", "struct", "super", "trait", "true", "type", "unsafe",
        "use", "where", "while", "async", "await", "dyn", "abstract", "become", "box", "do",
        "final", "macro", "override", "priv", "typeof", "unsized", "virtual", "yield", "try",
        "gen",
    ];
    if matches!(s, "self" | "Self" | "super" | "crate") {
        format!("{s}_")
    } else if KEYWORDS.contains(&s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}

/// Sanitize a crate name: lowercase, non-alphanumeric → '-'.
pub fn crate_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Type mapping (IDL type -> Rust type)
// ---------------------------------------------------------------------------

/// Convert an IDL type to a Rust type string.
/// `defined_prefix` qualifies user-defined types in instruction arguments.
fn rust_type(ty: &Type, defined_prefix: &str) -> String {
    match ty {
        Type::Bool => "bool".into(),
        Type::U8 => "u8".into(),
        Type::I8 => "i8".into(),
        Type::U16 => "u16".into(),
        Type::I16 => "i16".into(),
        Type::U32 => "u32".into(),
        Type::I32 => "i32".into(),
        Type::F32 => "f32".into(),
        Type::U64 => "u64".into(),
        Type::I64 => "i64".into(),
        Type::F64 => "f64".into(),
        Type::U128 => "u128".into(),
        Type::I128 => "i128".into(),
        // No native 256-bit ints in Rust/wincode; borsh encodes them as
        // a fixed 32-byte little-endian array.
        Type::U256 | Type::I256 => "[u8; 32]".into(),
        Type::Bytes => "Vec<u8>".into(),
        Type::String => "String".into(),
        Type::Pubkey => "Address".into(),
        Type::Option(inner) => format!("Option<{}>", rust_type(inner, defined_prefix)),
        Type::Vec(inner) => format!("Vec<{}>", rust_type(inner, defined_prefix)),
        Type::Array(inner, len) => {
            let len = match len {
                ArrayLen::Value(n) => n.to_string(),
                ArrayLen::Generic(g) => g.clone(),
            };
            format!("[{}; {}]", rust_type(inner, defined_prefix), len)
        }
        Type::Defined(d) => {
            let name = format!("{defined_prefix}{}", to_pascal_case(d.name()));
            match d {
                IdlDefined::Object { generics, .. } if !generics.is_empty() => format!(
                    "{name}<{}>",
                    generics
                        .iter()
                        .map(|arg| match arg {
                            IdlGenericArg::Type { ty } => rust_type(ty, defined_prefix),
                            IdlGenericArg::Const { value } => value.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                _ => name,
            }
        }
        Type::Generic(g) => to_pascal_case(g),
        // Borsh writes hash collections in key order, so use ordered Rust containers.
        Type::HashMap(k, v) | Type::BTreeMap(k, v) => format!(
            "::std::collections::BTreeMap<{}, {}>",
            rust_type(k, defined_prefix),
            rust_type(v, defined_prefix)
        ),
        Type::HashSet(v) | Type::BTreeSet(v) => format!(
            "::std::collections::BTreeSet<{}>",
            rust_type(v, defined_prefix)
        ),
    }
}

// ---------------------------------------------------------------------------
// Field view models
// ---------------------------------------------------------------------------

pub struct FieldVm {
    pub name: String,
    pub ty: String,
    pub docs: Vec<String>,
}

impl FieldVm {
    fn from_field(f: &crate::idl::Field, defined_prefix: &str) -> Self {
        Self {
            name: escape_keyword(&to_snake_case(&f.name)),
            ty: rust_type(&f.ty, defined_prefix),
            docs: f.docs.clone(),
        }
    }
}

pub struct TupleFieldVm {
    pub ty: String,
}

impl TupleFieldVm {
    fn from_type(ty: &Type) -> Self {
        Self {
            ty: rust_type(ty, ""),
        }
    }
}

// ---------------------------------------------------------------------------
// State (accounts + defined types) view models
// ---------------------------------------------------------------------------

pub struct VariantVm {
    pub name: String,
    pub tag: usize,
    /// "unit" | "named" | "tuple"
    pub style: &'static str,
    pub fields: Vec<FieldVm>,
    pub tuple: Vec<TupleFieldVm>,
}

pub struct TypeVm {
    pub name: String,
    pub docs: Vec<String>,
    /// "struct_named" | "struct_tuple" | "struct_unit" | "enum" | "alias"
    pub style: &'static str,
    pub repr_c: bool,
    /// Generic parameters declaration, e.g. `<T, const N: usize>` (or empty).
    pub generics: String,
    pub fields: Vec<FieldVm>,
    pub tuple: Vec<TupleFieldVm>,
    pub variants: Vec<VariantVm>,
    pub alias: String,
    /// Set when this type is one of the program's accounts.
    pub is_account: bool,
    pub has_discriminator: bool,
    pub disc_csv: String,
    pub disc_len: usize,
}

fn generics_decl(g: &[TypeDefGeneric]) -> String {
    if g.is_empty() {
        return String::new();
    }
    let params: Vec<String> = g
        .iter()
        .map(|p| match p {
            TypeDefGeneric::Type { name } => to_pascal_case(name),
            TypeDefGeneric::Const { name, ty } => format!("const {}: {}", name, ty),
        })
        .collect();
    format!("<{}>", params.join(", "))
}

fn type_vm(ty: &TypeDef, disc: Option<&[u8]>) -> TypeVm {
    match &ty.serialization {
        Serialization::Borsh | Serialization::Bytemuck => {}
        Serialization::BytemuckUnsafe => crate::fatal(format!(
            "type `{}` uses unsupported bytemuckunsafe serialization",
            ty.name
        )),
        Serialization::Custom(codec) => crate::fatal(format!(
            "type `{}` uses unsupported custom serialization `{codec}`",
            ty.name
        )),
    }
    let mut vm = TypeVm {
        name: to_pascal_case(&ty.name),
        docs: ty.docs.clone(),
        style: "struct_unit",
        repr_c: matches!(ty.repr.as_ref(), Some(Repr::C(_))),
        generics: generics_decl(&ty.generics),
        fields: Vec::new(),
        tuple: Vec::new(),
        variants: Vec::new(),
        alias: String::new(),
        is_account: disc.is_some(),
        has_discriminator: disc.is_some(),
        disc_csv: disc
            .map(|d| {
                d.iter()
                    .map(|b| b.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        disc_len: disc.map_or(0, |d| d.len()),
    };
    match &ty.ty {
        TypeDefTy::Struct { fields } => match fields {
            None => vm.style = "struct_unit",
            Some(DefinedFields::Named(fields)) => {
                vm.style = "struct_named";
                vm.fields = fields.iter().map(|f| FieldVm::from_field(f, "")).collect();
            }
            Some(DefinedFields::Tuple(types)) => {
                vm.style = "struct_tuple";
                vm.tuple = types.iter().map(TupleFieldVm::from_type).collect();
            }
        },
        TypeDefTy::Enum { variants } => {
            vm.style = "enum";
            if vm.repr_c {
                assert!(
                    ty.generics.is_empty(),
                    "generic repr(C) enums are unsupported"
                );
                assert!(
                    variants.len() <= 256,
                    "repr(C) enum has more than 256 variants"
                );
            }
            vm.variants = variants
                .iter()
                .enumerate()
                .map(|(tag, v)| {
                    let mut vm = VariantVm {
                        name: to_pascal_case(&v.name),
                        tag,
                        style: "unit",
                        fields: Vec::new(),
                        tuple: Vec::new(),
                    };
                    match &v.fields {
                        None => {}
                        Some(DefinedFields::Named(fields)) => {
                            vm.style = "named";
                            vm.fields = fields.iter().map(|f| FieldVm::from_field(f, "")).collect();
                        }
                        Some(DefinedFields::Tuple(types)) => {
                            vm.style = "tuple";
                            vm.tuple = types.iter().map(TupleFieldVm::from_type).collect();
                        }
                    }
                    vm
                })
                .collect();
        }
        TypeDefTy::Type { alias } => {
            vm.style = "alias";
            vm.alias = rust_type(alias, "");
        }
    }
    vm
}

fn named_discriminator(namespace: &[u8], name: &str, explicit: Option<&[u8]>) -> Vec<u8> {
    if let Some(explicit) = explicit {
        return explicit.to_vec();
    }
    let name = to_pascal_case(name);
    solana_sha256_hasher::hashv(&[namespace, name.as_bytes()]).as_ref()[..8].to_vec()
}

pub fn collect_types(idl: &Idl) -> Vec<TypeVm> {
    let account_discriminator = |account: &crate::idl::Account| {
        // Shank account data has no discriminator prefix.
        (idl.metadata.origin.as_deref() != Some("shank")).then(|| {
            named_discriminator(b"account:", &account.name, account.discriminator.as_deref())
        })
    };
    let mut vms: Vec<TypeVm> = idl
        .types
        .iter()
        .map(|ty| {
            if let Some(account) = idl
                .accounts
                .iter()
                .find(|a| to_pascal_case(&a.name) == to_pascal_case(&ty.name))
            {
                let disc = account_discriminator(account);
                let mut vm = type_vm(ty, disc.as_deref());
                vm.is_account = true;
                vm
            } else if let Some(event) = idl
                .events
                .iter()
                .find(|e| to_pascal_case(&e.name) == to_pascal_case(&ty.name))
            {
                let disc =
                    named_discriminator(b"event:", &event.name, event.discriminator.as_deref());
                let mut vm = type_vm(ty, Some(&disc));
                vm.is_account = false;
                vm
            } else {
                type_vm(ty, None)
            }
        })
        .collect();

    // Accounts whose type is not present in the `types` section: emit a
    // placeholder so the discriminator is still available.
    for account in &idl.accounts {
        let pascal = to_pascal_case(&account.name);
        if vms.iter().any(|t| t.name == pascal) {
            continue;
        }
        let disc = account_discriminator(account);
        let mut vm = match &account.ty {
            Some(ty_def) => type_vm(
                &TypeDef {
                    name: account.name.clone(),
                    ty: ty_def.clone(),
                    ..TypeDef::default()
                },
                disc.as_deref(),
            ),
            None => type_vm(&TypeDef::default(), disc.as_deref()),
        };
        vm.name = pascal;
        vm.is_account = true;
        vms.push(vm);
    }

    // Pre-0.30 Anchor IDLs define event fields inline instead of in `types`.
    for event in &idl.events {
        let pascal = to_pascal_case(&event.name);
        if vms.iter().any(|t| t.name == pascal) {
            continue;
        }
        let disc = named_discriminator(b"event:", &event.name, event.discriminator.as_deref());
        let mut vm = type_vm(
            &TypeDef {
                ty: TypeDefTy::Struct {
                    fields: event.fields.clone().map(DefinedFields::Named),
                },
                ..TypeDef::default()
            },
            Some(&disc),
        );
        vm.name = pascal;
        vm.is_account = false;
        vms.push(vm);
    }
    vms
}

// ---------------------------------------------------------------------------
// Instruction view models
// ---------------------------------------------------------------------------

pub struct IxAccountVm {
    pub name: String,
    pub docs: Vec<String>,
    pub optional: bool,
    /// Precomputed `AccountMeta` construction expression.
    pub meta_expr: String,
}

pub struct InstructionVm {
    pub snake: String,
    pub pascal: String,
    pub docs: Vec<String>,
    pub disc_csv: String,
    pub disc_len: usize,
    pub args: Vec<FieldVm>,
    pub has_args: bool,
    /// Accounts exposed in the `<Name>Accounts` struct.
    pub accounts: Vec<IxAccountVm>,
    /// All accounts in IDL order, used by `to_account_metas`.
    pub metas: Vec<IxAccountVm>,
}

fn meta_expr(
    name: &str,
    is_mut: bool,
    is_signer: bool,
    optional: bool,
    fixed: Option<&str>,
) -> String {
    let ctor = if is_mut {
        "AccountMeta::new"
    } else {
        "AccountMeta::new_readonly"
    };
    let addr = match fixed {
        Some(addr) => format!("Address::from_str_const(\"{addr}\")"),
        None if optional && (is_mut || is_signer) => {
            return format!(
                "match self.{name} {{ Some(address) => {ctor}(address, {is_signer}), None => AccountMeta::new_readonly(ID, false) }}"
            );
        }
        None if optional => format!("self.{name}.unwrap_or(ID)"),
        None => format!("self.{name}"),
    };
    format!("{ctor}({addr}, {is_signer})")
}

/// Flatten one account item (single or composite group) into view models.
fn flatten_account(
    item: &InstructionAccountItem,
    prefix: &str,
    instruction_name: &str,
    accounts: &mut Vec<IxAccountVm>,
    metas: &mut Vec<IxAccountVm>,
) {
    match item {
        InstructionAccountItem::Single(acc) => {
            let snake = escape_keyword(&to_snake_case(&format!("{prefix}{}", acc.name)));
            if metas.iter().any(|meta| meta.name == snake) {
                crate::fatal(format!(
                    "instruction `{instruction_name}` has duplicate flattened account `{snake}`"
                ));
            }
            let fixed = acc.address.is_some();
            let vm = IxAccountVm {
                name: snake.clone(),
                docs: acc.docs.clone(),
                optional: acc.optional,
                meta_expr: meta_expr(
                    &snake,
                    acc.is_mut,
                    acc.is_signer,
                    acc.optional,
                    acc.address.as_deref(),
                ),
            };
            if !fixed {
                accounts.push(IxAccountVm {
                    name: vm.name.clone(),
                    docs: vm.docs.clone(),
                    optional: vm.optional,
                    meta_expr: String::new(),
                });
            }
            metas.push(vm);
        }
        InstructionAccountItem::Composite(group) => {
            let prefix = format!("{}{}_", prefix, to_snake_case(&group.name));
            for item in &group.accounts {
                flatten_account(item, &prefix, instruction_name, accounts, metas);
            }
        }
    }
}

fn instruction_vm(ix: &Instruction) -> InstructionVm {
    let mut accounts = Vec::new();
    let mut metas = Vec::new();
    for item in &ix.accounts {
        flatten_account(item, "", &ix.name, &mut accounts, &mut metas);
    }
    let disc = ix.discriminator.as_ref().map_or_else(
        || {
            // Anchor IDLs before 0.30 omit instruction discriminators.
            let name = to_snake_case(&ix.name);
            solana_sha256_hasher::hashv(&[b"global:", name.as_bytes()]).as_ref()[..8].to_vec()
        },
        |disc| disc.value(),
    );
    InstructionVm {
        snake: to_snake_case(&ix.name),
        pascal: to_pascal_case(&ix.name),
        docs: ix.docs.clone(),
        disc_csv: disc
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(", "),
        disc_len: disc.len(),
        args: ix
            .args
            .iter()
            .map(|f| FieldVm::from_field(f, "crate::state::"))
            .collect(),
        has_args: !ix.args.is_empty(),
        accounts,
        metas,
    }
}

pub fn collect_instructions(idl: &Idl) -> Vec<InstructionVm> {
    idl.instructions.iter().map(instruction_vm).collect()
}

// ---------------------------------------------------------------------------
// Crate-level metadata
// ---------------------------------------------------------------------------

pub struct CrateMeta {
    pub crate_name: String,
    pub program_name: String,
    pub program_id: String,
}

pub fn crate_meta(
    idl: &Idl,
    name_override: Option<String>,
    id_override: Option<String>,
) -> CrateMeta {
    let program_name = idl
        .metadata
        .name
        .clone()
        .or_else(|| idl.name.clone())
        .unwrap_or_else(|| "program".to_string());
    let program_id = id_override
        .or_else(|| idl.address.clone())
        .or_else(|| idl.metadata.address.clone())
        .unwrap_or_else(|| crate::fatal("IDL has no program address; pass --program-id"));
    CrateMeta {
        crate_name: crate_name(&name_override.unwrap_or_else(|| format!("{program_name}-client"))),
        program_name,
        program_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defined_type_keeps_type_and_const_arguments() {
        let ty: Type = serde_json::from_str(
            r#"{"defined":{"name":"Foo","generics":[{"kind":"type","type":{"vec":"u8"}},{"kind":"const","value":"4"}]}}"#,
        )
        .unwrap();
        assert_eq!(rust_type(&ty, ""), "Foo<Vec<u8>, 4>");
    }

    #[test]
    fn instruction_args_qualify_defined_state_types() {
        let idl: Idl = serde_json::from_str(
            r#"{"name":"fixture","instructions":[{"name":"create_offer","discriminator":[1],"accounts":[],"args":[{"name":"args","type":{"defined":{"name":"CreateOfferArgs"}}},{"name":"maybe","type":{"option":{"defined":{"name":"CreateOfferArgs"}}}}]}]}"#,
        )
        .unwrap();
        let args = &collect_instructions(&idl)[0].args;
        assert_eq!(args[0].ty, "crate::state::CreateOfferArgs");
        assert_eq!(args[1].ty, "Option<crate::state::CreateOfferArgs>");
    }

    #[test]
    fn hash_collections_use_borsh_key_order() {
        let map: Type = serde_json::from_str(r#"{"hashMap":["u16","u8"]}"#).unwrap();
        let set: Type = serde_json::from_str(r#"{"hashSet":"u16"}"#).unwrap();
        assert_eq!(rust_type(&map, ""), "::std::collections::BTreeMap<u16, u8>");
        assert_eq!(rust_type(&set, ""), "::std::collections::BTreeSet<u16>");
    }

    #[test]
    fn shank_discriminant_is_accepted() {
        let ix: Instruction = serde_json::from_str(
            r#"{"name":"create_offer","discriminant":{"type":"u8","value":7},"accounts":[],"args":[]}"#,
        )
        .unwrap();
        let vm = instruction_vm(&ix);
        assert_eq!(vm.disc_len, 1);
        assert_eq!(vm.disc_csv, "7");
    }

    #[test]
    fn snake_case_preserves_uppercase_word_boundaries() {
        for (name, expected) in [
            ("withdrawXToken", "withdraw_x_token"),
            ("swapAToB", "swap_a_to_b"),
            ("HTTPServer", "http_server"),
            ("getURL", "get_url"),
            ("XToken", "x_token"),
            ("createOffer", "create_offer"),
            ("withdraw_x_token", "withdraw_x_token"),
            ("withdraw-Token", "withdraw_token"),
            ("withdraw Token", "withdraw_token"),
            ("token2Balance", "token2_balance"),
            ("_padding", "_padding"),
            ("_", "_"),
            ("__foo", "__foo"),
            ("foo__bar", "foo__bar"),
            ("", ""),
        ] {
            assert_eq!(to_snake_case(name), expected, "{name}");
        }
    }

    #[test]
    fn leading_underscore_fields_remain_distinct() {
        let idl: Idl = serde_json::from_str(
            r#"{"types":[{"name":"Fields","type":{"kind":"struct","fields":[{"name":"_foo","type":"u8"},{"name":"foo","type":"u8"},{"name":"_","type":"u8"}]}}]}"#,
        )
        .unwrap();
        let types = collect_types(&idl);
        let names: Vec<_> = types[0]
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect();
        assert_eq!(names, ["_foo", "foo", "_"]);
    }

    #[test]
    fn special_keywords_use_suffixes_instead_of_raw_identifiers() {
        for (name, expected) in [
            ("self", "self_"),
            ("Self", "Self_"),
            ("super", "super_"),
            ("crate", "crate_"),
            ("type", "r#type"),
        ] {
            assert_eq!(escape_keyword(name), expected);
        }
    }

    #[test]
    fn legacy_anchor_discriminator_uses_snake_case_name() {
        for (name, expected) in [
            ("createOffer", "237, 233, 192, 168, 248, 7, 249, 241"),
            ("withdrawXToken", "111, 77, 181, 5, 210, 156, 230, 213"),
            ("swapAToB", "57, 5, 83, 157, 173, 236, 126, 96"),
        ] {
            let ix: Instruction = serde_json::from_value(serde_json::json!({
                "name": name, "accounts": [], "args": []
            }))
            .unwrap();
            let vm = instruction_vm(&ix);
            assert_eq!(vm.disc_len, 8);
            assert_eq!(vm.disc_csv, expected, "{name}");
        }
    }

    #[test]
    fn explicit_instruction_discriminator_is_preserved() {
        let ix: Instruction = serde_json::from_str(
            r#"{"name":"withdrawXToken","discriminator":[1,2,3],"accounts":[],"args":[]}"#,
        )
        .unwrap();
        let vm = instruction_vm(&ix);
        assert_eq!(vm.disc_len, 3);
        assert_eq!(vm.disc_csv, "1, 2, 3");
    }

    #[test]
    fn legacy_account_gets_derived_discriminator() {
        let idl: Idl = serde_json::from_str(
            r#"{"accounts":[{"name":"Vault","type":{"kind":"struct","fields":[]}}]}"#,
        )
        .unwrap();
        let vm = &collect_types(&idl)[0];
        assert_eq!(vm.name, "Vault");
        assert!(vm.is_account);
        assert!(vm.has_discriminator);
        assert_eq!(vm.disc_len, 8);
        assert_eq!(vm.disc_csv, "211, 8, 232, 43, 2, 152, 117, 119");
    }

    #[test]
    fn event_keeps_idl_discriminator() {
        let idl: Idl = serde_json::from_str(
            r#"{"events":[{"name":"OfferCreated","discriminator":[1,2,3,4,5,6,7,8]}],"types":[{"name":"OfferCreated","type":{"kind":"struct","fields":[]}}]}"#,
        )
        .unwrap();
        let vm = &collect_types(&idl)[0];
        assert_eq!(vm.name, "OfferCreated");
        assert!(!vm.is_account);
        assert!(vm.has_discriminator);
        assert_eq!(vm.disc_csv, "1, 2, 3, 4, 5, 6, 7, 8");
    }

    #[test]
    fn missing_optional_signer_uses_readonly_program_id() {
        let expr = meta_expr("authority", true, true, true, None);
        assert!(expr.contains("Some(address) => AccountMeta::new(address, true)"));
        assert!(expr.contains("None => AccountMeta::new_readonly(ID, false)"));
    }
}
