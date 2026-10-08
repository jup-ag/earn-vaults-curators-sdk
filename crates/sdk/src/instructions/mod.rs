mod accept_curator;
mod cancel;
mod cancel_redeem;
mod claim;
mod claim_fees;
mod claim_redeem;
mod close_vault;
mod crystallize_fees;
mod flash;
mod fulfill;
mod fulfill_redeem;
mod grant_revoke_delegate;
mod initialize_vault;
mod migrate_vault;
mod policy;
mod prepare_nest_redeem;
mod price_opaque;
mod protocol_cpi;
mod redeem;
mod redeem_direct;
mod refresh_prices;
mod set_paused;
mod set_protocol_policy;
mod subscribe;
mod subscribe_direct;
mod sweep_idle;
mod update_share_meta;
mod update_vault;
mod validate_aum;

use {
    earn_vaults_interface::{VaultInstruction, zeropod::pod::PodOption},
    solana_address::Address,
};

/// Wrap an `Option<T>` in its zero-copy wire representation.
pub(crate) fn pod_opt<T, P: Copy + From<T>>(value: Option<T>) -> PodOption<P> {
    value.map_or_else(PodOption::none, |value| PodOption::some(P::from(value)))
}

/// Serialize a `VaultInstruction` to its on-chain wire bytes (`[disc][payload]`)
/// with wincode. The buffer is sized via `serialized_size` + written with
/// `serialize_into` (neither needs wincode's `alloc` feature), so wincode stays
/// `default-features = false` for the `no_std` program.
pub(crate) fn encode(ix: &VaultInstruction) -> Vec<u8> {
    let size = wincode::serialized_size(ix).expect("VaultInstruction size must be computable");
    let mut data = vec![0u8; size as usize];
    wincode::serialize_into(&mut data[..], ix)
        .expect("pre-sized VaultInstruction buffer must serialize");
    data
}

pub use {
    accept_curator::*, cancel::*, cancel_redeem::*, claim::*, claim_fees::*, claim_redeem::*,
    close_vault::*, crystallize_fees::*, flash::*, fulfill::*, fulfill_redeem::*,
    grant_revoke_delegate::*, initialize_vault::*, migrate_vault::*, policy::*,
    prepare_nest_redeem::*, price_opaque::*, protocol_cpi::*, redeem::*, redeem_direct::*,
    refresh_prices::*, set_paused::*, set_protocol_policy::*, subscribe::*, subscribe_direct::*,
    sweep_idle::*, update_share_meta::*, update_vault::*, validate_aum::*,
};

const TOKEN_PROGRAM_ID: Address =
    Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022_PROGRAM_ID: Address =
    Address::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const SYSTEM_PROGRAM_ID: Address = Address::from_str_const("11111111111111111111111111111111");
