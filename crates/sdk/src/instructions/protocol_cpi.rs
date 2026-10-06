use {
    crate::pda::derive_protocol_delegate,
    earn_vaults_interface::VaultInstruction,
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Builder for `ProtocolCPI`. The on-chain dispatcher routes on
/// `(target_program, first 8 bytes of cpi_data)` to a hardcoded validator
/// that checks the delegate bit, validates account positions, and CPIs
/// signing as the vault-authority PDA.
pub struct ProtocolCpi {
    pub signer: Address,
    pub vault: Address,
    /// Target program of the inner CPI. Also the `protocol_id` used to derive
    /// the `Delegate` PDA — so the curator's permission grant per protocol
    /// pins which programs this signer can drive.
    pub target_program: Address,
    /// CPI accounts in the exact order the inner ix expects (and the order
    /// the on-chain validator's shape table is indexed in), then the vault's
    /// policy accounts the call needs: Offerbook create/fill, CCIP and Nest
    /// take their protocol-wide policy (`pda::protocol_policy_meta`) right
    /// after the CPI's accounts; an Offerbook create against unlisted
    /// collateral then adds that mint's policy and a price update.
    pub forwarded: Vec<AccountMeta>,
    /// Full inner-ix data: first 8 bytes are the inner ix discriminator
    /// (used by the dispatcher to select a validator), rest is the inner args.
    pub cpi_data: Vec<u8>,
}

impl ProtocolCpi {
    pub fn into_instruction(mut self) -> Instruction {
        let delegate = derive_protocol_delegate(&self.vault, &self.signer, &self.target_program);

        let data = super::encode(&VaultInstruction::ProtocolCPI {
            data: self.cpi_data.as_slice(),
        });

        let mut accounts = vec![
            AccountMeta::new_readonly(self.signer, true),
            AccountMeta::new_readonly(delegate, false),
            AccountMeta::new(self.vault, false),
            AccountMeta::new_readonly(self.target_program, false),
        ];
        accounts.append(&mut self.forwarded);

        Instruction {
            program_id: crate::ID,
            accounts,
            data,
        }
    }
}
