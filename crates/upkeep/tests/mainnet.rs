//! Valuations over vault states recorded from mainnet, each read with one
//! `getMultipleAccounts` at one slot. `expected` holds this crate's values and,
//! for parity, the values the existing implementations computed from the same
//! bytes. See `fixtures/README.md` for where each number came from.

use {
    base64::{Engine, engine::general_purpose::STANDARD},
    earn_vaults_upkeep::{
        Account, Error, PriceFault, Snapshot,
        earn::EarnPlan,
        fluid::{ExchangePriceRead, FluidPlan, PositionState},
        jobs::{VaultKeys, attest},
        listing::{Ignored, TokenListing, derive_lending_admin},
        mark::{Mark, MarkDecision},
        offerbook::OfferbookPlan,
        price::Side,
        snapshot::{CLOCK_SYSVAR_ID, TOKEN_2022_PROGRAM_ID, TOKEN_PROGRAM_ID},
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
    /// Recorded with the accounts at the same slot; `None` in fixtures that
    /// predate listings.
    listings: Option<Vec<TokenListing>>,
    exchange_prices: Vec<ExchangePriceRead>,
    /// The Fluid `Position` scan results, and the candidates it found nothing for.
    positions: Vec<(Address, Account)>,
    unrelated: Vec<Address>,
    expected: Value,
}

fn addr(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}

fn accounts(v: &Value) -> Vec<(Address, Option<Account>)> {
    v.as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|a| {
            let account = a["account"].as_object().map(|o| Account {
                owner: addr(&o["owner"]),
                data: STANDARD.decode(o["data"].as_str().unwrap()).unwrap(),
            });
            (addr(&a["address"]), account)
        })
        .collect()
}

fn load(name: &str) -> Fixture {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let f: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let keys = |k: &str| {
        f[k].as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(addr)
            .collect()
    };
    let present = |v: Vec<(Address, Option<Account>)>| {
        v.into_iter()
            .map(|(k, a)| (k, a.unwrap()))
            .collect::<Vec<_>>()
    };
    Fixture {
        vault: addr(&f["vault"]),
        slot: f["slot"].as_u64().unwrap(),
        accounts: accounts(&f["accounts"]),
        lent: keys("lent"),
        borrowed: keys("borrowed"),
        listings: f["listings"].as_array().map(|ls| {
            ls.iter()
                .map(|l| TokenListing {
                    program: addr(&l["program"]),
                    slot: l["slot"].as_u64().unwrap(),
                    accounts: present(accounts(&l["accounts"])),
                })
                .collect()
        }),
        exchange_prices: f["exchange_prices"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|e| ExchangePriceRead {
                vault_id: e["vault_id"].as_u64().unwrap() as u16,
                slot: e["slot"].as_u64().unwrap(),
                program: addr(&e["program"]),
                data: STANDARD.decode(e["data"].as_str().unwrap()).unwrap(),
            })
            .collect(),
        positions: present(accounts(&f["positions"])),
        unrelated: keys("unrelated"),
        expected: f["expected"].clone(),
    }
}

/// The token accounts in `accounts` that `vault`'s authority owns, as both
/// listings at `slot`. Fixtures recorded before listings existed use this:
/// it holds the custody and receipt accounts the plans fetched, so it shows
/// the fold, not that the authority held nothing else.
fn rebuilt_listings(
    vault: &Address,
    slot: u64,
    accounts: &[(Address, Option<Account>)],
) -> Vec<TokenListing> {
    let authority = earn_vaults_sdk::pda::derive_vault_authority(vault);
    [TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID]
        .into_iter()
        .map(|program| TokenListing {
            program,
            slot,
            accounts: accounts
                .iter()
                .filter_map(|(k, a)| Some((*k, a.clone()?)))
                .filter(|(_, a)| {
                    a.owner == program
                        && a.data.len() >= 165
                        && a.data[32..64] == authority.to_bytes()
                })
                .collect(),
        })
        .collect()
}

impl Fixture {
    fn listings(&self) -> Vec<TokenListing> {
        self.listings
            .clone()
            .unwrap_or_else(|| rebuilt_listings(&self.vault, self.slot, &self.accounts))
    }

    fn fluid(&self) -> FluidPlan {
        FluidPlan::new(
            self.vault,
            &self.snapshot(),
            self.positions.clone(),
            self.unrelated.clone(),
        )
        .unwrap()
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot::new(self.slot, self.accounts.clone()).unwrap()
    }

    fn expect(&self, position: &str, key: &str) -> u64 {
        self.expected[position][key].as_u64().unwrap()
    }

    fn offerbook(&self) -> OfferbookPlan {
        // The fixture's counters equal the loan list's, so its one slot stands
        // in for a read taken before the scans.
        OfferbookPlan::new(
            self.vault,
            &self.snapshot(),
            self.slot,
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
    let plan = EarnPlan::new(f.vault, &snapshot, []).unwrap();
    f.covers(&plan.keys());
    let earn = plan.value(&snapshot, &f.listings()).unwrap();
    assert_eq!(earn.value, f.expect("earn", "sdk"));
    assert_eq!(earn.value, f.expect("earn", "optimizer"));
    // Marked seconds before, from the same exchange and row prices.
    assert_eq!(earn.value, f.expect("earn", "onchain_mark"));
    assert_eq!(earn.markets.len(), 5);
    assert!(
        OfferbookPlan::new(f.vault, &snapshot, f.slot, [], []).is_err(),
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
        EarnPlan::new(f.vault, &snapshot, []),
        Err(Error::Position(..))
    ));
    // A listing with no position NFT proves the slot is Earn receipts only.
    let plan = f.fluid();
    assert!(plan.positions.is_empty());
    f.covers(&plan.keys());
    let earn = plan.value(&snapshot, &f.listings(), &[]).unwrap();
    assert!(earn.positions.is_empty() && earn.liabilities == 0);
    assert_eq!(earn.value, earn.earn_value);
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
    // fee; the API and the borrowing path leave the jlUSDC out.
    assert!(book.value < f.expect("offerbook", "mm_managed"));
    assert!(book.value > f.expect("offerbook", "curator_api"));
    assert!(bps(book.value, f.expect("offerbook", "onchain_mark")) < 1.0);
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

    // Its Earn slot holds nothing: zero, and since it was marked before, a
    // zero mark is posted rather than skipped.
    let plan = EarnPlan::new(f.vault, &snapshot, []).unwrap();
    let earn = plan.value(&snapshot, &f.listings()).unwrap();
    assert_eq!(earn.value, f.expect("earn", "optimizer"));
    // nOPAL has no Earn market. SOL sent to its would-be receipt address
    // leaves an empty system account, which must not block the mark.
    let no_market = plan
        .rows
        .iter()
        .find(|r| snapshot.get(&r.market.lending).unwrap().is_none())
        .unwrap();
    let receipt = no_market.market.receipt_account(&plan.authority);
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
        .value(&Snapshot::new(f.slot, accounts).unwrap(), &f.listings())
        .unwrap();
    assert_eq!(griefed.value, earn.value);
    assert_eq!(
        earn.slot.decide(earn.value),
        MarkDecision::Post {
            value: 0,
            bootstrap: false
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
}

#[test]
fn a_stale_clock_or_tampered_account_never_values() {
    let f = load("bitwise-juplend-optimizer");
    let plan = EarnPlan::new(f.vault, &f.snapshot(), []).unwrap();
    let value = |snapshot: &Snapshot| {
        let accounts: Vec<_> = plan
            .keys()
            .iter()
            .filter_map(|k| Some((*k, snapshot.get(k).ok()??.clone())))
            .map(|(k, a)| (k, Some(a)))
            .collect();
        plan.value(snapshot, &rebuilt_listings(&f.vault, f.slot, &accounts))
    };
    // Move the clock past the USDC row's 60 s window.
    let late = f.with(&CLOCK_SYSVAR_ID, |a| {
        let now = i64::from_le_bytes(a.data[32..40].try_into().unwrap());
        a.data[32..40].copy_from_slice(&(now + 61).to_le_bytes());
    });
    assert!(matches!(
        value(&late),
        Err(Error::Price {
            reason: PriceFault::Stale,
            ..
        })
    ));
    let foreign = f.with(&CLOCK_SYSVAR_ID, |a| {
        a.owner = Address::new_from_array([1; 32])
    });
    assert!(matches!(value(&foreign), Err(Error::WrongOwner { .. })));
    // A Lending account that is missing, or of another layout.
    let lending = plan.rows[0].market.lending;
    let short = f.with(&lending, |a| a.data.truncate(100));
    assert!(matches!(value(&short), Err(Error::Layout { .. })));
    let unfetched = Snapshot::new(
        f.slot,
        f.accounts.iter().filter(|(k, _)| *k != lending).cloned(),
    )
    .unwrap();
    assert_eq!(value(&unfetched), Err(Error::NotFetched(lending)));
    // A receipt account that belongs to someone else: the listing no longer
    // shows it, and the plan's own read refuses it.
    let receipt = plan.rows[0].market.receipt_account(&plan.authority);
    let stolen = f.with(&receipt, |a| a.data[32..64].copy_from_slice(&[9; 32]));
    assert!(matches!(value(&stolen), Err(Error::Layout { .. })));
}

#[test]
fn a_lender_only_slot_refuses_receipts_with_no_row_and_ignores_strays() {
    let f = load("bitwise-juplend-optimizer");
    let authority = earn_vaults_sdk::pda::derive_vault_authority(&f.vault);
    // Two mints discovery found no Fluid Position for: one minted by Earn's
    // `lending_admin` (a jlToken whose row is gone), one by anyone else.
    let orphan = Address::new_from_array([0xa1; 32]);
    let stray = Address::new_from_array([0xa2; 32]);
    let mint_by = |authority: &Address| {
        let mut data = vec![0u8; 82];
        data[..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..36].copy_from_slice(authority.as_array());
        data[36..44].copy_from_slice(&1_000u64.to_le_bytes());
        data[44] = 6;
        data[45] = 1;
        Account {
            owner: TOKEN_PROGRAM_ID,
            data,
        }
    };
    let holding = |mint: &Address| {
        let mut data = vec![0u8; 165];
        data[..32].copy_from_slice(mint.as_array());
        data[32..64].copy_from_slice(authority.as_array());
        data[64..72].copy_from_slice(&7u64.to_le_bytes());
        data[108] = 1;
        Account {
            owner: TOKEN_PROGRAM_ID,
            data,
        }
    };
    let mut accounts = f.accounts.clone();
    accounts.push((orphan, Some(mint_by(&derive_lending_admin()))));
    accounts.push((stray, Some(mint_by(&Address::new_from_array([3; 32])))));
    let snapshot = Snapshot::new(f.slot, accounts).unwrap();
    let plan = EarnPlan::new(f.vault, &snapshot, [orphan, stray]).unwrap();
    let listed = |mints: &[Address]| {
        let mut listings = f.listings();
        for (i, mint) in mints.iter().enumerate() {
            listings[0]
                .accounts
                .push((Address::new_from_array([0xb0 + i as u8; 32]), holding(mint)));
        }
        listings
    };
    let clean = plan.value(&snapshot, &listed(&[])).unwrap();
    assert_eq!(clean.value, f.expect("earn", "sdk"));
    // A stray airdrop is reported, not valued, and does not block the mark.
    let with_stray = plan.value(&snapshot, &listed(&[stray])).unwrap();
    assert_eq!(with_stray.value, clean.value);
    assert_eq!(
        with_stray.ignored,
        vec![Ignored {
            account: Address::new_from_array([0xb0; 32]),
            mint: stray,
            amount: 7
        }]
    );
    // Earn receipts for a market with no configured row would be left out.
    assert!(matches!(
        plan.value(&snapshot, &listed(&[orphan])),
        Err(Error::Unattributed { mint, .. }) if mint == orphan
    ));
    // A mint discovery never classified: discover again.
    let unknown = Address::new_from_array([0xa3; 32]);
    assert!(matches!(
        plan.value(&snapshot, &listed(&[unknown])),
        Err(Error::PlanStale(_))
    ));
    // A listing from another slot proves nothing about this one.
    let mut late = listed(&[]);
    late[1].slot += 1;
    assert!(matches!(
        plan.value(&snapshot, &late),
        Err(Error::ReadSlot { .. })
    ));
    assert!(matches!(
        plan.value(&snapshot, &late[..1]),
        Err(Error::Listing(_))
    ));
}

#[test]
fn susdai_multiply_values_its_fluid_loop_at_the_report_formula() {
    let f = load("bitwise-susdai-multiply");
    let snapshot = f.snapshot();
    let listings = f.listings();
    // Recorded together: one slot for the accounts, both listings and the
    // simulated exchange prices.
    assert!(listings.iter().all(|l| l.slot == f.slot));
    assert!(f.exchange_prices.iter().all(|e| e.slot == f.slot));
    let plan = f.fluid();
    f.covers(&plan.keys());
    assert_eq!(plan.positions.len(), 1);
    assert_eq!(plan.exchange_price_instructions().len(), 1);
    let v = plan
        .value(&snapshot, &listings, &f.exchange_prices)
        .unwrap();
    assert_eq!(v.value, f.expect("jupiter_lend", "sdk"));
    // The report's formula, run over the same bytes outside this crate,
    // projects the same indices as the program's own view.
    assert_eq!(v.value, f.expect("jupiter_lend", "report_formula"));
    assert_eq!(v.earn_value, 0);
    let p = &v.positions[0];
    let PositionState::Borrowing { gross_debt_raw, .. } = p.state else {
        panic!("the loop borrows");
    };
    assert_eq!(
        gross_debt_raw,
        u128::from(f.expect("jupiter_lend", "gross_debt_raw"))
    );
    assert_eq!(p.collateral, f.expect("jupiter_lend", "collateral"));
    // The debt is rounded up: floor would read one raw unit lower.
    assert_eq!(p.debt, f.expect("jupiter_lend", "debt"));
    assert_eq!(v.assets - v.liabilities, v.value);

    // The mark posted the same second, off the same cached rows, prices the
    // USDC debt at price − confidence instead of price + confidence, and
    // carries about four seconds less accrual on it.
    let debt = v.totals.iter().find(|t| t.side == Side::Liability).unwrap();
    let at_floor = debt.price.value(debt.amount, Side::Asset).unwrap();
    let mark = f.expect("jupiter_lend", "onchain_mark");
    let explained = v.assets - at_floor;
    assert!(explained > v.value);
    assert!(explained.abs_diff(mark) <= 300, "{explained} vs {mark}");

    // Without projection the stored borrow index understates the debt.
    let mut stale = f.exchange_prices.clone();
    let stored = earn_vaults_upkeep::fluid::derive_vault_state(stale[0].vault_id);
    let state = snapshot.get(&stored).unwrap().unwrap();
    stale[0].data[48..64].copy_from_slice(
        &u128::from(u64::from_le_bytes(state.data[107..115].try_into().unwrap())).to_le_bytes(),
    );
    assert_eq!(
        plan.value(&snapshot, &listings, &stale).unwrap().value,
        f.expect("jupiter_lend", "stored_indices")
    );
}

#[test]
fn fluid_positions_outside_the_supported_states_fail_closed() {
    let f = load("bitwise-susdai-multiply");
    let plan = f.fluid();
    let listings = f.listings();
    let prices = &f.exchange_prices;
    let p = plan.positions[0];
    let tick = earn_vaults_upkeep::fluid::derive_tick(p.vault_id, p.tick.unwrap());
    let config = plan.markets[0].config;
    let fluid_error = |r: Result<_, Error>| matches!(r, Err(Error::Fluid { .. }));

    // Liquidation history on the position's tick.
    let liquidated = f.with(&tick, |a| a.data[14] = 1);
    assert!(fluid_error(plan.value(&liquidated, &listings, prices)));
    let reused = f.with(&tick, |a| {
        a.data[15..19].copy_from_slice(&2u32.to_le_bytes())
    });
    assert!(fluid_error(plan.value(&reused, &listings, prices)));

    // Smart vaults hold DEX shares.
    for vault_type in [2u8, 3, 4, 9] {
        let smart = f.with(&config, |a| a.data[25] = vault_type);
        assert!(fluid_error(plan.value(&smart, &listings, prices)));
        assert!(FluidPlan::new(f.vault, &smart, f.positions.clone(), []).is_err());
    }

    // A listing or simulation from another slot.
    let snapshot = f.snapshot();
    let mut late = listings.clone();
    late[0].slot -= 1;
    assert!(matches!(
        plan.value(&snapshot, &late, prices),
        Err(Error::ReadSlot { .. })
    ));
    let mut sim = prices.clone();
    sim[0].slot += 1;
    assert!(matches!(
        plan.value(&snapshot, &listings, &sim),
        Err(Error::ReadSlot { .. })
    ));
    // Return data from another program, or below the reserve's own index.
    let mut other = prices.clone();
    other[0].program = Address::new_from_array([5; 32]);
    assert!(fluid_error(plan.value(&snapshot, &listings, &other)));
    let mut low = prices.clone();
    low[0].data[16..32].copy_from_slice(&1u128.to_le_bytes());
    assert!(fluid_error(plan.value(&snapshot, &listings, &low)));
    assert!(fluid_error(plan.value(&snapshot, &listings, &[])));

    // An NFT the plan does not know, and a planned NFT that left.
    let mut unknown = listings.clone();
    let (nft_key, mut nft) = unknown[0]
        .accounts
        .iter()
        .find(|(_, a)| a.data[..32] == p.nft_mint.to_bytes())
        .cloned()
        .unwrap();
    nft.data[..32].copy_from_slice(&[0xc1; 32]);
    unknown[0]
        .accounts
        .push((Address::new_from_array([0xc2; 32]), nft));
    assert!(matches!(
        plan.value(&snapshot, &unknown, prices),
        Err(Error::PlanStale(_))
    ));
    let mut gone = listings.clone();
    gone[0].accounts.retain(|(k, _)| *k != nft_key);
    assert!(matches!(
        plan.value(&snapshot, &gone, prices),
        Err(Error::PlanStale(_))
    ));
    // The NFT delegated to a third party.
    let mut delegated = listings.clone();
    for (k, a) in &mut delegated[0].accounts {
        if *k == nft_key {
            a.data[72..76].copy_from_slice(&1u32.to_le_bytes());
            a.data[76..108].copy_from_slice(&[7; 32]);
        }
    }
    assert!(fluid_error(plan.value(&snapshot, &delegated, prices)));
}

#[test]
fn earn_rejects_a_zero_exchange_price() {
    let f = load("bitwise-juplend-optimizer");
    let plan = EarnPlan::new(f.vault, &f.snapshot(), []).unwrap();
    let lending = plan.rows[0].market.lending;
    // Receipts × 0 would read as a silent zero; it must be an error.
    let zero_price = f.with(&lending, |a| {
        a.data[115..123].copy_from_slice(&0u64.to_le_bytes())
    });
    assert_eq!(
        plan.value(&zero_price, &f.listings()),
        Err(Error::Layout {
            what: "Earn Lending",
            address: lending,
            reason: "zero exchange price",
        })
    );
}
