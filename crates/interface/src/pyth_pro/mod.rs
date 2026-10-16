//! Pyth Pro wire constants and the pinned Pro → Core identity mapping.
use {crate::errors::VaultError, solana_address::Address, solana_program_error::ProgramError};

pub const PROGRAM_ID: Address =
    Address::from_str_const("pytd2yyk641x7ak7mkaasSJVXh6YYZnC7wTmtgAyxPt");
pub const STORAGE_ID: Address =
    Address::from_str_const("3rdJbqfnagQ4yx9HXJViD4zc4xpiSqmFsKpPuSCQVyQL");
pub const INSTRUCTIONS_SYSVAR_ID: Address =
    Address::from_str_const("Sysvar1nstructions1111111111111111111111111");
pub const ED25519_PROGRAM_ID: Address =
    Address::from_str_const("Ed25519SigVerify111111111111111111111111111");
pub const SYSTEM_PROGRAM_ID: Address = Address::new_from_array([0; 32]);
pub const SOLANA_MAGIC: u32 = 2_182_742_457;
pub const PAYLOAD_MAGIC: u32 = 2_479_346_549;
pub const MESSAGE_HEADER_LEN: usize = 102;
pub const MAX_MESSAGE_LEN: usize = 1024;
/// Zero-length Chainlink frame (invalid in legacy reports), then format version.
pub const REFRESH_PRO_PREFIX: [u8; 3] = [0, 0, 1];
/// Prefix + Ed25519 instruction index (u16) + signed message length (u16).
pub const REFRESH_PRO_HEADER_LEN: usize = 7;
/// Existing instruction tag + reports length (u64) + Pro envelope header.
pub const MESSAGE_OFFSET: u16 = 1 + 8 + REFRESH_PRO_HEADER_LEN as u16;

/// Pro envelope inside the existing `RefreshPrices.reports` byte field.
pub struct ProRefreshPayload<'a> {
    pub ed25519_instruction_index: u16,
    pub message: &'a [u8],
    pub chainlink_reports: &'a [u8],
}

impl<'a> ProRefreshPayload<'a> {
    /// `None` preserves the legacy empty/Core or nonzero-length Chainlink path.
    /// A zero-length first frame reserves the extension namespace; unsupported
    /// versions and malformed envelopes fail instead of falling back to legacy.
    pub fn decode(reports: &'a [u8]) -> Result<Option<Self>, ProgramError> {
        if !reports.starts_with(&[0, 0]) {
            return Ok(None);
        }
        let (header, rest) = reports
            .split_at_checked(REFRESH_PRO_HEADER_LEN)
            .ok_or(VaultError::InvalidOracleReport)?;
        if header[..3] != REFRESH_PRO_PREFIX {
            return Err(VaultError::InvalidOracleReport.into());
        }
        let len = u16::from_le_bytes(header[5..7].try_into().unwrap()) as usize;
        let (message, chainlink_reports) = rest
            .split_at_checked(len)
            .ok_or(VaultError::InvalidOracleReport)?;
        message_payload(message).ok_or(VaultError::InvalidOracleReport)?;
        Ok(Some(Self {
            ed25519_instruction_index: u16::from_le_bytes(header[3..5].try_into().unwrap()),
            message,
            chainlink_reports,
        }))
    }
}

// Sorted records: Pro id (u32 LE), Core id (32 bytes). The reviewable source is
// feeds.tsv; the test below prevents the binary and source from diverging.
const FEEDS: &[u8] = include_bytes!("feeds.bin");

/// Unknown IDs (including feeds with no Core counterpart) fail closed.
pub fn core_feed_id(pro_id: u32) -> Option<[u8; 32]> {
    let (mut lo, mut hi) = (0, FEEDS.len() / 36);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let row = &FEEDS[mid * 36..(mid + 1) * 36];
        let id = u32::from_le_bytes(row[..4].try_into().unwrap());
        match pro_id.cmp(&id) {
            core::cmp::Ordering::Less => hi = mid,
            core::cmp::Ordering::Greater => lo = mid + 1,
            core::cmp::Ordering::Equal => return Some(row[4..].try_into().unwrap()),
        }
    }
    None
}

/// Checks framing only. This does **not** authenticate a message.
pub fn message_payload(message: &[u8]) -> Option<&[u8]> {
    if !(MESSAGE_HEADER_LEN..=MAX_MESSAGE_LEN).contains(&message.len())
        || message[..4] != SOLANA_MAGIC.to_le_bytes()
    {
        return None;
    }
    let len = u16::from_le_bytes(message[100..102].try_into().ok()?) as usize;
    let payload = &message[MESSAGE_HEADER_LEN..];
    (payload.len() == len && len > 0).then_some(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pro_envelope_preserves_legacy_namespace_and_checks_version_and_lengths() {
        assert!(ProRefreshPayload::decode(&[]).unwrap().is_none());
        assert!(ProRefreshPayload::decode(&[0]).unwrap().is_none());
        for len in 1u16..=u16::MAX {
            assert!(
                ProRefreshPayload::decode(&len.to_le_bytes())
                    .unwrap()
                    .is_none()
            );
        }
        let mut reports = [0u8; REFRESH_PRO_HEADER_LEN + 106 + 3];
        reports[..3].copy_from_slice(&REFRESH_PRO_PREFIX);
        reports[3..5].copy_from_slice(&2u16.to_le_bytes());
        reports[5..7].copy_from_slice(&106u16.to_le_bytes());
        let message = &mut reports[7..113];
        message[..4].copy_from_slice(&SOLANA_MAGIC.to_le_bytes());
        message[100..102].copy_from_slice(&4u16.to_le_bytes());
        message[102..].copy_from_slice(&PAYLOAD_MAGIC.to_le_bytes());
        reports[113..].copy_from_slice(&[1, 0, 42]);
        let pro = ProRefreshPayload::decode(&reports).unwrap().unwrap();
        assert_eq!(pro.ed25519_instruction_index, 2);
        assert_eq!(pro.message, &reports[7..113]);
        assert_eq!(pro.chainlink_reports, &[1, 0, 42]);
        assert!(
            ProRefreshPayload::decode(&reports[..113])
                .unwrap()
                .unwrap()
                .chainlink_reports
                .is_empty()
        );
        for len in 2..113 {
            assert!(
                ProRefreshPayload::decode(&reports[..len]).is_err(),
                "length {len}"
            );
        }
        for version in 0..=u8::MAX {
            if version == 1 {
                continue;
            }
            reports[2] = version;
            assert!(ProRefreshPayload::decode(&reports).is_err());
        }
        reports[2] = 1;
        for length in [0u16, 105, 107, u16::MAX] {
            reports[5..7].copy_from_slice(&length.to_le_bytes());
            assert!(ProRefreshPayload::decode(&reports).is_err());
        }
    }

    #[test]
    fn pinned_mapping_matches_reviewable_source() {
        let mut count = 0;
        let mut prior = 0;
        for line in include_str!("feeds.tsv")
            .lines()
            .filter(|l| !l.starts_with('#'))
        {
            let mut columns = line.split('\t');
            let id: u32 = columns.next().unwrap().parse().unwrap();
            assert!(id > prior);
            prior = id;
            let hex = columns.next().unwrap();
            let mut core = [0u8; 32];
            for (i, byte) in core.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap();
            }
            assert_eq!(core_feed_id(id), Some(core));
            count += 1;
        }
        assert_eq!(count * 36, FEEDS.len());
        assert_eq!(core_feed_id(0), None);
        assert_eq!(core_feed_id(u32::MAX), None);
    }
}
