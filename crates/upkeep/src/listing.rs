//! The vault authority's token accounts, listed at the snapshot's slot: the
//! proof of everything a JupiterLend slot holds.
//!
//! A keeper calls `getTokenAccountsByOwner` for the vault authority twice,
//! under SPL Token and Token-2022, and passes both answers with their context
//! slots. A JupiterLend valuation accepts them only at the snapshot's slot,
//! then attributes every positive balance:
//!
//! - a configured row's mint: custody, which `ValidateAum` prices itself;
//! - a configured row's jlToken in its canonical receipt account: Earn,
//!   valued by [`crate::earn`];
//! - a planned Fluid position NFT: valued by [`crate::fluid`].
//!
//! Any other mint must be one discovery proved is not a Fluid position: the
//! `getProgramAccounts` scan by [`crate::fluid::position_filters`] found no
//! `Position` for it. A Fluid position mint is a PDA the Vaults program creates
//! when it opens the position, so a mint that already exists without one can
//! never become one. Such a mint is ignored ([`Ignored`]), unless its mint
//! authority is Earn's `lending_admin`: then it is a jlToken whose market has no
//! configured row, receipts the plan would leave out, and the valuation fails
//! with [`Error::Unattributed`]. Ignoring the rest keeps a stray airdrop to the
//! authority from blocking a mark. A mint discovery never saw is
//! [`Error::PlanStale`]: discover again.

use {
    crate::{
        Error, Result,
        earn::{EARN_PROGRAM_ID, PlannedRow, derive_f_token_mint},
        snapshot::{
            Account, Snapshot, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID, TokenAccount, mint,
            parse_token_account,
        },
    },
    earn_vaults_sdk::state::Vault,
    solana_address::Address,
    std::collections::{BTreeMap, BTreeSet},
};

/// One `getTokenAccountsByOwner` answer for the vault authority under one
/// token program, with its context slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenListing {
    pub program: Address,
    pub slot: u64,
    pub accounts: Vec<(Address, Account)>,
}

/// Positive balances in `listings` that neither a configured row's mint nor
/// its jlToken explains. Scan for each one's `Position` with
/// [`crate::fluid::position_filters`]: the mints with one are Fluid positions,
/// the rest are a plan's `unrelated` mints.
pub fn candidate_mints(vault: &Vault, listings: &[TokenListing]) -> Result<Vec<Address>> {
    let known: BTreeSet<_> = vault
        .assets
        .iter()
        .filter(|a| a.configured())
        .flat_map(|a| [a.mint, derive_f_token_mint(&a.mint)])
        .collect();
    let mut mints = BTreeSet::new();
    for listing in listings {
        for (key, account) in &listing.accounts {
            let held = parse_token_account(account, key, "listed token account")?;
            if held.amount != 0 && !known.contains(&held.mint) {
                mints.insert(held.mint);
            }
        }
    }
    Ok(mints.into_iter().collect())
}

/// `["lending_admin"]` under Earn: the mint authority of every jlToken.
pub fn derive_lending_admin() -> Address {
    Address::find_program_address(&[b"lending_admin"], &EARN_PROGRAM_ID).0
}

/// A positive balance the listing showed in a mint discovery proved is
/// neither Earn nor Fluid. It is not part of the mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ignored {
    pub account: Address,
    pub mint: Address,
    pub amount: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Attribution {
    /// The account holding each NFT mint the listing shows.
    pub nfts: BTreeMap<Address, (Address, TokenAccount)>,
    pub ignored: Vec<Ignored>,
}

/// Check `listings` cover both token programs at the snapshot's slot and
/// attribute every positive balance (see the module docs). `unrelated` mints
/// must be in the snapshot.
pub(crate) fn attribute(
    snapshot: &Snapshot,
    holder: &Address,
    rows: &[PlannedRow],
    nft_mints: &[Address],
    unrelated: &[Address],
    listings: &[TokenListing],
) -> Result<Attribution> {
    for program in [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID] {
        match listings.iter().filter(|l| l.program == program).count() {
            1 => {}
            0 => return Err(Error::Listing("a token program's listing is missing")),
            _ => return Err(Error::Listing("a token program is listed twice")),
        }
    }
    if listings.len() != 2 {
        return Err(Error::Listing("a listing names an unknown token program"));
    }
    let custody: BTreeSet<_> = rows.iter().map(|r| r.market.mint).collect();
    let receipts: BTreeMap<_, _> = rows
        .iter()
        .map(|r| (r.market.f_token_mint, r.market.receipt_account(holder)))
        .collect();
    let lending_admin = derive_lending_admin();
    let mut out = Attribution::default();
    let mut seen = BTreeSet::new();
    for listing in listings {
        if listing.slot != snapshot.slot() {
            return Err(Error::ReadSlot {
                what: "token listing",
                read: listing.slot,
                snapshot: snapshot.slot(),
            });
        }
        for (key, account) in &listing.accounts {
            if !seen.insert(*key) {
                return Err(Error::Listing("an account is listed twice"));
            }
            if account.owner != listing.program {
                return Err(Error::Listing("an account is under another token program"));
            }
            // Same slot, so a key the snapshot also fetched has the same bytes.
            if let Ok(Some(fetched)) = snapshot.get(key)
                && fetched != account
            {
                return Err(Error::Conflicting(*key));
            }
            let parsed = parse_token_account(account, key, "listed token account")?;
            if parsed.owner != *holder {
                return Err(Error::Listing(
                    "lists an account the vault authority does not own",
                ));
            }
            let unattributed = |reason| Error::Unattributed {
                account: *key,
                mint: parsed.mint,
                reason,
            };
            if parsed.amount == 0 || custody.contains(&parsed.mint) {
                continue;
            }
            if let Some(canonical) = receipts.get(&parsed.mint) {
                if canonical != key {
                    return Err(unattributed(
                        "Earn receipts outside the canonical account are not valued",
                    ));
                }
                continue;
            }
            if nft_mints.contains(&parsed.mint) {
                if out.nfts.insert(parsed.mint, (*key, parsed)).is_some() {
                    return Err(unattributed("a second account holds this position NFT"));
                }
                continue;
            }
            if !unrelated.contains(&parsed.mint) {
                return Err(Error::PlanStale(
                    "the listing holds a mint discovery did not classify",
                ));
            }
            if mint(snapshot, &parsed.mint, "listed mint")?.mint_authority == Some(lending_admin) {
                return Err(unattributed(
                    "Earn receipts for a market with no configured row are not valued",
                ));
            }
            out.ignored.push(Ignored {
                account: *key,
                mint: parsed.mint,
                amount: parsed.amount,
            });
        }
    }
    Ok(out)
}
