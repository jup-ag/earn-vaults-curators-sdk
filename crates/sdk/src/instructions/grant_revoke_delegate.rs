use {
    crate::{instructions::SYSTEM_PROGRAM_ID, pda::derive_protocol_delegate},
    earn_vaults_interface::{VaultInstruction, zeropod::pod::PodU64},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Curator-only `grant_revoke_delegate`.
///
/// Grants are scoped per-protocol: the `Delegate` PDA is keyed on
/// `protocol_id`, so one signer can hold several grants against the same
/// vault (one per protocol it's allowed to drive). Pass
/// `protocol_id == vault_program_id` for vault-internal permissions
/// (`Fulfill`, pricers, KYC) and the external program ID for `ProtocolCPI`
/// rights. This writes the same PDA `protocol_cpi` / the vault-internal
/// handlers read.
///
/// `permissions == 0` revokes (closes the `Delegate` PDA, rent returned to
/// the curator). Any non-zero bitmap grants — creates the PDA if absent,
/// otherwise overwrites the existing bitmap. Revoking one PDA leaves the
/// signer's grants on other protocols untouched.
/// A LENDER_ONLY Offerbook slot rejects borrower permissions; a LENDER_ONLY
/// JupiterLend slot rejects every Fluid Vaults grant.
pub struct GrantRevokeDelegate {
    pub curator: Address,
    pub vault: Address,
    pub delegate: Address,
    /// Protocol this grant scopes to. `vault_program_id` = vault-internal;
    /// an external program ID = `ProtocolCPI` rights against that program.
    pub protocol_id: Address,
    pub permissions: u64,
}

impl GrantRevokeDelegate {
    pub fn into_instruction(self) -> Instruction {
        let delegate_pda = derive_protocol_delegate(&self.vault, &self.delegate, &self.protocol_id);

        let permissions = PodU64::from(self.permissions);
        let data = super::encode(&VaultInstruction::GrantRevokeDelegate {
            permissions: &permissions,
        });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.curator, true),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(delegate_pda, false),
                AccountMeta::new_readonly(self.delegate, false),
                AccountMeta::new_readonly(self.protocol_id, false),
                AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            ],
            data,
        }
    }
}
