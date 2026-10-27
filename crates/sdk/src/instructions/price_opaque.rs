use {
    crate::pda::derive_protocol_delegate,
    earn_vaults_interface::{
        VaultInstruction, arguments::PriceOpaqueArgs, constants::EVENT_PDA, zeropod::U64,
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// `price_opaque` — a curator or `permissions::vault::PRICE` delegate posts
/// `(slot, value)` into `Vault.positions[slot]`. Delegate marks obey the
/// configured drift/bootstrap guard; curator non-debt marks are outright.
///
/// With `ASSET_TARGET` set on `slot` the same call writes
/// `Vault.assets[idx].last_price` instead — the hand-priced-row path, drift-
/// guarded against the price on the row, no bootstrap (the curator seeds it with
/// `UpdateVault { asset.manual_price }`).
///
/// The gating delegate is the vault-internal one, keyed on
/// `protocol_id == vault_program_id` — the same PDA `GrantRevokeDelegate`
/// creates when granted with `protocol_id: <vault program id>`.
/// Use [`Self::into_earn_instruction`] for a scoped Jupiter Earn PRICE grant.
pub struct PriceOpaque {
    /// Curator or delegate keypair; a delegate must hold the `PRICE` bit.
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
        self.into_instruction_for(&crate::ID)
    }

    /// Use `permissions::jupiter_lend::PRICE` on the Jupiter Earn program.
    /// Only an enabled, non-debt, lender-only JupiterLend position is allowed;
    /// the same delegate drift, bootstrap, pause, and flash guards apply.
    pub fn into_earn_instruction(self) -> Instruction {
        const JUPITER_EARN_PROGRAM_ID: Address =
            Address::from_str_const("jup3YeL8QhtSx1e253b2FDvsMNC87fDrgQZivbrndc9");
        self.into_instruction_for(&JUPITER_EARN_PROGRAM_ID)
    }

    fn into_instruction_for(self, protocol_id: &Address) -> Instruction {
        let delegate = derive_protocol_delegate(&self.vault, &self.signer, protocol_id);

        let args = PriceOpaqueArgs {
            slot_idx: self.slot,
            value: U64::from(self.value),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earn_instruction_only_changes_delegate_pda() {
        let build = || PriceOpaque {
            signer: Address::new_from_array([1; 32]),
            vault: Address::new_from_array([2; 32]),
            slot: 3,
            value: 42,
        };
        let mut expected = build().into_instruction();
        let earn = Address::from_str_const("jup3YeL8QhtSx1e253b2FDvsMNC87fDrgQZivbrndc9");
        expected.accounts[1].pubkey =
            derive_protocol_delegate(&build().vault, &build().signer, &earn);
        assert_eq!(build().into_earn_instruction(), expected);
    }
}
