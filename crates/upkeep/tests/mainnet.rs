//! Valuations over vault states recorded from mainnet, each read with one
//! `getMultipleAccounts` at one slot. `expected` holds this crate's values and,
//! for parity, the values the existing implementations computed from the same
//! bytes. See `fixtures/README.md` for where each number came from.

use {
    base64::{Engine, engine::general_purpose::STANDARD},
    earn_vaults_upkeep::{
        Account, Error, PriceFault, Snapshot,
        earn::{EarnPlan, EarnScope},
        jobs::{VaultKeys, attest},
        mark::{Mark, MarkDecision},
        offerbook::OfferbookPlan,
        snapshot::CLOCK_SYSVAR_ID,
    },
    offerbook_client::state::Loan,
    serde_json::Value,
    solana_address::Address,
};

struct Fixture {
    vault: Address,
    slot: u64,
    accounts: Vec<(Address, Option<Account>)>,
    lent: Vec<Address>,
    borrowed: Vec<Address>,
    expected: Value,
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn load(name: &str) -> Fixture {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let keys = |k: &str| f[k].as_array().unwrap().iter().map(addr).collect();
    Fixture {
        vault: addr(&f["vault"]),
        slot: f["slot"].as_u64().unwrap(),
        accounts: f["accounts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let account = a["account"].as_object().map(|o| Account {
                    owner: addr(&o["owner"]),
                    data: STANDARD.decode(o["data"].as_str().unwrap()).unwrap(),
                });
                (addr(&a["address"]), account)
            })
            .collect(),
        lent: keys("lent"),
        borrowed: keys("borrowed"),
        expected: f["expected"].clone(),
    }
}

impl Fixture {
    fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.slot, self.accounts.clone()).unwrap()
    }

    fn expect(&self, position: &str, key: &str) -> u64 {
        self.expected[position][key].as_u64().unwrap()
    }

    fn offerbook(&self) -> OfferbookPlan {
        OfferbookPlan::new(
            self.vault,
            &self.snapshot(),
            self.lent.clone(),
            self.borrowed.clone(),
        )
        .unwrap()
    }

    /// Every key a plan names was fetched, so one read covers it.
    fn covers(&self, keys: &[Address]) {
        for key in keys {
            assert!(
                self.accounts.iter().any(|(k, _)| k == key),
                "{key} not recorded"
            );
        }
        assert!(keys.len() <= 100, "one getMultipleAccounts takes 100 keys");
    }

    /// The same accounts with `edit` applied to one of them.
    fn with(&self, key: &Address, edit: impl Fn(&mut Account)) -> Snapshot {
        let mut accounts = self.accounts.clone();
        for (k, a) in &mut accounts {
            if k == key {
                edit(a.as_mut().unwrap());
            }
        }
        Snapshot::new(self.slot, accounts).unwrap()
    }
}

/// Basis points of `a` away from `b`.
fn bps(a: u64, b: u64) -> f64 {
    (a as f64 - b as f64).abs() * 1e4 / b as f64
}

#[test]
fn juplend_optimizer_earn_equals_the_optimizer_and_the_chain() {
    let f = load("bitwise-juplend-optimizer");
    let snapshot = f.snapshot();
    let plan = EarnPlan::new(f.vault, &snapshot, EarnScope::LenderOnly).unwrap();
    f.covers(&plan.keys());
    let earn = plan.value(&snapshot).unwrap();
    assert_eq!(earn.value, f.expect("earn", "sdk"));
    assert_eq!(earn.value, f.expect("earn", "optimizer"));
    // Marked 8 s before, from the same exchange and row prices.
    assert_eq!(earn.value, f.expect("earn", "onchain_mark"));
    assert_eq!(earn.markets.len(), 5);
    assert!(
        OfferbookPlan::new(f.vault, &snapshot, [], []).is_err(),
        "no Offerbook slot"
    );

    // refresh → mark → ValidateAum with every priced row's custody in order.
    let MarkDecision::Post { value, bootstrap } = earn.slot.decide(earn.value) else {
        panic!("an initialized slot is always marked");
    };
    let state = snapshot.vault(&f.vault).unwrap();
    let keys = VaultKeys::new(f.vault, state, &plan.rows).unwrap();
    assert_eq!(keys.custody.len(), 5);
    for (_, _, custody) in &keys.custody {
        assert!(
            snapshot.get(custody).unwrap().is_some(),
            "custody ATA exists"
        );
    }
    let signer = Address::new_from_array([1; 32]);
    let mark = Mark {
        signer,
        slot: earn.slot.index,
        value,
        bootstrap,
        earn_scoped: true,
    };
    let ixs = attest(&keys, vec![], &[mark]).unwrap();
    assert_eq!(ixs.len(), 2);
    assert_eq!(ixs[1], keys.validate_aum());
}

#[test]
fn susdai_yield_matches_the_existing_copies_and_the_chain() {
    let f = load("bitwise-susdai-yield");
    let snapshot = f.snapshot();
    // Its JupiterLend slot is not lender-only, so it could hold Fluid vaults.
    assert!(matches!(
        EarnPlan::new(f.vault, &snapshot, EarnScope::LenderOnly),
        Err(Error::Position(..))
    ));
    let plan = EarnPlan::new(f.vault, &snapshot, EarnScope::AssumeNoFluid).unwrap();
    f.covers(&plan.keys());
    let earn = plan.value(&snapshot).unwrap();
    assert_eq!(earn.value, f.expect("earn", "sdk"));
    assert_eq!(earn.value, f.expect("earn", "optimizer"));
    assert_eq!(earn.value, f.expect("earn", "curator_api"));
    assert!(bps(earn.value, f.expect("earn", "onchain_mark")) < 0.5);

    let plan = f.offerbook();
    f.covers(&plan.keys());
    assert_eq!(plan.lent.len(), 68);
    let book = plan.value(&snapshot).unwrap();
    assert_eq!(book.value, f.expect("offerbook", "sdk"));
    // The market maker's managed mark counts escrow yield too; the API and the
    // borrowing path leave its jlUSDC out.
    assert_eq!(book.value, f.expect("offerbook", "mm_managed"));
    assert!(book.value > f.expect("offerbook", "curator_api"));
    assert!(bps(book.value, f.expect("offerbook", "onchain_mark")) < 1.0);
    assert_eq!(book.liabilities, 0);
}

#[test]
fn re7_books_with_extended_loans_fail_closed() {
    for name in ["re7-nopal-lending", "re7-nopal-looping"] {
        let f = load(name);
        let snapshot = f.snapshot();
        let plan = f.offerbook();
        f.covers(&plan.keys());
        let error = plan.value(&snapshot).unwrap_err();
        assert!(
            matches!(
                error,
                Error::Loan {
                    reason: "extended loans need manual valuation",
                    ..
                }
            ),
            "{name}: {error}"
        );
    }
    // Its Earn slot holds nothing: zero, and since it was marked before, a
    // zero mark is posted rather than skipped.
    let f = load("re7-nopal-lending");
    let snapshot = f.snapshot();
    let earn = EarnPlan::new(f.vault, &snapshot, EarnScope::LenderOnly)
        .unwrap()
        .value(&snapshot)
        .unwrap();
    assert_eq!(earn.value, f.expect("earn", "optimizer"));
    assert_eq!(
        earn.slot.decide(earn.value),
        MarkDecision::Post {
            value: 0,
            bootstrap: false
        }
    );
}

/// The loans with `extension_count` cleared, so the netting runs on real bytes.
fn unextended(f: &Fixture) -> Snapshot {
    let mut accounts = f.accounts.clone();
    for (key, account) in &mut accounts {
        if f.lent.contains(key) || f.borrowed.contains(key) {
            let account = account.as_mut().unwrap();
            let mut loan = Loan::from_account_data(&account.data).unwrap();
            loan.extension_count = 0;
            account.data = [
                Loan::DISCRIMINATOR.to_vec(),
                offerbook_client::serialize(&loan).unwrap(),
            ]
            .concat();
        }
    }
    Snapshot::new(f.slot, accounts).unwrap()
}

#[test]
fn re7_looping_nets_collateral_against_full_debt() {
    let f = load("re7-nopal-looping");
    let book = f.offerbook().value(&unextended(&f)).unwrap();
    assert_eq!(book.value, f.expect("offerbook", "sdk_if_extended_allowed"));
    // Per-loan rounding in the other copies differs by at most a micro-unit per loan.
    for other in ["curator_api_if_extended_allowed", "mm_borrowing"] {
        assert!(
            book.value.abs_diff(f.expect("offerbook", other)) <= 3,
            "{other}"
        );
    }
    // Three borrowed loans: 157,633 USDC principal + 166 USDC interest, owed in full.
    assert_eq!(book.liabilities, 157_788_279_244);
    assert_eq!(book.value, book.assets - book.liabilities);
}

#[test]
fn re7_lending_accrues_extended_loans_from_the_current_period() {
    let f = load("re7-nopal-lending");
    let book = f.offerbook().value(&unextended(&f)).unwrap();
    assert_eq!(book.value, f.expect("offerbook", "sdk_if_extended_allowed"));
    // The other copies accrue from `created_at`, which after an extension
    // books the whole period's interest again: 123 USDC more here.
    let api = f.expect("offerbook", "curator_api_if_extended_allowed");
    assert_eq!(api - book.value, 123_058_137);
    assert_eq!(api, f.expect("offerbook", "mm_borrowing"));
}

#[test]
fn a_stale_clock_or_tampered_account_never_values() {
    let f = load("bitwise-juplend-optimizer");
    let plan = EarnPlan::new(f.vault, &f.snapshot(), EarnScope::LenderOnly).unwrap();
    // Move the clock past the USDC row's 60 s window.
    let late = f.with(&CLOCK_SYSVAR_ID, |a| {
        let now = i64::from_le_bytes(a.data[32..40].try_into().unwrap());
        a.data[32..40].copy_from_slice(&(now + 61).to_le_bytes());
    });
    assert!(matches!(
        plan.value(&late),
        Err(Error::Price {
            reason: PriceFault::Stale,
            ..
        })
    ));
    let foreign = f.with(&CLOCK_SYSVAR_ID, |a| {
        a.owner = Address::new_from_array([1; 32])
    });
    assert!(matches!(
        plan.value(&foreign),
        Err(Error::WrongOwner { .. })
    ));
    // A Lending account that is missing, or of another layout.
    let lending = plan.rows[0].market.lending;
    let short = f.with(&lending, |a| a.data.truncate(100));
    assert!(matches!(plan.value(&short), Err(Error::Layout { .. })));
    let unfetched = Snapshot::new(
        f.slot,
        f.accounts.iter().filter(|(k, _)| *k != lending).cloned(),
    )
    .unwrap();
    assert_eq!(plan.value(&unfetched), Err(Error::NotFetched(lending)));
    // A receipt account that belongs to someone else.
    let receipt = plan.rows[0].market.receipt_account(&plan.authority);
    let stolen = f.with(&receipt, |a| a.data[32..64].copy_from_slice(&[9; 32]));
    assert!(matches!(plan.value(&stolen), Err(Error::Layout { .. })));
}
