//! Jupiter Earn positions: jlToken receipts, valued at the market's exchange
//! price and the underlying row's floor price.
//!
//! The vault's Earn position is one lender-only JupiterLend slot carrying every
//! Earn market the vault holds. Its value is, per market,
//! `floor(receipts × token_exchange_price / 1e12)` underlying units at the
//! row's price − confidence, rounded down, summed. Idle custody is not part of
//! the mark: `ValidateAum` prices custody itself.
//!
//! The exchange price is the one stored on the `Lending` account, without the
//! interest accrued since its last update. That lags the true value slightly,
//! never above it.

use {
    crate::{
        Error, Result,
        mark::{SlotState, find_slot},
        price::{RowPrice, Side, gate},
        snapshot::{CLOCK_SYSVAR_ID, Snapshot, mint, owned_by, token_balance},
    },
    earn_vaults_sdk::{
        pda::{derive_associated_token_account, derive_vault_authority},
        state::{Protocol, Vault},
    },
    solana_address::Address,
};

pub const EARN_PROGRAM_ID: Address =
    Address::from_str_const("jup3YeL8QhtSx1e253b2FDvsMNC87fDrgQZivbrndc9");
pub const LENDING_DISCRIMINATOR: [u8; 8] = [135, 199, 82, 16, 249, 131, 182, 241];
pub const LENDING_LEN: usize = 196;
/// Underlying units per jlToken, scaled by 1e12.
pub const EXCHANGE_PRICE_PRECISION: u128 = 1_000_000_000_000;

/// `["f_token_mint", mint]`: the jlToken of an Earn market.
pub fn derive_f_token_mint(mint: &Address) -> Address {
    Address::find_program_address(&[b"f_token_mint", mint.as_array()], &EARN_PROGRAM_ID).0
}

/// `["lending", mint, f_token_mint]`: the market's `Lending` account.
pub fn derive_lending(mint: &Address, f_token_mint: &Address) -> Address {
    Address::find_program_address(
        &[b"lending", mint.as_array(), f_token_mint.as_array()],
        &EARN_PROGRAM_ID,
    )
    .0
}

/// The accounts that locate one Earn market for one underlying mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Market {
    pub mint: Address,
    pub token_program: Address,
    pub f_token_mint: Address,
    pub lending: Address,
}

impl Market {
    pub fn new(mint: Address, token_program: Address) -> Self {
        let f_token_mint = derive_f_token_mint(&mint);
        Self {
            mint,
            token_program,
            f_token_mint,
            lending: derive_lending(&mint, &f_token_mint),
        }
    }

    /// `holder`'s jlToken account: the vault pins it to the canonical ATA
    /// under the underlying's token program.
    pub fn receipt_account(&self, holder: &Address) -> Address {
        derive_associated_token_account(holder, &self.f_token_mint, &self.token_program)
    }

    pub(crate) fn keys(&self, holder: &Address) -> [Address; 3] {
        [
            self.lending,
            self.f_token_mint,
            self.receipt_account(holder),
        ]
    }

    /// What `holder`'s receipts redeem for. `None` when no market exists for
    /// this mint and `holder` has no receipt account.
    pub(crate) fn redeemable(
        &self,
        snapshot: &Snapshot,
        holder: &Address,
        decimals: u8,
    ) -> Result<Option<Receipts>> {
        let receipt_account = self.receipt_account(holder);
        let Some(lending) = snapshot.get(&self.lending)? else {
            // No market, so no receipts can exist; a receipt account would be
            // an account we do not understand.
            if snapshot.get(&receipt_account)?.is_some() {
                return Err(Error::Missing {
                    what: "Earn Lending",
                    address: self.lending,
                });
            }
            return Ok(None);
        };
        owned_by(lending, &self.lending, &EARN_PROGRAM_ID, "Earn Lending")?;
        let bad = |reason| Error::Layout {
            what: "Earn Lending",
            address: self.lending,
            reason,
        };
        let data = &lending.data;
        if data.len() != LENDING_LEN || data[..8] != LENDING_DISCRIMINATOR {
            return Err(bad("not a Lending account"));
        }
        if data[8..40] != *self.mint.as_array() || data[40..72] != *self.f_token_mint.as_array() {
            return Err(bad("belongs to another market"));
        }
        if data[74] != decimals {
            return Err(bad("decimals differ from the vault row"));
        }
        let exchange_price = u64::from_le_bytes(data[115..123].try_into().expect("8 bytes"));
        if exchange_price == 0 {
            return Err(bad("zero exchange price"));
        }
        let f_token = mint(snapshot, &self.f_token_mint, "jlToken mint")?;
        if f_token.token_program != self.token_program {
            return Err(Error::Layout {
                what: "jlToken mint",
                address: self.f_token_mint,
                reason: "token program differs from the underlying's",
            });
        }
        let receipts = token_balance(
            snapshot,
            &receipt_account,
            &self.f_token_mint,
            holder,
            "jlToken account",
        )?;
        let underlying = u64::try_from(
            u128::from(receipts) * u128::from(exchange_price) / EXCHANGE_PRICE_PRECISION,
        )
        .map_err(|_| Error::Overflow("Earn redeemable balance"))?;
        Ok(Some(Receipts {
            receipt_account,
            receipts,
            exchange_price,
            underlying,
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receipts {
    pub receipt_account: Address,
    pub receipts: u64,
    /// `Lending.token_exchange_price` (1e12 precision).
    pub exchange_price: u64,
    /// Underlying units the receipts redeem for, rounded down.
    pub underlying: u64,
}

/// One row the plan values, with the token program its accounts derive under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedRow {
    pub row: u8,
    pub decimals: u8,
    pub market: Market,
}

/// Every configured row's mint, read from `vault`; fetch these once with the
/// vault to build a plan.
pub fn discovery_keys(vault: &Vault) -> Vec<Address> {
    vault
        .assets
        .iter()
        .filter(|a| a.configured())
        .map(|a| a.mint)
        .collect()
}

pub(crate) fn planned_rows(snapshot: &Snapshot, vault: &Vault) -> Result<Vec<PlannedRow>> {
    vault
        .assets
        .iter()
        .enumerate()
        .filter(|(_, a)| a.configured())
        .map(|(i, a)| {
            let m = mint(snapshot, &a.mint, "asset mint")?;
            if m.decimals != a.decimals {
                return Err(Error::Layout {
                    what: "asset mint",
                    address: a.mint,
                    reason: "decimals differ from the vault row",
                });
            }
            Ok(PlannedRow {
                row: i as u8,
                decimals: a.decimals,
                market: Market::new(a.mint, m.token_program),
            })
        })
        .collect()
}

/// The rows a plan was built from must still be the vault's rows.
pub(crate) fn check_rows(snapshot: &Snapshot, vault: &Vault, rows: &[PlannedRow]) -> Result<()> {
    let configured = vault
        .assets
        .iter()
        .enumerate()
        .filter(|(_, a)| a.configured());
    if !configured
        .clone()
        .map(|(i, a)| (i as u8, a.mint, a.decimals))
        .eq(rows.iter().map(|r| (r.row, r.market.mint, r.decimals)))
    {
        return Err(Error::PlanStale("asset rows"));
    }
    for row in rows {
        if mint(snapshot, &row.market.mint, "asset mint")?.token_program != row.market.token_program
        {
            return Err(Error::PlanStale("asset token program"));
        }
    }
    Ok(())
}

/// Which JupiterLend slots the Earn valuation accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarnScope {
    /// Only a lender-only slot, which the program keeps Earn-only.
    LenderOnly,
    /// Also a slot that is not lender-only, on the caller's word that it holds
    /// no Fluid vault positions. shortcut: the vault cannot prove that from
    /// account reads alone; replace with Fluid valuation (S6).
    AssumeNoFluid,
}

/// Where the Earn mark's inputs live. Build once per vault configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarnPlan {
    pub vault: Address,
    pub authority: Address,
    pub slot: u8,
    pub scope: EarnScope,
    pub rows: Vec<PlannedRow>,
}

impl EarnPlan {
    /// `snapshot` holds the vault and [`discovery_keys`].
    pub fn new(vault: Address, snapshot: &Snapshot, scope: EarnScope) -> Result<Self> {
        let state = snapshot.vault(&vault)?;
        let (slot, _) = earn_slot(state, scope)?;
        Ok(Self {
            vault,
            authority: derive_vault_authority(&vault),
            slot,
            scope,
            rows: planned_rows(snapshot, state)?,
        })
    }

    /// Every account [`Self::value`] reads, for one `getMultipleAccounts`.
    pub fn keys(&self) -> Vec<Address> {
        let mut keys = vec![self.vault, CLOCK_SYSVAR_ID];
        for row in &self.rows {
            keys.push(row.market.mint);
            keys.extend(row.market.keys(&self.authority));
        }
        keys
    }

    pub fn value(&self, snapshot: &Snapshot) -> Result<EarnValuation> {
        let now = snapshot.now()?;
        let vault = snapshot.vault(&self.vault)?;
        let (slot, position) = earn_slot(vault, self.scope)?;
        if slot != self.slot {
            return Err(Error::PlanStale("Earn slot"));
        }
        check_rows(snapshot, vault, &self.rows)?;
        let mut value = 0u64;
        let mut markets = Vec::new();
        for row in &self.rows {
            let Some(receipts) = row
                .market
                .redeemable(snapshot, &self.authority, row.decimals)?
            else {
                continue;
            };
            // A zero holding needs no price, as in `ValidateAum`.
            let (price, worth) = if receipts.underlying == 0 {
                (None, 0)
            } else {
                let price = gate(row.row, &vault.assets[row.row as usize], now)?;
                let worth = price.value(receipts.underlying.into(), Side::Asset)?;
                (Some(price), worth)
            };
            value = value
                .checked_add(worth)
                .ok_or(Error::Overflow("Earn value"))?;
            markets.push(EarnMarketValue {
                row: row.row,
                market: row.market,
                receipts,
                price,
                value: worth,
            });
        }
        Ok(EarnValuation {
            slot: SlotState::of(slot, position),
            now,
            value,
            markets,
        })
    }
}

/// The vault's Earn slot: enabled, lender-only JupiterLend, counted in AUM,
/// not debt. A JupiterLend slot that is not lender-only can hold Fluid vault
/// positions, which this crate does not value.
fn earn_slot(
    vault: &Vault,
    scope: EarnScope,
) -> Result<(u8, &earn_vaults_sdk::state::PositionConfig)> {
    let (slot, position) = find_slot(vault, Protocol::JupiterLend)
        .ok_or(Error::Position("Jupiter Earn", "no enabled slot"))?;
    if !position.lender_only() && scope == EarnScope::LenderOnly {
        return Err(Error::Position(
            "Jupiter Earn",
            "slot is not lender-only, so it may hold Fluid vault positions",
        ));
    }
    if position.debt() || !position.required_in_aum() {
        return Err(Error::Position(
            "Jupiter Earn",
            "slot is debt or not counted in AUM",
        ));
    }
    Ok((slot, position))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarnMarketValue {
    pub row: u8,
    pub market: Market,
    pub receipts: Receipts,
    /// `None` when the market holds nothing, so no price was needed.
    pub price: Option<RowPrice>,
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EarnValuation {
    /// The slot as the vault recorded it; [`SlotState::decide`] turns
    /// `value` into a mark.
    pub slot: SlotState,
    /// Clock time the valuation holds at.
    pub now: i64,
    /// `PriceOpaqueArgs.value` for the slot.
    pub value: u64,
    pub markets: Vec<EarnMarketValue>,
}

impl EarnValuation {
    /// The last second every price used still passes the gate.
    pub fn valid_until(&self) -> Option<i64> {
        self.markets
            .iter()
            .filter_map(|m| m.price.map(|p| p.valid_until))
            .min()
    }
}
