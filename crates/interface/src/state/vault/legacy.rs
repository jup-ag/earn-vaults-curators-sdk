//! Frozen deployed V1 layout. Only migration tooling may read this account type.
use {
    super::{PositionConfig, SinkSlot},
    crate::state::Account,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI32, PodI64, PodU16, PodU32, PodU64, PodU128},
};

#[derive(SchemaRead, SchemaWrite)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct LegacyAssetConfig {
    pub mint: Address,
    pub oracle: Address,
    pub flags: u8,
    pub decimals: u8,
    pub ata_bump: u8,
    pub max_staleness_seconds: PodU32,
    pub report_decimals: u8,
    pub max_confidence_bps: PodU16,
    pub deployed: PodU64,
    pub max_deployed: PodU64,
    pub max_deposit: PodU64,
    pub last_price: PodI64,
    pub last_conf: PodU64,
    pub last_price_expo: PodI32,
    pub last_publish_time: PodI64,
    pub quote_oracle: Address,
    pub quote_report_decimals: u8,
    pub base_publish_time: PodI64,
    pub quote_publish_time: PodI64,
}

#[derive(SchemaRead, SchemaWrite)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct LegacyVault {
    // Identity
    pub curator: Address,
    pub share_mint: Address,
    pub pending_curator: Address,
    pub created_at: PodI64,
    pub authority_bump: u8,

    // Configuration
    pub paused: u8,
    pub management_fee_bps: PodU16,
    pub instant_redeem_fee_bps: PodU16,
    pub sink_haircut_bps: PodU16,
    pub sink_max_staleness_seconds: PodU64,
    pub timelock_delay: PodU64,
    pub max_flash_liability: PodU64,

    // Live state
    pub aum: PodU64,
    pub last_priced_at: PodI64,
    pub last_priced_slot: PodU64,
    pub pending_redeem_shares: PodU64,
    pub flash_liability: PodU64,
    pub hwm_nav: PodU128,
    pub fee_capital_time: PodU128,
    pub last_fee_accrual_at: PodI64,
    pub perf_fee_accrued: PodU64,
    pub swap_slippage_spent: PodU64,
    pub swap_slippage_last_refill_at: PodI64,
    pub liq_slippage_spent: PodU64,
    pub liq_slippage_last_refill_at: PodI64,

    // Rows
    pub assets: [LegacyAssetConfig; 8],
    pub positions: [PositionConfig; 8],
    pub sinks: [SinkSlot; 4],
    pub sink_bootstrap_cap_bps: PodU16,
    pub sink_capacity: u8,
    pub performance_fee_bps: PodU16,
    pub curator_fee_account_bump: u8,
    pub protocol_fee_account_bump: u8,
    pub aum_conf_bps: PodU16,
    pub child_count: PodU16,
    pub reserved: [u8; 64],
}

impl Account for LegacyVault {
    const LEN: usize = 2_152;
    const DISCRIMINATOR: u8 = 1;
}
const _: () = assert!(core::mem::size_of::<LegacyVault>() + 1 == LegacyVault::LEN);
const _: () = assert!(core::mem::size_of::<LegacyAssetConfig>() == 175);
const _: () = assert!(core::mem::offset_of!(LegacyVault, reserved) == 2_087);
