//! Loader-v3 `ProgramData` header, used to authenticate the upgrade authority.
use {
    crate::constants::UPGRADEABLE_LOADER_ID, solana_address::Address,
    solana_program_error::ProgramError, wincode::SchemaRead,
};

/// `UpgradeableLoaderState::ProgramData` variant tag.
const PROGRAM_DATA_TAG: u32 = 3;
/// Tag, slot, and `Some` authority; the program's ELF follows.
const HEADER_LEN: usize = 4 + 8 + 1 + 32;

/// Bincode `UpgradeableLoaderState::ProgramData`, matched by wincode's schema.
#[derive(SchemaRead)]
struct ProgramDataHeader {
    tag: u32,
    _slot: u64,
    upgrade_authority: Option<Address>,
}

/// Return the upgrade authority stored in a loader-v3 `ProgramData` account.
/// Callers must bind the account address to the expected program; an immutable
/// program (no authority) is rejected.
pub fn upgrade_authority(owner: &Address, data: &[u8]) -> Result<Address, ProgramError> {
    if owner != &UPGRADEABLE_LOADER_ID {
        return Err(ProgramError::InvalidAccountOwner);
    }
    let header: ProgramDataHeader = wincode::deserialize(
        data.get(..HEADER_LEN)
            .ok_or(ProgramError::InvalidAccountData)?,
    )
    .map_err(|_| ProgramError::InvalidAccountData)?;
    match header {
        ProgramDataHeader {
            tag: PROGRAM_DATA_TAG,
            upgrade_authority: Some(authority),
            ..
        } => Ok(authority),
        _ => Err(ProgramError::InvalidAccountData),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_loader_header() {
        let authority = Address::new_from_array([9; 32]);
        let mut data = [0u8; HEADER_LEN + 8];
        data[..4].copy_from_slice(&PROGRAM_DATA_TAG.to_le_bytes());
        data[12] = 1;
        data[13..HEADER_LEN].copy_from_slice(authority.as_array());
        let loader = UPGRADEABLE_LOADER_ID;
        assert_eq!(upgrade_authority(&loader, &data), Ok(authority));
        assert_eq!(
            upgrade_authority(&Address::default(), &data),
            Err(ProgramError::InvalidAccountOwner)
        );
        let bad = |edit: fn(&mut [u8])| {
            let mut d = data;
            edit(&mut d);
            upgrade_authority(&loader, &d)
        };
        assert_eq!(bad(|d| d[12] = 0), Err(ProgramError::InvalidAccountData));
        assert_eq!(bad(|d| d[12] = 2), Err(ProgramError::InvalidAccountData));
        assert_eq!(bad(|d| d[0] = 2), Err(ProgramError::InvalidAccountData));
        assert_eq!(
            upgrade_authority(&loader, &data[..HEADER_LEN - 1]),
            Err(ProgramError::InvalidAccountData)
        );
    }
}
