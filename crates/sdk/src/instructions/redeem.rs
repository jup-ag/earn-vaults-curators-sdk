use {
    crate::{
        instructions::TOKEN_2022_PROGRAM_ID,
        pda::{derive_escrow, derive_share_mint, derive_vault_authority},
    },
    earn_vaults_interface::{VaultInstruction, constants::EVENT_PDA, zeropod::U64},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
};

/// Burns the user's shares (atomic, matching GLAM's burn-on-enqueue model)
/// and stamps an `Escrow` record the manager later fulfills against.
/// `nonce` discriminates the escrow PDA — a user may hold several concurrent
/// requests with distinct nonces.
/// The user pins `payout_mint` and a minimum token/share ratio. Every fill must
/// pay at least `ceil(min_out * settled_shares / original_shares)` raw tokens.
/// The original denominator stays fixed across partial fills. Unfilled shares
/// may be cancelled; collecting partial proceeds still observes the timelock.
/// Appends the vault authority PDA for the share mint's permissioned burn.
pub struct Redeem {
    pub user: Address,
    pub user_share_account: Address,
    pub vault: Address,
    pub shares: u64,
    pub nonce: u64,
    /// Minimum raw payout tokens for the whole original request; zero opts out
    /// of the rate bound but still pins the payout mint.
    pub min_out: u64,
    /// Must be a configured, deposit-enabled, AUM-priced vault asset.
    pub payout_mint: Address,
}

impl Redeem {
    pub fn into_instruction(self) -> Instruction {
        let share_mint = derive_share_mint(&self.vault);
        let escrow = derive_escrow(&self.vault, &self.user, self.nonce);

        let shares = U64::from(self.shares);
        let nonce = U64::from(self.nonce);
        let min_out = U64::from(self.min_out);
        let data = super::encode(&VaultInstruction::Redeem {
            shares: &shares,
            nonce: &nonce,
            min_out: &min_out,
            payout_mint: &self.payout_mint,
        });

        Instruction {
            program_id: crate::ID,
            accounts: vec![
                AccountMeta::new(self.user, true),
                AccountMeta::new(self.user_share_account, false),
                AccountMeta::new(self.vault, false),
                AccountMeta::new(share_mint, false),
                AccountMeta::new(escrow, false),
                AccountMeta::new_readonly(crate::instructions::SYSTEM_PROGRAM_ID, false),
                AccountMeta::new_readonly(TOKEN_2022_PROGRAM_ID, false),
                AccountMeta::new_readonly(EVENT_PDA.0, false),
                AccountMeta::new_readonly(crate::ID, false),
                AccountMeta::new_readonly(derive_vault_authority(&self.vault), false),
            ],
            data,
        }
    }
}
