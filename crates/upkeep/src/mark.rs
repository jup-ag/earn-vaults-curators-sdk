//! Which position slots need a mark, and whether a value may be posted.

use {
    crate::{Result, snapshot::Snapshot},
    earn_vaults_sdk::{
        constants::{MAX_STALENESS_SECONDS, drift_budget},
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
    /// A liability slot. Its marks go through the program's debt gate, which
    /// this crate does not model.
    pub debt: bool,
    pub last_value: u64,
    pub last_priced_at: i64,
    /// Seconds a mark stays valid for `ValidateAum`.
    pub max_staleness_seconds: i64,
    /// Drift cap per window, in bps of the prior mark; `None` when waived.
    pub drift_cap_bps: Option<u16>,
    /// `Vault.aum` as last validated: a later mark's drift budget is measured
    /// against the smaller of this and the prior mark.
    pub validated_aum: u64,
    /// The vault has never held anything: no shares, no AUM, no marks.
    /// See [`vault_in_bootstrap`].
    pub vault_bootstrap: bool,
}

impl SlotState {
    /// # Panics
    ///
    /// If `index` is not below `MAX_POSITIONS`.
    pub fn of(vault: &Vault, index: u8, vault_bootstrap: bool) -> Self {
        let position = &vault.positions[usize::from(index)];
        Self {
            vault_bootstrap,
            index,
            protocol: position.protocol,
            initialized: position.initialized(),
            priced: position.priced(),
            debt: position.debt(),
            last_value: position.last_value.get(),
            last_priced_at: position.last_priced_at.get(),
            max_staleness_seconds: match position.max_staleness_seconds.get() {
                0 => i64::from(MAX_STALENESS_SECONDS),
                seconds => i64::try_from(seconds).unwrap_or(i64::MAX),
            },
            // `drift_budget` widens the stored `u16`; a cap is at most `u16::MAX`.
            drift_cap_bps: drift_budget(position.max_drift_bps.get())
                .map(|bps| u16::try_from(bps).unwrap_or(u16::MAX)),
            validated_aum: vault.aum.get(),
        }
    }

    /// What to do with a valuation of `value` at clock time `now`.
    ///
    /// - On a vault still in bootstrap, a zero first mark is never posted: it
    ///   would end the bootstrap, after which a deposit needs nonzero AUM, so
    ///   the first deposit could never land. Leaving the slot unmarked is
    ///   correct there: `ValidateAum` skips a never-marked slot.
    /// - Once the vault holds anything, a never-marked slot blocks deposits
    ///   (`PricedSlotMissing`) until marked, and the program takes a zero
    ///   first mark as attesting an empty slot, so zero is posted.
    /// - A later mark may move from the prior one by at most the slot's drift
    ///   cap of `min(prior, validated AUM)` per staleness window (the
    ///   program's `check_drift`). A value outside that is
    ///   [`MarkDecision::OutOfBudget`]: the chain would refuse it, and posting
    ///   anything else would not be the value.
    ///
    /// Not modelled, so `Post` may still fail on chain:
    /// - a first mark's cap of the drift bps of the AUM the preceding
    ///   `ValidateAum` computes;
    /// - a debt slot's gate;
    /// - `Pause::Strategy`, which refuses every delegate mark;
    /// - the JupiterLend same-second fold path, where the program re-attests
    ///   the prior value: for that second `decide` may report `OutOfBudget`
    ///   where the program would accept.
    ///
    /// `now` earlier than execution only shrinks the budget.
    pub fn decide(&self, value: u64, now: i64) -> MarkDecision {
        if !self.initialized {
            if value == 0 && self.vault_bootstrap {
                return MarkDecision::SkipEmptyBootstrap;
            }
            return MarkDecision::Post {
                slot: self.index,
                value,
                bootstrap: true,
            };
        }
        if let (false, Some(cap)) = (self.debt, self.drift_cap_bps) {
            let max_move = self.max_move(cap, now);
            if value.abs_diff(self.last_value) > max_move {
                let nearest = if value > self.last_value {
                    self.last_value.saturating_add(max_move)
                } else {
                    self.last_value - max_move
                };
                return MarkDecision::OutOfBudget {
                    slot: self.index,
                    target: value,
                    nearest,
                };
            }
        }
        MarkDecision::Post {
            slot: self.index,
            value,
            bootstrap: false,
        }
    }

    /// The largest move `check_drift` accepts at `now`: `diff × 10⁴ × window`
    /// may not exceed `basis × cap × elapsed`, elapsed clamped to one window.
    fn max_move(&self, cap_bps: u16, now: i64) -> u64 {
        let window = i64::from(MAX_STALENESS_SECONDS).max(1) as u128;
        let elapsed = (now.saturating_sub(self.last_priced_at).max(0) as u128).min(window);
        let basis = u128::from(self.last_value.min(self.validated_aum));
        let budget = basis * u128::from(cap_bps) * elapsed / (10_000 * window);
        u64::try_from(budget).unwrap_or(u64::MAX)
    }
}

/// Slot `index` of `vault`, with the vault's bootstrap state read from its
/// share mint in `snapshot`.
pub(crate) fn slot_state(
    snapshot: &Snapshot,
    vault_key: &Address,
    vault: &Vault,
    index: u8,
) -> Result<SlotState> {
    let bootstrap = vault_in_bootstrap(vault, snapshot.share_supply(vault_key)?);
    Ok(SlotState::of(vault, index, bootstrap))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkDecision {
    /// Post `value` into `slot`. A `bootstrap` mark needs a fresh
    /// `ValidateAum` before it, which [`crate::jobs::attest`] places.
    Post {
        slot: u8,
        value: u64,
        bootstrap: bool,
    },
    /// The vault is in bootstrap and the slot is worth zero: post nothing.
    SkipEmptyBootstrap,
    /// `target` is outside the slot's drift budget, so the program would
    /// refuse it. `nearest` is the closest value it accepts now. Nothing is
    /// posted: the curator resets the slot or waives its cap.
    OutOfBudget { slot: u8, target: u64, nearest: u64 },
}

impl MarkDecision {
    /// The mark to post, signed by `signer` under `grant`; `None` when
    /// nothing should be posted.
    pub fn mark(self, signer: Address, grant: Grant) -> Option<Mark> {
        match self {
            Self::Post {
                slot,
                value,
                bootstrap,
            } => Some(Mark {
                signer,
                slot,
                value,
                bootstrap,
                grant,
            }),
            Self::SkipEmptyBootstrap | Self::OutOfBudget { .. } => None,
        }
    }
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
            let slot = SlotState::of(vault, i as u8, bootstrap);
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

/// Which `PRICE` grant signs a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grant {
    /// The vault-wide `PRICE` delegate.
    Vault,
    /// The Earn-scoped `jupiter_lend::PRICE` delegate. The program accepts it
    /// only on a lender-only, non-debt JupiterLend slot, so a slot valued under
    /// `EarnScope::AssumeNoFluid` signs with [`Grant::Vault`].
    JupiterLend,
}

/// A value ready to post into `slot`, with who signs it. Build it with
/// [`MarkDecision::mark`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub signer: Address,
    pub slot: u8,
    pub value: u64,
    pub bootstrap: bool,
    pub grant: Grant,
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
            debt: false,
            last_value: 0,
            last_priced_at: 0,
            max_staleness_seconds: 60,
            drift_cap_bps: Some(200),
            validated_aum: 0,
        }
    }

    const fn post(value: u64, bootstrap: bool) -> MarkDecision {
        MarkDecision::Post {
            slot: 0,
            value,
            bootstrap,
        }
    }

    #[test]
    fn an_empty_vault_never_gets_a_first_zero_mark() {
        assert_eq!(
            slot(false, true).decide(0, 0),
            MarkDecision::SkipEmptyBootstrap
        );
        // Past bootstrap a zero first mark attests the slot is empty, which
        // deposits wait for.
        assert_eq!(slot(false, false).decide(0, 0), post(0, true));
        assert_eq!(slot(false, true).decide(1, 0), post(1, true));
        // Once marked, zero is a real value that must be attested.
        assert_eq!(slot(true, false).decide(0, 0), post(0, false));
    }

    #[test]
    fn a_mark_outside_the_drift_budget_is_reported_not_posted() {
        let window = i64::from(MAX_STALENESS_SECONDS);
        let marked = SlotState {
            last_value: 1_000_000,
            last_priced_at: 1_000,
            validated_aum: 5_000_000,
            ..slot(true, false)
        };
        // 2% of the prior after a full window: 20,000 either way.
        let now = 1_000 + window;
        assert_eq!(marked.decide(1_020_000, now), post(1_020_000, false));
        assert_eq!(marked.decide(980_000, now), post(980_000, false));
        assert_eq!(
            marked.decide(1_020_001, now),
            MarkDecision::OutOfBudget {
                slot: 0,
                target: 1_020_001,
                nearest: 1_020_000
            }
        );
        // Half a window earns half the budget, and waiting longer than one
        // window earns no more.
        assert_eq!(
            marked.decide(1_020_000, 1_000 + window / 2),
            MarkDecision::OutOfBudget {
                slot: 0,
                target: 1_020_000,
                nearest: 1_010_000
            }
        );
        assert_eq!(
            marked
                .decide(1_020_001, now * 100)
                .mark(Address::default(), Grant::Vault),
            None
        );
        // The budget is measured against the smaller of the prior and AUM.
        let small_aum = SlotState {
            validated_aum: 500_000,
            ..marked
        };
        assert!(matches!(
            small_aum.decide(1_010_001, now),
            MarkDecision::OutOfBudget {
                nearest: 1_010_000,
                ..
            }
        ));
        // A one-unit mark has no budget at 2%: it can never reach zero.
        let dust = SlotState {
            last_value: 1,
            ..marked
        };
        assert!(matches!(
            dust.decide(0, now),
            MarkDecision::OutOfBudget { nearest: 1, .. }
        ));
        // A waived cap, or a debt slot, posts whatever the value is.
        for free in [
            SlotState {
                drift_cap_bps: None,
                ..dust
            },
            SlotState { debt: true, ..dust },
        ] {
            assert_eq!(free.decide(0, now), post(0, false));
        }
        let mark = marked
            .decide(1_000_001, now)
            .mark(Address::default(), Grant::JupiterLend)
            .unwrap();
        assert_eq!(
            (mark.value, mark.bootstrap, mark.grant),
            (1_000_001, false, Grant::JupiterLend)
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
