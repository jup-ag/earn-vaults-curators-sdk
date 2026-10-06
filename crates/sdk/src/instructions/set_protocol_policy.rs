use {
    crate::{instructions::SYSTEM_PROGRAM_ID, pda::derive_protocol_policy},
    earn_vaults_interface::{
        VaultInstruction,
        arguments::SetProtocolPolicyArgs,
        state::{CollateralTerms, Protocol},
        zeropod::ZcElem,
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// A fixed-size policy payload's bytes (`ZcElem`: align 1, no padding).
fn bytes_of<T: ZcElem>(value: &T) -> Vec<u8> {
    // SAFETY: `T: ZcElem` — every byte of `value` is initialized.
    unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    }
    .to_vec()
}

/// Curator-only `set_protocol_policy`: create, overwrite or (empty `payload`)
/// close the `["protocol_policy", vault, protocol, key]` account, rent paid by
/// and returned to the curator. `extra` are the accounts the protocol's policy
/// validation reads.
pub struct SetProtocolPolicy {
    pub curator: Address,
    pub vault: Address,
    pub protocol: Protocol,
    pub key: Address,
    pub payload: Vec<u8>,
    pub extra: Vec<Address>,
}

impl SetProtocolPolicy {
    /// A protocol-wide policy (key = the zero address); `None` closes it,
    /// which turns the protocol off.
    pub fn protocol_wide<T: ZcElem>(
        curator: Address,
        vault: Address,
        protocol: Protocol,
        payload: Option<T>,
    ) -> Self {
        Self {
            curator,
            vault,
            protocol,
            key: Address::default(),
            payload: payload.as_ref().map_or_else(Vec::new, bytes_of),
            extra: vec![],
        }
    }

    /// Approve `mint` as collateral for the vault's Offerbook principal offers
    /// without listing it; `None` withdraws the approval. A delegate uses the
    /// policy by appending `derive_protocol_policy(vault, Protocol::Offerbook,
    /// mint)` and a Pyth `PriceUpdateV2` for `terms.feed_id` after the 10
    /// `create_token_principal_offer` accounts.
    pub fn offerbook_collateral(
        curator: Address,
        vault: Address,
        mint: Address,
        terms: Option<CollateralTerms>,
    ) -> Self {
        let payload = terms.map_or_else(Vec::new, |terms| {
            let mut bytes = vec![0u8; core::mem::size_of::<CollateralTerms>()];
            wincode::serialize_into(&mut bytes[..], &terms).expect("CollateralTerms is fixed-size");
            bytes
        });
        Self {
            curator,
            vault,
            protocol: Protocol::Offerbook,
            key: mint,
            payload,
            extra: vec![mint],
        }
    }

    pub fn into_instruction(self) -> Instruction {
        let args = SetProtocolPolicyArgs {
            protocol: self.protocol as u8,
            key: self.key,
        };
        let mut accounts = vec![
            AccountMeta::new(self.curator, true),
            AccountMeta::new(self.vault, false),
            AccountMeta::new(
                derive_protocol_policy(&self.vault, self.protocol, &self.key),
                false,
            ),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
        ];
        accounts.extend(
            self.extra
                .into_iter()
                .map(|a| AccountMeta::new_readonly(a, false)),
        );
        Instruction {
            program_id: crate::ID,
            accounts,
            data: super::encode(&VaultInstruction::SetProtocolPolicy {
                args: &args,
                payload: &self.payload,
            }),
        }
    }
}
