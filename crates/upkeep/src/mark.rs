//! Which position slots need a mark, and whether a value may be posted.

use {
    earn_vaults_sdk::{
        constants::MAX_STALENESS_SECONDS,
        state::{PositionConfig, Protocol, Vault},
    },
    solana_address::Address,
};

/// The slot's mark as the vault last recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotState {
    pub index: u8,
    pub protocol: u8,
    /// Set by the first mark. A slot that was never marked bootstraps: its
    /// first mark is capped against fresh validated AUM.
    pub initialized: bool,
    /// Cleared by every settlement that moves the position; `ValidateAum`
    /// refuses an initialized slot until it is marked again.
    pub priced: bool,
    pub last_value: u64,
    pub last_priced_at: i64,
    /// Seconds a mark stays valid for `ValidateAum`.
    pub max_staleness_seconds: i64,
    /// The vault has never held anything: no shares, no AUM, no marks.
    /// See [`vault_in_bootstrap`].
    pub vault_bootstrap: bool,
}

impl SlotState {
    pub fn of(index: u8, position: &PositionConfig, vault_bootstrap: bool) -> Self {
        Self {
            vault_bootstrap,
            index,
            protocol: position.protocol,
            initialized: position.initialized(),
            priced: position.priced(),
            last_value: position.last_value.get(),
            last_priced_at: position.last_priced_at.get(),
            max_staleness_seconds: match position.max_staleness_seconds.get() {
                0 => i64::from(MAX_STALENESS_SECONDS),
                seconds => i64::try_from(seconds).unwrap_or(i64::MAX),
            },
        }
    }

    /// Whether posting `value` now is allowed to be built.
    ///
    /// On a vault still in bootstrap, a zero first mark is never posted (D34):
    /// it would end the bootstrap, after which a deposit needs nonzero AUM, so
    /// the first deposit could never land. Leaving the slot unmarked is correct
    /// there: `ValidateAum` skips a never-marked slot.
    ///
    /// Once the vault holds anything, a never-marked slot blocks deposits
    /// (`PricedSlotMissing`) until marked, and the program takes a zero first
    /// mark as attesting an empty slot, so zero is posted.
    pub fn decide(&self, value: u64) -> MarkDecision {
        if value == 0 && !self.initialized && self.vault_bootstrap {
            MarkDecision::SkipEmptyBootstrap
        } else {
            MarkDecision::Post {
                value,
                bootstrap: !self.initialized,
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkDecision {
    /// Post `value`. A `bootstrap` mark needs a fresh `ValidateAum` before it,
    /// which [`crate::jobs::attest`] places.
    Post { value: u64, bootstrap: bool },
    /// The vault is in bootstrap and the slot is worth zero: post nothing.
    SkipEmptyBootstrap,
}

/// The program's bootstrap test: no shares, nothing queued, zero AUM, and no
/// mark, deployment or sink ever recorded. The first deposit prices at 1:1
/// only in this state.
pub fn vault_in_bootstrap(vault: &Vault, share_supply: u64) -> bool {
    share_supply == 0
        && vault.aum.get() == 0
        && vault.pending_redeem_shares.get() == 0
        && vault
            .positions
            .iter()
            .all(|p| !p.initialized() && p.last_value.get() == 0)
        && vault.assets.iter().all(|a| a.deployed.get() == 0)
        && vault
            .sinks
            .iter()
            .all(|s| s.is_free() && s.value.get() == 0)
}

/// Why a slot needs a mark before `ValidateAum` or a share flow can pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Marked before, then dirtied by a settlement: `ValidateAum` fails until marked.
    Unpriced,
    /// Older than the slot's staleness window: `ValidateAum` fails.
    Stale,
    /// A JupiterLend mark older than the last price-cache advance: direct share
    /// flows are refused until it is marked again (program 0.2.0, #274). With
    /// `refreshing`, every priced JupiterLend slot is due, since the refresh
    /// in the same transaction advances the cache.
    BeforePriceChange,
    /// Never marked, on a vault past bootstrap: deposits are refused
    /// (`PricedSlotMissing`) until a first mark, zero if the slot is empty.
    NeverMarked,
}

/// Every enabled, AUM-counted slot that needs a mark at `now`.
/// `share_supply` is the share mint's supply.
pub fn due_slots(
    vault: &Vault,
    share_supply: u64,
    now: i64,
    refreshing: bool,
) -> Vec<(SlotState, Due)> {
    let changed_at = vault.prices_changed_at.get();
    let bootstrap = vault_in_bootstrap(vault, share_supply);
    vault
        .positions
        .iter()
        .enumerate()
        .filter(|(_, p)| p.enabled() && p.required_in_aum())
        .filter_map(|(i, p)| {
            let slot = SlotState::of(i as u8, p, bootstrap);
            let due = if slot.initialized && !slot.priced {
                Due::Unpriced
            } else if !slot.initialized && !p.debt() && !bootstrap {
                Due::NeverMarked
            } else if slot.priced
                && now.saturating_sub(slot.last_priced_at) > slot.max_staleness_seconds
            {
                Due::Stale
            } else if slot.priced
                && slot.protocol == Protocol::JupiterLend as u8
                && (refreshing || slot.last_priced_at < changed_at)
            {
                Due::BeforePriceChange
            } else {
                return None;
            };
            Some((slot, due))
        })
        .collect()
}

/// The single enabled slot of `protocol`. The program admits at most one.
pub fn find_slot(vault: &Vault, protocol: Protocol) -> Option<(u8, &PositionConfig)> {
    vault
        .positions
        .iter()
        .enumerate()
        .find(|(_, p)| p.enabled() && p.protocol == protocol as u8)
        .map(|(i, p)| (i as u8, p))
}

/// A value ready to post into `slot`, with who signs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub signer: Address,
    pub slot: u8,
    pub value: u64,
    pub bootstrap: bool,
    /// Sign with the Earn-scoped `jupiter_lend::PRICE` grant instead of vault `PRICE`.
    pub earn_scoped: bool,
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        earn_vaults_sdk::{
            state::{
                POSITION_FLAG_ENABLED, POSITION_FLAG_INITIALIZED, POSITION_FLAG_PRICED,
                POSITION_FLAG_REQUIRED_IN_AUM,
            },
            zeropod::I64,
        },
    };

    fn slot(initialized: bool, vault_bootstrap: bool) -> SlotState {
        SlotState {
            vault_bootstrap,
            index: 0,
            protocol: Protocol::Offerbook as u8,
            initialized,
            priced: initialized,
            last_value: 0,
            last_priced_at: 0,
            max_staleness_seconds: 60,
        }
    }

    #[test]
    fn an_empty_vault_never_gets_a_first_zero_mark() {
        assert_eq!(
            slot(false, true).decide(0),
            MarkDecision::SkipEmptyBootstrap
        );
        // Past bootstrap a zero first mark attests the slot is empty, which
        // deposits wait for.
        assert_eq!(
            slot(false, false).decide(0),
            MarkDecision::Post {
                value: 0,
                bootstrap: true
            }
        );
        assert_eq!(
            slot(false, true).decide(1),
            MarkDecision::Post {
                value: 1,
                bootstrap: true
            }
        );
        // Once marked, zero is a real value that must be attested.
        assert_eq!(
            slot(true, false).decide(0),
            MarkDecision::Post {
                value: 0,
                bootstrap: false
            }
        );
    }

    #[test]
    fn due_slots_follow_validate_aum_and_the_jupiter_lend_rule() {
        let mut data = vec![0u8; <Vault as earn_vaults_sdk::state::Account>::LEN];
        data[0] = <Vault as earn_vaults_sdk::state::Account>::DISCRIMINATOR;
        let vault = <Vault as earn_vaults_sdk::state::Account>::read_mut(&mut data).unwrap();
        let on = POSITION_FLAG_ENABLED | POSITION_FLAG_REQUIRED_IN_AUM;
        let marked = on | POSITION_FLAG_INITIALIZED | POSITION_FLAG_PRICED;
        for (i, (protocol, flags, at)) in [
            (Protocol::Offerbook, marked, 1_000),
            (Protocol::Offerbook, marked, 900),
            (Protocol::JupiterLend, marked, 990),
            (Protocol::Nest, on | POSITION_FLAG_INITIALIZED, 1_000),
            (Protocol::Ccip, on, 0),
        ]
        .into_iter()
        .enumerate()
        {
            vault.positions[i].protocol = protocol as u8;
            vault.positions[i].flags = flags;
            vault.positions[i].last_priced_at = I64::from(at);
        }
        vault.prices_changed_at = I64::from(995);
        let due = |vault: &Vault, refreshing| {
            due_slots(vault, 0, 1_000, refreshing)
                .into_iter()
                .map(|(s, d)| (s.index, d))
                .collect::<Vec<_>>()
        };
        let never = (4, Due::NeverMarked);
        assert_eq!(
            due(vault, false),
            vec![
                (1, Due::Stale),
                (2, Due::BeforePriceChange),
                (3, Due::Unpriced),
                never,
            ]
        );
        vault.prices_changed_at = I64::from(0);
        assert_eq!(
            due(vault, false),
            vec![(1, Due::Stale), (3, Due::Unpriced), never]
        );
        assert_eq!(
            due(vault, true),
            vec![
                (1, Due::Stale),
                (2, Due::BeforePriceChange),
                (3, Due::Unpriced),
                never,
            ]
        );
        // A vault in bootstrap has nothing due: no slot was ever marked.
        for p in &mut vault.positions {
            p.flags &= !(POSITION_FLAG_INITIALIZED | POSITION_FLAG_PRICED);
        }
        assert!(vault_in_bootstrap(vault, 0));
        assert_eq!(due(vault, false), vec![]);
        assert!(!vault_in_bootstrap(vault, 1));
    }
}
