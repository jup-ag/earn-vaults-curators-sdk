use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{derive_share_mint, derive_vault_authority},
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::U64},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Instant withdrawal: burns `shares` and pays the base asset straight to the
/// user, net of the vault's `instant_redeem_fee_bps` (minted to the curator as
/// shares). Reverts if the path is disabled (`InstantRedeemDisabled`), the AUM
/// snapshot is stale (`StalePrice`), or idle base can't cover the full payout
/// (`InsufficientIdleLiquidity`) — fall back to the queued [`crate::Redeem`].
/// `min_out` is the least base-asset amount (base units, after the fee) the
/// user accepts; a smaller payout reverts with `RedeemOutputBelowMinimum`.
/// `0` disables the bound.
/// `base_mint` must be the vault's canonical base asset (the first configured,
/// deposit-enabled row); later deposit-enabled rows are queued-only.
///
/// `curator_share_account` is the curator's account for the share mint (fee
/// destination, validated on-chain); `user_token_account` must already exist.
pub struct RedeemDirect {
    pub user: Address,
    pub user_token_account: Address,
    pub user_share_account: Address,
    pub vault: Address,
    pub base_mint: Address,
    pub vault_token_account: Address,
    pub curator_share_account: Address,
    pub base_token_program: Address,
    pub shares: u64,
    pub min_out: u64,
}

impl RedeemDirect {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let vault_authority = derive_vault_authority(&self.vault);

        let shares = U64::from(self.shares);
        let min_out = U64::from(self.min_out);
        let data = super::encode(&VaultInstruction::RedeemDirect {
            shares: &shares,
            min_out: &min_out,
        });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new(self.user_token_account, false),
                AccountMeta::new(self.user_share_account, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(share_mint, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new(self.vault_token_account, false),
                AccountMeta::new(self.curator_share_account, false),
                AccountMeta::new_readonly(vault_authority, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(self.base_token_program, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
            ],
            data,
        }
    }
}
