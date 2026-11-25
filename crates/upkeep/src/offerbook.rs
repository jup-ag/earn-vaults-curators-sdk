//! Offerbook positions: escrow, escrow yield, loans the vault lent, and loans
//! it borrowed, netted into one mark.
//!
//! The vault authority is the vault's Offerbook identity: it owns the Offerbook
//! `User` (`["user", authority]`), whose ATAs are the escrow, and it is the
//! `lender` or `borrower` on every loan. Each holding counts once:
//!
//! | Holding | Side | Amount |
//! |---|---|---|
//! | Escrow cash | asset | `User` ATA balance per row mint |
//! | Escrow yield | asset | `User` jlToken receipts × stored exchange price |
//! | Lent loan | asset | outstanding principal + interest accrued this period |
//! | Borrowed loan, collateral | asset | the loan vault's balance |
//! | Borrowed loan, debt | liability | outstanding principal + the period's full interest |
//!
//! Open offers hold no funds outside the escrow, so they add nothing. Amounts
//! are summed per mint and side, then priced off the vault rows: assets at
//! price − confidence rounded down, liabilities at price + confidence rounded
//! up. Overdue or extended loans, and any mint without a row, fail closed.
//!
//! Loans are found with `getProgramAccounts` ([`loan_filters`]), outside the
//! one-slot read. The `User`'s loan counters bind the two: a loan created after
//! discovery changes them, and [`OfferbookPlan::value`] refuses with
//! [`Error::LoansChanged`].

use {
    crate::{
        Error, Result,
        earn::{PlannedRow, Receipts, check_rows, planned_rows},
        mark::{SlotState, find_slot},
        price::{RowPrice, Side, gate},
        snapshot::{CLOCK_SYSVAR_ID, Snapshot, owned_by, token_balance},
    },
    earn_vaults_sdk::{
        offerbook::{PROGRAM_ID, derive_loan_vault, derive_user},
        pda::{derive_associated_token_account, derive_vault_authority},
        state::{Protocol, Vault},
    },
    offerbook_client::state::{Asset, Loan, LoanStatus, LoanType, User},
    solana_address::Address,
    std::collections::BTreeMap,
};

/// Byte offsets of `Loan.lender`, `Loan.borrower` and `Loan.status`,
/// discriminator included.
pub const LOAN_LENDER_OFFSET: usize = 8;
pub const LOAN_BORROWER_OFFSET: usize = 40;
pub const LOAN_STATUS_OFFSET: usize = 136;
/// `LoanStatus::Active` on the wire.
pub const LOAN_STATUS_ACTIVE: u8 = 0;

/// `getProgramAccounts` memcmp filters, as `(offset, bytes)`, for the active
/// loans the vault lent and the active loans it borrowed. Status is terminal
/// once a loan leaves `Active`, so a loan inactive at discovery never needs
/// valuing; a loan created after discovery moves the `User` counters.
pub fn loan_filters(vault: &Address) -> [[(usize, Vec<u8>); 3]; 2] {
    let authority = derive_vault_authority(vault).to_bytes().to_vec();
    let filter = |offset| {
        [
            (0, Loan::DISCRIMINATOR.to_vec()),
            (offset, authority.clone()),
            (LOAN_STATUS_OFFSET, vec![LOAN_STATUS_ACTIVE]),
        ]
    };
    [filter(LOAN_LENDER_OFFSET), filter(LOAN_BORROWER_OFFSET)]
}

/// The vault's Offerbook `User`, and the configured row mints; fetch these with
/// the vault and pass the result to [`OfferbookPlan::new`].
pub fn discovery_keys(vault_key: &Address, vault: &Vault) -> Vec<Address> {
    let mut keys = vec![*vault_key, derive_user(&derive_vault_authority(vault_key))];
    keys.extend(crate::earn::discovery_keys(vault));
    keys
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferbookPlan {
    pub vault: Address,
    pub authority: Address,
    pub user: Address,
    pub slot: u8,
    pub rows: Vec<PlannedRow>,
    /// Active loans the vault lent, then borrowed, at discovery.
    pub lent: Vec<Address>,
    pub borrowed: Vec<Address>,
    /// `User.total_lender_loans` and `total_borrower_loans` at discovery.
    pub loan_counters: (u64, u64),
}

impl OfferbookPlan {
    /// `snapshot` holds [`discovery_keys`], read no earlier than the
    /// [`loan_filters`] scans that produced `lent` and `borrowed`.
    pub fn new(
        vault: Address,
        snapshot: &Snapshot,
        lent: impl IntoIterator<Item = Address>,
        borrowed: impl IntoIterator<Item = Address>,
    ) -> Result<Self> {
        let state = snapshot.vault(&vault)?;
        let (slot, _) = offerbook_slot(state)?;
        let authority = derive_vault_authority(&vault);
        let user = derive_user(&authority);
        let sorted = |loans: &mut Vec<Address>| {
            loans.sort();
            loans.dedup();
        };
        let (mut lent, mut borrowed): (Vec<_>, Vec<_>) =
            (lent.into_iter().collect(), borrowed.into_iter().collect());
        sorted(&mut lent);
        sorted(&mut borrowed);
        if lent.iter().any(|l| borrowed.contains(l)) {
            return Err(Error::Job("a loan cannot be both lent and borrowed"));
        }
        Ok(Self {
            vault,
            authority,
            user,
            slot,
            rows: planned_rows(snapshot, state)?,
            loan_counters: user_counters(snapshot, &user, &authority)?,
            lent,
            borrowed,
        })
    }

    pub fn escrow_account(&self, row: &PlannedRow) -> Address {
        derive_associated_token_account(&self.user, &row.market.mint, &row.market.token_program)
    }

    /// Every account [`Self::value`] reads, for one `getMultipleAccounts`.
    pub fn keys(&self) -> Vec<Address> {
        let mut keys = vec![self.vault, CLOCK_SYSVAR_ID, self.user];
        for row in &self.rows {
            keys.push(row.market.mint);
            keys.push(self.escrow_account(row));
            keys.extend(row.market.keys(&self.user));
        }
        keys.extend(&self.lent);
        for loan in &self.borrowed {
            keys.push(*loan);
            keys.push(derive_loan_vault(loan));
        }
        keys
    }

    pub fn value(&self, snapshot: &Snapshot) -> Result<OfferbookValuation> {
        let now = snapshot.now()?;
        let vault = snapshot.vault(&self.vault)?;
        let (slot, position) = offerbook_slot(vault)?;
        if slot != self.slot {
            return Err(Error::PlanStale("Offerbook slot"));
        }
        check_rows(snapshot, vault, &self.rows)?;
        if user_counters(snapshot, &self.user, &self.authority)? != self.loan_counters {
            return Err(Error::LoansChanged);
        }

        let mut components = Vec::new();
        for row in &self.rows {
            let escrow = self.escrow_account(row);
            let cash = token_balance(
                snapshot,
                &escrow,
                &row.market.mint,
                &self.user,
                "Offerbook escrow",
            )?;
            components.push(Component {
                kind: Kind::Escrow,
                mint: row.market.mint,
                account: escrow,
                amount: cash,
                side: Side::Asset,
            });
            if let Some(Receipts {
                receipt_account,
                underlying,
                ..
            }) = row.market.redeemable(snapshot, &self.user, row.decimals)?
            {
                components.push(Component {
                    kind: Kind::EscrowYield,
                    mint: row.market.mint,
                    account: receipt_account,
                    amount: underlying,
                    side: Side::Asset,
                });
            }
        }
        let mut expires_at = None::<i64>;
        let loans = self.lent.iter().map(|k| (k, true));
        for (key, lent) in loans.chain(self.borrowed.iter().map(|k| (k, false))) {
            let Some(loan) = self.loan(snapshot, key, lent, now)? else {
                continue;
            };
            let expiry = i64::try_from(loan.expired_at).map_err(|_| Error::Loan {
                loan: *key,
                reason: "expiry out of range",
            })?;
            expires_at = Some(expires_at.map_or(expiry, |e| e.min(expiry)));
            components.extend(self.loan_components(snapshot, key, &loan, now)?);
        }
        components.retain(|c| c.amount != 0);

        let mut sums = BTreeMap::<(Address, bool), u128>::new();
        for c in &components {
            let entry = sums.entry((c.mint, c.side == Side::Liability)).or_default();
            *entry = entry
                .checked_add(u128::from(c.amount))
                .ok_or(Error::Overflow("Offerbook holdings"))?;
        }
        let mut totals = Vec::new();
        let (mut assets, mut liabilities) = (0u64, 0u64);
        for ((mint, liability), amount) in sums {
            let (row, asset) = crate::price::row(vault, &mint).ok_or(Error::Unpriced { mint })?;
            let price = gate(row, asset, now)?;
            let side = if liability {
                Side::Liability
            } else {
                Side::Asset
            };
            let value = price.value(amount, side)?;
            let total = if liability {
                &mut liabilities
            } else {
                &mut assets
            };
            *total = total
                .checked_add(value)
                .ok_or(Error::Overflow("Offerbook value"))?;
            totals.push(MintTotal {
                mint,
                side,
                amount,
                price,
                value,
            });
        }
        let value = assets
            .checked_sub(liabilities)
            .ok_or_else(|| Error::Negative {
                shortfall: liabilities - assets,
            })?;
        let valid_until = totals
            .iter()
            .map(|t| t.price.valid_until)
            .chain(expires_at)
            .min();
        Ok(OfferbookValuation {
            slot: SlotState::of(slot, position),
            now,
            value,
            assets,
            liabilities,
            components,
            totals,
            valid_until,
        })
    }

    /// The loan if it is active and the vault is party to it.
    fn loan(
        &self,
        snapshot: &Snapshot,
        key: &Address,
        lent: bool,
        now: i64,
    ) -> Result<Option<Loan>> {
        let fail = |reason| Error::Loan { loan: *key, reason };
        // Offerbook never closes token loans; a vanished one means the plan
        // is not what is on chain.
        let account = snapshot.get(key)?.ok_or(Error::LoansChanged)?;
        owned_by(account, key, &PROGRAM_ID, "Offerbook loan")?;
        let loan = Loan::from_account_data(&account.data).ok_or(Error::Layout {
            what: "Offerbook loan",
            address: *key,
            reason: "not a Loan account",
        })?;
        if (loan.lender == self.authority) != lent || (loan.borrower == self.authority) == lent {
            return Err(fail("the vault is not the loan's planned side"));
        }
        if loan.status != LoanStatus::Active {
            return Ok(None);
        }
        if loan.loan_type != LoanType::Classic {
            return Err(fail("only classic loans are supported"));
        }
        if loan.duration == 0 {
            return Err(fail("zero duration"));
        }
        if loan.extension_count != 0 {
            return Err(fail("extended loans need manual valuation"));
        }
        if u64::try_from(now).map_or(true, |now| now > loan.expired_at) {
            return Err(fail("past due"));
        }
        if loan.repaid_principal > loan.principal_amount {
            return Err(fail("repaid principal exceeds principal"));
        }
        Ok(Some(loan))
    }

    fn loan_components(
        &self,
        snapshot: &Snapshot,
        key: &Address,
        loan: &Loan,
        now: i64,
    ) -> Result<Vec<Component>> {
        let fail = |reason| Error::Loan { loan: *key, reason };
        let Asset::Token(principal) = &loan.principal else {
            return Err(fail("principal is not a token"));
        };
        let outstanding = u128::from(loan.principal_amount - loan.repaid_principal);
        if loan.lender == self.authority {
            // Offerbook's own accrual: interest over the elapsed share of the
            // current period, which starts `duration` before expiry.
            let duration = u64::from(loan.duration);
            let elapsed = u64::try_from(now)
                .unwrap_or(0)
                .saturating_sub(loan.expired_at.saturating_sub(duration))
                .min(duration);
            let accrued = u128::from(loan.interest) * u128::from(elapsed) / u128::from(duration);
            return Ok(vec![Component {
                kind: Kind::LoanReceivable,
                mint: principal.mint,
                account: *key,
                amount: to_u64(outstanding + accrued)?,
                side: Side::Asset,
            }]);
        }
        let Asset::Token(collateral) = &loan.collateral else {
            return Err(fail("collateral is not a token"));
        };
        // Released collateral is the paid-down share of the booked debt,
        // floored; what is left sits in the loan vault.
        let booked = u128::from(loan.principal_amount) + u128::from(loan.interest);
        let released = (u128::from(loan.collateral_amount) * u128::from(loan.repaid_principal))
            .checked_div(booked)
            .unwrap_or(0);
        let locked = u128::from(loan.collateral_amount) - released;
        let loan_vault = derive_loan_vault(key);
        let held = snapshot
            .require(&loan_vault, "Offerbook loan vault")
            .and_then(|_| {
                token_balance(
                    snapshot,
                    &loan_vault,
                    &collateral.mint,
                    key,
                    "Offerbook loan vault",
                )
            })?;
        if u128::from(held) != locked {
            return Err(fail(
                "loan vault balance differs from the locked collateral",
            ));
        }
        Ok(vec![
            Component {
                kind: Kind::PledgedCollateral,
                mint: collateral.mint,
                account: loan_vault,
                amount: held,
                side: Side::Asset,
            },
            Component {
                kind: Kind::BorrowDebt,
                mint: principal.mint,
                account: *key,
                amount: to_u64(outstanding + u128::from(loan.interest))?,
                side: Side::Liability,
            },
        ])
    }
}

fn to_u64(amount: u128) -> Result<u64> {
    u64::try_from(amount).map_err(|_| Error::Overflow("Offerbook loan amount"))
}

/// The enabled Offerbook slot: counted in AUM, not debt (debt nets into it).
fn offerbook_slot(vault: &Vault) -> Result<(u8, &earn_vaults_sdk::state::PositionConfig)> {
    let (slot, position) = find_slot(vault, Protocol::Offerbook)
        .ok_or(Error::Position("Offerbook", "no enabled slot"))?;
    if position.debt() || !position.required_in_aum() {
        return Err(Error::Position(
            "Offerbook",
            "slot is debt or not counted in AUM",
        ));
    }
    Ok((slot, position))
}

fn user_counters(snapshot: &Snapshot, key: &Address, authority: &Address) -> Result<(u64, u64)> {
    let Some(account) = snapshot.get(key)? else {
        return Ok((0, 0));
    };
    owned_by(account, key, &PROGRAM_ID, "Offerbook User")?;
    let user = User::from_account_data(&account.data).ok_or(Error::Layout {
        what: "Offerbook User",
        address: *key,
        reason: "not a User account",
    })?;
    if user.owner != *authority {
        return Err(Error::Layout {
            what: "Offerbook User",
            address: *key,
            reason: "belongs to another owner",
        });
    }
    Ok((user.total_lender_loans, user.total_borrower_loans))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Escrow,
    EscrowYield,
    LoanReceivable,
    PledgedCollateral,
    BorrowDebt,
}

/// One holding, in raw units of `mint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Component {
    pub kind: Kind,
    pub mint: Address,
    /// The account the amount was read from.
    pub account: Address,
    pub amount: u64,
    pub side: Side,
}

/// The holdings of one mint on one side, priced once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MintTotal {
    pub mint: Address,
    pub side: Side,
    pub amount: u128,
    pub price: RowPrice,
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferbookValuation {
    pub slot: SlotState,
    pub now: i64,
    /// `PriceOpaqueArgs.value`: `assets − liabilities`.
    pub value: u64,
    pub assets: u64,
    pub liabilities: u64,
    pub components: Vec<Component>,
    pub totals: Vec<MintTotal>,
    /// The earliest of every price's gate expiry and every active loan's due time.
    pub valid_until: Option<i64>,
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            Account,
            price::tests::asset,
            snapshot::tests::{clock, token},
        },
        earn_vaults_sdk::{
            state::{
                Account as _, POSITION_FLAG_ENABLED, POSITION_FLAG_INITIALIZED,
                POSITION_FLAG_PRICED, POSITION_FLAG_REQUIRED_IN_AUM,
            },
            zeropod::U32,
        },
        offerbook_client::state::TokenAsset,
    };

    const VAULT: Address = Address::new_from_array([7; 32]);
    const USDC: Address = Address::new_from_array([1; 32]);
    const SOL: Address = Address::new_from_array([2; 32]);
    const NOW: i64 = 1_000_000;
    const TOKEN: Address = crate::snapshot::TOKEN_PROGRAM_ID;

    fn some(owner: Address, data: Vec<u8>) -> Option<Account> {
        Some(Account { owner, data })
    }

    fn vault() -> Vec<u8> {
        let mut data = vec![0u8; Vault::LEN];
        data[0] = Vault::DISCRIMINATOR;
        let v = Vault::read_mut(&mut data).unwrap();
        for (i, (mint, price)) in [(USDC, 100_000_000), (SOL, 10_000_000_000)]
            .into_iter()
            .enumerate()
        {
            v.assets[i] = asset(mint, price, 10_000, -8, NOW);
            v.assets[i].max_staleness_seconds = U32::from(60u32);
        }
        v.positions[0].protocol = Protocol::Offerbook as u8;
        v.positions[0].flags = POSITION_FLAG_ENABLED
            | POSITION_FLAG_REQUIRED_IN_AUM
            | POSITION_FLAG_INITIALIZED
            | POSITION_FLAG_PRICED;
        data
    }

    fn mint() -> Option<Account> {
        let mut data = vec![0u8; 82];
        data[44] = 6;
        data[45] = 1;
        some(TOKEN, data)
    }

    fn user(lent: u64, borrowed: u64) -> Option<Account> {
        let user = User {
            owner: derive_vault_authority(&VAULT),
            referrer: Address::default(),
            referral_count: 0,
            offer_count: 0,
            total_lender_loans: lent,
            total_borrower_loans: borrowed,
            total_lender_liquidations: 0,
            total_borrower_liquidations: 0,
            updated_at: 0,
            created_at: 0,
            bump: 0,
            flags: 0,
            _padding: [0; 6],
            yield_operator: Address::default(),
            origination_fee_bps: 0,
            repay_fee_bps: 0,
            liquidation_fee_bps: 0,
            _padding2: [0; 2],
            _reserved: [0; 24],
        };
        let mut data = User::DISCRIMINATOR.to_vec();
        data.extend(offerbook_client::serialize(&user).unwrap());
        some(PROGRAM_ID, data)
    }

    fn token_asset(mint: Address) -> Asset {
        Asset::Token(TokenAsset {
            mint,
            token_program: TOKEN,
            reserved: [0; 200],
        })
    }

    /// 1,000 USDC for 100 s at 10 USDC interest, against 20 SOL.
    fn loan(lent: bool) -> Loan {
        let authority = derive_vault_authority(&VAULT);
        let other = Address::new_from_array([5; 32]);
        Loan {
            lender: if lent { authority } else { other },
            borrower: if lent { other } else { authority },
            creator: other,
            offer: other,
            status: LoanStatus::Active,
            loan_type: LoanType::Classic,
            _padding: [0; 6],
            fill_index: 0,
            principal: token_asset(USDC),
            _padding1: [0; 7],
            collateral: token_asset(SOL),
            _padding2: [0; 7],
            apy: 0,
            duration: 100,
            principal_amount: 1_000_000_000,
            collateral_amount: 20_000_000,
            interest: 10_000_000,
            created_at: NOW as u64 - 25,
            expired_at: NOW as u64 + 75,
            updated_at: 0,
            bump: 0,
            collateral_account_bump: 0,
            extendable: 0,
            extension_count: 0,
            _padding4: [0; 4],
            repaid_principal: 0,
            _reserved: [0; 40],
        }
    }

    fn encode(loan: &Loan) -> Option<Account> {
        let mut data = Loan::DISCRIMINATOR.to_vec();
        data.extend(offerbook_client::serialize(loan).unwrap());
        some(PROGRAM_ID, data)
    }

    const LENT: Address = Address::new_from_array([11; 32]);
    const BORROWED: Address = Address::new_from_array([12; 32]);

    /// A vault with 250 USDC escrow, one lent loan and one borrowed loan.
    fn accounts(lent: &Loan, borrowed: &Loan) -> Vec<(Address, Option<Account>)> {
        let authority = derive_vault_authority(&VAULT);
        let user_key = derive_user(&authority);
        let mut accounts = vec![
            clock(1, NOW),
            (VAULT, some(earn_vaults_sdk::ID, vault())),
            (USDC, mint()),
            (SOL, mint()),
            (user_key, user(1, 1)),
            (
                derive_associated_token_account(&user_key, &USDC, &TOKEN),
                token(&USDC, &user_key, 250_000_000),
            ),
            (
                derive_associated_token_account(&user_key, &SOL, &TOKEN),
                None,
            ),
            (LENT, encode(lent)),
            (BORROWED, encode(borrowed)),
            (
                derive_loan_vault(&BORROWED),
                token(&SOL, &BORROWED, 20_000_000),
            ),
        ];
        for mint in [USDC, SOL] {
            for key in crate::earn::Market::new(mint, TOKEN).keys(&user_key) {
                accounts.push((key, None));
            }
        }
        accounts.push((derive_loan_vault(&LENT), None));
        accounts
    }

    fn value(accounts: Vec<(Address, Option<Account>)>) -> Result<OfferbookValuation> {
        let snapshot = Snapshot::new(1, accounts)?;
        OfferbookPlan::new(VAULT, &snapshot, [LENT], [BORROWED])?.value(&snapshot)
    }

    #[test]
    fn escrow_and_loans_count_once_and_borrowing_is_subtracted() {
        let v = value(accounts(&loan(true), &loan(false))).unwrap();
        // Assets at $0.9999 / $99.9999 floor, liabilities at $1.0001 ceiling.
        // Escrow 250 + lent 1,000 + 25% of 10 interest = 1,252.5 USDC.
        let usdc_assets = 1_252_500_000u128 * 99_990_000 / 100_000_000;
        // 20 SOL of collateral still locked.
        let sol = 20_000_000u128 * 9_999_990_000 / 100_000_000;
        // Debt: 1,000 + the full 10 interest, rounded up.
        let debt = (1_010_000_000u128 * 100_010_000).div_ceil(100_000_000);
        assert_eq!(u128::from(v.assets), usdc_assets + sol);
        assert_eq!(u128::from(v.liabilities), debt);
        assert_eq!(u128::from(v.value), usdc_assets + sol - debt);
        let kinds: Vec<_> = v.components.iter().map(|c| (c.kind, c.amount)).collect();
        assert_eq!(
            kinds,
            [
                (Kind::Escrow, 250_000_000),
                (Kind::LoanReceivable, 1_002_500_000),
                (Kind::PledgedCollateral, 20_000_000),
                (Kind::BorrowDebt, 1_010_000_000),
            ]
        );
        // The loan's own expiry bounds how long the value holds.
        assert_eq!(v.valid_until, Some(NOW + 60));
    }

    #[test]
    fn paydowns_shrink_principal_and_release_collateral_pro_rata() {
        let mut lent = loan(true);
        // Repaid 400: 600 outstanding; the paid 400 sits in escrow.
        lent.repaid_principal = 400_000_000;
        let v = value(accounts(&lent, &loan(false))).unwrap();
        assert_eq!(v.components[1].amount, 602_500_000);
        let mut borrowed = loan(false);
        // A paydown releases collateral pro rata to the booked debt, floored.
        borrowed.repaid_principal = 505_000_000;
        let mut accounts = accounts(&loan(true), &borrowed);
        let vault_key = derive_loan_vault(&BORROWED);
        accounts.retain(|(k, _)| *k != vault_key);
        accounts.push((vault_key, token(&SOL, &BORROWED, 10_000_000)));
        let v = value(accounts).unwrap();
        assert_eq!(v.components[2].amount, 10_000_000);
        assert_eq!(v.components[3].amount, 505_000_000);
    }

    #[test]
    fn a_loan_vault_that_disagrees_with_the_loan_fails_closed() {
        let mut accounts = accounts(&loan(true), &loan(false));
        let vault_key = derive_loan_vault(&BORROWED);
        accounts.retain(|(k, _)| *k != vault_key);
        accounts.push((vault_key, token(&SOL, &BORROWED, 19_999_999)));
        assert!(matches!(value(accounts.clone()), Err(Error::Loan { .. })));
        accounts.retain(|(k, _)| *k != vault_key);
        accounts.push((vault_key, None));
        assert!(matches!(value(accounts), Err(Error::Missing { .. })));
    }

    #[test]
    fn overdue_extended_and_unknown_loans_never_become_zero() {
        for (mutate, side) in [
            (
                (|l: &mut Loan| l.expired_at = NOW as u64 - 1) as fn(&mut Loan),
                true,
            ),
            (|l: &mut Loan| l.expired_at = NOW as u64 - 1, false),
            (|l: &mut Loan| l.extension_count = 1, true),
            (|l: &mut Loan| l.extension_count = 1, false),
            (|l: &mut Loan| l.duration = 0, true),
            (|l: &mut Loan| l.loan_type = LoanType::Leverage, true),
            (
                |l: &mut Loan| l.principal = token_asset(Address::new_from_array([9; 32])),
                true,
            ),
            (
                |l: &mut Loan| l.collateral = token_asset(Address::new_from_array([9; 32])),
                false,
            ),
            (
                |l: &mut Loan| l.lender = Address::new_from_array([9; 32]),
                true,
            ),
        ] {
            let (mut lent, mut borrowed) = (loan(true), loan(false));
            mutate(if side { &mut lent } else { &mut borrowed });
            let result = value(accounts(&lent, &borrowed));
            assert!(result.is_err(), "{lent:?} {borrowed:?} gave {result:?}");
        }
        // Settled loans are not holdings.
        let mut repaid = loan(true);
        repaid.status = LoanStatus::Repaid;
        let v = value(accounts(&repaid, &loan(false))).unwrap();
        assert!(v.components.iter().all(|c| c.kind != Kind::LoanReceivable));
    }

    #[test]
    fn a_loan_created_after_discovery_stops_the_mark() {
        let accounts = accounts(&loan(true), &loan(false));
        let snapshot = Snapshot::new(1, accounts.clone()).unwrap();
        let plan = OfferbookPlan::new(VAULT, &snapshot, [LENT], [BORROWED]).unwrap();
        let user_key = plan.user;
        let mut later = accounts;
        later.retain(|(k, _)| *k != user_key);
        later.push((user_key, user(2, 1)));
        let later = Snapshot::new(1, later).unwrap();
        assert_eq!(plan.value(&later), Err(Error::LoansChanged));
        // A planned loan that vanished also means the plan is wrong.
        let mut gone = self::accounts(&loan(true), &loan(false));
        gone.retain(|(k, _)| *k != LENT);
        gone.push((LENT, None));
        assert_eq!(
            plan.value(&Snapshot::new(1, gone).unwrap()),
            Err(Error::LoansChanged)
        );
    }

    #[test]
    fn a_book_worth_less_than_its_debt_is_an_error_not_zero() {
        let mut borrowed = loan(false);
        borrowed.principal_amount = 4_000_000_000;
        let mut accounts = accounts(&loan(true), &borrowed);
        accounts.retain(|(k, _)| *k != LENT);
        let mut repaid = loan(true);
        repaid.status = LoanStatus::Repaid;
        accounts.push((LENT, encode(&repaid)));
        assert!(matches!(value(accounts), Err(Error::Negative { .. })));
    }

    #[test]
    fn a_stale_price_fails_the_whole_mark() {
        let mut accounts = accounts(&loan(true), &loan(false));
        accounts.retain(|(k, _)| *k != crate::snapshot::CLOCK_SYSVAR_ID);
        accounts.push(clock(1, NOW + 61));
        assert!(matches!(
            value(accounts),
            Err(Error::Price {
                reason: crate::PriceFault::Stale,
                ..
            })
        ));
    }
}
