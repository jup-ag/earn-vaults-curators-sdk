use {
    crate::{instructions::SYSTEM_PROGRAM_ID, pda::derive_protocol_policy},
    earn_vaults_interface::{
        VaultInstruction,
        arguments::SetProtocolPolicyArgs,
        encoding::ZcElem,
        state::{CollateralTerms, NestRoute, Protocol},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// A fixed-size policy payload's bytes (`ZcElem`: align 1, no padding).
fn bytes_of<T: ZcElem>(value: &T) -> Vec<u8> {
    // SAFETY: `T: ZcElem` — every byte of `value` is initialized.
    unsafe {
        core::slice::from_raw_parts(
            core::ptr::from_ref(value).cast::<u8>(),
            core::mem::size_of::<T>(),
        )
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
    /// Pin the Nest receipt mint as position-only custody. The binding survives
    /// policy close; a new receipt is accepted only once no Nest position remains.
    pub fn nest_route(
        curator: Address,
        vault: Address,
        route: NestRoute,
        oft_store: Address,
        receipt_mint: Address,
    ) -> Self {
        let enabled = route.enabled();
        Self {
            extra: if enabled {
                vec![oft_store, receipt_mint]
            } else {
                vec![]
            },
            ..Self::protocol_wide(curator, vault, Protocol::Nest, enabled.then_some(route))
        }
    }

    /// A protocol-wide policy (key = the zero address); `None` closes it,
    /// which turns the protocol off.
    ///
    /// Use [`Self::nest_route`] for Nest so its receipt proof accounts are included.
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

    /// Approve `mint` as collateral for Offerbook lending and borrowing.
    /// Lending collateral may be unlisted; borrowing collateral must be listed.
    /// Every create and fill needs this policy. `None` withdraws the approval.
    /// The delegate appends it after the Offerbook accounts and global loan limits.
    /// `mint = GENERAL_COLLATERAL` approves lending against any mint.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nest_route_includes_receipt_proof_only_when_enabled() {
        let curator = Address::new_from_array([1; 32]);
        let vault = Address::new_from_array([2; 32]);
        let store = Address::new_from_array([3; 32]);
        let mint = Address::new_from_array([4; 32]);
        let route = NestRoute {
            composer: [5; 20],
            relayer: [6; 20],
            oft_commitment: NestRoute::commitment(&store, &mint),
        };
        let mut expected =
            SetProtocolPolicy::protocol_wide(curator, vault, Protocol::Nest, Some(route))
                .into_instruction();
        expected.accounts.extend([
            AccountMeta::new_readonly(store, false),
            AccountMeta::new_readonly(mint, false),
        ]);
        assert_eq!(
            SetProtocolPolicy::nest_route(curator, vault, route, store, mint).into_instruction(),
            expected
        );
        assert_eq!(
            SetProtocolPolicy::nest_route(curator, vault, NestRoute::zero(), store, mint)
                .into_instruction(),
            SetProtocolPolicy::protocol_wide::<NestRoute>(curator, vault, Protocol::Nest, None)
                .into_instruction()
        );
    }
}
