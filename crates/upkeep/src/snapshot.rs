//! Accounts read at one slot, and the decoders every valuation shares.

use {
    crate::{Error, Result},
    earn_vaults_sdk::state::{Account as _, Vault},
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
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    slot: u64,
    accounts: BTreeMap<Address, Option<Account>>,
}

impl Snapshot {
    pub fn new(
        slot: u64,
        accounts: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> Result<Self> {
        let mut map = BTreeMap::new();
        for (key, account) in accounts {
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

    /// `Clock.unix_timestamp`, after checking the sysvar belongs to this slot.
    pub fn now(&self) -> Result<i64> {
        let clock = self.require(&CLOCK_SYSVAR_ID, "clock sysvar")?;
        owned_by(clock, &CLOCK_SYSVAR_ID, &SYSVAR_PROGRAM_ID, "clock sysvar")?;
        let field = |at: usize| -> Result<[u8; 8]> {
            clock
                .data
                .get(at..at + 8)
                .and_then(|bytes| bytes.try_into().ok())
                .ok_or(Error::Layout {
                    what: "clock sysvar",
                    address: CLOCK_SYSVAR_ID,
                    reason: "shorter than 40 bytes",
                })
        };
        let slot = u64::from_le_bytes(field(0)?);
        if slot != self.slot {
            return Err(Error::ClockSlot {
                clock: slot,
                snapshot: self.slot,
            });
        }
        Ok(i64::from_le_bytes(field(32)?))
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

/// An SPL `COption<Pubkey>`: a four-byte tag, then the key.
fn option_key_at(data: &[u8], at: usize) -> Option<Address> {
    (data[at..at + 4] != [0; 4])
        .then(|| Address::new_from_array(data[at + 4..at + 36].try_into().expect("32-byte key")))
}

/// An SPL Token or Token-2022 mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mint {
    pub token_program: Address,
    pub mint_authority: Option<Address>,
    pub supply: u64,
    pub decimals: u8,
    /// A Token-2022 mint carrying extensions.
    pub extended: bool,
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
        mint_authority: option_key_at(data, 0),
        supply: u64_at(data, 36),
        decimals: data[44],
        extended: data.len() > 82,
    })
}

/// An SPL Token or Token-2022 token account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenAccount {
    pub token_program: Address,
    pub mint: Address,
    pub owner: Address,
    pub amount: u64,
    pub delegate: Option<Address>,
    /// `AccountState`: 1 initialized, 2 frozen.
    pub state: u8,
    /// A Token-2022 account carrying extensions.
    pub extended: bool,
}

/// Parse `account` as a token account.
pub(crate) fn parse_token_account(
    account: &Account,
    key: &Address,
    what: &'static str,
) -> Result<TokenAccount> {
    let token_program = token_program(account, key, what)?;
    let bad = |reason| Error::Layout {
        what,
        address: *key,
        reason,
    };
    let data = &account.data;
    if data.len() < 165 || (data.len() > 165 && data.get(165) != Some(&2)) {
        return Err(bad("not a token account"));
    }
    if data[108] == 0 {
        return Err(bad("token account is not initialized"));
    }
    Ok(TokenAccount {
        token_program,
        mint: Address::new_from_array(data[..32].try_into().expect("32-byte key")),
        owner: Address::new_from_array(data[32..64].try_into().expect("32-byte key")),
        amount: u64_at(data, 64),
        delegate: option_key_at(data, 72),
        state: data[108],
        extended: data.len() > 165,
    })
}

/// The account at a token-account address, `None` when no token account is
/// there. Anyone can send SOL to an unused address, leaving an empty
/// system-owned account; that holds no tokens either, and must not stop a
/// valuation.
pub(crate) fn token_account<'a>(
    snapshot: &'a Snapshot,
    key: &Address,
) -> Result<Option<&'a Account>> {
    Ok(snapshot
        .get(key)?
        .filter(|a| !(a.owner == SYSTEM_PROGRAM_ID && a.data.is_empty())))
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
    let Some(account) = token_account(snapshot, key)? else {
        return Ok(0);
    };
    let parsed = parse_token_account(account, key, what)?;
    let bad = |reason| Error::Layout {
        what,
        address: *key,
        reason,
    };
    if parsed.mint != *mint {
        return Err(bad("holds a different mint"));
    }
    if parsed.owner != *owner {
        return Err(bad("belongs to a different owner"));
    }
    Ok(parsed.amount)
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
