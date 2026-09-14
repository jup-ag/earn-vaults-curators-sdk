use {
    crate::{check_account_layout, errors::VaultError, state::Account},
    codama_macros::CodamaAccount,
    solana_account_view::AccountView,
    solana_address::{Address, address_eq},
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::PodU64,
};

#[derive(SchemaRead, SchemaWrite, CodamaAccount)]
#[codama(discriminator(bytes = [2]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Delegate {
    pub vault: Address,
    pub delegate: Address,
    pub protocol_id: Address,
    #[codama(type = number(u64))]
    pub permissions: PodU64,
    pub bump: u8,
}

impl Delegate {
    #[inline(always)]
    pub fn check_state(
        &self,
        signer: &Address,
        vault: &Address,
        protocol: &Address,
    ) -> Result<(), ProgramError> {
        if !address_eq(&self.vault, vault)
            || !address_eq(&self.delegate, signer)
            || !address_eq(&self.protocol_id, protocol)
        {
            Err(VaultError::InvalidDelegate.into())
        } else {
            Ok(())
        }
    }

    #[inline]
    pub fn require_permission(&self, bits: u64) -> Result<(), ProgramError> {
        if self.permissions.get() & bits == 0 {
            Err(VaultError::MissingPermission.into())
        } else {
            Ok(())
        }
    }
}

/// Which way the vault's token account moves during a routed CPI. `Out` =
/// funds leave (deployed↑); `In` = funds return (deployed↓).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Out,
    In,
}

/// The debt side of a [`Settlement::Operate`] reconciliation.
///
/// A borrow folds the full custody inflow (plus any authenticated protocol
/// borrow fee) into the position mark. A repay folds the full observed custody
/// outflow, but is permitted only immediately after a same-second opaque mark.
/// That attestation first books accrued interest into the position; folding the
/// extinguished liability then keeps NAV continuous without manufacturing an
/// asset from previously unmarked interest.
#[derive(Clone, Copy)]
pub enum DebtLeg {
    Borrow { ta_index: usize },
    Repay { ta_index: usize },
}

/// What `protocol_cpi` must reconcile after a routed CPI. `None` from the
/// validator means no vault-held balance moves (e.g. create/cancel offer).
/// Indices are into the forwarded account list.
pub enum Settlement {
    Deployed {
        ta_index: usize,
        /// Mint account already present in the authenticated protocol CPI
        /// layout. The dispatcher binds its address to the custody token
        /// account's recorded mint before using it for extension checks.
        mint_index: usize,
        flow: Flow,
        protocol: u8,
        /// Clear the tracked position's `PRICED` pre-CPI, failing
        /// `validate_aum` closed until re-priced. Set for out-flows that would
        /// otherwise vanish from NAV. Every successful listed fold also clears
        /// `PRICED` after updating the mark, independently of this pre-CPI flag.
        dirty_tracked: bool,
        /// Fold this leg's priced value into the tracked position's mark. A
        /// listed mint with this clear is `CpiSettlementInvalid` — the
        /// validator must decide, not the dispatcher. Ignored for an unlisted
        /// mint, which has no row to price against. Listed folds use the tighter
        /// settlement freshness window because the cached value is booked into
        /// the mark, even though the mark then requires keeper reconciliation.
        fold_mark: bool,
        /// Allow the moved mint to have no `assets[]` row: skips valuation and
        /// `deployed` bookkeeping, so the value is carried only by the tracked
        /// position's opaque mark. A validator setting this MUST pin both
        /// endpoints itself. This is restricted to authenticated in-flows such
        /// as seized collateral returning from Offerbook; an unlisted out-flow
        /// is `DisallowedAsset`, because it could otherwise round-trip arbitrary
        /// custody into a sink claim.
        allow_unlisted: bool,
    },
    /// A swap: `source_index` debited, `dest_index` credited. Nothing is added
    /// to `deployed` (idle→idle). The dispatcher resolves the source mint
    /// against the asset list and takes one of two paths:
    ///
    /// - **Listed source** (`allow_listed`, the `SWAP` bit): pins both token
    ///   accounts to the vault's custody ATAs and floors the received amount
    ///   at the oracle-derived value of the sold amount (source row's cached
    ///   price → destination row's, minus `MAX_SWAP_SLIPPAGE_BPS`). Oracle rows
    ///   must have been refreshed inside the tighter settlement freshness
    ///   window — the delegate can't price its own route. Manual rows retain
    ///   their configured row window.
    /// - **Unlisted source** (`allow_unlisted`, the `LIQUIDATE` bit): seized
    ///   collateral, no asset row, so no oracle floor. Destination must be a
    ///   deposit-enabled (base) asset, and the floor comes from the `Vault.sinks`
    ///   slot holding the mint being sold: `mark × (1 − MAX_LIQ_SLIPPAGE_BPS)`,
    ///   where `mark` must be positive (a written-off sink must be re-marked).
    ///   Destination proceeds are valued through the same tighter settlement
    ///   freshness gate, with the shortfall charged to a linearly refilling
    ///   leaky bucket. The
    ///   sink is looked up *by* that mint, so the floor can only ever be the
    ///   mint's own mark — there is no stamp to compare, and so none to forget.
    ///   Must be a **full sale** of the source ATA —
    ///   a partial sale of a permissionlessly-fundable ATA would let a
    ///   delegate dilute the pro-rata denominator. The mark is written down
    ///   atomically (custody +V, mark −V, AUM-neutral) so proceeds can't be
    ///   double-counted. It does not debit `deployed` on the destination row:
    ///   sale proceeds prove where value landed, not which strategy outflow's
    ///   principal they repay. Without authenticated attribution the counter
    ///   remains conservatively charged. Fluid position ownership mints look
    ///   like unlisted custody but convey the whole position, so they're
    ///   rejected before the CPI.
    ///
    /// A listed source never takes the unlisted path, so `LIQUIDATE` is not a
    /// floor bypass.
    Swap {
        source_index: usize,
        dest_index: usize,
        /// Delegate holds `jupiter::SWAP` — listed-source floored path allowed.
        allow_listed: bool,
        /// Delegate holds `jupiter::LIQUIDATE` — unlisted-source path allowed.
        allow_unlisted: bool,
    },
    /// A routed leg that moves value into the vault as idle spot balance
    /// against an opaque position (offerbook loan claim/fill/repay). No single
    /// TA delta to reconcile, but the proceeds land as spot AUM, so the
    /// dispatcher requires a matching `positions[*]` (by `protocol` field, not
    /// a fixed slot) that is enabled + `required_in_aum` — else the principal
    /// counts as untracked equity and inflates NAV. `required_in_aum` makes
    /// `validate_aum` fail closed on a missing/stale mark, forcing the
    /// (drift-guarded) re-price.
    RequireTrackedPosition { protocol: u8 },
    /// A protocol operation: one or two custody TAs move against an opaque
    /// position (JupiterLend collateral/debt or an Offerbook borrow/repay).
    /// Each active
    /// leg is snapshot-reconciled like [`Settlement::Deployed`] and its
    /// oracle-valued delta folded into the tracked position's mark — out-flows
    /// raise it and in-flows lower it. A debt repay additionally requires the
    /// tracked position to have been explicitly marked in the current unix
    /// second. The fresh mark first recognizes accrued interest; the full
    /// observed repayment can then restore the extinguished liability without
    /// turning old, unmarked interest into a phantom asset. Borrow fees are
    /// still folded into the liability on the borrow side. Both legs price
    /// through the tighter settlement window; oracle-backed rows therefore
    /// need a near-contemporaneous `refresh_prices` (manual rows retain their
    /// configured row window).
    ///
    /// The fold is objective (actual token movements × last validated price),
    /// so it needs no drift guard and updates the stored mark by the complete
    /// movement. But a later reverse leg can read a different cached price, so
    /// every fold clears `PRICED`: `validate_aum` stays closed until a keeper
    /// reconciles the resulting mark. Re-attesting the folded value consumes no
    /// drift budget, including after a large legitimate movement.
    ///
    /// It never extends the mark's **freshness**: a fold proves only the delta
    /// it moved, while the rest of the position keeps accruing invisibly (Fluid
    /// interest, an Earn fToken appreciating past its cost basis, anything past
    /// the borrow fee). Stamping `last_priced_at` here would let a $1 `operate`
    /// reset the staleness clock forever and suppress the `PricedSlotStale`
    /// halt that forces the keeper to book that accrual — `price_opaque` is the
    /// only writer of freshness.
    Operate {
        /// `(ta_index, flow)` of the collateral leg, when `new_col != 0`.
        col_leg: Option<(usize, Flow)>,
        /// Borrow or same-second-mark-gated repay debt leg, when debt moves.
        debt_leg: Option<DebtLeg>,
        protocol: u8,
        /// Fluid `VaultConfig.borrow_fee`, in four-decimal basis points.
        borrow_fee_bps: u16,
    },
}

impl Account for Delegate {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 2;
}

check_account_layout!(Delegate);

pub trait ProtocolCPI {
    const PROGRAM_ID: Address;

    fn validate(
        vault_authority: &Address,
        accounts: &mut [AccountView],
        data: &[u8],
        permissions: u64,
    ) -> Result<Option<Settlement>, ProgramError>;
}
