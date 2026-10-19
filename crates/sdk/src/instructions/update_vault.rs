use {
    crate::instructions::pod_opt,
    earn_vaults_interface::{
        VaultInstruction,
        arguments::{AssetAdd, AssetUpdate, PositionAdd, PositionUpdate, UpdateVaultArgs},
        constants::EVENT_PDA,
        encoding::PodOption,
        state::{ASSET_FLAG_CHAINLINK, ASSET_FLAG_DEPOSIT_ENABLED},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Curator-only knob update. `None` fields are left untouched on the vault.
///
/// - `paused`: absolute write of the `Pause` bitmask (`Some(0)` clears every
///   pause). The delegate trip-wire is [`crate::instructions::SetPaused`].
/// - `timelock_delay`: seconds between Redeem and ClaimRedeem, capped at
///   `MAX_TIMELOCK_DELAY`.
/// - `pending_curator`: propose the next curator (two-step; they complete via
///   [`crate::instructions::AcceptCurator`]). `Some(Address::default())`
///   cancels an open proposal.
/// - `management_fee_bps`: new curator streaming fee, capped at
///   `MAX_MANAGEMENT_FEE_BPS`; only applies right after `crystallize_fees`
///   (the program rejects with `FeeAccrualPending` otherwise). The global
///   protocol rate is additive.
/// - `instant_redeem_fee_bps`: new instant-redeem fee; `Some(u16::MAX)`
///   disables instant redeems.
/// - `asset_update`: rewrite part of an asset row's oracle config (the
///   rotation path for a deprecated Pyth feed).
/// - `position_update`: rewrite part of a position's risk parameters.
/// - `max_flash_liability`: transient liability cap; `0` disables it.
/// - `sink_*`: seized-collateral capacity, pricing, and release controls.
#[derive(Default)]
pub struct UpdateVault {
    pub curator: Address,
    pub vault: Address,
    pub paused: Option<u8>,
    pub timelock_delay: Option<u64>,
    pub pending_curator: Option<Address>,
    pub management_fee_bps: Option<u16>,
    /// New curator HWM performance fee in bps, capped at
    /// `MAX_PERFORMANCE_FEE_BPS`; same crystallize-first gate as
    /// `management_fee_bps`. The global protocol rate is additive.
    pub performance_fee_bps: Option<u16>,
    pub instant_redeem_fee_bps: Option<u16>,
    pub asset_update: Option<AssetUpdateFields>,
    pub position_update: Option<PositionUpdateFields>,
    /// Transient liability cap in numéraire micro-units.
    pub max_flash_liability: Option<u64>,
    /// Validated AUM age limit; Some(0) resets to 60s. Does not relax oracle limits.
    pub max_aum_staleness_seconds: Option<u32>,
    /// Discount `validate_aum` applies to every sink mark.
    pub sink_haircut_bps: Option<u16>,
    /// Freshness window every sink mark is held to; `0` = the global default.
    pub sink_max_staleness_seconds: Option<u64>,
    /// Vault-wide ceiling on aggregate first sink marks, in bps of validated
    /// AUM; `0` selects the global default, `u16::MAX` waives the ceiling,
    /// and `None` leaves it unchanged. Later marks use the global walk budget.
    pub sink_bootstrap_cap_bps: Option<u16>,
    /// How many sink slots may be claimed, up to `MAX_SINKS`. `Some(0)` stops
    /// new claims; occupied slots keep their marks.
    pub sink_capacity: Option<u8>,
    /// Write a sink's mark off to zero, keeping its mint bound so the balance
    /// stays sellable.
    pub sink_release: Option<u8>,
    /// Onboard a new asset into the first free `Vault.assets` row. Stales the
    /// cached AUM: create the custody ATA, refresh the price, and re-validate
    /// before the vault flows again.
    pub add_asset: Option<AssetAddFields>,
    /// Onboard a new position into a free `Vault.positions` slot (index,
    /// protocol, flags, risk params). Occupied slots stay immutable.
    pub add_position: Option<PositionAdd>,
    /// Retire an emptied asset row and free its slot: deposits must be
    /// disabled, `deployed == 0`, and the row's custody ATA (passed as the
    /// trailing account) must hold zero.
    pub remove_asset: Option<RemoveAssetFields>,
    /// Retire a position row with no live mark (never-priced, reset, or marked
    /// to zero) and free its slot. LENDER_ONLY slots cannot be removed.
    pub remove_position: Option<u8>,
    /// Attest full repayment of an explicit DEBT slot after verifying it
    /// off-chain. Records zero without changing drift limits; revalidate AUM
    /// afterwards. May be combined with `remove_position` for the same slot.
    pub retire_debt: Option<u8>,
}

/// Asset-row removal: the row index plus its canonical custody ATA, which the
/// program checks holds zero before freeing the slot.
pub struct RemoveAssetFields {
    pub index: u8,
    pub custody: Address,
}

/// New asset row: `mint` rides as the instruction's trailing account; a zero
/// `oracle` makes the row hand-priced (and then it may not be
/// deposit-enabled).
pub struct AssetAddFields {
    pub mint: Address,
    pub oracle: Address,
    pub chainlink: bool,
    /// The decimal scale the feed quotes its raw mantissa at; `0` leaves the
    /// source's own. Required for a Data Streams stream that is not the
    /// documented 18 — a v3 report cannot state its scale, and getting it wrong
    /// by 1e10 prices the asset low enough to redeem the holding for dust.
    pub report_decimals: u8,
    pub deposit_enabled: bool,
    /// Strategy-outflow ceiling; `0` disables deployment, `u64::MAX` is unlimited.
    pub max_deployed: u64,
    /// Deposit capacity ceiling; `0` disables deposits, `u64::MAX` is unlimited.
    pub max_deposit: u64,
}

/// Asset-row update: `index` into `Vault.assets` plus the fields to
/// rewrite (`None` fields stay untouched). Oracle rotation is the deprecated
/// feed path. `max_deployed = 0` disables deployment; `u64::MAX` is unlimited.
/// `max_deposit` uses the same convention: `0` disables deposits, `u64::MAX` is unlimited.
#[derive(Default)]
pub struct AssetUpdateFields {
    pub index: u8,
    pub oracle: Option<Address>,
    pub chainlink: Option<bool>,
    /// New declared scale for the row's feed; `Some(0)` hands it back to the
    /// source. Retires the cached reading, as a rotation does.
    pub report_decimals: Option<u8>,
    /// Valuation and settlement age, 0..=u32::MAX; zero selects the 15s default.
    pub max_staleness_seconds: Option<i64>,
    /// Independent future timestamp tolerance; zero rejects future observations.
    pub max_future_skew_seconds: Option<u32>,
    pub max_confidence_bps: Option<u16>,
    /// Strategy-outflow ceiling; `0` disables deployment, `u64::MAX` is unlimited.
    pub max_deployed: Option<u64>,
    /// Restate the remaining `deployed` charge across all protocols in raw token
    /// units after verifying a principal return. CCIP OffRamp deliveries and
    /// Offerbook escrow withdrawals leave this counter unchanged, consuming both
    /// deployment and deposit capacity. Preserve unrelated deployment charges.
    /// Listed Offerbook withdrawals already fold the position mark: do not reset
    /// it; reconcile and re-attest the remaining value with `PriceOpaque`.
    pub deployed: Option<u64>,
    /// Deposit capacity ceiling; `0` disables deposits, `u64::MAX` is unlimited.
    pub max_deposit: Option<u64>,
    /// Toggle deposits for this asset; a hand-priced row may not be
    /// deposit-enabled. The lowest-index enabled row is canonical for instant
    /// settlement; later rows are queued-only.
    pub deposit_enabled: Option<bool>,
    /// Quote leg feed id: the row's price becomes `oracle` × this feed.
    /// `Some(Address::default())` makes it single-feed again. Retires the
    /// cached reading.
    pub quote_oracle: Option<Address>,
    /// Declared scale for the quote leg; retires the cached reading.
    pub quote_report_decimals: Option<u8>,
}

/// Position-row risk-parameter update: `index` into `Vault.positions` plus
/// the fields to rewrite (`None` fields stay untouched). `reset` returns the
/// row to never-priced — the unbrick path for a slot frozen at a zero/dust
/// mark, and the escape hatch for a wrong first mark the drift guard has
/// anchored on (reset → `validate_aum` → re-mark; see `PositionUpdate::reset`).
#[derive(Default)]
pub struct PositionUpdateFields {
    /// Index into `Vault.positions`.
    pub index: u8,
    pub max_staleness_seconds: Option<u64>,
    /// Per-position drift override; `0` selects the global default,
    /// `u16::MAX` waives the budget, and `None` leaves it unchanged.
    pub max_drift_bps: Option<u16>,
    pub reset: Option<bool>,
}

impl AssetUpdateFields {
    fn to_wire(&self) -> AssetUpdate {
        AssetUpdate {
            index: self.index,
            oracle: pod_opt(self.oracle),
            flags: pod_opt(
                self.chainlink
                    .map(|enabled| if enabled { ASSET_FLAG_CHAINLINK } else { 0 }),
            ),
            report_decimals: pod_opt(self.report_decimals),
            max_staleness_seconds: pod_opt(self.max_staleness_seconds),
            max_confidence_bps: pod_opt(self.max_confidence_bps),
            max_deployed: pod_opt(self.max_deployed),
            deployed: pod_opt(self.deployed),
            max_deposit: pod_opt(self.max_deposit),
            deposit_enabled: pod_opt(self.deposit_enabled.map(u8::from)),
            quote_oracle: pod_opt(self.quote_oracle),
            quote_report_decimals: pod_opt(self.quote_report_decimals),
        }
    }
}

impl PositionUpdateFields {
    fn to_wire(&self) -> PositionUpdate {
        PositionUpdate {
            index: self.index,
            max_staleness_seconds: pod_opt(self.max_staleness_seconds),
            max_drift_bps: pod_opt(self.max_drift_bps),
            reset: pod_opt(self.reset.map(u8::from)),
        }
    }
}

impl AssetAddFields {
    fn to_wire(&self) -> AssetAdd {
        let mut flags = 0u8;
        if self.chainlink {
            flags |= ASSET_FLAG_CHAINLINK;
        }
        if self.deposit_enabled {
            flags |= ASSET_FLAG_DEPOSIT_ENABLED;
        }
        AssetAdd {
            oracle: self.oracle,
            flags,
            report_decimals: self.report_decimals,
            max_deployed: self.max_deployed.into(),
            max_deposit: self.max_deposit.into(),
        }
    }
}

impl UpdateVault {
    fn arguments(&self) -> UpdateVaultArgs {
        UpdateVaultArgs {
            paused: pod_opt(self.paused),
            timelock_delay: pod_opt(self.timelock_delay),
            pending_curator: pod_opt(self.pending_curator),
            management_fee_bps: pod_opt(self.management_fee_bps),
            performance_fee_bps: pod_opt(self.performance_fee_bps),
            instant_redeem_fee_bps: pod_opt(self.instant_redeem_fee_bps),
            asset: self
                .asset_update
                .as_ref()
                .map_or_else(PodOption::none, |asset| PodOption::some(asset.to_wire())),
            position: self
                .position_update
                .as_ref()
                .map_or_else(PodOption::none, |position| {
                    PodOption::some(position.to_wire())
                }),
            max_flash_liability: pod_opt(self.max_flash_liability),
            max_aum_staleness_seconds: pod_opt(self.max_aum_staleness_seconds),
            sink_haircut_bps: pod_opt(self.sink_haircut_bps),
            sink_max_staleness_seconds: pod_opt(self.sink_max_staleness_seconds),
            sink_bootstrap_cap_bps: pod_opt(self.sink_bootstrap_cap_bps),
            sink_capacity: pod_opt(self.sink_capacity),
            sink_release: pod_opt(self.sink_release),
            add_asset: self
                .add_asset
                .as_ref()
                .map_or_else(PodOption::none, |asset| PodOption::some(asset.to_wire())),
            add_position: self
                .add_position
                .as_ref()
                .map_or_else(PodOption::none, |position| PodOption::some(*position)),
            remove_asset: pod_opt(self.remove_asset.as_ref().map(|r| r.index)),
            remove_position: pod_opt(self.remove_position),
            retire_debt: pod_opt(self.retire_debt),
            asset_max_future_skew_seconds: pod_opt(
                self.asset_update
                    .as_ref()
                    .and_then(|a| a.max_future_skew_seconds),
            ),
        }
    }

    fn accounts(&self) -> Vec<AccountMeta> {
        let mut accounts = vec![
            AccountMeta::new_readonly(self.curator, true),
            AccountMeta::new(self.vault, false),
            AccountMeta::new_readonly(EVENT_PDA.0, false),
            AccountMeta::new_readonly(crate::ID, false),
        ];
        if let Some(a) = &self.add_asset {
            accounts.push(AccountMeta::new_readonly(a.mint, false));
        }
        if let Some(r) = &self.remove_asset {
            accounts.push(AccountMeta::new_readonly(r.custody, false));
        }
        accounts
    }

    pub fn into_instruction(self) -> Instruction {
        let args = self.arguments();
        let data = super::encode(&VaultInstruction::UpdateVault { args: &args });
        let accounts = self.accounts();

        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> Address {
        Address::new_from_array([byte; 32])
    }

    fn fixture() -> UpdateVault {
        UpdateVault {
            curator: address(1),
            vault: address(2),
            paused: Some(4),
            timelock_delay: Some(0),
            pending_curator: Some(address(3)),
            max_aum_staleness_seconds: Some(900),
            management_fee_bps: Some(100),
            performance_fee_bps: Some(200),
            instant_redeem_fee_bps: Some(u16::MAX),
            asset_update: Some(AssetUpdateFields {
                index: 2,
                oracle: Some(address(4)),
                chainlink: Some(true),
                report_decimals: Some(18),
                max_staleness_seconds: Some(300),
                max_future_skew_seconds: Some(120),
                max_confidence_bps: Some(50),
                max_deployed: Some(1_000),
                deployed: Some(500),
                max_deposit: Some(2_000),
                deposit_enabled: Some(false),
                quote_oracle: Some(address(5)),
                quote_report_decimals: Some(8),
            }),
            position_update: Some(PositionUpdateFields {
                index: 3,
                max_staleness_seconds: Some(60),
                max_drift_bps: Some(200),
                reset: Some(true),
            }),
            max_flash_liability: Some(10_000),
            sink_haircut_bps: Some(100),
            sink_max_staleness_seconds: Some(600),
            sink_bootstrap_cap_bps: Some(250),
            sink_capacity: Some(4),
            sink_release: Some(1),
            add_asset: Some(AssetAddFields {
                mint: address(5),
                oracle: address(6),
                chainlink: true,
                report_decimals: 8,
                deposit_enabled: true,
                max_deployed: 3_000,
                max_deposit: 4_000,
            }),
            remove_asset: Some(RemoveAssetFields {
                index: 7,
                custody: address(7),
            }),
            remove_position: Some(6),
            retire_debt: Some(6),
            ..Default::default()
        }
    }

    #[test]
    fn converts_nested_updates_without_losing_option_semantics() {
        let args = fixture().arguments();
        let asset = args.asset.get().unwrap();
        let position = args.position.get().unwrap();
        let add_asset = args.add_asset.get().unwrap();

        assert_eq!(args.max_aum_staleness_seconds.get().unwrap().get(), 900);
        assert_eq!(args.paused.get(), Some(4));
        assert_eq!(args.timelock_delay.get().map(|value| value.get()), Some(0));
        assert_eq!(asset.index, 2);
        assert_eq!(args.asset_max_future_skew_seconds.get().unwrap().get(), 120);
        assert_eq!(asset.oracle.get(), Some(address(4)));
        assert_eq!(asset.flags.get(), Some(ASSET_FLAG_CHAINLINK));
        assert_eq!(asset.report_decimals.get(), Some(18));
        assert_eq!(
            asset.max_staleness_seconds.get().map(|value| value.get()),
            Some(300)
        );
        assert_eq!(asset.deposit_enabled.get(), Some(0));
        assert_eq!(asset.quote_oracle.get(), Some(address(5)));
        assert_eq!(asset.quote_report_decimals.get(), Some(8));
        assert_eq!(asset.deployed.get().map(|value| value.get()), Some(500));
        assert_eq!(position.index, 3);
        assert_eq!(position.reset.get(), Some(1));
        assert_eq!(
            add_asset.flags,
            ASSET_FLAG_CHAINLINK | ASSET_FLAG_DEPOSIT_ENABLED
        );
        assert_eq!(add_asset.max_deployed.get(), 3_000);
        assert_eq!(add_asset.max_deposit.get(), 4_000);
        assert_eq!(args.remove_asset.get(), Some(7));
        assert_eq!(args.remove_position.get(), Some(6));
        assert_eq!(args.retire_debt.get(), Some(6));
    }

    #[test]
    fn keeps_trailing_accounts_in_program_consumption_order() {
        let update = fixture();
        let accounts = update.accounts();

        assert_eq!(accounts.len(), 6);
        assert_eq!(accounts[4].pubkey, address(5));
        assert_eq!(accounts[5].pubkey, address(7));
        assert!(!accounts[4].is_writable);
        assert!(!accounts[5].is_writable);
    }
}
