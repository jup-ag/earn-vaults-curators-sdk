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

#[cfg(test)]
mod tests {
    use super::*;
    const DUMP: &[u8; 248] = include_bytes!("../tests/fixtures/ocr2_susdai_usdai.bin");

    #[test]
    fn mainnet_round_is_exact_and_ring_bounds_fail_closed() {
        let reading = read(&CHAINLINK_OCR2_STORE_ID, DUMP).unwrap();
        assert_eq!(reading.price, 1_117_211_300_154_803_563);
        assert_eq!(
            (
                reading.decimals,
                reading.timestamp,
                reading.round_id,
                reading.flagged
            ),
            (18, 1_791_372_630, 7395, true)
        );
        assert!(read(&Address::default(), DUMP).is_err());
        for len in 0..DUMP.len() {
            assert!(read(&CHAINLINK_OCR2_STORE_ID, &DUMP[..len]).is_err());
        }
        for (offset, value) in [(0, 0), (8, 1), (9, 2), (138, 39)] {
            let mut bad = *DUMP;
            bad[offset] = value;
            assert!(read(&CHAINLINK_OCR2_STORE_ID, &bad).is_err());
        }
        for (offset, value) in [(143, 0u32), (148, 0), (148, u32::MAX), (152, 1), (208, 0)] {
            let mut bad = *DUMP;
            bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(read(&CHAINLINK_OCR2_STORE_ID, &bad).is_err());
        }
        for answer in [i128::MIN, -1, 0, i64::MAX as i128 + 1, i128::MAX] {
            let mut bad = *DUMP;
            bad[216..232].copy_from_slice(&answer.to_le_bytes());
            assert!(read(&CHAINLINK_OCR2_STORE_ID, &bad).is_err());
        }
        let mut ring = [0u8; 296];
        ring[..248].copy_from_slice(DUMP);
        ring[248..].copy_from_slice(&DUMP[200..]);
        ring[148..152].copy_from_slice(&2u32.to_le_bytes());
        ring[264..280].copy_from_slice(&123i128.to_le_bytes());
        assert_eq!(read(&CHAINLINK_OCR2_STORE_ID, &ring).unwrap().price, 123);
        ring[152..156].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(
            read(&CHAINLINK_OCR2_STORE_ID, &ring).unwrap().price,
            reading.price
        );
        // Deterministic mutation sweep: no malformed length/cursor/answer may panic.
        for seed in 0..2048u32 {
            let mut bytes = *DUMP;
            let offset = (seed as usize * 37) % bytes.len();
            bytes[offset] ^= seed.wrapping_mul(73) as u8;
            let _ = read(&CHAINLINK_OCR2_STORE_ID, &bytes);
        }
    }
}
