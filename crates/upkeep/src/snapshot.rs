//! Accounts read at one slot, and the decoders every valuation shares.

use {
    crate::{Error, Result},
    earn_vaults_sdk::{
        pda::derive_share_mint,
        state::{Account as _, Vault},
    },
    solana_address::Address,
    std::collections::BTreeMap,
};

pub const CLOCK_SYSVAR_ID: Address =
    Address::from_str_const("SysvarC1ock11111111111111111111111111111111");
pub const SYSVAR_PROGRAM_ID: Address =
    Address::from_str_const("Sysvar1111111111111111111111111111111111111");
pub const TOKEN_PROGRAM_ID: Address =
    Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022_PROGRAM_ID: Address =
    Address::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
pub const SYSTEM_PROGRAM_ID: Address = Address::from_str_const("11111111111111111111111111111111");

/// One account as `getMultipleAccounts` returns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    pub owner: Address,
    pub data: Vec<u8>,
}

/// Accounts fetched together at `slot`.
///
/// `None` means the RPC answered that the account does not exist at that slot.
/// A read that failed is not `None`: leave it out of the snapshot, and the
/// valuation fails with [`Error::NotFetched`] instead of seeing an empty
/// account.
///
/// An empty, system-owned account also reads as `None`. Anyone can send SOL
/// to an unused address, a PDA included, which leaves exactly that; it holds
/// nothing and must not stop a valuation.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    slot: u64,
    accounts: BTreeMap<Address, Option<Account>>,
    /// The clock time of a simulated refresh, set by [`Self::with_refresh`].
    refreshed_at: Option<i64>,
}

impl Snapshot {
    pub fn new(
        slot: u64,
        accounts: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> Result<Self> {
        let mut map = BTreeMap::new();
        for (key, account) in accounts {
            let account = account.filter(|a| !(a.owner == SYSTEM_PROGRAM_ID && a.data.is_empty()));
            match map.get(&key) {
                Some(seen) if *seen != account => return Err(Error::Conflicting(key)),
                _ => {
                    map.insert(key, account);
                }
            }
        }
        Ok(Self {
            slot,
            accounts: map,
            refreshed_at: None,
        })
    }

    /// This snapshot with the prices a `RefreshPrices` installs, for marks
    /// sent in the same transaction as that refresh.
    ///
    /// `vault` and `clock` are the vault and clock sysvar a simulation of the
    /// refresh returns (`simulateTransaction` with `accounts`). The simulation
    /// cannot return the whole snapshot: it returns at most as many accounts as
    /// the transaction lists. So holdings stay as read at [`Self::slot`] and only the
    /// asset rows' cached readings and `prices_changed_at` are taken from
    /// the refreshed vault. The transaction installs the same signed readings.
    /// Valuations then gate prices at the simulated clock time.
    pub fn with_refresh(
        &self,
        vault_key: &Address,
        vault: &Account,
        clock: &Account,
    ) -> Result<Self> {
        owned_by(vault, vault_key, &earn_vaults_sdk::ID, "refreshed vault")?;
        let fresh = Vault::read(&vault.data).map_err(|_| Error::Layout {
            what: "refreshed vault",
            address: *vault_key,
            reason: "not a vault account",
        })?;
        let (slot, now) = read_clock(clock)?;
        if slot < self.slot {
            return Err(Error::ReadOrder(
                "simulate the refresh after the snapshot read",
            ));
        }
        let mut accounts = self.accounts.clone();
        let current = accounts
            .get_mut(vault_key)
            .ok_or(Error::NotFetched(*vault_key))?
            .as_mut()
            .ok_or(Error::Missing {
                what: "vault",
                address: *vault_key,
            })?;
        owned_by(current, vault_key, &earn_vaults_sdk::ID, "vault")?;
        let state = Vault::read_mut(&mut current.data).map_err(|_| Error::Layout {
            what: "vault",
            address: *vault_key,
            reason: "not a vault account",
        })?;
        for (row, new) in state.assets.iter_mut().zip(&fresh.assets) {
            let identity = |a: &earn_vaults_sdk::state::AssetConfig| {
                (a.configured(), a.mint, a.decimals, a.oracle, a.quote_oracle)
            };
            if identity(row) != identity(new) {
                return Err(Error::PlanStale("asset rows changed before the refresh"));
            }
            row.last_price = new.last_price;
            row.last_conf = new.last_conf;
            row.last_price_expo = new.last_price_expo;
            row.last_publish_time = new.last_publish_time;
            row.base_publish_time = new.base_publish_time;
            row.quote_publish_time = new.quote_publish_time;
        }
        state.prices_changed_at = fresh.prices_changed_at;
        Ok(Self {
            slot: self.slot,
            accounts,
            refreshed_at: Some(now),
        })
    }

    pub fn slot(&self) -> u64 {
        self.slot
    }

    /// The account, `None` when it does not exist; an error when never fetched.
    pub fn get(&self, key: &Address) -> Result<Option<&Account>> {
        self.accounts
            .get(key)
            .map(Option::as_ref)
            .ok_or(Error::NotFetched(*key))
    }

    pub fn require(&self, key: &Address, what: &'static str) -> Result<&Account> {
        self.get(key)?.ok_or(Error::Missing {
            what,
            address: *key,
        })
    }

    /// `Clock.unix_timestamp`, after checking the sysvar belongs to this
    /// slot; the simulated refresh's time after [`Self::with_refresh`].
    pub fn now(&self) -> Result<i64> {
        let (slot, now) = read_clock(self.require(&CLOCK_SYSVAR_ID, "clock sysvar")?)?;
        if slot != self.slot {
            return Err(Error::ClockSlot {
                clock: slot,
                snapshot: self.slot,
            });
        }
        Ok(self.refreshed_at.unwrap_or(now))
    }

    /// An SPL Token or Token-2022 mint.
    pub fn mint(&self, key: &Address) -> Result<Mint> {
        mint(self, key, "mint")
    }

    /// The supply of the vault's share mint, checked to be the vault's PDA.
    pub fn share_supply(&self, vault_key: &Address) -> Result<u64> {
        let share_mint = derive_share_mint(vault_key);
        if self.vault(vault_key)?.share_mint != share_mint {
            return Err(Error::Layout {
                what: "vault",
                address: *vault_key,
                reason: "share mint is not the vault's PDA",
            });
        }
        Ok(mint(self, &share_mint, "share mint")?.supply)
    }

    /// The vault account, checked for owner and exact layout.
    pub fn vault(&self, key: &Address) -> Result<&Vault> {
        let account = self.require(key, "vault")?;
        owned_by(account, key, &earn_vaults_sdk::ID, "vault")?;
        Vault::read(&account.data).map_err(|_| Error::Layout {
            what: "vault",
            address: *key,
            reason: "not a vault account",
        })
    }
}

/// `(slot, unix_timestamp)` of a clock sysvar account.
fn read_clock(clock: &Account) -> Result<(u64, i64)> {
    owned_by(clock, &CLOCK_SYSVAR_ID, &SYSVAR_PROGRAM_ID, "clock sysvar")?;
    if clock.data.len() < 40 {
        return Err(Error::Layout {
            what: "clock sysvar",
            address: CLOCK_SYSVAR_ID,
            reason: "shorter than 40 bytes",
        });
    }
    Ok((u64_at(&clock.data, 0), u64_at(&clock.data, 32) as i64))
}

pub(crate) fn owned_by(
    account: &Account,
    address: &Address,
    owner: &Address,
    what: &'static str,
) -> Result<()> {
    if account.owner != *owner {
        return Err(Error::WrongOwner {
            what,
            address: *address,
            owner: account.owner,
        });
    }
    Ok(())
}

fn token_program(account: &Account, address: &Address, what: &'static str) -> Result<Address> {
    if account.owner == TOKEN_PROGRAM_ID || account.owner == TOKEN_2022_PROGRAM_ID {
        Ok(account.owner)
    } else {
        Err(Error::WrongOwner {
            what,
            address: *address,
            owner: account.owner,
        })
    }
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().expect("8-byte field"))
}

/// An SPL Token or Token-2022 mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mint {
    pub token_program: Address,
    pub supply: u64,
    pub decimals: u8,
}

pub(crate) fn mint(snapshot: &Snapshot, key: &Address, what: &'static str) -> Result<Mint> {
    let account = snapshot.require(key, what)?;
    let token_program = token_program(account, key, what)?;
    let bad = |reason| Error::Layout {
        what,
        address: *key,
        reason,
    };
    let data = &account.data;
    // A Token-2022 mint with extensions is padded to the token-account size
    // and tagged with account type 1 at byte 165.
    if data.len() < 82 || (data.len() > 82 && data.get(165) != Some(&1)) {
        return Err(bad("not a mint"));
    }
    if data[45] != 1 {
        return Err(bad("mint is not initialized"));
    }
    Ok(Mint {
        token_program,
        supply: u64_at(data, 36),
        decimals: data[44],
    })
}

/// The balance of a token account that must hold `mint` for `owner`.
/// An address with no token account holds nothing; any other mismatch fails.
pub(crate) fn token_balance(
    snapshot: &Snapshot,
    key: &Address,
    mint: &Address,
    owner: &Address,
    what: &'static str,
) -> Result<u64> {
    let Some(account) = snapshot.get(key)? else {
        return Ok(0);
    };
    token_program(account, key, what)?;
    let bad = |reason| Error::Layout {
        what,
        address: *key,
        reason,
    };
    let data = &account.data;
    if data.len() < 165 || (data.len() > 165 && data.get(165) != Some(&2)) {
        return Err(bad("not a token account"));
    }
    if data[..32] != *mint.as_array() {
        return Err(bad("holds a different mint"));
    }
    if data[32..64] != *owner.as_array() {
        return Err(bad("belongs to a different owner"));
    }
    if data[108] == 0 {
        return Err(bad("token account is not initialized"));
    }
    Ok(u64_at(data, 64))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn clock(slot: u64, now: i64) -> (Address, Option<Account>) {
        let mut data = vec![0; 40];
        data[..8].copy_from_slice(&slot.to_le_bytes());
        data[32..40].copy_from_slice(&now.to_le_bytes());
        (
            CLOCK_SYSVAR_ID,
            Some(Account {
                owner: SYSVAR_PROGRAM_ID,
                data,
            }),
        )
    }

    pub fn token(mint: &Address, owner: &Address, amount: u64) -> Option<Account> {
        let mut data = vec![0; 165];
        data[..32].copy_from_slice(mint.as_array());
        data[32..64].copy_from_slice(owner.as_array());
        data[64..72].copy_from_slice(&amount.to_le_bytes());
        data[108] = 1;
        Some(Account {
            owner: TOKEN_PROGRAM_ID,
            data,
        })
    }

    #[test]
    fn unfetched_missing_and_conflicting_accounts_fail_closed() {
        let key = Address::new_from_array([1; 32]);
        let mint = Address::new_from_array([2; 32]);
        let owner = Address::new_from_array([3; 32]);
        let empty = Snapshot::new(9, []).unwrap();
        assert_eq!(
            token_balance(&empty, &key, &mint, &owner, "t"),
            Err(Error::NotFetched(key))
        );
        assert_eq!(empty.now(), Err(Error::NotFetched(CLOCK_SYSVAR_ID)));
        let absent = Snapshot::new(9, [(key, None)]).unwrap();
        assert_eq!(token_balance(&absent, &key, &mint, &owner, "t"), Ok(0));
        assert!(matches!(mint_of(&absent, &key), Err(Error::Missing { .. })));
        assert_eq!(
            Snapshot::new(9, [(key, None), (key, token(&mint, &owner, 1))]).unwrap_err(),
            Error::Conflicting(key)
        );
        assert!(Snapshot::new(9, [(key, None), (key, None)]).is_ok());
    }

    fn mint_of(snapshot: &Snapshot, key: &Address) -> Result<Mint> {
        mint(snapshot, key, "mint")
    }

    #[test]
    fn token_accounts_must_match_mint_owner_and_program() {
        let key = Address::new_from_array([1; 32]);
        let mint = Address::new_from_array([2; 32]);
        let owner = Address::new_from_array([3; 32]);
        let read = |account: Option<Account>| {
            token_balance(
                &Snapshot::new(1, [(key, account)]).unwrap(),
                &key,
                &mint,
                &owner,
                "t",
            )
        };
        assert_eq!(read(token(&mint, &owner, 7)), Ok(7));
        assert!(read(token(&owner, &owner, 7)).is_err(), "wrong mint");
        assert!(read(token(&mint, &mint, 7)).is_err(), "wrong owner");
        let mut foreign = token(&mint, &owner, 7).unwrap();
        foreign.owner = owner;
        assert!(matches!(read(Some(foreign)), Err(Error::WrongOwner { .. })));
        let sol_only = Account {
            owner: SYSTEM_PROGRAM_ID,
            data: vec![],
        };
        assert_eq!(read(Some(sol_only)), Ok(0), "SOL sent to the address");
        let squatted = Account {
            owner: SYSTEM_PROGRAM_ID,
            data: vec![0; 165],
        };
        assert!(matches!(
            read(Some(squatted)),
            Err(Error::WrongOwner { .. })
        ));
        let mut short = token(&mint, &owner, 7).unwrap();
        short.data.truncate(100);
        assert!(matches!(read(Some(short)), Err(Error::Layout { .. })));
    }

    #[test]
    fn a_refresh_replaces_row_prices_and_nothing_else() {
        use {crate::price::tests::asset, earn_vaults_sdk::zeropod::U64};
        let key = Address::new_from_array([7; 32]);
        let usdc = Address::new_from_array([1; 32]);
        let vault = |price: i64, at: i64, aum: u64| {
            let mut data = vec![0u8; Vault::LEN];
            data[0] = Vault::DISCRIMINATOR;
            let v = Vault::read_mut(&mut data).unwrap();
            v.assets[0] = asset(usdc, price, 1_000, -8, at);
            v.aum = U64::from(aum);
            Account {
                owner: earn_vaults_sdk::ID,
                data,
            }
        };
        let snapshot = Snapshot::new(
            10,
            [(key, Some(vault(100_000_000, 990, 5))), clock(10, 1_000)],
        )
        .unwrap();
        let fresh = vault(99_000_000, 1_004, 6);
        let refreshed = snapshot
            .with_refresh(&key, &fresh, &clock(12, 1_005).1.unwrap())
            .unwrap();
        let v = refreshed.vault(&key).unwrap();
        assert_eq!(v.assets[0].last_price.get(), 99_000_000);
        assert_eq!(v.assets[0].last_publish_time.get(), 1_004);
        assert_eq!(v.aum.get(), 5, "only prices come from the simulation");
        assert_eq!(refreshed.now(), Ok(1_005));
        assert_eq!(refreshed.slot(), 10);
        // A simulation older than the read, or rows that changed, cannot be used.
        assert!(matches!(
            snapshot.with_refresh(&key, &fresh, &clock(9, 999).1.unwrap()),
            Err(Error::ReadOrder(_))
        ));
        let mut moved = fresh.clone();
        Vault::read_mut(&mut moved.data).unwrap().assets[0].mint = key;
        assert!(matches!(
            snapshot.with_refresh(&key, &moved, &clock(12, 1_005).1.unwrap()),
            Err(Error::PlanStale(_))
        ));
    }

    #[test]
    fn the_clock_must_come_from_the_snapshot_slot() {
        let snapshot = Snapshot::new(10, [clock(10, 1_000)]).unwrap();
        assert_eq!(snapshot.now(), Ok(1_000));
        let snapshot = Snapshot::new(11, [clock(10, 1_000)]).unwrap();
        assert_eq!(
            snapshot.now(),
            Err(Error::ClockSlot {
                clock: 10,
                snapshot: 11
            })
        );
    }
}
