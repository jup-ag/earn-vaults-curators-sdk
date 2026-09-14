use {
    crate::{
        SdkErrors,
        instructions::{SYSTEM_PROGRAM_ID, TOKEN_2022_PROGRAM_ID, pod_opt},
        pda::{derive_policy, derive_share_mint, derive_vault, derive_vault_authority},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{
        VaultInstruction,
        arguments::{AssetInit, InitializeVaultArgs, PositionInit},
        constants::EVENT_PDA,
        state::{ASSET_FLAG_CHAINLINK, MAX_ALLOWED_ASSETS, MAX_POSITIONS},
        zeropod::pod::{PodOption, PodU16, PodU64},
    },
};

pub struct InitializeVault<'a> {
    pub curator: Address,
    pub created_at: i64,
    /// Ordered deposit assets. The first is canonical for `SubscribeDirect`
    /// and `RedeemDirect`; any remaining entries are queued-only.
    pub base_assets: Vec<Address>,
    pub base_oracles: Vec<Address>,
    pub other_assets: Vec<Address>,
    pub other_oracles: Vec<Address>,
    /// Per-asset strategy-outflow ceilings, same order as `base_assets` then
    /// `other_assets`; empty = unlimited for all.
    pub max_deployed: Vec<u64>,
    /// Per-asset deposit capacity ceilings; empty = unlimited for all.
    pub max_deposit: Vec<u64>,
    /// Rows whose `oracle` is a **Chainlink Data Streams** feed id rather than a
    /// Pyth one, by index into `base_assets` then `other_assets`; empty = all
    /// Pyth. A Chainlink row is refreshed with `RefreshPrice`, not by
    /// `ValidateAum`, and takes no feed account in its `PricedAssetLeg`.
    pub chainlink_assets: Vec<usize>,
    /// The decimal scale each feed quotes its raw mantissa at, same order as
    /// `max_deployed`; empty = the source's own for all. A Data Streams v3
    /// report carries no exponent and the schema allows 8 or 18 per stream, so
    /// an undeclared 8-decimal stream prices its asset 1e10 low — a free
    /// redemption. For a Pyth row a non-zero entry pins the expected exponent
    /// instead, and a feed that re-scales is rejected.
    pub report_decimals: Vec<u8>,
    pub positions: Vec<PositionInit>,
    /// Seconds between `Redeem` and `ClaimRedeem`. `None` takes the default
    /// cool-down (`DEFAULT_TIMELOCK_DELAY`, 1 day), `Some(0)` waives it,
    /// `Some(n)` is `n` seconds (capped at `MAX_TIMELOCK_DELAY`).
    pub timelock_delay: Option<u64>,
    /// Annual curator management fee on AUM, in bps. The global protocol
    /// management fee is additive.
    pub management_fee_bps: u16,
    /// Curator HWM performance fee on NAV gains, in bps, capped at
    /// `MAX_PERFORMANCE_FEE_BPS`. `0` disables the curator component; the
    /// global protocol performance fee remains additive.
    pub performance_fee_bps: u16,
    /// Exit fee on the instant (`RedeemDirect`) path. `None` leaves the path
    /// disabled; `Some(n)` enables it at `n` bps, which must sit inside
    /// `MIN_INSTANT_REDEEM_FEE_BPS..=MAX_INSTANT_REDEEM_FEE_BPS`.
    pub instant_redeem_fee_bps: Option<u16>,
    /// Ceiling on the transient flash liability, in numéraire micro-units. `0`
    /// leaves the path disabled.
    pub max_flash_liability: u64,
    /// Discount `validate_aum` applies to every sink mark.
    pub sink_haircut_bps: u16,
    /// Freshness window every sink mark is held to. `None` = the global
    /// default, `Some(0)` = the week-long ceiling, `Some(n)` = `n` seconds
    /// (capped at that week, so the argument is 32-bit).
    pub sink_max_staleness_seconds: Option<u32>,
    /// Vault-wide ceiling on aggregate first sink marks, in bps of validated
    /// AUM. `None` = the global default, `Some(0)` waives the ceiling,
    /// `Some(n)` = `n` bps. Later marks use the global walk budget.
    pub sink_bootstrap_cap_bps: Option<u16>,
    /// How many sink slots may be claimed, up to `MAX_SINKS`. `0` leaves
    /// seized-collateral custody disabled — an unlisted in-flow then reverts
    /// rather than provisioning a delegate-attested AUM row.
    pub sink_capacity: u8,
    pub metadata_name: &'a str,
    pub metadata_symbol: &'a str,
    pub metadata_uri: &'a str,
}

impl InitializeVault<'_> {
    fn asset_count(&self) -> Result<usize, SdkErrors> {
        if self.base_oracles.len() != self.base_assets.len() {
            return Err(SdkErrors::InvalidArgs("need an oracle per base asset"));
        }
        if self.other_oracles.len() != self.other_assets.len() {
            return Err(SdkErrors::InvalidArgs("need an oracle per other asset"));
        }
        if self.base_assets.len() + self.other_assets.len() > MAX_ALLOWED_ASSETS {
            return Err(SdkErrors::InvalidArgs("too many assets"));
        }
        let asset_count = self.base_assets.len() + self.other_assets.len();
        if !self.max_deployed.is_empty() && self.max_deployed.len() != asset_count {
            return Err(SdkErrors::InvalidArgs("need a max_deployed per asset"));
        }
        if !self.max_deposit.is_empty() && self.max_deposit.len() != asset_count {
            return Err(SdkErrors::InvalidArgs("need a max_deposit per asset"));
        }
        if !self.report_decimals.is_empty() && self.report_decimals.len() != asset_count {
            return Err(SdkErrors::InvalidArgs("need a report_decimals per asset"));
        }

        Ok(asset_count)
    }

    fn pack_assets(
        &self,
        asset_count: usize,
    ) -> Result<(u8, [AssetInit; MAX_ALLOWED_ASSETS], Vec<Address>), SdkErrors> {
        let mut based_assets = 0u8;
        let mut assets = [AssetInit {
            oracle: Address::new_from_array([0; 32]),
            flags: 0,
            report_decimals: 0,
            max_deployed: PodU64::ZERO,
            max_deposit: PodU64::ZERO,
        }; MAX_ALLOWED_ASSETS];
        let mut mints: Vec<Address> = Vec::with_capacity(asset_count);
        for (i, (asset_mint, oracle)) in self.base_assets.iter().zip(&self.base_oracles).enumerate()
        {
            based_assets |= 1u8 << i;
            assets[i].oracle = *oracle;
            mints.push(*asset_mint);
        }
        for (j, (asset_mint, oracle)) in self
            .other_assets
            .iter()
            .zip(&self.other_oracles)
            .enumerate()
        {
            assets[self.base_assets.len() + j].oracle = *oracle;
            mints.push(*asset_mint);
        }
        for (i, v) in self.max_deployed.iter().enumerate() {
            assets[i].max_deployed = PodU64::from(*v);
        }
        for (i, v) in self.max_deposit.iter().enumerate() {
            assets[i].max_deposit = PodU64::from(*v);
        }
        for i in &self.chainlink_assets {
            if *i >= asset_count {
                return Err(SdkErrors::InvalidArgs("chainlink asset index out of range"));
            }
            assets[*i].flags |= ASSET_FLAG_CHAINLINK;
        }
        for (i, v) in self.report_decimals.iter().enumerate() {
            assets[i].report_decimals = *v;
        }

        Ok((based_assets, assets, mints))
    }

    fn pack_positions(&self) -> Result<[PositionInit; MAX_POSITIONS as usize], SdkErrors> {
        // Slots are free: the caller's order is the slot order, and each
        // position names its own protocol. The program rejects two *enabled*
        // slots on one protocol.
        let zero = PositionInit {
            protocol: 0,
            flags: 0,
            max_staleness_seconds: PodOption::none(),
            max_drift_bps: PodOption::none(),
            haircut_bps: PodU16::from(0),
        };
        let mut positions = [zero; MAX_POSITIONS as usize];
        if self.positions.len() > MAX_POSITIONS as usize {
            return Err(SdkErrors::InvalidArgs("too many positions"));
        }
        for (slot, p) in self.positions.iter().enumerate() {
            positions[slot] = *p;
        }

        Ok(positions)
    }

    pub fn into_instruction(self) -> Result<Instruction, SdkErrors> {
        let vault = derive_vault(&self.curator, self.created_at);
        let mint = derive_share_mint(&vault);
        let asset_count = self.asset_count()?;
        let (based_assets, assets, mints) = self.pack_assets(asset_count)?;
        let positions = self.pack_positions()?;

        let arguments = InitializeVaultArgs {
            based_assets,
            assets,
            positions,
            created_at: self.created_at.into(),
            timelock_delay: pod_opt(self.timelock_delay),
            management_fee_bps: self.management_fee_bps.into(),
            performance_fee_bps: self.performance_fee_bps.into(),
            instant_redeem_fee_bps: pod_opt(self.instant_redeem_fee_bps),
            max_flash_liability: self.max_flash_liability.into(),
            sink_haircut_bps: self.sink_haircut_bps.into(),
            sink_max_staleness_seconds: pod_opt(self.sink_max_staleness_seconds),
            sink_bootstrap_cap_bps: pod_opt(self.sink_bootstrap_cap_bps),
            sink_capacity: self.sink_capacity,
            metadata_name: self.metadata_name.try_into()?,
            metadata_symbol: self.metadata_symbol.try_into()?,
            metadata_uri: self.metadata_uri.try_into()?,
        };

        let data = super::encode(&VaultInstruction::InitializeVault { args: &arguments });

        let mut accounts = vec![
            AccountMeta::new(self.curator, true),
            AccountMeta::new(vault, false),
            AccountMeta::new_readonly(derive_vault_authority(&vault), false),
            AccountMeta::new(mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
            AccountMeta::new_readonly(EVENT_PDA.0, false),
            AccountMeta::new_readonly(crate::ID, false),
            AccountMeta::new(derive_policy(&vault), false),
        ];
        for asset_mint in &mints {
            accounts.push(AccountMeta::new_readonly(*asset_mint, false));
        }

        Ok(Instruction {
            program_id: crate::ID,
            accounts,
            data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(byte: u8) -> Address {
        Address::new_from_array([byte; 32])
    }

    fn fixture() -> InitializeVault<'static> {
        InitializeVault {
            curator: address(1),
            created_at: 42,
            base_assets: vec![address(2)],
            base_oracles: vec![address(3)],
            other_assets: vec![address(4)],
            other_oracles: vec![address(5)],
            max_deployed: vec![10, 20],
            max_deposit: vec![30, 40],
            chainlink_assets: vec![1],
            report_decimals: vec![8, 18],
            positions: vec![PositionInit {
                protocol: 2,
                flags: 9,
                max_staleness_seconds: PodOption::none(),
                max_drift_bps: PodOption::none(),
                haircut_bps: PodU16::from(50),
            }],
            timelock_delay: Some(0),
            management_fee_bps: 100,
            performance_fee_bps: 200,
            instant_redeem_fee_bps: None,
            max_flash_liability: 0,
            sink_haircut_bps: 0,
            sink_max_staleness_seconds: None,
            sink_bootstrap_cap_bps: None,
            sink_capacity: 0,
            metadata_name: "Share Token",
            metadata_symbol: "YLD",
            metadata_uri: "https://example.com",
        }
    }

    #[test]
    fn packs_assets_and_positions_in_wire_order() {
        let init = fixture();
        let count = init.asset_count().unwrap();
        let (based_assets, assets, mints) = init.pack_assets(count).unwrap();
        let positions = init.pack_positions().unwrap();

        assert_eq!(count, 2);
        assert_eq!(based_assets, 1);
        assert_eq!(mints, vec![address(2), address(4)]);
        assert_eq!(assets[0].oracle, address(3));
        assert_eq!(assets[0].max_deployed.get(), 10);
        assert_eq!(assets[0].max_deposit.get(), 30);
        assert_eq!(assets[0].report_decimals, 8);
        assert_eq!(assets[1].oracle, address(5));
        assert_eq!(assets[1].flags, ASSET_FLAG_CHAINLINK);
        assert_eq!(assets[1].max_deployed.get(), 20);
        assert_eq!(assets[1].max_deposit.get(), 40);
        assert_eq!(assets[1].report_decimals, 18);
        assert_eq!(positions[0].protocol, 2);
        assert_eq!(positions[0].flags, 9);
        assert_eq!(positions[0].haircut_bps.get(), 50);
        assert_eq!(positions[1].flags, 0);
    }

    #[test]
    fn rejects_incoherent_vector_lengths_and_chainlink_indices() {
        let mut init = fixture();
        init.base_oracles.clear();
        assert!(matches!(
            init.asset_count(),
            Err(SdkErrors::InvalidArgs("need an oracle per base asset"))
        ));

        let mut init = fixture();
        init.max_deposit.pop();
        assert!(matches!(
            init.asset_count(),
            Err(SdkErrors::InvalidArgs("need a max_deposit per asset"))
        ));

        let mut init = fixture();
        init.chainlink_assets = vec![2];
        let count = init.asset_count().unwrap();
        assert!(matches!(
            init.pack_assets(count),
            Err(SdkErrors::InvalidArgs("chainlink asset index out of range"))
        ));
    }

    #[test]
    fn instruction_keeps_asset_mints_after_fixed_accounts() {
        let ix = fixture().into_instruction().unwrap();

        assert_eq!(ix.accounts.len(), 11);
        assert_eq!(ix.accounts[9].pubkey, address(2));
        assert_eq!(ix.accounts[10].pubkey, address(4));
        assert!(!ix.accounts[9].is_writable);
        assert!(!ix.accounts[10].is_writable);
    }
}
