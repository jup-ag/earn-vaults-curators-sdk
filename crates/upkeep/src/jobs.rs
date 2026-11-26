//! Upkeep jobs as instruction lists, in the order program 0.2.0 requires.
//!
//! Every job returns `Vec<Instruction>`, never a transaction: the curator's
//! wallet, a KMS key and a Squads proposal each wrap the same list. Compute
//! budget and fee payer are the caller's.
//!
//! - [`attest`]: refresh → marks → `ValidateAum` (#274). A first mark is
//!   capped against fresh validated AUM, and every mark clears that snapshot,
//!   so each first mark gets a `ValidateAum` right before it.
//! - [`fill_redeem`]: a fresh `ValidateAum` right before the settlement, even
//!   after a deposit settlement earlier in the list (#278), optionally
//!   `CrystallizeFees` (#290), then `FulfillRedeem`.
//! - [`crystallize`]: a fresh `ValidateAum`, then `CrystallizeFees`.

use {
    crate::{
        Error, Result,
        mark::{Grant, Mark},
        rows::AssetRow,
        snapshot::Account,
    },
    earn_vaults_sdk::{
        instructions::{CrystallizeFees, FulfillRedeem, PriceOpaque, ValidateAum},
        pda::{derive_associated_token_account, derive_escrow, derive_share_mint},
        state::{
            Account as _, Escrow, FULFILLED_PARTIAL, FULFILLED_PENDING, MAX_POSITIONS, Pause,
            REQUEST_TYPE_REDEEM, Vault,
        },
    },
    solana_address::Address,
    solana_instruction::Instruction,
};

/// One `PRICE_IN_AUM` row's custody: the vault authority's ATA of its mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Custody {
    pub mint: Address,
    pub token_program: Address,
    pub account: Address,
}

/// The accounts every job on one vault shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultKeys {
    pub vault: Address,
    pub share_mint: Address,
    /// Per `PRICE_IN_AUM` row, in row order: the order `ValidateAum` checks.
    pub custody: Vec<Custody>,
}

impl VaultKeys {
    /// `rows` are the vault's configured rows ([`crate::rows::read`], or a
    /// plan's `rows`) as of `state`.
    pub fn new(vault: Address, state: &Vault, rows: &[AssetRow]) -> Result<Self> {
        let authority = earn_vaults_sdk::pda::derive_vault_authority(&vault);
        let custody = state
            .assets
            .iter()
            .enumerate()
            .filter(|(_, a)| a.price_in_aum())
            .map(|(i, a)| {
                let row = rows
                    .iter()
                    .find(|r| usize::from(r.index) == i && r.mint == a.mint)
                    .ok_or(Error::PlanStale("asset rows"))?;
                Ok(Custody {
                    mint: a.mint,
                    token_program: row.token_program,
                    account: derive_associated_token_account(
                        &authority,
                        &a.mint,
                        &row.token_program,
                    ),
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            vault,
            share_mint: derive_share_mint(&vault),
            custody,
        })
    }

    pub fn validate_aum(&self) -> Instruction {
        ValidateAum {
            vault: self.vault,
            share_mint: self.share_mint,
            custody_token_accounts: self.custody.iter().map(|c| c.account).collect(),
        }
        .into_instruction()
    }

    fn is_validate_aum(&self, ix: &Instruction) -> bool {
        *ix == self.validate_aum()
    }
}

fn mark_ix(keys: &VaultKeys, mark: &Mark) -> Instruction {
    let build = PriceOpaque {
        signer: mark.signer,
        vault: keys.vault,
        slot: mark.slot,
        value: mark.value,
    };
    match mark.grant {
        Grant::JupiterLend => build.into_earn_instruction(),
        Grant::Vault => build.into_instruction(),
    }
}

/// `refresh`, then each mark, then `ValidateAum`. `refresh` is the caller's
/// `RefreshPrices` list (Pyth Core accounts, a Pyth Pro pair, or Chainlink
/// reports), placed first and unchanged; value the marks against
/// [`crate::Snapshot::with_refresh`] so they use the prices it installs.
///
/// Build marks with [`crate::mark::MarkDecision::mark`]. Later marks go first
/// and one `ValidateAum` follows them. Every mark clears the validated AUM a
/// first mark is capped against, so each first (`bootstrap`) mark gets its
/// own `ValidateAum` before it, and a last one closes the list.
pub fn attest(
    keys: &VaultKeys,
    refresh: Vec<Instruction>,
    marks: &[Mark],
) -> Result<Vec<Instruction>> {
    for (i, mark) in marks.iter().enumerate() {
        // A higher `slot` byte addresses an asset row or a sink, not a position.
        if mark.slot >= MAX_POSITIONS {
            return Err(Error::Job("mark slot is not a position"));
        }
        if marks[..i].iter().any(|m| m.slot == mark.slot) {
            return Err(Error::Job("two marks for one slot"));
        }
    }
    let mut ixs = refresh;
    ixs.extend(
        marks
            .iter()
            .filter(|m| !m.bootstrap)
            .map(|m| mark_ix(keys, m)),
    );
    for mark in marks.iter().filter(|m| m.bootstrap) {
        ixs.push(keys.validate_aum());
        ixs.push(mark_ix(keys, mark));
    }
    ixs.push(keys.validate_aum());
    Ok(ixs)
}

/// A queued redeem read from its `Escrow` account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedRedeem {
    pub escrow: Address,
    pub owner: Address,
    pub nonce: u64,
    /// Shares still to settle.
    pub shares: u64,
    /// The token the redeemer chose; `None` for a migrated request, which
    /// pays in the canonical base asset.
    pub payout_mint: Option<Address>,
    pub unlock_at: i64,
}

impl QueuedRedeem {
    pub fn read(vault: &Address, escrow: &Address, account: &Account) -> Result<Self> {
        crate::snapshot::owned_by(account, escrow, &earn_vaults_sdk::ID, "escrow")?;
        let bad = |reason| Error::Layout {
            what: "escrow",
            address: *escrow,
            reason,
        };
        let e = Escrow::read(&account.data).map_err(|_| bad("not an escrow account"))?;
        if e.vault != *vault {
            return Err(bad("belongs to another vault"));
        }
        if e.request_type != REQUEST_TYPE_REDEEM {
            return Err(bad("not a redeem request"));
        }
        if e.fulfilled != FULFILLED_PENDING && e.fulfilled != FULFILLED_PARTIAL {
            return Err(bad("already fulfilled"));
        }
        if derive_escrow(vault, &e.owner, e.nonce.get()) != *escrow {
            return Err(bad("address is not the escrow PDA"));
        }
        Ok(Self {
            escrow: *escrow,
            owner: e.owner,
            nonce: e.nonce.get(),
            shares: e.amount.get(),
            payout_mint: (e.original_shares.get() != 0).then_some(e.payout_mint),
            unlock_at: e.unlock_at.get(),
        })
    }
}

/// Settle one queued redeem. `prior` is everything that must run first in the
/// same transaction, typically [`attest`]'s list when marks are due. A fresh
/// `ValidateAum` is appended unless `prior` already ends with one.
///
/// `crystallize_payer` adds `CrystallizeFees` before the fill. Leave it `None`
/// unless a simulation of the list fails with `FeeDebtAboveCap` (#290): then
/// the fill would leave fee debt above half the AUM left behind, and must
/// crystallize first. Whether it does depends on the AUM this transaction's
/// `ValidateAum` computes, which no builder sees. Crystallizing when not
/// needed is not free: it fails on dust fees (`FeeAmountTooSmall`) and moves
/// the redeem's payout.
pub fn fill_redeem(
    keys: &VaultKeys,
    state: &Vault,
    prior: Vec<Instruction>,
    signer: Address,
    redeem: &QueuedRedeem,
    crystallize_payer: Option<Address>,
) -> Result<Vec<Instruction>> {
    if state.paused & Pause::Redeem as u8 != 0 {
        return Err(Error::Job("redemptions are paused"));
    }
    let payout = match redeem.payout_mint {
        Some(mint) => Some(mint),
        None => state.canonical_base_asset().map(|(_, base)| base.mint),
    };
    // `FulfillRedeem` pays only from a row that takes deposits and is priced.
    let custody = payout
        .filter(|mint| {
            state
                .assets
                .iter()
                .any(|a| a.mint == *mint && a.deposit_enabled() && a.price_in_aum())
        })
        .and_then(|mint| keys.custody.iter().find(|c| c.mint == mint))
        .copied()
        .ok_or(Error::Job(
            "the payout asset has no priced, deposit-enabled custody row",
        ))?;
    let mut ixs = prior;
    if !ixs.last().is_some_and(|ix| keys.is_validate_aum(ix)) {
        ixs.push(keys.validate_aum());
    }
    if let Some(payer) = crystallize_payer {
        ixs.push(
            CrystallizeFees {
                signer: payer,
                vault: keys.vault,
            }
            .into_instruction(),
        );
    }
    ixs.push(
        FulfillRedeem {
            authority: signer,
            vault: keys.vault,
            user: redeem.owner,
            nonce: redeem.nonce,
            base_mint: custody.mint,
            vault_token_account: custody.account,
            token_program: custody.token_program,
        }
        .into_instruction(),
    );
    Ok(ixs)
}

/// `prior`, a fresh `ValidateAum`, then `CrystallizeFees`. Permissionless:
/// `payer` pays rent if a fee account must be created.
pub fn crystallize(keys: &VaultKeys, prior: Vec<Instruction>, payer: Address) -> Vec<Instruction> {
    let mut ixs = prior;
    if !ixs.last().is_some_and(|ix| keys.is_validate_aum(ix)) {
        ixs.push(keys.validate_aum());
    }
    ixs.push(
        CrystallizeFees {
            signer: payer,
            vault: keys.vault,
        }
        .into_instruction(),
    );
    ixs
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        earn_vaults_sdk::{
            state::{ASSET_FLAG_DEPOSIT_ENABLED, ASSET_FLAG_PRICE_IN_AUM, REQUEST_TYPE_REDEEM},
            zeropod::U64,
        },
    };

    const VAULT: Address = Address::new_from_array([7; 32]);
    const SIGNER: Address = Address::new_from_array([8; 32]);
    const USDC: Address = Address::new_from_array([9; 32]);
    const TOKEN: Address = crate::snapshot::TOKEN_PROGRAM_ID;

    fn vault_bytes() -> Vec<u8> {
        let mut data = vec![0u8; Vault::LEN];
        data[0] = Vault::DISCRIMINATOR;
        let v = Vault::read_mut(&mut data).unwrap();
        v.assets[0].mint = USDC;
        v.assets[0].decimals = 6;
        v.assets[0].flags = ASSET_FLAG_PRICE_IN_AUM | ASSET_FLAG_DEPOSIT_ENABLED;
        data
    }

    fn keys(data: &[u8]) -> VaultKeys {
        let rows = [AssetRow {
            index: 0,
            mint: USDC,
            decimals: 6,
            token_program: TOKEN,
        }];
        VaultKeys::new(VAULT, Vault::read(data).unwrap(), &rows).unwrap()
    }

    fn mark(slot: u8, value: u64, bootstrap: bool) -> Mark {
        Mark {
            signer: SIGNER,
            slot,
            value,
            bootstrap,
            grant: Grant::Vault,
        }
    }

    fn slot_of(ix: &Instruction) -> Option<u8> {
        (ix.program_id == earn_vaults_sdk::ID && ix.data[0] == 15).then(|| ix.data[1])
    }

    #[test]
    fn attest_orders_refresh_marks_validate_and_bootstraps_after_a_fresh_validate() {
        let data = vault_bytes();
        let keys = keys(&data);
        let refresh = Instruction {
            program_id: earn_vaults_sdk::ID,
            accounts: vec![],
            data: vec![23],
        };
        let ixs = attest(
            &keys,
            vec![refresh.clone()],
            &[mark(2, 5, true), mark(0, 10, false), mark(1, 0, false)],
        )
        .unwrap();
        let validate = keys.validate_aum();
        assert_eq!(ixs[0], refresh);
        assert_eq!(slot_of(&ixs[1]), Some(0));
        assert_eq!(
            slot_of(&ixs[2]),
            Some(1),
            "a zero re-mark of an initialized slot is posted"
        );
        assert_eq!(ixs[3], validate);
        assert_eq!(slot_of(&ixs[4]), Some(2));
        assert_eq!(ixs[5], validate);
        assert_eq!(ixs.len(), 6);
        // Without a bootstrap mark there is exactly one validation, last.
        let ixs = attest(&keys, vec![], &[mark(0, 10, false)]).unwrap();
        assert_eq!(ixs.len(), 2);
        assert_eq!(ixs[1], validate);
    }

    #[test]
    fn attest_refuses_duplicate_slots_and_places_a_zero_first_mark() {
        let data = vault_bytes();
        let keys = keys(&data);
        assert!(attest(&keys, vec![], &[mark(0, 1, false), mark(0, 2, false)]).is_err());
        // 128 is asset row 0's manual price, not a position.
        assert!(attest(&keys, vec![], &[mark(128, 1, false)]).is_err());
        // Past bootstrap, `decide` yields a zero first mark to attest an
        // empty slot; it goes after a validation like any first mark.
        let ixs = attest(&keys, vec![], &[mark(0, 0, true)]).unwrap();
        assert_eq!(ixs.len(), 3);
        assert_eq!(ixs[0], keys.validate_aum());
        assert_eq!(slot_of(&ixs[1]), Some(0));
        assert_eq!(ixs[2], keys.validate_aum());
    }

    #[test]
    fn each_first_mark_gets_its_own_fresh_validate() {
        let data = vault_bytes();
        let keys = keys(&data);
        let validate = keys.validate_aum();
        // Every mark clears the validated AUM the next first mark is capped
        // against (`PricedSlotStale` otherwise).
        let ixs = attest(&keys, vec![], &[mark(0, 5, true), mark(1, 0, true)]).unwrap();
        assert_eq!(ixs.len(), 5);
        assert_eq!(ixs[0], validate);
        assert_eq!(slot_of(&ixs[1]), Some(0));
        assert_eq!(ixs[2], validate);
        assert_eq!(slot_of(&ixs[3]), Some(1));
        assert_eq!(ixs[4], validate);
    }

    #[test]
    fn jupiter_lend_grant_marks_use_the_earn_delegate() {
        let data = vault_bytes();
        let keys = keys(&data);
        let scoped = Mark {
            grant: Grant::JupiterLend,
            ..mark(0, 10, false)
        };
        let ixs = attest(&keys, vec![], &[scoped]).unwrap();
        let expected = PriceOpaque {
            signer: SIGNER,
            vault: VAULT,
            slot: 0,
            value: 10,
        }
        .into_earn_instruction();
        assert_eq!(ixs[0], expected);
    }

    fn redeem() -> QueuedRedeem {
        let owner = Address::new_from_array([3; 32]);
        QueuedRedeem {
            escrow: derive_escrow(&VAULT, &owner, 4),
            owner,
            nonce: 4,
            shares: 1_000,
            payout_mint: Some(USDC),
            unlock_at: 0,
        }
    }

    #[test]
    fn a_fill_gets_a_fresh_validate_even_after_a_deposit_settlement() {
        let data = vault_bytes();
        let keys = keys(&data);
        let state = Vault::read(&data).unwrap();
        let validate = keys.validate_aum();
        let deposit_fill = Instruction {
            program_id: earn_vaults_sdk::ID,
            accounts: vec![],
            data: vec![11],
        };
        // #278: a deposit settlement before the fill needs its own ValidateAum.
        let ixs = fill_redeem(
            &keys,
            state,
            vec![validate.clone(), deposit_fill.clone()],
            SIGNER,
            &redeem(),
            None,
        )
        .unwrap();
        assert_eq!(ixs[..2], [validate.clone(), deposit_fill]);
        assert_eq!(ixs[2], validate);
        assert_eq!(ixs[3].data, [12]);
        assert_eq!(ixs.len(), 4);
        // An attest list already ends with a validation: no second one.
        let ixs = fill_redeem(
            &keys,
            state,
            attest(&keys, vec![], &[]).unwrap(),
            SIGNER,
            &redeem(),
            Some(SIGNER),
        )
        .unwrap();
        assert_eq!(ixs.len(), 3);
        assert_eq!(ixs[0], validate);
        assert_eq!(ixs[1].data, [17], "#290: crystallize before the fill");
        assert_eq!(ixs[2].data, [12]);
        assert_eq!(ixs[2].accounts[4].pubkey, USDC);
    }

    #[test]
    fn a_fill_refuses_paused_redemptions_and_payout_rows_it_cannot_pay_from() {
        let mut data = vault_bytes();
        let keys = keys(&data);
        let other = QueuedRedeem {
            payout_mint: Some(SIGNER),
            ..redeem()
        };
        assert!(
            fill_redeem(
                &keys,
                Vault::read(&data).unwrap(),
                vec![],
                SIGNER,
                &other,
                None
            )
            .is_err()
        );
        // A priced row that no longer takes deposits cannot pay out.
        Vault::read_mut(&mut data).unwrap().assets[0].flags = ASSET_FLAG_PRICE_IN_AUM;
        assert!(
            fill_redeem(
                &keys,
                Vault::read(&data).unwrap(),
                vec![],
                SIGNER,
                &redeem(),
                None
            )
            .is_err()
        );
        Vault::read_mut(&mut data).unwrap().assets[0].flags =
            ASSET_FLAG_PRICE_IN_AUM | ASSET_FLAG_DEPOSIT_ENABLED;
        Vault::read_mut(&mut data).unwrap().paused = Pause::Redeem as u8;
        assert!(
            fill_redeem(
                &keys,
                Vault::read(&data).unwrap(),
                vec![],
                SIGNER,
                &redeem(),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn queued_redeems_are_read_from_their_own_escrow() {
        let r = redeem();
        let mut data = vec![0u8; Escrow::LEN];
        data[0] = Escrow::DISCRIMINATOR;
        {
            let e = Escrow::read_mut(&mut data).unwrap();
            e.amount = U64::from(1_000u64);
            e.owner = r.owner;
            e.vault = VAULT;
            e.request_type = REQUEST_TYPE_REDEEM;
            e.nonce = U64::from(4u64);
            e.original_shares = U64::from(1_000u64);
            e.payout_mint = USDC;
        }
        let account = Account {
            owner: earn_vaults_sdk::ID,
            data,
        };
        assert_eq!(QueuedRedeem::read(&VAULT, &r.escrow, &account), Ok(r));
        assert!(
            QueuedRedeem::read(&SIGNER, &r.escrow, &account).is_err(),
            "other vault"
        );
        assert!(
            QueuedRedeem::read(&VAULT, &SIGNER, &account).is_err(),
            "not the PDA"
        );
        let foreign = Account {
            owner: SIGNER,
            ..account
        };
        assert!(QueuedRedeem::read(&VAULT, &r.escrow, &foreign).is_err());
    }
}
