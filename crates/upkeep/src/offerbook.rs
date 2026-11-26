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
//! | Lent loan | asset | outstanding principal + interest accrued this period, less Offerbook's repay fee |
//! | Borrowed loan, collateral | asset | the loan vault's balance |
//! | Borrowed loan, debt | liability | outstanding principal + the period's full interest |
//!
//! Open offers hold no funds outside the escrow, so they add nothing. Amounts
//! are summed per mint and side, then priced off the vault rows: assets at
//! price − confidence rounded down, liabilities at price + confidence rounded
//! up. Interest follows Offerbook's own accounting: it accrues from the start
//! of the current period (`expired_at − duration`), so an extended loan counts
//! only the period it is in; the borrower paid the earlier ones at extension.
//! Overdue loans, and any mint without a row, fail closed.
//!
//! A lent loan's collateral mint must be a priced row too, while the loan is
//! active or claimed: a claim moves that collateral into the escrow, where
//! only row mints are read. shortcut: a loan claimed before discovery is not
//! in the plan, so collateral with no row in escrow reads as nothing; the
//! curator must have listed the mint to value the loan while it was active.
//! Close it with a listing of the `User`'s token accounts (as #24 does for
//! the authority).
//!
//! Loans are found with `getProgramAccounts` ([`loan_filters`]), outside the
//! one-slot read, and the reads are ordered so the scans prove the loan list:
//! the discovery read (with the `User` loan counters) comes before the scans,
//! and the valuation read after them. A loan created after discovery moves the
//! counters, and [`OfferbookPlan::value`] refuses with [`Error::LoansChanged`].
//! A loan that settled before the scans is excluded by them and settled at the
//! valuation slot too.

use {
    crate::{
        Error, LoanFault, Result,
        earn::{Market, Receipts},
        mark::{SlotState, find_slot, slot_state},
        price::{RowPrice, Side, gate},
        rows::{self, AssetRow},
        snapshot::{CLOCK_SYSVAR_ID, Snapshot, owned_by, token_balance},
    },
    earn_vaults_sdk::{
        offerbook::{PROGRAM_ID, derive_config, derive_loan_vault, derive_user},
        pda::{derive_associated_token_account, derive_share_mint, derive_vault_authority},
        state::{Protocol, Vault},
    },
    offerbook_client::state::{Asset, Config, Loan, LoanStatus, LoanType, User},
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

/// One `getProgramAccounts` memcmp filter set, as `(offset, bytes)`.
pub type LoanFilter = [(usize, Vec<u8>); 3];

/// The memcmp filters for the vault's active loans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoanFilters {
    /// Active loans the vault lent.
    pub lent: LoanFilter,
    /// Active loans the vault borrowed.
    pub borrowed: LoanFilter,
}

/// `getProgramAccounts` filters for the active loans the vault lent and
/// borrowed. Status is terminal once a loan leaves `Active`, so a loan
/// inactive at the scan never needs valuing; a loan created after discovery
/// moves the `User` counters.
pub fn loan_filters(vault: &Address) -> LoanFilters {
    let authority = derive_vault_authority(vault).to_bytes().to_vec();
    let filter = |offset| {
        [
            (0, Loan::DISCRIMINATOR.to_vec()),
            (offset, authority.clone()),
            (LOAN_STATUS_OFFSET, vec![LOAN_STATUS_ACTIVE]),
        ]
    };
    LoanFilters {
        lent: filter(LOAN_LENDER_OFFSET),
        borrowed: filter(LOAN_BORROWER_OFFSET),
    }
}

/// What the two [`loan_filters`] scans returned, with their context slots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoanScan {
    pub lent: Vec<Address>,
    pub borrowed: Vec<Address>,
    /// The earlier of the two scans' context slots.
    pub first_slot: u64,
    /// The later of the two.
    pub last_slot: u64,
}

/// The vault's Offerbook `User`, plus [`rows::discovery_keys`]; fetch these
/// before the loan scans and pass the result to [`OfferbookPlan::new`].
pub fn discovery_keys(vault_key: &Address, vault: &Vault) -> Vec<Address> {
    let mut keys = rows::discovery_keys(vault_key, vault);
    keys.push(derive_user(&derive_vault_authority(vault_key)));
    keys
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferbookPlan {
    pub vault: Address,
    pub authority: Address,
    pub user: Address,
    pub slot: u8,
    pub rows: Vec<AssetRow>,
    /// Active loans the vault lent, then borrowed, at the scans.
    pub lent: Vec<Address>,
    pub borrowed: Vec<Address>,
    /// `User.total_lender_loans` and `total_borrower_loans` at discovery.
    pub loan_counters: (u64, u64),
    /// The later scan's slot: a valuation must be read at or after it.
    pub scanned_at: u64,
}

impl OfferbookPlan {
    /// `snapshot` holds [`discovery_keys`] and must be read **before** the
    /// [`loan_filters`] scans that produced `scan`. Its `User` counters are
    /// then no newer than the loan list, so a loan created in between moves
    /// the counters and [`Self::value`] refuses until the next discovery.
    /// Read after the scans, the counters would already count a loan the list
    /// lacks, and the omission would never be seen.
    pub fn new(vault: Address, snapshot: &Snapshot, scan: LoanScan) -> Result<Self> {
        if snapshot.slot() > scan.first_slot || scan.first_slot > scan.last_slot {
            return Err(Error::ReadOrder(
                "read the discovery snapshot before the loan scans",
            ));
        }
        let state = snapshot.vault(&vault)?;
        let (slot, _) = offerbook_slot(state)?;
        let authority = derive_vault_authority(&vault);
        let user = derive_user(&authority);
        let LoanScan {
            mut lent,
            mut borrowed,
            last_slot,
            ..
        } = scan;
        for loans in [&mut lent, &mut borrowed] {
            loans.sort();
            loans.dedup();
        }
        if lent.iter().any(|l| borrowed.contains(l)) {
            return Err(Error::Job("a loan cannot be both lent and borrowed"));
        }
        Ok(Self {
            vault,
            authority,
            user,
            slot,
            rows: rows::read(snapshot, state)?,
            loan_counters: counters(read_user(snapshot, &user, &authority)?.as_ref()),
            lent,
            borrowed,
            scanned_at: last_slot,
        })
    }

    pub fn escrow_account(&self, row: &AssetRow) -> Address {
        derive_associated_token_account(&self.user, &row.mint, &row.token_program)
    }

    /// Every account [`Self::value`] reads, for one `getMultipleAccounts`.
    pub fn keys(&self) -> Vec<Address> {
        let mut keys = vec![
            self.vault,
            CLOCK_SYSVAR_ID,
            derive_share_mint(&self.vault),
            self.user,
        ];
        for row in &self.rows {
            keys.push(row.mint);
            keys.push(self.escrow_account(row));
            keys.extend(Market::of(row).keys(&self.user));
        }
        if !self.lent.is_empty() {
            // Its repay fee comes out of the interest the vault earns as lender.
            keys.push(derive_config());
        }
        keys.extend(&self.lent);
        for loan in &self.borrowed {
            keys.push(*loan);
            keys.push(derive_loan_vault(loan));
        }
        keys
    }

    pub fn value(&self, snapshot: &Snapshot) -> Result<OfferbookValuation> {
        if snapshot.slot() < self.scanned_at {
            // A loan that settled after this slot but before the scans is
            // missing from the plan and still active here.
            return Err(Error::ReadOrder("read the valuation after the loan scans"));
        }
        let now = snapshot.now()?;
        let vault = snapshot.vault(&self.vault)?;
        let (slot, _) = offerbook_slot(vault)?;
        if slot != self.slot {
            return Err(Error::PlanStale("Offerbook slot"));
        }
        rows::check(snapshot, vault, &self.rows)?;
        let user = read_user(snapshot, &self.user, &self.authority)?;
        if counters(user.as_ref()) != self.loan_counters {
            return Err(Error::LoansChanged);
        }
        // Lazily: only a lent loan needs the fee, and only then must Config exist.
        let mut repay_fee_bps = None;

        let mut components = Vec::new();
        for row in &self.rows {
            let escrow = self.escrow_account(row);
            let cash = token_balance(snapshot, &escrow, &row.mint, &self.user, "Offerbook escrow")?;
            components.push(Component {
                kind: Kind::Escrow,
                mint: row.mint,
                account: escrow,
                amount: cash,
                side: Side::Asset,
            });
            if let Some(Receipts {
                receipt_account,
                underlying,
                ..
            }) = Market::of(row).redeemable(snapshot, &self.user, row.decimals)?
            {
                components.push(Component {
                    kind: Kind::EscrowYield,
                    mint: row.mint,
                    account: receipt_account,
                    amount: underlying,
                    side: Side::Asset,
                });
            }
        }
        let mut expires_at = None::<i64>;
        let loans = self.lent.iter().map(|k| (k, true));
        for (key, lent) in loans.chain(self.borrowed.iter().map(|k| (k, false))) {
            let Some(loan) = self.loan(snapshot, vault, key, lent, now)? else {
                continue;
            };
            // Due, and so unvaluable, from `expired_at` on.
            let last_second = i64::try_from(loan.expired_at - 1).map_err(|_| Error::Loan {
                loan: *key,
                reason: LoanFault::ExpiryOutOfRange,
            })?;
            expires_at = Some(expires_at.map_or(last_second, |e| e.min(last_second)));
            let fee_bps = if lent {
                match repay_fee_bps {
                    Some(bps) => bps,
                    None => {
                        *repay_fee_bps.insert(lender_fee_bps(snapshot, &self.user, user.as_ref())?)
                    }
                }
            } else {
                0
            };
            components.extend(self.loan_components(snapshot, key, &loan, now, fee_bps)?);
        }
        components.retain(|c| c.amount != 0);

        let mut sums = BTreeMap::<(Address, Side), u128>::new();
        for c in &components {
            let entry = sums.entry((c.mint, c.side)).or_default();
            *entry = entry
                .checked_add(u128::from(c.amount))
                .ok_or(Error::Overflow("Offerbook holdings"))?;
        }
        let mut totals = Vec::new();
        let (mut assets, mut liabilities) = (0u64, 0u64);
        for ((mint, side), amount) in sums {
            let (row, asset) = crate::price::row(vault, &mint).ok_or(Error::Unpriced { mint })?;
            let price = gate(row, asset, now)?;
            let value = price.value(amount, side)?;
            let total = match side {
                Side::Liability => &mut liabilities,
                Side::Asset => &mut assets,
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
            slot: slot_state(snapshot, &self.vault, vault, slot)?,
            now,
            value,
            assets,
            liabilities,
            components,
            totals,
            valid_until,
            repay_fee_bps,
        })
    }

    /// The loan if it is active and the vault is party to it.
    fn loan(
        &self,
        snapshot: &Snapshot,
        vault: &Vault,
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
            return Err(fail(LoanFault::WrongSide));
        }
        if lent && loan.status != LoanStatus::Repaid {
            // Claimed collateral lands in the escrow, which is read only for
            // row mints: without a row it would vanish from the mark. A repaid
            // loan's collateral went back to the borrower.
            let Asset::Token(collateral) = &loan.collateral else {
                return Err(fail(LoanFault::CollateralNotToken));
            };
            if crate::price::row(vault, &collateral.mint).is_none() {
                return Err(Error::Unpriced {
                    mint: collateral.mint,
                });
            }
        }
        if loan.status != LoanStatus::Active {
            return Ok(None);
        }
        if loan.loan_type != LoanType::Classic {
            return Err(fail(LoanFault::NotClassic));
        }
        if loan.duration == 0 {
            return Err(fail(LoanFault::ZeroDuration));
        }
        // Offerbook's `fail_if_due`: due at `expired_at`, when repayment is
        // refused and the lender may claim.
        if u64::try_from(now).map_or(true, |now| now >= loan.expired_at) {
            return Err(fail(LoanFault::PastDue));
        }
        if loan.repaid_principal > loan.principal_amount {
            return Err(fail(LoanFault::OverRepaid));
        }
        Ok(Some(loan))
    }

    fn loan_components(
        &self,
        snapshot: &Snapshot,
        key: &Address,
        loan: &Loan,
        now: i64,
        repay_fee_bps: u16,
    ) -> Result<Vec<Component>> {
        let fail = |reason| Error::Loan { loan: *key, reason };
        let Asset::Token(principal) = &loan.principal else {
            return Err(fail(LoanFault::PrincipalNotToken));
        };
        let outstanding = u128::from(loan.principal_amount - loan.repaid_principal);
        if loan.lender == self.authority {
            // Offerbook's own accrual: interest over the elapsed share of the
            // current period, which starts `duration` before expiry. At repay
            // or extension Offerbook keeps `repay_fee_bps` of the interest; the
            // fee is rounded up here, so the receivable never reads high.
            let duration = u64::from(loan.duration);
            let elapsed = u64::try_from(now)
                .unwrap_or(0)
                .saturating_sub(loan.expired_at.saturating_sub(duration))
                .min(duration);
            let accrued = u128::from(loan.interest) * u128::from(elapsed) / u128::from(duration);
            let fee = (accrued * u128::from(repay_fee_bps)).div_ceil(10_000);
            return Ok(vec![Component {
                kind: Kind::LoanReceivable,
                mint: principal.mint,
                account: *key,
                amount: to_u64(outstanding + accrued.saturating_sub(fee))?,
                side: Side::Asset,
            }]);
        }
        let Asset::Token(collateral) = &loan.collateral else {
            return Err(fail(LoanFault::CollateralNotToken));
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
        // Anyone can send tokens to the loan vault: count only what the loan
        // locks, and fail only when the vault holds less than that.
        if u128::from(held) < locked {
            return Err(fail(LoanFault::CollateralShort));
        }
        Ok(vec![
            Component {
                kind: Kind::PledgedCollateral,
                mint: collateral.mint,
                account: loan_vault,
                amount: to_u64(locked)?,
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

fn read_user(snapshot: &Snapshot, key: &Address, authority: &Address) -> Result<Option<User>> {
    let Some(account) = snapshot.get(key)? else {
        return Ok(None);
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
    Ok(Some(user))
}

fn counters(user: Option<&User>) -> (u64, u64) {
    user.map_or((0, 0), |u| (u.total_lender_loans, u.total_borrower_loans))
}

/// `User.FLAG_CUSTOM_FEES`: the user's own fee rates replace the config's.
const USER_FLAG_CUSTOM_FEES: u8 = 1 << 1;

/// The repay fee Offerbook takes from the vault's interest as lender: its own
/// fee profile when set, otherwise `Config.repay_fee_bps`.
fn lender_fee_bps(snapshot: &Snapshot, key: &Address, user: Option<&User>) -> Result<u16> {
    let user = user.ok_or(Error::Missing {
        what: "Offerbook User",
        address: *key,
    })?;
    if user.flags & USER_FLAG_CUSTOM_FEES != 0 {
        return Ok(user.repay_fee_bps);
    }
    let key = derive_config();
    let account = snapshot.require(&key, "Offerbook Config")?;
    owned_by(account, &key, &PROGRAM_ID, "Offerbook Config")?;
    let config = Config::from_account_data(&account.data).ok_or(Error::Layout {
        what: "Offerbook Config",
        address: key,
        reason: "not a Config account",
    })?;
    Ok(config.repay_fee_bps)
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
    /// The last second every price passes its gate and no active loan is due.
    pub valid_until: Option<i64>,
    /// The repay fee taken out of lent interest; `None` without active lent loans.
    pub repay_fee_bps: Option<u16>,
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
        v.share_mint = derive_share_mint(&VAULT);
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

    fn share_mint(supply: u64) -> Option<Account> {
        let mut data = mint().unwrap().data;
        data[36..44].copy_from_slice(&supply.to_le_bytes());
        some(crate::snapshot::TOKEN_2022_PROGRAM_ID, data)
    }

    fn mint() -> Option<Account> {
        let mut data = vec![0u8; 82];
        data[44] = 6;
        data[45] = 1;
        some(TOKEN, data)
    }

    fn config(repay_fee_bps: u16) -> Option<Account> {
        let config = Config {
            admin: Address::default(),
            fee_authority: Address::default(),
            config_bump: 0,
            fee_authority_bump: 0,
            _padding: [0; 6],
            interest_fee_bps: 0,
            repay_fee_bps,
            liquidation_fee_bps: 0,
            referral_rewards_fee_bps: 0,
            referee_rewards_fee_bps: 0,
            _padding1: [0; 6],
            min_principal_amount: 0,
            min_collateral_amount: 0,
            min_duration: 0,
            min_expiry: 0,
            max_apy: 0,
            max_duration: 0,
            max_expiry: 0,
            is_paused: 0,
            disable_repayment: 0,
            _padding2: [0; 2],
            operator: Address::default(),
            gated_collections: [Address::default(); 4],
            _reserved: [0; 240],
        };
        let mut data = Config::DISCRIMINATOR.to_vec();
        data.extend(offerbook_client::serialize(&config).unwrap());
        some(PROGRAM_ID, data)
    }

    fn user(lent: u64, borrowed: u64) -> Option<Account> {
        custom_fee_user(lent, borrowed, None)
    }

    /// A `User`, with its own repay fee when `repay_fee_bps` is set.
    fn custom_fee_user(lent: u64, borrowed: u64, repay_fee_bps: Option<u16>) -> Option<Account> {
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
            flags: if repay_fee_bps.is_some() {
                USER_FLAG_CUSTOM_FEES
            } else {
                0
            },
            _padding: [0; 6],
            yield_operator: Address::default(),
            origination_fee_bps: 0,
            repay_fee_bps: repay_fee_bps.unwrap_or(0),
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
            (derive_share_mint(&VAULT), share_mint(1_000)),
            (USDC, mint()),
            (SOL, mint()),
            (user_key, user(1, 1)),
            (derive_config(), config(1_000)),
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

    /// The scans of [`accounts`]' two loans, run at `first` and `last`.
    fn scan(first: u64, last: u64) -> LoanScan {
        LoanScan {
            lent: vec![LENT],
            borrowed: vec![BORROWED],
            first_slot: first,
            last_slot: last,
        }
    }

    fn value(accounts: Vec<(Address, Option<Account>)>) -> Result<OfferbookValuation> {
        let snapshot = Snapshot::new(1, accounts)?;
        OfferbookPlan::new(VAULT, &snapshot, scan(1, 1))?.value(&snapshot)
    }

    #[test]
    fn escrow_and_loans_count_once_and_borrowing_is_subtracted() {
        let v = value(accounts(&loan(true), &loan(false))).unwrap();
        // Assets at $0.9999 / $99.9999 floor, liabilities at $1.0001 ceiling.
        // Escrow 250 + lent 1,000 + 25% of 10 interest less the 10% repay fee.
        let usdc_assets = 1_252_250_000u128 * 99_990_000 / 100_000_000;
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
                (Kind::LoanReceivable, 1_002_250_000),
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
        assert_eq!(v.components[1].amount, 602_250_000);
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
        // A donation on top of the locked collateral counts for nothing and
        // stops nothing.
        accounts.retain(|(k, _)| *k != vault_key);
        accounts.push((vault_key, token(&SOL, &BORROWED, 20_000_001)));
        let v = value(accounts.clone()).unwrap();
        assert_eq!(v.components[2].amount, 20_000_000);
        accounts.retain(|(k, _)| *k != vault_key);
        accounts.push((vault_key, None));
        assert!(matches!(value(accounts), Err(Error::Missing { .. })));
    }

    #[test]
    fn overdue_and_unknown_loans_never_become_zero() {
        for (mutate, side) in [
            (
                (|l: &mut Loan| l.expired_at = NOW as u64 - 1) as fn(&mut Loan),
                true,
            ),
            (|l: &mut Loan| l.expired_at = NOW as u64 - 1, false),
            // Due at `expired_at` itself: Offerbook already refuses repayment.
            (|l: &mut Loan| l.expired_at = NOW as u64, true),
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
        let mut due = loan(true);
        due.expired_at = NOW as u64;
        assert_eq!(
            value(accounts(&due, &loan(false))),
            Err(Error::Loan {
                loan: LENT,
                reason: LoanFault::PastDue
            })
        );
        // Settled loans are not holdings.
        let mut repaid = loan(true);
        repaid.status = LoanStatus::Repaid;
        let v = value(accounts(&repaid, &loan(false))).unwrap();
        assert!(v.components.iter().all(|c| c.kind != Kind::LoanReceivable));
    }

    #[test]
    fn a_lent_loan_against_unpriced_collateral_never_values() {
        // A claim would move the collateral into an escrow no row reads, so
        // the loan fails closed, active or already claimed.
        let unlisted = Address::new_from_array([9; 32]);
        for status in [LoanStatus::Active, LoanStatus::Defaulted] {
            let mut lent = loan(true);
            lent.collateral = token_asset(unlisted);
            lent.status = status.clone();
            assert_eq!(
                value(accounts(&lent, &loan(false))),
                Err(Error::Unpriced { mint: unlisted }),
                "{status:?}"
            );
        }
        let mut repaid = loan(true);
        repaid.collateral = token_asset(unlisted);
        repaid.status = LoanStatus::Repaid;
        assert!(value(accounts(&repaid, &loan(false))).is_ok());
    }

    #[test]
    fn reads_out_of_order_with_the_loan_scans_are_refused() {
        let snapshot = Snapshot::new(1, accounts(&loan(true), &loan(false))).unwrap();
        // The scans ran at slot 0, before this slot-1 read: its counters could
        // already count a loan the scans missed.
        assert!(matches!(
            OfferbookPlan::new(VAULT, &snapshot, scan(0, 0)),
            Err(Error::ReadOrder(_))
        ));
        assert!(matches!(
            OfferbookPlan::new(VAULT, &snapshot, scan(3, 2)),
            Err(Error::ReadOrder(_))
        ));
        // Valuing a read from before the later scan could see a loan that
        // settled after it was read but before that scan: still active, and
        // missing from the plan.
        let plan = OfferbookPlan::new(VAULT, &snapshot, scan(1, 2)).unwrap();
        assert!(matches!(plan.value(&snapshot), Err(Error::ReadOrder(_))));
        let after = Snapshot::new(2, accounts(&loan(true), &loan(false))).unwrap();
        assert!(
            matches!(plan.value(&after), Err(Error::ClockSlot { .. })),
            "the clock fixture is at slot 1"
        );
    }

    #[test]
    fn a_loan_created_after_discovery_stops_the_mark() {
        let accounts = accounts(&loan(true), &loan(false));
        let snapshot = Snapshot::new(1, accounts.clone()).unwrap();
        let plan = OfferbookPlan::new(VAULT, &snapshot, scan(1, 1)).unwrap();
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

    fn replace(
        mut accounts: Vec<(Address, Option<Account>)>,
        key: Address,
        account: Option<Account>,
    ) -> Vec<(Address, Option<Account>)> {
        accounts.retain(|(k, _)| *k != key);
        accounts.push((key, account));
        accounts
    }

    fn receivable(accounts: Vec<(Address, Option<Account>)>) -> Result<u64> {
        let v = value(accounts)?;
        Ok(v.components
            .iter()
            .find(|c| c.kind == Kind::LoanReceivable)
            .unwrap()
            .amount)
    }

    #[test]
    fn the_repay_fee_comes_from_the_fee_profile_or_the_config() {
        let base = accounts(&loan(true), &loan(false));
        // Config's 10% of the 2.5 accrued: 2.25 net.
        assert_eq!(receivable(base.clone()), Ok(1_002_250_000));
        // A fee profile on the vault's own User wins, even at zero.
        let user_key = derive_user(&derive_vault_authority(&VAULT));
        let own = replace(base.clone(), user_key, custom_fee_user(1, 1, Some(0)));
        assert_eq!(receivable(own), Ok(1_002_500_000));
        let own = replace(base.clone(), user_key, custom_fee_user(1, 1, Some(2_000)));
        assert_eq!(receivable(own), Ok(1_002_000_000));
        // The fee rounds up: 10% of 1 raw unit of interest is a whole unit.
        let mut dust = loan(true);
        dust.interest = 4;
        assert_eq!(receivable(accounts(&dust, &loan(false))), Ok(1_000_000_000));
        // Without Config the fee is unknown: fail closed.
        let gone = replace(base, derive_config(), None);
        assert!(matches!(value(gone), Err(Error::Missing { .. })));
    }

    #[test]
    fn an_extended_loan_accrues_only_its_current_period() {
        // Rolled 25 s ago into a fresh 100 s period; created long before.
        let mut lent = loan(true);
        lent.extension_count = 3;
        lent.created_at = NOW as u64 - 10_000;
        let mut borrowed = loan(false);
        borrowed.extension_count = 1;
        let v = value(accounts(&lent, &borrowed)).unwrap();
        let unextended = value(accounts(&loan(true), &loan(false))).unwrap();
        assert_eq!(v.value, unextended.value);
        assert_eq!(v.components[1].amount, 1_002_250_000);
    }
}
