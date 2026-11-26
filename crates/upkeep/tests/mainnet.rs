//! Valuations over vault states recorded from mainnet, each read with one
//! `getMultipleAccounts` at one slot. `expected` holds this crate's values and,
//! for parity, the values the existing implementations computed from the same
//! bytes. See `fixtures/README.md` for where each number came from.

use {
    base64::{Engine, engine::general_purpose::STANDARD},
    earn_vaults_upkeep::{
        Account, Error, PriceFault, Snapshot,
        earn::{EarnPlan, EarnScope, Market},
        jobs::{VaultKeys, attest},
        mark::{Grant, MarkDecision},
        offerbook::{Kind, LoanScan, OfferbookPlan},
        snapshot::CLOCK_SYSVAR_ID,
    },
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
        // The scans ran just before the read and the fixture's counters equal
        // the loan list's, so its one slot stands in for both sides of them.
        let scan = LoanScan {
            lent: self.lent.clone(),
            borrowed: self.borrowed.clone(),
            first_slot: self.slot,
            last_slot: self.slot,
        };
        OfferbookPlan::new(self.vault, &self.snapshot(), scan).unwrap()
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
    // Marked seconds before, from the same exchange and row prices.
    assert_eq!(earn.value, f.expect("earn", "onchain_mark"));
    assert_eq!(earn.markets.len(), 5);
    assert!(
        OfferbookPlan::new(f.vault, &snapshot, LoanScan::default()).is_err(),
        "no Offerbook slot"
    );

    // mark → ValidateAum with every priced row's custody in order.
    let signer = Address::new_from_array([1; 32]);
    let mark = earn
        .slot
        .decide(earn.value, earn.now)
        .mark(signer, Grant::JupiterLend)
        .expect("equal to the last mark, so inside any drift budget");
    assert!(!mark.bootstrap);
    let state = snapshot.vault(&f.vault).unwrap();
    let keys = VaultKeys::new(f.vault, state, &plan.rows).unwrap();
    assert_eq!(keys.custody.len(), 5);
    for custody in &keys.custody {
        assert!(
            snapshot.get(&custody.account).unwrap().is_some(),
            "custody ATA exists"
        );
    }
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
    // The market maker's managed mark counts escrow yield too, but no repay
    // fee; the API and the borrowing path leave the jlUSDC out. The gap
    // between those two is the escrow yield, give or take its price.
    let managed = f.expect("offerbook", "mm_managed");
    let api = f.expect("offerbook", "curator_api");
    let escrow_yield: u64 = book
        .components
        .iter()
        .filter(|c| c.kind == Kind::EscrowYield)
        .map(|c| c.amount)
        .sum();
    assert!(
        bps(managed - api, escrow_yield) < 5.0,
        "{managed} {api} {escrow_yield}"
    );
    // Below the managed mark by the repay fee alone: well under 1 bp.
    assert!(book.value < managed && bps(book.value, managed) < 1.0);
    assert!(bps(book.value, f.expect("offerbook", "onchain_mark")) < 1.0);
    assert!(matches!(
        book.slot.decide(book.value, book.now),
        MarkDecision::Post {
            bootstrap: false,
            ..
        }
    ));
    assert_eq!(book.liabilities, 0);
    assert_eq!(
        book.repay_fee_bps,
        Some(f.expect("offerbook", "repay_fee_bps") as u16)
    );
}

#[test]
fn re7_lending_values_extended_loans_by_their_current_period() {
    let f = load("re7-nopal-lending");
    let snapshot = f.snapshot();
    let plan = f.offerbook();
    f.covers(&plan.keys());
    let book = plan.value(&snapshot).unwrap();
    assert_eq!(book.value, f.expect("offerbook", "sdk"));
    // The vault's own Offerbook fee profile sets the repay fee.
    assert_eq!(
        book.repay_fee_bps,
        Some(f.expect("offerbook", "repay_fee_bps") as u16)
    );
    // The API refuses extended loans. With the extension count cleared, it and
    // the market maker accrue from `created_at`, which books the whole period's
    // interest again, and take no repay fee: both read high.
    let other = f.expect("offerbook", "curator_api_if_extended_allowed");
    assert_eq!(other, f.expect("offerbook", "mm_borrowing"));
    assert!(other > book.value);
    // Its on-chain mark is 16 h old, so no parity there; the move to this
    // value is 3 bps, inside the slot's drift budget.
    assert!(bps(book.value, f.expect("offerbook", "onchain_mark")) < 5.0);
    assert!(matches!(
        book.slot.decide(book.value, book.now),
        MarkDecision::Post {
            bootstrap: false,
            ..
        }
    ));

    // Its Earn slot holds nothing: zero, and since it was marked before, a
    // zero mark is posted rather than skipped.
    let earn = EarnPlan::new(f.vault, &snapshot, EarnScope::LenderOnly)
        .unwrap()
        .value(&snapshot)
        .unwrap();
    assert_eq!(earn.value, f.expect("earn", "optimizer"));
    // nOPAL has no Earn market. SOL sent to its would-be receipt address
    // leaves an empty system account, which must not block the mark.
    let plan = EarnPlan::new(f.vault, &snapshot, EarnScope::LenderOnly).unwrap();
    let no_market = plan
        .rows
        .iter()
        .map(Market::of)
        .find(|m| snapshot.get(&m.lending).unwrap().is_none())
        .unwrap();
    let receipt = no_market.receipt_account(&plan.authority);
    let mut accounts = f.accounts.clone();
    accounts.retain(|(k, _)| *k != receipt);
    accounts.push((
        receipt,
        Some(Account {
            owner: earn_vaults_upkeep::snapshot::SYSTEM_PROGRAM_ID,
            data: vec![],
        }),
    ));
    let griefed = plan
        .value(&Snapshot::new(f.slot, accounts).unwrap())
        .unwrap();
    assert_eq!(griefed.value, earn.value);
    // Its last mark is 1 micro-unit. 2% of 1 rounds to nothing, so the only
    // value the program accepts is 1 again: no mark is built for 0.
    assert_eq!(f.expect("earn", "onchain_mark"), 1);
    assert_eq!(
        earn.slot.decide(earn.value, earn.now),
        MarkDecision::OutOfBudget {
            slot: earn.slot.index,
            target: 0,
            nearest: 1
        }
    );
}

#[test]
fn re7_looping_nets_collateral_against_full_debt() {
    let f = load("re7-nopal-looping");
    let snapshot = f.snapshot();
    let plan = f.offerbook();
    f.covers(&plan.keys());
    let book = plan.value(&snapshot).unwrap();
    assert_eq!(book.value, f.expect("offerbook", "sdk"));
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
    assert_eq!(book.repay_fee_bps, None, "a borrower pays no repay fee");
    // Its on-chain mark is 16 h old and 24 bps higher; the slot's 10 bps cap
    // allows one window's move, so the keeper is told, not handed a mark the
    // program refuses.
    let last = f.expect("offerbook", "onchain_mark");
    let MarkDecision::OutOfBudget {
        target, nearest, ..
    } = book.slot.decide(book.value, book.now)
    else {
        panic!("a 24 bps move is outside a 10 bps budget");
    };
    assert_eq!(target, book.value);
    assert!(nearest > target && nearest < last);
    assert!(
        (bps(nearest, last) - 10.0).abs() < 0.01,
        "one window: 10 bps"
    );
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
    let lending = Market::of(&plan.rows[0]).lending;
    let short = f.with(&lending, |a| a.data.truncate(100));
    assert!(matches!(plan.value(&short), Err(Error::Layout { .. })));
    let unfetched = Snapshot::new(
        f.slot,
        f.accounts.iter().filter(|(k, _)| *k != lending).cloned(),
    )
    .unwrap();
    assert_eq!(plan.value(&unfetched), Err(Error::NotFetched(lending)));
    // A receipt account that belongs to someone else.
    let receipt = Market::of(&plan.rows[0]).receipt_account(&plan.authority);
    let stolen = f.with(&receipt, |a| a.data[32..64].copy_from_slice(&[9; 32]));
    assert!(matches!(plan.value(&stolen), Err(Error::Layout { .. })));
}

#[test]
fn earn_rejects_a_zero_exchange_price() {
    let f = load("bitwise-juplend-optimizer");
    let plan = EarnPlan::new(f.vault, &f.snapshot(), EarnScope::LenderOnly).unwrap();
    let lending = Market::of(&plan.rows[0]).lending;
    // Receipts × 0 would read as a silent zero; it must be an error.
    let zero_price = f.with(&lending, |a| {
        a.data[115..123].copy_from_slice(&0u64.to_le_bytes())
    });
    assert_eq!(
        plan.value(&zero_price),
        Err(Error::Layout {
            what: "Earn Lending",
            address: lending,
            reason: "zero exchange price",
        })
    );
}
