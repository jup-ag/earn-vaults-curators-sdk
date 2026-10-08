//! Legacy Chainlink OCR2 Store `Transmissions` v2, shared by program and CLI.
use {
    crate::constants::{CHAINLINK_OCR2_STORE_ID, MAX_ORACLE_REPORT_DECIMALS},
    solana_address::Address,
    solana_program_error::ProgramError,
    wincode::SchemaRead,
};

pub const DISCRIMINATOR: [u8; 8] = [96, 179, 69, 66, 128, 129, 73, 117];
const ENTRIES: usize = 8 + 192;
const ENTRY_SIZE: usize = 48;

/// Packed Borsh fields followed by padding to the store's 192-byte header.
/// Wincode's fixed-width little-endian schema matches these fields without
/// depending on Rust's in-memory alignment or field offsets.
#[derive(SchemaRead)]
struct TransmissionsHeader {
    discriminator: [u8; 8],
    version: u8,
    state: u8,
    _owner: Address,
    _proposed_owner: Address,
    _writer: Address,
    _description: [u8; 32],
    decimals: u8,
    _flagging_threshold: u32,
    latest_round_id: u32,
    _granularity: u8,
    live_length: u32,
    live_cursor: u32,
    _historical_cursor: u32,
    _reserved: [u8; 40],
}

#[derive(SchemaRead)]
struct Transmission {
    _slot: u64,
    timestamp: u32,
    _padding: u32,
    answer: i128,
    _reserved: [u8; 16],
}

#[derive(Debug, PartialEq, Eq)]
pub struct Reading {
    pub price: i64,
    pub decimals: u8,
    pub timestamp: i64,
    pub round_id: u32,
    /// Historical, latched deviation flag, not validity of the latest round.
    /// A zero flagging threshold flags even ordinary accrual on the target feed.
    pub flagged: bool,
}

/// Authenticate the store owner and decode its latest live entry. Callers must
/// also bind the account address to the configured asset (there is no feed id
/// inside this layout), and gate the observation timestamp before using it.
/// Values outside the signed-i64 cache range fail rather than lose precision.
pub fn read(owner: &Address, data: &[u8]) -> Result<Reading, ProgramError> {
    if owner != &CHAINLINK_OCR2_STORE_ID {
        return Err(ProgramError::InvalidAccountOwner);
    }
    let invalid = ProgramError::InvalidAccountData;
    let header: TransmissionsHeader =
        wincode::deserialize_exact(data.get(..ENTRIES).ok_or(invalid.clone())?)
            .map_err(|_| invalid.clone())?;
    let entries = &data[ENTRIES..];
    if header.discriminator != DISCRIMINATOR
        || header.version != 2
        || header.state > 1
        || header.decimals > MAX_ORACLE_REPORT_DECIMALS
        || !entries.len().is_multiple_of(ENTRY_SIZE)
    {
        return Err(invalid);
    }
    let len = header.live_length as usize;
    let cursor = header.live_cursor as usize;
    if header.latest_round_id == 0 || len == 0 || cursor >= len || len > entries.len() / ENTRY_SIZE
    {
        return Err(invalid);
    }
    // Avoid cursor + len overflow, including on the narrow usize targets.
    let index = if cursor == 0 { len - 1 } else { cursor - 1 };
    let entry: Transmission =
        wincode::deserialize_exact(&entries[index * ENTRY_SIZE..(index + 1) * ENTRY_SIZE])
            .map_err(|_| invalid.clone())?;
    let price = i64::try_from(entry.answer).map_err(|_| ProgramError::InvalidAccountData)?;
    if price <= 0 || entry.timestamp == 0 {
        return Err(invalid);
    }
    Ok(Reading {
        price,
        decimals: header.decimals,
        timestamp: i64::from(entry.timestamp),
        round_id: header.latest_round_id,
        flagged: header.state == 1,
    })
}
