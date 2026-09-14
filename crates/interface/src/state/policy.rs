use {
    crate::{check_account_layout, state::Account},
    codama_macros::CodamaAccount,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
};

/// Per-vault compliance / emergency-halt policy. Read by the share-mint
/// transfer hook on every `TransferChecked`. Today we only carry one switch
/// (`paused_transfers`) so the curator can halt all secondary-market share
/// movement during an incident. Future fields: investor-allowlist mode,
/// default lockup duration, capacity caps.
#[derive(SchemaRead, SchemaWrite, CodamaAccount)]
#[codama(discriminator(bytes = [4]))]
#[repr(C)]
pub struct Policy {
    pub vault: Address,
    /// `0` = transfers allowed, `1` = transfers blocked.
    pub paused_transfers: u8,
    pub bump: u8,
    pub authority_bump: u8,
    pub _padding: [u8; 5],
}

impl Account for Policy {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 4;
}

check_account_layout!(Policy);
