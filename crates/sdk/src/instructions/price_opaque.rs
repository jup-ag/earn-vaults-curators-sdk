use {
    crate::pda::derive_protocol_delegate,
    earn_vaults_interface::{
        VaultInstruction, arguments::PriceOpaqueArgs, constants::EVENT_PDA, zeropod::pod::PodU64,
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// `price_opaque` — delegate-only (`permissions::vault::PRICE`). Posts
/// `(slot, value)` into `Vault.positions[slot]` with a mandatory drift
/// guard (`|value − prior| / prior ≤ MAX_DRIFT_BPS`; first write bootstraps).
///
/// With `ASSET_TARGET` set on `slot` the same call writes
/// `Vault.assets[idx].last_price` instead — the hand-priced-row path, drift-
/// guarded against the price on the row, no bootstrap (the curator seeds it with
/// `UpdateVault { asset.manual_price }`).
///
/// The gating delegate is the vault-internal one, keyed on
/// `protocol_id == vault_program_id` — the same PDA `GrantRevokeDelegate`
/// creates when granted with `protocol_id: <vault program id>`.
pub struct PriceOpaque {
    /// Delegate keypair; signs the tx and must hold the `PRICE` bit.
    pub signer: Address,
    pub vault: Address,
    /// Index into `Vault.positions` — or
    /// `ASSET_TARGET | <Vault.assets index>` to price a hand-priced asset row
    /// (where `value` is a price in numéraire micro-units per whole token, not
    /// a worth), or `SINK_TARGET | <Vault.sinks index>` to mark seized
    /// collateral.
    pub slot: u8,
    /// Base-asset-denominated value of the position.
    pub value: u64,
}

impl PriceOpaque {
    pub fn into_instruction(self) -> Instruction {
        // protocol_id == vault program id for vault-internal permissions.
        let delegate = derive_protocol_delegate(&self.vault, &self.signer, &crate::ID);

        let args = PriceOpaqueArgs {
            slot_idx: self.slot,
            value: PodU64::from(self.value),
        };
        let data = super::encode(&VaultInstruction::PriceOpaque { args: &args });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new_readonly(self.signer, true),
                AccountMeta::new_readonly(delegate, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
