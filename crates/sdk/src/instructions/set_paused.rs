use {
    crate::pda::derive_protocol_delegate,
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA},
};

/// `set_paused` — granular pause over `Vault.paused` (`Pause::Subscribe` /
/// `Pause::Redeem` / `Pause::Strategy`). Delegate-only trip-wire: requires the
/// `vault::PAUSE` bit and ORs bits in — never clears. Clearing is curator-only
/// via [`crate::instructions::UpdateVault`].
pub struct SetPaused {
    /// PAUSE-delegate keypair; signs the tx.
    pub signer: Address,
    pub vault: Address,
    /// `Pause` bitmask.
    pub flags: u8,
}

impl SetPaused {
    pub fn into_instruction(self) -> Instruction {
        // protocol_id == vault program id for vault-internal permissions.
        let delegate = derive_protocol_delegate(&self.vault, &self.signer, &crate::ID);

        let data = super::encode(&VaultInstruction::SetPaused { flags: &self.flags });

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
