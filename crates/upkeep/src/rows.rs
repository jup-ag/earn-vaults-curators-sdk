//! The vault's configured asset rows, with the token program each mint lives
//! under. Every valuation and job derives its token accounts from these.

use {
    crate::{Error, Result, snapshot::Snapshot},
    earn_vaults_sdk::state::Vault,
    solana_address::Address,
};

/// One configured asset row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetRow {
    /// Index into `Vault.assets`.
    pub index: u8,
    pub mint: Address,
    pub decimals: u8,
    /// The mint's owner: SPL Token or Token-2022.
    pub token_program: Address,
}

/// The vault and every configured row's mint: fetch these together, then
/// [`read`] the rows from the result.
pub fn discovery_keys(vault_key: &Address, vault: &Vault) -> Vec<Address> {
    let mut keys = vec![*vault_key];
    keys.extend(
        vault
            .assets
            .iter()
            .filter(|a| a.configured())
            .map(|a| a.mint),
    );
    keys
}

/// Every configured row of `vault`, with its mint read from `snapshot`.
pub fn read(snapshot: &Snapshot, vault: &Vault) -> Result<Vec<AssetRow>> {
    vault
        .assets
        .iter()
        .enumerate()
        .filter(|(_, a)| a.configured())
        .map(|(i, a)| {
            let mint = snapshot.mint(&a.mint)?;
            if mint.decimals != a.decimals {
                return Err(Error::Layout {
                    what: "asset mint",
                    address: a.mint,
                    reason: "decimals differ from the vault row",
                });
            }
            Ok(AssetRow {
                index: i as u8,
                mint: a.mint,
                decimals: a.decimals,
                token_program: mint.token_program,
            })
        })
        .collect()
}

/// The rows a plan was built from must still be the vault's rows.
pub(crate) fn check(snapshot: &Snapshot, vault: &Vault, rows: &[AssetRow]) -> Result<()> {
    if read(snapshot, vault)? != rows {
        return Err(Error::PlanStale("asset rows"));
    }
    Ok(())
}
