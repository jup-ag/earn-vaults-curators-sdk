//! Fluid (Jupiter Lend) instruction encoder → `protocol-cpi` route file.
//!
//! Discriminators, arg layouts, and per-account signer/writable flags are
//! transcribed from the mainnet Anchor IDLs (`Instadapp/fluid-contracts-solana@main`:
//! programs `vaults` `jupr81…` and `flashloan` `jupgf…`). The same values the
//! vault's own `jupiter_lend.rs` validator checks.
//!
//! What this encodes: the instruction **data** (borsh) and the signer/writable
//! **flags** on the account list. What it does NOT do: derive the accounts.
//! `operate`'s position tick/branch accounts (`current/final_position_tick[_id]`,
//! `new_branch`) are Fluid-internal and depend on the position's resulting tick —
//! not derivable without reimplementing Fluid's math. The caller supplies the
//! full ordered pubkey list (from Fluid's SDK or a simulated tx); this module
//! stamps the correct flags and encodes the args, then emits the JSON that
//! `protocol-cpi --route-file` forwards. Optional slots that are unused should be
//! passed as the target program id (Anchor's `None` placeholder).

use {
    anyhow::{Result, anyhow, bail},
    base64::{Engine, engine::general_purpose::STANDARD as B64},
    serde_json::{Value, json},
};

pub const VAULTS_PROGRAM: &str = "jupr81YtYssSyPt8jbnGuiWon5f6x9TcDEFxYe3Bdzi";
pub const FLASHLOAN_PROGRAM: &str = "jupgfSgfuAXv4B6R2Uxu85Z1qdzgju79s6MfZekN6XS";

// Per-account flag bits (from the IDL): writable | signer | optional.
const W: u8 = 1;
const S: u8 = 2;

/// One Fluid instruction's static shape.
pub struct Ix {
    pub program: &'static str,
    pub disc: [u8; 8],
    /// Per-account flag bytes, IDL order. `& W` = writable, `& S` = signer.
    pub flags: &'static [u8],
}

pub const OPERATE: Ix = Ix {
    program: VAULTS_PROGRAM,
    disc: [217, 106, 208, 99, 116, 151, 42, 135],
    flags: &[
        3, 1, 1, 4, 5, 5, 0, 1, 0, 0, 0, 1, 0, 1, 1, 0, 1, 1, 1, 1, 1, 1, 0, 0, 1, 1, 5, 5, 0, 0,
        0, 0, 0, 0, 0,
    ],
};

pub const INIT_POSITION: Ix = Ix {
    program: VAULTS_PROGRAM,
    disc: [197, 20, 10, 1, 97, 160, 177, 91],
    flags: &[3, 0, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0],
};

pub const FLASHLOAN_BORROW: Ix = Ix {
    program: FLASHLOAN_PROGRAM,
    disc: [103, 19, 78, 24, 240, 9, 135, 63],
    flags: &[3, 1, 1, 0, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0],
};

pub const FLASHLOAN_PAYBACK: Ix = Ix {
    program: FLASHLOAN_PROGRAM,
    disc: [213, 47, 153, 137, 84, 243, 94, 232],
    flags: &[3, 1, 1, 0, 1, 1, 0, 1, 0, 0, 0, 0, 0, 0],
};

/// `operate`'s `transfer_type: Option<TransferType>` — borsh variant index.
#[derive(Clone, Copy)]
pub enum TransferType {
    Skip = 0,
    Direct = 1,
    Claim = 2,
}

impl TransferType {
    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "skip" => Self::Skip,
            "direct" => Self::Direct,
            "claim" => Self::Claim,
            _ => bail!("transfer-type must be skip|direct|claim, got {s:?}"),
        })
    }
}

/// `operate` data: disc + `new_col: i128` + `new_debt: i128`
/// + `transfer_type: Option<TransferType>` + `remaining_accounts_indices: Vec<u8>`.
pub fn operate_data(
    new_col: i128,
    new_debt: i128,
    transfer: Option<TransferType>,
    remaining: &[u8],
) -> Vec<u8> {
    let mut d = Vec::with_capacity(8 + 16 + 16 + 2 + 4 + remaining.len());
    d.extend_from_slice(&OPERATE.disc);
    d.extend_from_slice(&new_col.to_le_bytes());
    d.extend_from_slice(&new_debt.to_le_bytes());
    match transfer {
        None => d.push(0), // Option::None
        Some(t) => {
            d.push(1); // Option::Some
            d.push(t as u8); // enum variant index
        }
    }
    d.extend_from_slice(&(remaining.len() as u32).to_le_bytes()); // Vec<u8> len
    d.extend_from_slice(remaining);
    d
}

/// `init_position` data: disc + `vault_id: u16` + `next_position_id: u32`.
pub fn init_position_data(vault_id: u16, next_position_id: u32) -> Vec<u8> {
    let mut d = Vec::with_capacity(8 + 2 + 4);
    d.extend_from_slice(&INIT_POSITION.disc);
    d.extend_from_slice(&vault_id.to_le_bytes());
    d.extend_from_slice(&next_position_id.to_le_bytes());
    d
}

/// `flashloan_borrow` / `flashloan_payback` data: disc + `amount: u64`.
pub fn flashloan_data(ix: &Ix, amount: u64) -> Vec<u8> {
    let mut d = Vec::with_capacity(8 + 8);
    d.extend_from_slice(&ix.disc);
    d.extend_from_slice(&amount.to_le_bytes());
    d
}

/// Assemble the `protocol-cpi` route JSON: pin the target program, base64 the
/// data, and stamp each supplied pubkey with its IDL signer/writable flags.
/// `pubkeys` must be in IDL order and match the account count exactly.
pub fn build_route(ix: &Ix, data: Vec<u8>, pubkeys: &[String]) -> Result<Value> {
    if pubkeys.len() != ix.flags.len() {
        return Err(anyhow!(
            "expected {} accounts (IDL order), got {}",
            ix.flags.len(),
            pubkeys.len()
        ));
    }
    let accounts: Vec<Value> = pubkeys
        .iter()
        .zip(ix.flags)
        .map(|(pk, &f)| {
            json!({
                "pubkey": pk,
                "is_signer": f & S != 0,
                "is_writable": f & W != 0,
            })
        })
        .collect();
    Ok(json!({
        "target_program": ix.program,
        "data_base64": B64.encode(&data),
        "accounts": accounts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operate_data_borsh_layout() {
        // new_col = 1, new_debt = -1, Some(Direct), no remaining indices.
        let d = operate_data(1, -1, Some(TransferType::Direct), &[]);
        assert_eq!(&d[..8], &OPERATE.disc);
        assert_eq!(&d[8..24], &1i128.to_le_bytes()); // new_col
        assert_eq!(&d[24..40], &(-1i128).to_le_bytes()); // new_debt
        assert_eq!(&d[40..42], &[1, 1]); // Some + Direct(1)
        assert_eq!(&d[42..46], &[0, 0, 0, 0]); // empty Vec<u8> len
        assert_eq!(d.len(), 46);

        // None transfer + 2 remaining bytes.
        let d = operate_data(0, 0, None, &[7, 9]);
        assert_eq!(d[40], 0); // Option::None
        assert_eq!(&d[41..45], &2u32.to_le_bytes());
        assert_eq!(&d[45..47], &[7, 9]);
    }

    #[test]
    fn init_position_and_flashloan_data() {
        let d = init_position_data(3, 42);
        assert_eq!(&d[..8], &INIT_POSITION.disc);
        assert_eq!(&d[8..10], &3u16.to_le_bytes());
        assert_eq!(&d[10..14], &42u32.to_le_bytes());

        let d = flashloan_data(&FLASHLOAN_BORROW, 1_000);
        assert_eq!(&d[..8], &FLASHLOAN_BORROW.disc);
        assert_eq!(&d[8..16], &1_000u64.to_le_bytes());
    }

    #[test]
    fn route_stamps_flags_and_checks_count() {
        // signer[0] = signer+writable, [7] vault_state = writable, [8] ro.
        let pks: Vec<String> = (0..OPERATE.flags.len())
            .map(|i| format!("key{i}"))
            .collect();
        let r = build_route(&OPERATE, operate_data(0, 1, None, &[]), &pks).unwrap();
        let a = r["accounts"].as_array().unwrap();
        assert_eq!(a[0]["is_signer"], true);
        assert_eq!(a[0]["is_writable"], true);
        assert_eq!(a[7]["is_signer"], false);
        assert_eq!(a[7]["is_writable"], true);
        assert_eq!(a[8]["is_writable"], false);

        // wrong count fails fast.
        assert!(build_route(&OPERATE, vec![], &pks[..3]).is_err());
    }
}
