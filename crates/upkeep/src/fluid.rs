//! Jupiter Lend vault (Fluid) positions, valued together with the slot's Earn
//! receipts.
//!
//! A JupiterLend slot that is not lender-only can hold Fluid T1 positions:
//! collateral supplied and debt borrowed through Fluid's Vaults program. Each
//! position is an NFT the vault authority holds. The slot's mark is, net:
//!
//! | Holding | Side | Amount, in token units |
//! |---|---|---|
//! | Earn receipts | asset | as [`crate::earn`] values them |
//! | Fluid collateral | asset | `floor(C × Vs / (1e12 × 10^(9 − d)))` |
//! | Fluid debt | liability | `ceil(D × Vb / (1e12 × 10^(9 − d)))` |
//!
//! `C` and `D` are the position's collateral and debt in Fluid's nine-decimal
//! raw units. `C` is the stored supply. For a borrowing position, `D` is the
//! debt its tick implies, `floor(ratio(tick) × (C + 1) / 2^48) + 1`, less the
//! stored dust debt. `Vs` and `Vb` are the Fluid vault's supply and borrow
//! exchange prices (1e12 precision) at the snapshot's clock. Fluid amounts
//! are summed per mint and side, then priced off the vault rows like
//! Offerbook's: collateral at price − confidence rounded down, debt at
//! price + confidence rounded up.
//!
//! Only supply-only positions and borrowing positions on a live tick are
//! valued. A tick that was liquidated since the position joined it
//! (`is_liquidated`, or `total_ids` past the position's `tick_id`) needs the
//! liquidation branch rules, which this crate does not implement, so it fails
//! closed. Smart vaults (types 2–4) hold DEX shares, so they fail closed too.
//!
//! ## Reads, all at the snapshot's slot
//!
//! - [`FluidPlan::keys`] with one `getMultipleAccounts`.
//! - The vault authority's token accounts with `getTokenAccountsByOwner`,
//!   once under SPL Token and once under Token-2022: [`TokenListing`]. This
//!   listing is the proof of which positions exist: every positive balance
//!   must be custody, a canonical Earn receipt account, a planned position
//!   NFT, or a mint discovery proved is neither Earn nor Fluid (see
//!   [`crate::listing`]). A listing with no NFT proves the slot holds Earn
//!   receipts only.
//! - `simulateTransaction` of [`FluidPlan::exchange_price_instructions`],
//!   one per Fluid vault: [`ExchangePriceRead`]. The Vaults program's
//!   `get_exchange_prices` view projects Liquidity's and the vault's indices
//!   to the bank's clock and returns them. That is how Jupiter's own SDK
//!   reads them; this crate does not reimplement the projection.
//!
//! Each listing and simulation carries its RPC context slot, and
//! [`FluidPlan::value`] refuses any slot that differs from the snapshot's.
//!
//! ## Source
//!
//! The tick ratio and the debt rebuild are ported from `@jup-ag/lend` 0.4.0
//! (MIT): `getRatioAtTick` and the active-tick branch of
//! `getCurrentPositionState`. Both are unchanged in 1.0.0-alpha.1. Account
//! layouts follow the Vaults 0.1.8 and Liquidity 0.1.9 IDLs that
//! 1.0.0-alpha.1 bundles. The SDK returns raw amounts; rounding the token
//! conversion, collateral down and debt up, is this crate's NAV rule.

use {
    crate::{
        Error, Result,
        earn::{
            EarnMarketValue, PlannedRow, check_rows, in_bootstrap, jupiter_lend_slot, planned_rows,
            sorted,
        },
        listing::{Attribution, Ignored, TokenListing, attribute},
        mark::SlotState,
        offerbook::MintTotal,
        price::{Side, gate},
        snapshot::{Account, CLOCK_SYSVAR_ID, Snapshot, TokenAccount, mint, owned_by},
    },
    earn_vaults_sdk::{
        pda::{derive_share_mint, derive_vault_authority},
        state::Vault,
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    std::collections::{BTreeMap, BTreeSet},
};

pub const VAULTS_PROGRAM_ID: Address =
    Address::from_str_const("jupr81YtYssSyPt8jbnGuiWon5f6x9TcDEFxYe3Bdzi");
pub const LIQUIDITY_PROGRAM_ID: Address =
    Address::from_str_const("jupeiUmn818Jg1ekPURTpr4mFo29p46vygyykFJ3wZC");

pub const POSITION_DISCRIMINATOR: [u8; 8] = [0xaa, 0xbc, 0x8f, 0xe4, 0x7a, 0x40, 0xf7, 0xd0];
pub const POSITION_LEN: usize = 71;
/// Byte offset of `Position.position_mint`, discriminator included.
pub const POSITION_MINT_OFFSET: usize = 14;
const VAULT_CONFIG_DISCRIMINATOR: [u8; 8] = [0x63, 0x56, 0x2b, 0xd8, 0xb8, 0x66, 0x77, 0x4d];
const VAULT_CONFIG_LEN: usize = 219;
const VAULT_STATE_DISCRIMINATOR: [u8; 8] = [0xe4, 0xc4, 0x52, 0xa5, 0x62, 0xd2, 0xeb, 0x98];
const VAULT_STATE_LEN: usize = 127;
const TICK_DISCRIMINATOR: [u8; 8] = [0xb0, 0x5e, 0x43, 0xf7, 0x85, 0xad, 0x07, 0x73];
const TICK_LEN: usize = 40;
const TOKEN_RESERVE_DISCRIMINATOR: [u8; 8] = [0x15, 0x12, 0x3b, 0x87, 0x78, 0x14, 0x1f, 0x0c];
const TOKEN_RESERVE_LEN: usize = 192;
/// `get_exchange_prices`, the Vaults program's read-only view.
pub const GET_EXCHANGE_PRICES_DISCRIMINATOR: [u8; 8] = [237, 128, 83, 152, 52, 21, 231, 86];

/// `VaultConfig.vault_type` of a T1 vault: plain collateral, plain debt.
pub const VAULT_T1_TYPE: u8 = 1;
pub const MIN_TICK: i32 = -16383;
pub const MAX_TICK: i32 = 16383;
/// `Position.tick` of a position that never borrowed.
pub const INIT_TICK: i32 = i32::MIN;
/// Fluid's internal amounts carry nine decimals whatever the mint's.
pub const RAW_DECIMALS: u8 = 9;
/// Exchange prices are scaled by 1e12.
pub const EXCHANGE_PRICE_PRECISION: u128 = 1_000_000_000_000;

fn pda(seeds: &[&[u8]], program: &Address) -> Address {
    Address::find_program_address(seeds, program).0
}

/// `["vault_config", vault_id]` under Vaults.
pub fn derive_vault_config(vault_id: u16) -> Address {
    pda(
        &[b"vault_config", &vault_id.to_le_bytes()],
        &VAULTS_PROGRAM_ID,
    )
}

/// `["vault_state", vault_id]` under Vaults.
pub fn derive_vault_state(vault_id: u16) -> Address {
    pda(
        &[b"vault_state", &vault_id.to_le_bytes()],
        &VAULTS_PROGRAM_ID,
    )
}

/// `["position", vault_id, nft_id]` under Vaults.
pub fn derive_position(vault_id: u16, nft_id: u32) -> Address {
    pda(
        &[b"position", &vault_id.to_le_bytes(), &nft_id.to_le_bytes()],
        &VAULTS_PROGRAM_ID,
    )
}

/// `["position_mint", vault_id, nft_id]` under Vaults: the position's NFT.
pub fn derive_position_mint(vault_id: u16, nft_id: u32) -> Address {
    pda(
        &[
            b"position_mint",
            &vault_id.to_le_bytes(),
            &nft_id.to_le_bytes(),
        ],
        &VAULTS_PROGRAM_ID,
    )
}

/// `["tick", vault_id, tick + 16383]` under Vaults. `tick` must be in range.
pub fn derive_tick(vault_id: u16, tick: i32) -> Address {
    let index = u32::try_from(tick - MIN_TICK).expect("tick in range");
    pda(
        &[b"tick", &vault_id.to_le_bytes(), &index.to_le_bytes()],
        &VAULTS_PROGRAM_ID,
    )
}

/// `["reserve", mint]` under Liquidity.
pub fn derive_reserve(mint: &Address) -> Address {
    pda(&[b"reserve", mint.as_array()], &LIQUIDITY_PROGRAM_ID)
}

/// `getProgramAccounts` memcmp filters, as `(offset, bytes)`, for the Vaults
/// `Position` of the NFT `mint`. Run it for each of
/// [`crate::listing::candidate_mints`].
pub fn position_filters(mint: &Address) -> [(usize, Vec<u8>); 2] {
    [
        (0, POSITION_DISCRIMINATOR.to_vec()),
        (POSITION_MINT_OFFSET, mint.to_bytes().to_vec()),
    ]
}

/// One `simulateTransaction` of a `get_exchange_prices` instruction, with its
/// context slot and the transaction's return data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangePriceRead {
    pub vault_id: u16,
    pub slot: u64,
    /// `returnData.programId`.
    pub program: Address,
    /// `returnData.data`, decoded.
    pub data: Vec<u8>,
}

/// The four indices `get_exchange_prices` returns, 1e12 precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExchangePrices {
    pub liquidity_supply: u128,
    pub liquidity_borrow: u128,
    pub vault_supply: u128,
    pub vault_borrow: u128,
}

fn bad(what: &'static str, address: &Address) -> impl Fn(&'static str) -> Error {
    let address = *address;
    move |reason| Error::Layout {
        what,
        address,
        reason,
    }
}

fn fluid(account: &Address) -> impl Fn(&'static str) -> Error {
    let account = *account;
    move |reason| Error::Fluid { account, reason }
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().expect("2 bytes"))
}
fn i16_at(data: &[u8], at: usize) -> i16 {
    i16::from_le_bytes(data[at..at + 2].try_into().expect("2 bytes"))
}
fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes"))
}
fn i32_at(data: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(data[at..at + 4].try_into().expect("4 bytes"))
}
fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().expect("8 bytes"))
}
fn u128_at(data: &[u8], at: usize) -> u128 {
    u128::from_le_bytes(data[at..at + 16].try_into().expect("16 bytes"))
}
fn key_at(data: &[u8], at: usize) -> Address {
    Address::new_from_array(data[at..at + 32].try_into().expect("32 bytes"))
}

/// A Vaults or Liquidity account of the expected owner, discriminator and
/// exact length.
fn layout<'a>(
    account: &'a Account,
    address: &Address,
    owner: &Address,
    discriminator: &[u8; 8],
    len: usize,
    what: &'static str,
) -> Result<&'a [u8]> {
    owned_by(account, address, owner, what)?;
    if account.data.len() != len || account.data[..8] != *discriminator {
        return Err(bad(what, address)("unexpected discriminator or length"));
    }
    Ok(&account.data)
}

/// A Vaults `Position`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub vault_id: u16,
    pub nft_id: u32,
    pub mint: Address,
    pub supply_only: bool,
    pub tick: i32,
    pub tick_id: u32,
    /// Collateral, nine-decimal raw units.
    pub supply_amount: u64,
    /// Debt below the tick's implied debt, nine-decimal raw units.
    pub dust_debt_amount: u64,
}

impl Position {
    /// Parse `account` at `address`, checking it is the PDA its IDs name.
    pub fn read(address: &Address, account: &Account) -> Result<Self> {
        let what = "Fluid position";
        let data = layout(
            account,
            address,
            &VAULTS_PROGRAM_ID,
            &POSITION_DISCRIMINATOR,
            POSITION_LEN,
            what,
        )?;
        let supply_only = match data[46] {
            0 => false,
            1 => true,
            _ => return Err(bad(what, address)("supply-only flag is not 0 or 1")),
        };
        let position = Self {
            vault_id: u16_at(data, 8),
            nft_id: u32_at(data, 10),
            mint: key_at(data, POSITION_MINT_OFFSET),
            supply_only,
            tick: i32_at(data, 47),
            tick_id: u32_at(data, 51),
            supply_amount: u64_at(data, 55),
            dust_debt_amount: u64_at(data, 63),
        };
        if derive_position(position.vault_id, position.nft_id) != *address {
            return Err(bad(what, address)("not the PDA of its vault and NFT IDs"));
        }
        if derive_position_mint(position.vault_id, position.nft_id) != position.mint {
            return Err(bad(what, address)("mint is not the PDA of its IDs"));
        }
        Ok(position)
    }

    /// The tick it borrows at; `None` when it carries no debt. Fails on a
    /// state with no defined debt.
    pub fn debt_tick(&self, address: &Address) -> Result<Option<i32>> {
        if self.supply_only {
            return Ok(None);
        }
        match self.tick {
            INIT_TICK if self.supply_amount == 0 && self.dust_debt_amount == 0 => Ok(None),
            INIT_TICK => Err(fluid(address)("an unborrowed tick with collateral or dust")),
            tick if tick > MIN_TICK && tick <= MAX_TICK => Ok(Some(tick)),
            _ => Err(fluid(address)("tick out of range")),
        }
    }
}

/// The Fluid vault one or more planned positions belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidMarket {
    pub vault_id: u16,
    pub config: Address,
    pub state: Address,
    pub supply_mint: Address,
    pub borrow_mint: Address,
    pub supply_reserve: Address,
    pub borrow_reserve: Address,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Config {
    supply_magnifier: i16,
    borrow_magnifier: i16,
    supply_mint: Address,
    borrow_mint: Address,
}

fn read_config(snapshot: &Snapshot, vault_id: u16) -> Result<Config> {
    let key = derive_vault_config(vault_id);
    let what = "Fluid vault config";
    let data = layout(
        snapshot.require(&key, what)?,
        &key,
        &VAULTS_PROGRAM_ID,
        &VAULT_CONFIG_DISCRIMINATOR,
        VAULT_CONFIG_LEN,
        what,
    )?;
    if u16_at(data, 8) != vault_id {
        return Err(bad(what, &key)("belongs to another vault ID"));
    }
    match data[25] {
        VAULT_T1_TYPE => {}
        2..=4 => return Err(fluid(&key)("smart vault: its legs are DEX shares")),
        _ => return Err(fluid(&key)("unknown vault type")),
    }
    if key_at(data, 90) != LIQUIDITY_PROGRAM_ID {
        return Err(fluid(&key)("unsupported Liquidity program"));
    }
    Ok(Config {
        supply_magnifier: i16_at(data, 10),
        borrow_magnifier: i16_at(data, 12),
        supply_mint: key_at(data, 154),
        borrow_mint: key_at(data, 186),
    })
}

impl FluidMarket {
    fn of(snapshot: &Snapshot, vault_id: u16) -> Result<Self> {
        let config = read_config(snapshot, vault_id)?;
        Ok(Self {
            vault_id,
            config: derive_vault_config(vault_id),
            state: derive_vault_state(vault_id),
            supply_mint: config.supply_mint,
            borrow_mint: config.borrow_mint,
            supply_reserve: derive_reserve(&config.supply_mint),
            borrow_reserve: derive_reserve(&config.borrow_mint),
        })
    }

    /// `get_exchange_prices` for this vault, to simulate at the snapshot slot.
    pub fn exchange_price_instruction(&self) -> Instruction {
        Instruction {
            program_id: VAULTS_PROGRAM_ID,
            accounts: [
                self.state,
                self.config,
                self.supply_reserve,
                self.borrow_reserve,
            ]
            .into_iter()
            .map(|key| AccountMeta::new_readonly(key, false))
            .collect(),
            data: GET_EXCHANGE_PRICES_DISCRIMINATOR.to_vec(),
        }
    }

    fn keys(&self) -> [Address; 6] {
        [
            self.config,
            self.state,
            self.supply_mint,
            self.borrow_mint,
            self.supply_reserve,
            self.borrow_reserve,
        ]
    }
}

/// A position the vault authority's listing showed at discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedPosition {
    pub position: Address,
    pub nft_mint: Address,
    pub vault_id: u16,
    pub nft_id: u32,
    /// The tick it borrowed at, at discovery.
    pub tick: Option<i32>,
}

/// Fetch these to build a plan: the vault, its row mints, and the vault config
/// of each `Position` the [`position_filters`] scans returned.
pub fn discovery_keys(
    vault_key: &Address,
    vault: &Vault,
    positions: &[(Address, Account)],
) -> Result<Vec<Address>> {
    let mut keys = vec![*vault_key];
    keys.extend(crate::earn::discovery_keys(vault));
    for (address, account) in positions {
        keys.push(derive_vault_config(
            Position::read(address, account)?.vault_id,
        ));
    }
    Ok(dedup(keys))
}

fn dedup(keys: Vec<Address>) -> Vec<Address> {
    let mut seen = BTreeSet::new();
    keys.into_iter().filter(|k| seen.insert(*k)).collect()
}

/// Where a non-lender-only JupiterLend slot's inputs live. Build again when
/// [`FluidPlan::value`] returns [`Error::PlanStale`]: a position moved tick,
/// or an NFT left or joined the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidPlan {
    pub vault: Address,
    pub authority: Address,
    pub slot: u8,
    pub rows: Vec<PlannedRow>,
    pub markets: Vec<FluidMarket>,
    pub positions: Vec<PlannedPosition>,
    /// Mints the authority held at discovery that have no Fluid `Position`
    /// (see [`crate::listing`]).
    pub unrelated: Vec<Address>,
}

impl FluidPlan {
    /// `snapshot` holds [`discovery_keys`]. `positions` are the accounts the
    /// [`position_filters`] scans returned for
    /// [`crate::listing::candidate_mints`]; `unrelated` are the candidates
    /// they returned nothing for.
    pub fn new(
        vault: Address,
        snapshot: &Snapshot,
        positions: impl IntoIterator<Item = (Address, Account)>,
        unrelated: impl IntoIterator<Item = Address>,
    ) -> Result<Self> {
        let state = snapshot.vault(&vault)?;
        let (slot, _) = jupiter_lend_slot(state, false)?;
        let mut planned = BTreeMap::new();
        for (address, account) in positions {
            let position = Position::read(&address, &account)?;
            planned.insert(
                address,
                PlannedPosition {
                    position: address,
                    nft_mint: position.mint,
                    vault_id: position.vault_id,
                    nft_id: position.nft_id,
                    tick: position.debt_tick(&address)?,
                },
            );
        }
        let positions: Vec<_> = planned.into_values().collect();
        let markets = positions
            .iter()
            .map(|p| p.vault_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|id| FluidMarket::of(snapshot, id))
            .collect::<Result<_>>()?;
        Ok(Self {
            vault,
            authority: derive_vault_authority(&vault),
            slot,
            rows: planned_rows(snapshot, state)?,
            markets,
            positions,
            unrelated: sorted(unrelated),
        })
    }

    /// Every account [`Self::value`] reads, for one `getMultipleAccounts`.
    pub fn keys(&self) -> Vec<Address> {
        let mut keys = vec![self.vault, CLOCK_SYSVAR_ID, derive_share_mint(&self.vault)];
        for row in &self.rows {
            keys.push(row.market.mint);
            keys.extend(row.market.keys(&self.authority));
        }
        for market in &self.markets {
            keys.extend(market.keys());
        }
        for p in &self.positions {
            keys.extend([p.position, p.nft_mint]);
            keys.extend(p.tick.map(|tick| derive_tick(p.vault_id, tick)));
        }
        keys.extend(&self.unrelated);
        dedup(keys)
    }

    /// One `get_exchange_prices` per Fluid vault, to simulate at the slot of
    /// [`Self::keys`].
    pub fn exchange_price_instructions(&self) -> Vec<(u16, Instruction)> {
        self.markets
            .iter()
            .map(|m| (m.vault_id, m.exchange_price_instruction()))
            .collect()
    }

    pub fn value(
        &self,
        snapshot: &Snapshot,
        listings: &[TokenListing],
        exchange_prices: &[ExchangePriceRead],
    ) -> Result<FluidValuation> {
        let now = snapshot.now()?;
        let vault = snapshot.vault(&self.vault)?;
        let (slot, position) = jupiter_lend_slot(vault, false)?;
        if slot != self.slot {
            return Err(Error::PlanStale("Jupiter Lend slot"));
        }
        check_rows(snapshot, vault, &self.rows)?;
        let nft_mints: Vec<_> = self.positions.iter().map(|p| p.nft_mint).collect();
        let Attribution { mut nfts, ignored } = attribute(
            snapshot,
            &self.authority,
            &self.rows,
            &nft_mints,
            &self.unrelated,
            listings,
        )?;
        let nfts = self
            .positions
            .iter()
            .map(|p| {
                nfts.remove(&p.nft_mint).ok_or(Error::PlanStale(
                    "a planned Fluid position NFT left the vault",
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let prices = read_exchange_prices(snapshot.slot(), exchange_prices)?;

        let (earn_value, earn) =
            crate::earn::value_markets(snapshot, vault, &self.authority, &self.rows, now)?;

        let mut markets = BTreeMap::new();
        for market in &self.markets {
            let indices = prices.get(&market.vault_id).ok_or(Error::Fluid {
                account: market.state,
                reason: "no exchange-price read for this vault",
            })?;
            markets.insert(
                market.vault_id,
                (market, self.check_market(snapshot, vault, market, indices)?),
            );
        }

        let mut positions = Vec::new();
        let mut sums = BTreeMap::<(Address, bool), u128>::new();
        for (planned, token_account) in self.positions.iter().zip(nfts) {
            let (market, legs) = markets[&planned.vault_id];
            let value = value_position(snapshot, planned, token_account, market, &legs)?;
            for (mint, liability, amount) in [
                (market.supply_mint, false, value.collateral),
                (market.borrow_mint, true, value.debt),
            ] {
                if amount != 0 {
                    let entry = sums.entry((mint, liability)).or_default();
                    *entry = entry
                        .checked_add(u128::from(amount))
                        .ok_or(Error::Overflow("Fluid holdings"))?;
                }
            }
            positions.push(value);
        }

        let (mut assets, mut liabilities) = (earn_value, 0u64);
        let mut totals = Vec::new();
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
                .ok_or(Error::Overflow("Fluid value"))?;
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
        let valid_until = earn
            .iter()
            .filter_map(|m| m.price.map(|p| p.valid_until))
            .chain(totals.iter().map(|t| t.price.valid_until))
            .min();
        Ok(FluidValuation {
            slot: SlotState::of(slot, position, in_bootstrap(snapshot, &self.vault, vault)?),
            now,
            value,
            assets,
            liabilities,
            earn_value,
            earn,
            positions,
            totals,
            ignored,
            valid_until,
        })
    }

    /// Check one Fluid vault's config, state, mints and reserves against the
    /// plan and the exchange-price read; return what its legs need.
    fn check_market(
        &self,
        snapshot: &Snapshot,
        vault: &Vault,
        market: &FluidMarket,
        indices: &ExchangePrices,
    ) -> Result<MarketLegs> {
        let config = read_config(snapshot, market.vault_id)?;
        if config.supply_mint != market.supply_mint || config.borrow_mint != market.borrow_mint {
            return Err(Error::PlanStale("Fluid vault mints"));
        }
        let what = "Fluid vault state";
        let state = layout(
            snapshot.require(&market.state, what)?,
            &market.state,
            &VAULTS_PROGRAM_ID,
            &VAULT_STATE_DISCRIMINATOR,
            VAULT_STATE_LEN,
            what,
        )?;
        if u16_at(state, 8) != market.vault_id {
            return Err(bad(what, &market.state)("belongs to another vault ID"));
        }
        let decimals = |mint_key: &Address| -> Result<u8> {
            let m = mint(snapshot, mint_key, "Fluid vault mint")?;
            if m.decimals > RAW_DECIMALS {
                return Err(fluid(mint_key)("more than nine decimals"));
            }
            if let Some((_, row)) = crate::price::row(vault, mint_key)
                && row.decimals != m.decimals
            {
                return Err(bad("Fluid vault mint", mint_key)(
                    "decimals differ from the vault row",
                ));
            }
            Ok(m.decimals)
        };
        let reserve = |key: &Address, mint_key: &Address| -> Result<(u64, u64)> {
            let what = "Liquidity reserve";
            let data = layout(
                snapshot.require(key, what)?,
                key,
                &LIQUIDITY_PROGRAM_ID,
                &TOKEN_RESERVE_DISCRIMINATOR,
                TOKEN_RESERVE_LEN,
                what,
            )?;
            if key_at(data, 8) != *mint_key {
                return Err(bad(what, key)("belongs to another mint"));
            }
            Ok((u64_at(data, 86), u64_at(data, 94)))
        };
        let (stored_supply, _) = reserve(&market.supply_reserve, &market.supply_mint)?;
        let (_, stored_borrow) = reserve(&market.borrow_reserve, &market.borrow_mint)?;
        // Interest only accrues: the projection cannot sit below what is
        // stored, and the vault's index cannot fall unless its magnifier is
        // negative.
        let fail = fluid(&market.state);
        let floor = |projected: u128, stored: u64, reason| {
            if projected == 0 || projected < u128::from(stored) {
                Err(fail(reason))
            } else {
                Ok(())
            }
        };
        floor(
            indices.liquidity_supply,
            stored_supply,
            "projected Liquidity supply index is below the reserve's",
        )?;
        floor(
            indices.liquidity_borrow,
            stored_borrow,
            "projected Liquidity borrow index is below the reserve's",
        )?;
        let vault_floor = |projected, at, magnifier: i16, reason| {
            floor(
                projected,
                if magnifier >= 0 { u64_at(state, at) } else { 0 },
                reason,
            )
        };
        vault_floor(
            indices.vault_supply,
            99,
            config.supply_magnifier,
            "projected vault supply index is below the stored one",
        )?;
        vault_floor(
            indices.vault_borrow,
            107,
            config.borrow_magnifier,
            "projected vault borrow index is below the stored one",
        )?;
        Ok(MarketLegs {
            supply_decimals: decimals(&market.supply_mint)?,
            borrow_decimals: decimals(&market.borrow_mint)?,
            exchange_prices: *indices,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct MarketLegs {
    supply_decimals: u8,
    borrow_decimals: u8,
    exchange_prices: ExchangePrices,
}

fn read_exchange_prices(
    slot: u64,
    reads: &[ExchangePriceRead],
) -> Result<BTreeMap<u16, ExchangePrices>> {
    let mut out = BTreeMap::new();
    for read in reads {
        let account = derive_vault_state(read.vault_id);
        if read.slot != slot {
            return Err(Error::ReadSlot {
                what: "exchange-price simulation",
                read: read.slot,
                snapshot: slot,
            });
        }
        if read.program != VAULTS_PROGRAM_ID {
            return Err(fluid(&account)(
                "exchange prices returned by another program",
            ));
        }
        if read.data.len() != 64 {
            return Err(fluid(&account)(
                "exchange-price return data is not 64 bytes",
            ));
        }
        let prices = ExchangePrices {
            liquidity_supply: u128_at(&read.data, 0),
            liquidity_borrow: u128_at(&read.data, 16),
            vault_supply: u128_at(&read.data, 32),
            vault_borrow: u128_at(&read.data, 48),
        };
        if out
            .insert(read.vault_id, prices)
            .is_some_and(|seen| seen != prices)
        {
            return Err(fluid(&account)("two different exchange-price reads"));
        }
    }
    Ok(out)
}

/// Fluid's tick ratio, `1.0015^tick` in Q48 fixed point: `getRatioAtTick` of
/// `@jup-ag/lend` 0.4.0 (MIT), integer for integer.
pub fn ratio_at_tick(tick: i32) -> Result<u128> {
    // FACTOR02..FACTOR15, for tick bits 2..16384. Bit 16384 is out of range.
    const FACTORS: [u128; 14] = [
        18_391_528_108_445_969_703,
        18_336_477_419_114_433_396,
        18_226_869_890_870_665_593,
        18_009_616_477_100_071_088,
        17_582_847_377_087_825_313,
        16_759_408_633_341_240_198,
        15_226_414_841_393_184_936,
        12_568_272_644_527_235_157,
        8_563_108_841_104_354_677,
        3_975_055_583_337_633_975,
        856_577_552_520_149_366,
        39_775_317_560_084_773,
        85_764_505_686_420,
        398_745_188,
    ];
    if !(MIN_TICK..=MAX_TICK).contains(&tick) {
        return Err(Error::Overflow("tick out of range"));
    }
    let abs = tick.unsigned_abs();
    // Every factor is below 2^64 and `factor` never exceeds 2^64, so each
    // product fits in u128.
    let mut factor: u128 = if abs & 1 != 0 {
        18_419_115_400_608_638_658
    } else {
        1 << 64
    };
    for (bit, f) in FACTORS.iter().enumerate() {
        if abs & (2 << bit) != 0 {
            factor = (factor * f) >> 64;
        }
    }
    let mut precision = 0;
    if tick > 0 {
        factor = u128::MAX / factor;
        if !factor.is_multiple_of(65_536) {
            precision = 1;
        }
    }
    Ok((factor >> 16) + precision)
}

/// The debt a position at `tick` with `supply` collateral was booked at,
/// before its dust: `floor(ratio × (supply + 1) / 2^48) + 1`, as
/// `getCurrentPositionState` rebuilds it. Rounds up twice, so the debt never
/// reads low.
pub fn gross_debt(tick: i32, supply: u64) -> Result<u128> {
    ratio_at_tick(tick)?
        .checked_mul(u128::from(supply) + 1)
        .map(|r| (r >> 48) + 1)
        .ok_or(Error::Overflow("Fluid gross debt"))
}

/// Nine-decimal raw `amount` at `exchange_price`, in token units of a mint
/// with `decimals`: rounded down for an asset, up for a liability.
pub fn to_token_units(amount: u128, exchange_price: u128, decimals: u8, side: Side) -> Result<u64> {
    let scale = 10u128
        .checked_pow(u32::from(
            RAW_DECIMALS
                .checked_sub(decimals)
                .ok_or(Error::Overflow("Fluid mint decimals"))?,
        ))
        .and_then(|s| s.checked_mul(EXCHANGE_PRICE_PRECISION))
        .ok_or(Error::Overflow("Fluid token units"))?;
    let product = amount
        .checked_mul(exchange_price)
        .ok_or(Error::Overflow("Fluid token units"))?;
    let units = match side {
        Side::Asset => product / scale,
        Side::Liability => product.div_ceil(scale),
    };
    u64::try_from(units).map_err(|_| Error::Overflow("Fluid token units"))
}

fn value_position(
    snapshot: &Snapshot,
    planned: &PlannedPosition,
    (token_key, nft): (Address, TokenAccount),
    market: &FluidMarket,
    legs: &MarketLegs,
) -> Result<PositionValue> {
    let key = &planned.position;
    let fail = fluid(key);
    let position = Position::read(key, snapshot.require(key, "Fluid position")?)?;
    if position.mint != planned.nft_mint || position.vault_id != planned.vault_id {
        return Err(Error::PlanStale("Fluid position"));
    }
    // The NFT: one unit, in a plain account the authority alone controls.
    let nft_mint = mint(snapshot, &planned.nft_mint, "Fluid position mint")?;
    if nft_mint.token_program != nft.token_program
        || nft_mint.extended
        || nft_mint.decimals != 0
        || nft_mint.supply != 1
        || nft_mint.mint_authority.is_some()
    {
        return Err(fail("position mint is not a fixed one-unit NFT"));
    }
    if nft.amount != 1 || nft.extended || nft.state != 1 || nft.delegate.is_some() {
        return Err(fail(
            "position NFT account is frozen, delegated, extended or not one unit",
        ));
    }

    let tick = position.debt_tick(key)?;
    if tick != planned.tick {
        return Err(Error::PlanStale("Fluid position tick"));
    }
    let (state, debt_raw) = match tick {
        None => (PositionState::Unborrowed, 0),
        Some(tick) => {
            let tick_key = derive_tick(position.vault_id, tick);
            let what = "Fluid tick";
            let data = layout(
                snapshot.require(&tick_key, what)?,
                &tick_key,
                &VAULTS_PROGRAM_ID,
                &TICK_DISCRIMINATOR,
                TICK_LEN,
                what,
            )?;
            if u16_at(data, 8) != position.vault_id || i32_at(data, 10) != tick {
                return Err(bad(what, &tick_key)("belongs to another vault or tick"));
            }
            let total_ids = u32_at(data, 15);
            if data[14] != 0 || total_ids > position.tick_id {
                return Err(fail("its tick was liquidated since the position joined it"));
            }
            if position.tick_id == 0 || total_ids < position.tick_id {
                return Err(fail("tick ID is zero or ahead of the tick"));
            }
            if u64_at(data, 19) == 0 {
                return Err(fail("a borrowing position on a tick with no debt"));
            }
            let gross = gross_debt(tick, position.supply_amount)?;
            let dust = u128::from(position.dust_debt_amount);
            if dust >= gross {
                return Err(fail("dust debt is not below the tick's debt"));
            }
            (
                PositionState::Borrowing {
                    tick,
                    tick_id: position.tick_id,
                    gross_debt_raw: gross,
                    dust_debt_raw: position.dust_debt_amount,
                },
                gross - dust,
            )
        }
    };
    let prices = legs.exchange_prices;
    Ok(PositionValue {
        position: *key,
        nft_mint: planned.nft_mint,
        token_account: token_key,
        vault_id: position.vault_id,
        nft_id: position.nft_id,
        state,
        collateral_raw: position.supply_amount,
        debt_raw,
        exchange_prices: prices,
        collateral: to_token_units(
            position.supply_amount.into(),
            prices.vault_supply,
            legs.supply_decimals,
            Side::Asset,
        )?,
        debt: to_token_units(
            debt_raw,
            prices.vault_borrow,
            legs.borrow_decimals,
            Side::Liability,
        )?,
        supply_mint: market.supply_mint,
        borrow_mint: market.borrow_mint,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionState {
    /// Supply-only, or never borrowed: collateral, no debt.
    Unborrowed,
    /// On a live tick: debt is the tick's implied debt less the dust.
    Borrowing {
        tick: i32,
        tick_id: u32,
        gross_debt_raw: u128,
        dust_debt_raw: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionValue {
    pub position: Address,
    pub nft_mint: Address,
    pub token_account: Address,
    pub vault_id: u16,
    pub nft_id: u32,
    pub state: PositionState,
    /// Nine-decimal raw units.
    pub collateral_raw: u64,
    pub debt_raw: u128,
    pub exchange_prices: ExchangePrices,
    pub supply_mint: Address,
    pub borrow_mint: Address,
    /// Token units of `supply_mint`, rounded down.
    pub collateral: u64,
    /// Token units of `borrow_mint`, rounded up.
    pub debt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FluidValuation {
    pub slot: SlotState,
    pub now: i64,
    /// `PriceOpaqueArgs.value` for the slot: `assets − liabilities`.
    pub value: u64,
    /// Earn receipts plus Fluid collateral.
    pub assets: u64,
    /// Fluid debt.
    pub liabilities: u64,
    pub earn_value: u64,
    pub earn: Vec<EarnMarketValue>,
    pub positions: Vec<PositionValue>,
    /// Fluid legs, per mint and side, priced once.
    pub totals: Vec<MintTotal>,
    /// Holdings the listing showed that are neither Earn nor Fluid.
    pub ignored: Vec<Ignored>,
    /// The last second every price used still passes the gate.
    pub valid_until: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tick_ratio_is_1_0015_to_the_tick_in_q48() {
        assert_eq!(ratio_at_tick(0), Ok(1 << 48));
        // Bitwise sUSDai Multiply's tick, as Fluid's integer math gives it.
        assert_eq!(ratio_at_tick(-424), Ok(149_085_516_043_224));
        // The bounds `getTickAtRatio` states for the range.
        assert_eq!(ratio_at_tick(MIN_TICK), Ok(6_093));
        assert_eq!(
            ratio_at_tick(MAX_TICK),
            Ok(13_002_088_162_051_014_298_412_982)
        );
        // Positive ticks take the reciprocal and round up.
        assert_eq!(ratio_at_tick(1), Ok(281_897_189_175_722));
        assert_eq!(ratio_at_tick(424), Ok(531_427_630_376_204));
        assert_eq!(ratio_at_tick(-1), Ok(281_053_396_615_732));
        let mut last = 0;
        for tick in (MIN_TICK..=MAX_TICK).step_by(97).chain([MAX_TICK]) {
            let ratio = ratio_at_tick(tick).unwrap();
            assert!(ratio > last, "increasing at {tick}");
            last = ratio;
            // Within the integer math's own precision: near the top, the ratio
            // is the reciprocal of a factor around 1e9.
            let expected = 1.0015f64.powi(tick) * (1u64 << 48) as f64;
            let error = (ratio as f64 - expected).abs();
            assert!(error / expected < 1e-8 || error < 2.0, "{tick}");
        }
        assert!(ratio_at_tick(MAX_TICK + 1).is_err());
        assert!(ratio_at_tick(MIN_TICK - 1).is_err());
    }

    #[test]
    fn gross_debt_rounds_up_twice() {
        // floor(ratio × (supply + 1) / 2^48) + 1 for the Multiply position.
        assert_eq!(gross_debt(-424, 82_012_035_338_000), Ok(43_438_343_091_826));
        // At tick 0 the ratio is exactly one: supply + 1, plus one.
        assert_eq!(gross_debt(0, 1_000), Ok(1_002));
        assert!(gross_debt(MAX_TICK, u64::MAX).is_err());
    }

    #[test]
    fn collateral_rounds_down_and_debt_rounds_up() {
        // 1.5 raw units of a 9-decimal mint's thousandth at index 1.0: a
        // 6-decimal mint divides by 1e3.
        let one = EXCHANGE_PRICE_PRECISION;
        assert_eq!(to_token_units(1_500, one, 6, Side::Asset), Ok(1));
        assert_eq!(to_token_units(1_500, one, 6, Side::Liability), Ok(2));
        assert_eq!(to_token_units(2_000, one, 6, Side::Liability), Ok(2));
        assert_eq!(to_token_units(0, one, 6, Side::Liability), Ok(0));
        // The index scales before rounding: 1_000 raw at 1.0000000000005.
        assert_eq!(to_token_units(1_000, one + 1, 6, Side::Asset), Ok(1));
        assert_eq!(to_token_units(1_000, one + 1, 6, Side::Liability), Ok(2));
        assert_eq!(to_token_units(7, one, 9, Side::Asset), Ok(7));
        assert!(to_token_units(1, one, 10, Side::Asset).is_err());
        assert!(to_token_units(u128::MAX, 2, 9, Side::Asset).is_err());
    }

    #[test]
    fn only_supply_only_and_live_ticks_have_a_defined_debt() {
        let key = Address::new_from_array([1; 32]);
        let base = Position {
            vault_id: 1,
            nft_id: 1,
            mint: key,
            supply_only: false,
            tick: -424,
            tick_id: 1,
            supply_amount: 10,
            dust_debt_amount: 0,
        };
        assert_eq!(base.debt_tick(&key), Ok(Some(-424)));
        let supply_only = Position {
            supply_only: true,
            tick: 5,
            ..base
        };
        assert_eq!(supply_only.debt_tick(&key), Ok(None));
        let empty = Position {
            tick: INIT_TICK,
            supply_amount: 0,
            ..base
        };
        assert_eq!(empty.debt_tick(&key), Ok(None));
        for tick in [INIT_TICK, MIN_TICK, MAX_TICK + 1] {
            let ambiguous = Position { tick, ..base };
            assert!(ambiguous.debt_tick(&key).is_err(), "{tick}");
        }
    }

    #[test]
    fn jltoken_mints_are_minted_by_lending_admin() {
        // The mint authority of jlUSDC and jlJupUSD on mainnet.
        assert_eq!(
            crate::listing::derive_lending_admin(),
            Address::from_str_const("5nmGjA4s7ATzpBQXC5RNceRpaJ7pYw2wKsNBWyuSAZV6")
        );
    }
}
