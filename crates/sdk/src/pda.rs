//! Centralized PDA derivations for the vault program and the protocols it
//! integrates with. The SDK builders (in `crate::instructions::*`) call into
//! these instead of inlining `find_program_address` so callers and tests
//! share a single source of truth.

use {
    earn_vaults_interface::{
        constants::HOOK_ID,
        seeds::{
            AuthoritySeeds, CuratorFeeAccountSeeds, DelegateSeeds, EscrowSeeds,
            EscrowShareAccountSeeds, EscrowTokenAccountSeeds, ExtraAccountMetasSeeds, PolicySeeds,
            ProtocolFeeAccountSeeds, ProtocolPolicySeeds, ShareMintSeeds, VaultSeeds,
        },
        state::Protocol,
    },
    solana_address::Address,
};

pub const ASSOCIATED_TOKEN_PROGRAM_ID: Address =
    Address::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

// --- Vault-program PDAs ------------------------------------------------------

/// `["vault", curator, created_at_le]` on the vault program.
pub fn derive_vault(curator: &Address, created_at: i64) -> Address {
    let created_at = created_at.to_le_bytes();
    Address::find_program_address(
        &VaultSeeds {
            curator,
            created_at: &created_at,
        }
        .as_seeds(),
        &crate::ID,
    )
    .0
}

/// `["share", vault]` — Token-2022 share mint.
pub fn derive_share_mint(vault: &Address) -> Address {
    Address::find_program_address(&ShareMintSeeds { vault }.as_seeds(), &crate::ID).0
}

/// `["escrow", vault, user, nonce]` — pending-request escrow state.
pub fn derive_escrow(vault: &Address, user: &Address, nonce: u64) -> Address {
    Address::find_program_address(
        &EscrowSeeds {
            vault,
            owner: user,
            nonce: &nonce.to_le_bytes(),
        }
        .as_seeds(),
        &crate::ID,
    )
    .0
}

/// `["escrow", vault, user, nonce, "ta"]` — base-asset token account held by
/// the escrow PDA between subscribe and fulfill / fulfill_redeem and claim.
pub fn derive_escrow_token_account(vault: &Address, user: &Address, nonce: u64) -> Address {
    Address::find_program_address(
        &EscrowTokenAccountSeeds {
            vault,
            owner: user,
            nonce: &nonce.to_le_bytes(),
        }
        .as_seeds(),
        &crate::ID,
    )
    .0
}

/// `["escrow", vault, user, nonce, "shares"]` — fulfilled-but-not-yet-claimed
/// share token account.
pub fn derive_escrow_share_account(vault: &Address, user: &Address, nonce: u64) -> Address {
    Address::find_program_address(
        &EscrowShareAccountSeeds {
            vault,
            owner: user,
            nonce: &nonce.to_le_bytes(),
        }
        .as_seeds(),
        &crate::ID,
    )
    .0
}

/// `["fee", vault, "curator"]` — accrued management + performance shares.
pub fn derive_curator_fee_account(vault: &Address) -> Address {
    Address::find_program_address(&CuratorFeeAccountSeeds { vault }.as_seeds(), &crate::ID).0
}

/// `["fee", vault, "protocol"]` — accrued protocol management + performance
/// fee shares.
pub fn derive_protocol_fee_account(vault: &Address) -> Address {
    Address::find_program_address(&ProtocolFeeAccountSeeds { vault }.as_seeds(), &crate::ID).0
}

/// `["authority", vault]` — system-owned signer PDA the vault uses to
/// drive external-protocol CPIs (the vault PDA itself carries data and
/// can't be used as Anchor `init`'s payer).
pub fn derive_vault_authority(vault: &Address) -> Address {
    Address::find_program_address(&AuthoritySeeds { vault }.as_seeds(), &crate::ID).0
}

/// `["protocol_policy", vault, protocol, key]` — one curator policy for a routed protocol
/// (Offerbook: key = collateral mint).
/// A routed call's protocol-wide policy account (key = the zero address),
/// read-only, as the dispatcher expects it right after the CPI's accounts.
pub fn protocol_policy_meta(
    vault: &Address,
    protocol: Protocol,
) -> solana_instruction::AccountMeta {
    solana_instruction::AccountMeta::new_readonly(
        derive_protocol_policy(vault, protocol, &Address::default()),
        false,
    )
}

pub fn derive_protocol_policy(vault: &Address, protocol: Protocol, key: &Address) -> Address {
    Address::find_program_address(
        &ProtocolPolicySeeds {
            vault,
            protocol: &[protocol as u8],
            key,
        }
        .as_seeds(),
        &crate::ID,
    )
    .0
}

/// `["delegate", vault, signer, protocol_program]` — Delegate PDA, scoped to
/// a specific target program. `protocol_program == vault_program_id` for
/// vault-internal grants; an external program ID for `ProtocolCPI` rights.
pub fn derive_protocol_delegate(
    vault: &Address,
    signer: &Address,
    protocol_program: &Address,
) -> Address {
    Address::find_program_address(
        &DelegateSeeds {
            vault,
            delegate: signer,
            protocol_id: protocol_program,
        }
        .as_seeds(),
        &crate::ID,
    )
    .0
}

/// `["policy", vault]` on the vault program — read by the share-mint
/// transfer hook to gate transfers.
pub fn derive_policy(vault: &Address) -> Address {
    Address::find_program_address(&PolicySeeds { vault }.as_seeds(), &crate::ID).0
}

/// `["extra-account-metas", mint]` on the **hook** program (`HOOK_ID`) — the
/// SPL transfer-hook `ExtraAccountMetaList` PDA for the share mint. Derived
/// under the hook's ID, not the vault's: that is where Token-2022 looks.
pub fn derive_extra_account_metas(mint: &Address) -> Address {
    Address::find_program_address(&ExtraAccountMetasSeeds { mint }.as_seeds(), &HOOK_ID).0
}

// --- Associated Token Account -----------------------------------------------

/// `[owner, token_program, mint]` on the ATA program.
pub fn derive_associated_token_account(
    owner: &Address,
    mint: &Address,
    token_program: &Address,
) -> Address {
    Address::find_program_address(
        &[owner.as_array(), token_program.as_array(), mint.as_array()],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}
