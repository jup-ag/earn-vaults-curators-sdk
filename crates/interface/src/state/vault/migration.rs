//! Convert the deployed 2152-byte vault to inline oracle configuration.
use {
    super::{ASSET_FLAG_MANUAL_PRICE, AssetConfig, LegacyAssetConfig, LegacyVault, Vault},
    crate::{errors::VaultError, state::Account},
    solana_address::{Address, address_eq},
    solana_program_error::ProgramError,
};

const ASSETS: usize = 1 + core::mem::offset_of!(LegacyVault, assets);
const OLD_ROW: usize = core::mem::size_of::<LegacyAssetConfig>();
const NEW_ROW: usize = core::mem::size_of::<AssetConfig>();
const OLD_TAIL: usize = 1 + core::mem::offset_of!(LegacyVault, positions);
const NEW_TAIL: usize = 1 + core::mem::offset_of!(Vault, positions);
const OLD_RESERVED: usize = 1 + core::mem::offset_of!(LegacyVault, reserved);
const _: () = assert!(ASSETS == 1 + core::mem::offset_of!(Vault, assets));
const _: () = assert!(core::mem::offset_of!(AssetConfig, max_future_skew_seconds) == OLD_ROW);
const _: () = assert!(Vault::LEN - LegacyVault::LEN == 32);

/// Prepare the migrated bytes before resizing the account. The caller checks
/// program ownership and the curator signature. An already migrated vault is
/// an authenticated no-op. All errors occur before changing `destination`.
pub fn migrate_vault_layout(
    source: &[u8],
    destination: &mut [u8],
    curator: &Address,
) -> Result<bool, ProgramError> {
    if destination.len() != Vault::LEN {
        return Err(ProgramError::InvalidAccountData);
    }
    if source.len() == Vault::LEN {
        if !address_eq(&Vault::read(source)?.curator, curator) {
            return Err(VaultError::MissingPermission.into());
        }
        destination.copy_from_slice(source);
        return Ok(false);
    }
    let old = LegacyVault::read(source)?;
    if !address_eq(&old.curator, curator) {
        return Err(VaultError::MissingPermission.into());
    }
    if old.flash_liability.get() != 0 || old.reserved.iter().any(|b| *b != 0) {
        return Err(ProgramError::InvalidAccountData);
    }
    destination.fill(0);
    destination[..ASSETS].copy_from_slice(&source[..ASSETS]);
    for index in 0..8 {
        let start = ASSETS + index * OLD_ROW;
        destination[ASSETS + index * NEW_ROW..ASSETS + index * NEW_ROW + OLD_ROW]
            .copy_from_slice(&source[start..start + OLD_ROW]);
    }
    destination[NEW_TAIL..NEW_TAIL + OLD_RESERVED - OLD_TAIL]
        .copy_from_slice(&source[OLD_TAIL..OLD_RESERVED]);
    let vault = Vault::read_mut(destination)?;
    for asset in &mut vault.assets {
        let window = if asset.configured() {
            let stored = asset.max_staleness_seconds.get();
            let age = if stored == 0 { 15 } else { stored };
            // Preserve the deployed program's effective settlement limits.
            age.min(if asset.flags & ASSET_FLAG_MANUAL_PRICE != 0 {
                600
            } else {
                15
            })
        } else {
            15
        };
        if asset.configured() {
            asset.max_staleness_seconds = window.into();
        }
        asset.max_future_skew_seconds = window.into();
    }
    vault.last_priced_at = 0.into();
    vault.last_priced_slot = 0.into();
    Ok(true)
}
