use {
    crate::{
        check_account_layout,
        constants::{
            BOOTSTRAP_HWM_NAV, MAX_DRIFT_BPS, MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS,
            MAX_STALENESS_SECONDS, MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS, NAV_SCALE,
            PROTOCOL_MANAGEMENT_FEE_BPS, PROTOCOL_PERFORMANCE_FEE_BPS, SECONDS_PER_YEAR,
            SWAP_SLIPPAGE_WINDOW_SECONDS, drift_budget,
        },
        errors::VaultError,
        state::{
            Account, AssetConfig, MAX_ALLOWED_ASSETS, MAX_POSITIONS, MAX_SINKS, PositionConfig,
            Protocol, SinkSlot,
        },
    },
    codama_macros::CodamaAccount,
    solana_address::{Address, address_eq},
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::{I64, U16, U32, U64, U128},
};

#[derive(SchemaRead, SchemaWrite, CodamaAccount)]
#[codama(discriminator(bytes = [1]))]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct Vault {
    // Identity
    pub curator: Address,
    pub share_mint: Address,
    pub pending_curator: Address,
    #[codama(type = number(i64))]
    pub created_at: I64,
    pub authority_bump: u8,

    // Configuration
    pub paused: u8,
    #[codama(type = number(u16))]
    pub management_fee_bps: U16,
    #[codama(type = number(u16))]
    pub instant_redeem_fee_bps: U16,
    /// Haircut applied to occupied sink marks by `validate_aum`.
    #[codama(type = number(u16))]
    pub sink_haircut_bps: U16,
    /// Sink-mark freshness window; `0` uses `MAX_STALENESS_SECONDS`.
    #[codama(type = number(u64))]
    pub sink_max_staleness_seconds: U64,
    /// Minimum seconds between `Redeem` and `ClaimRedeem`, giving the manager
    /// time to recall capital. `0` disables; capped at `MAX_TIMELOCK_DELAY`.
    #[codama(type = number(u64))]
    pub timelock_delay: U64,
    /// Transient liability cap in numéraire micro-units; `0` disables it.
    #[codama(type = number(u64))]
    pub max_flash_liability: U64,

    // Live state
    #[codama(type = number(u64))]
    pub aum: U64,
    #[codama(type = number(i64))]
    pub last_priced_at: I64,
    #[codama(type = number(u64))]
    pub last_priced_slot: U64,
    #[codama(type = number(u64))]
    pub pending_redeem_shares: U64,
    /// Raw principal temporarily lent to custody by a `FLASH` delegate.
    #[codama(type = number(u64))]
    pub flash_liability: U64,
    /// Performance-fee high-water mark: peak folded fee-net NAV, adjusted
    /// upward for forgiven fee debt. Re-armed when effective supply is empty.
    #[codama(type = number(u128))]
    pub hwm_nav: U128,
    #[codama(type = number(u128))]
    pub fee_capital_time: U128,
    #[codama(type = number(i64))]
    pub last_fee_accrual_at: I64,
    #[codama(type = number(u64))]
    pub perf_fee_accrued: U64,
    /// Unrefilled swap slippage in numeraire; may exceed capacity after an AUM drop.
    #[codama(type = number(u64))]
    pub swap_slippage_spent: U64,
    /// Timestamp of the last successful swap-slippage bucket refill.
    #[codama(type = number(i64))]
    pub swap_slippage_last_refill_at: I64,
    /// Unrefilled liquidation slippage in numeraire; may exceed capacity after an AUM drop.
    /// Shared liquidation shortfall across all sinks in the current window.
    #[codama(type = number(u64))]
    pub liq_slippage_spent: U64,
    /// Timestamp of the last successful liquidation-slippage bucket refill.
    #[codama(type = number(i64))]
    pub liq_slippage_last_refill_at: I64,

    // Rows
    pub assets: [AssetConfig; 8],
    /// Free slots; each position names its own protocol.
    pub positions: [PositionConfig; 8],
    /// Seized collateral awaiting sale, keyed by mint. See [`SinkSlot`].
    pub sinks: [SinkSlot; 4],
    #[codama(type = number(u16))]
    pub sink_bootstrap_cap_bps: U16,
    pub sink_capacity: u8,
    /// HWM performance fee on NAV gains, in bps, minted to the curator by
    /// `crystallize_fees`.
    #[codama(type = number(u16))]
    pub performance_fee_bps: U16,
    pub curator_fee_account_bump: u8,
    pub protocol_fee_account_bump: u8,
    /// Confidence discount `validate_aum` left in `aum`: `Σ(mid − floor)` over
    /// the spot rows, in bps of `aum`, rounded up. Every share mint adds it
    /// back so a band that closes on the next report pays the holders who
    /// carried it, not the new shares (L-04). `u16::MAX` = not representable;
    /// mints fail closed.
    #[codama(type = number(u16))]
    pub aum_conf_bps: U16,
    /// Live `Delegate` and `ProtocolPolicy` accounts. `grant_revoke_delegate`
    /// and `set_protocol_policy` count creates and closes; `close_vault`
    /// refuses while any remain, so no child can outlive the vault and rebind
    /// to a same-address recreation (I-01).
    #[codama(type = number(u16))]
    pub child_count: U16,
    /// Maximum age of validated AUM for settlement and first-mark bounds.
    /// Zero selects 60 seconds; positive values apply exactly. Oracle,
    /// position and sink freshness limits remain independent.
    #[codama(type = number(u32))]
    pub max_aum_staleness_seconds: U32,
    /// Nest receipt custody belongs to its position mark, never a sink. If
    /// also listed, `ValidateAum` deducts the row from that gross mark. Zero
    /// means unbound; retained after policy close.
    pub nest_receipt_mint: Address,
    /// Vault-retained fee on settled queued shares, in bps; zero disables it.
    #[codama(type = number(u16))]
    pub queued_redeem_fee_bps: U16,
    /// Zero-initialised room for future fields.
    _reserved: [u8; 128],
}

/// Fee assets removed from the vault's accrued liabilities by one
/// crystallization. Recipient totals are converted to shares independently.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FeeBreakdown {
    pub curator_management: u64,
    pub protocol_management: u64,
    pub curator_performance: u64,
    pub protocol_performance: u64,
}

impl FeeBreakdown {
    #[inline]
    pub fn curator_total(self) -> u64 {
        self.curator_management
            .saturating_add(self.curator_performance)
    }

    #[inline]
    pub fn protocol_total(self) -> u64 {
        self.protocol_management
            .saturating_add(self.protocol_performance)
    }

    #[inline]
    pub fn total(self) -> u64 {
        self.curator_total().saturating_add(self.protocol_total())
    }
}

/// Streaming-fee divisor: bps × seconds-per-year.
const FEE_DENOM: u128 = 10_000 * SECONDS_PER_YEAR;

/// u128 → u64, saturating: callers are AUM-bounded, so a saturated
/// intermediate must stay huge rather than wrap into a small wrong number.
#[inline(always)]
fn narrow(x: u128) -> u64 {
    u64::try_from(x).unwrap_or(u64::MAX)
}

/// Apply one charge to a linearly refilling slippage bucket. Returns the new
/// `(spent, last_refill_at)` without mutating state on rejection.
#[inline]
fn refill_slippage_bucket(
    aum: u64,
    prior_spent: u64,
    last_refill_at: i64,
    charge: u64,
    now: i64,
    capacity_bps: u128,
) -> Result<(u64, i64), VaultError> {
    let capacity = u128::from(aum).saturating_mul(capacity_bps) / 10_000;
    // Excess debt after an AUM shrink can take more than one window to refill.
    let elapsed = u128::try_from(now.saturating_sub(last_refill_at)).unwrap_or(0);
    let refilled = capacity.saturating_mul(elapsed) / SWAP_SLIPPAGE_WINDOW_SECONDS as u128;
    // H-01: only elapsed refill, never an AUM resize, forgives historical losses.
    let decayed = u128::from(prior_spent).saturating_sub(refilled);
    let updated = decayed.saturating_add(u128::from(charge));
    if charge > 0 && updated > capacity {
        return Err(VaultError::SwapSlippageBudgetExceeded);
    }
    Ok((narrow(updated), now.max(last_refill_at)))
}

impl Vault {
    #[inline]
    pub fn aum_staleness_window(&self) -> i64 {
        match self.max_aum_staleness_seconds.get() {
            0 => i64::from(MAX_STALENESS_SECONDS),
            seconds => i64::from(seconds),
        }
    }

    /// Count a newly created `Delegate` or `ProtocolPolicy` toward the
    /// `close_vault` gate.
    pub fn add_child(&mut self) -> Result<(), ProgramError> {
        let count = self
            .child_count
            .get()
            .checked_add(1)
            .ok_or(ProgramError::InvalidArgument)?;
        self.child_count = count.into();
        Ok(())
    }

    /// Uncount a closed child. Saturating: delegates granted before the
    /// counter existed (I-01) were never counted.
    pub fn remove_child(&mut self) {
        self.child_count = self.child_count.get().saturating_sub(1).into();
    }

    /// The first configured, deposit-enabled asset priced into AUM is the
    /// canonical asset for instant deposits and withdrawals. Return its row so
    /// callers can use the matching token program and asset metadata.
    #[inline]
    pub fn canonical_base_asset(&self) -> Option<(usize, &AssetConfig)> {
        self.assets.iter().enumerate().find(|(_, asset)| {
            asset.configured() && asset.deposit_enabled() && asset.price_in_aum()
        })
    }

    /// The enabled, AUM-counted position tracking `protocol`. A scan, because
    /// slots are free: `initialize_vault` admits at most one enabled slot per
    /// protocol, which is what makes "first match" unambiguous.
    #[inline]
    pub fn find_position_mut(&mut self, protocol: u8) -> Option<&mut PositionConfig> {
        self.positions
            .iter_mut()
            .find(|p| p.protocol == protocol && p.enabled() && p.required_in_aum())
    }

    /// Whether `protocol` has an enabled [`POSITION_FLAG_LENDER_ONLY`] slot —
    /// the commitment every grant and routed call for it is checked against.
    ///
    /// [`POSITION_FLAG_LENDER_ONLY`]: crate::state::POSITION_FLAG_LENDER_ONLY
    #[inline]
    pub fn lender_only(&self, protocol: Protocol) -> bool {
        self.positions
            .iter()
            .any(|p| p.protocol == protocol as u8 && p.enabled() && p.lender_only())
    }

    #[inline]
    pub fn find_sink_mut(&mut self, mint: &Address) -> Option<&mut SinkSlot> {
        self.sinks.iter_mut().find(|s| s.holds(mint))
    }

    /// Find the sink for `mint`, or claim a free slot within
    /// [`Self::sink_capacity`].
    ///
    /// An existing occupant is found across *every* slot, so lowering capacity
    /// never orphans collateral already in custody; only new claims are bounded.
    #[inline]
    pub fn find_or_claim_sink(&mut self, mint: &Address) -> Result<&mut SinkSlot, VaultError> {
        self.require_sink_mint(mint)?;
        if let Some(i) = self.sinks.iter().position(|s| s.holds(mint)) {
            return Ok(&mut self.sinks[i]);
        }
        let capacity = (self.sink_capacity as usize).min(MAX_SINKS);
        let i = self.sinks[..capacity]
            .iter()
            .position(SinkSlot::is_free)
            .ok_or(VaultError::SinkCapacityExhausted)?;
        self.sinks[i].mint = *mint;
        self.sinks[i].balance = U64::ZERO;
        Ok(&mut self.sinks[i])
    }

    /// Admission and AUM validation must agree on a mint's accounting role.
    pub fn require_sink_mint(&self, mint: &Address) -> Result<(), VaultError> {
        // The zero mint is the vacancy marker (`SinkSlot::is_free`): stamping
        // it into a slot would leave an occupant that still reads as free. No
        // initialized mint has the zero address, so this is the encoding's
        // invariant made explicit rather than a reachable settlement path.
        if address_eq(mint, &Address::default())
            || address_eq(mint, &self.share_mint)
            || address_eq(mint, &self.nest_receipt_mint)
            // Legacy Nest positions must bind their receipt before any sink
            // can be admitted or counted: its mint is otherwise unknown.
            || (address_eq(&self.nest_receipt_mint, &Address::default())
                && self.positions.iter().any(|p| {
                    p.protocol == Protocol::Nest as u8 && p.required_in_aum()
                }))
            || self
                .assets
                .iter()
                .any(|asset| asset.configured() && address_eq(&asset.mint, mint))
        {
            return Err(VaultError::DisallowedAsset);
        }
        Ok(())
    }

    /// Curator + protocol management fees. Both are linear in AUM·time, so
    /// one integral serves both; split by rate in [`Self::take_fees`].
    #[inline]
    fn streaming_bps(&self) -> u128 {
        self.management_fee_bps.get() as u128 + PROTOCOL_MANAGEMENT_FEE_BPS as u128
    }

    /// Curator + protocol performance rate applied to the same HWM gain.
    #[inline]
    fn performance_bps(&self) -> u128 {
        self.performance_fee_bps.get() as u128 + PROTOCOL_PERFORMANCE_FEE_BPS as u128
    }

    /// AUM is the accrual base as-is: a queued redeem stays share-denominated
    /// until `fulfill_redeem` prices and pays it in one instruction, which
    /// debits `aum` there and then. Fee-on-fee is not netted out (≤ bps²).
    #[inline]
    fn accrual_base(&self) -> u64 {
        self.aum.get()
    }

    /// Capital-time integral as of `now`, including the unfolded segment since
    /// [`Self::last_fee_accrual_at`]. Pause never freezes it — pause is a
    /// liveness control, and freezing would hand fee revenue to holders.
    #[inline]
    pub fn fee_capital_time_at(&self, now: i64) -> u128 {
        let elapsed = now
            .saturating_sub(i64::from(self.last_fee_accrual_at))
            .max(0) as u128;
        u128::from(self.fee_capital_time)
            .saturating_add((self.accrual_base() as u128).saturating_mul(elapsed))
    }

    /// Uncrystallized streaming-fee liability, in numéraire. Subtracted from
    /// AUM everywhere so no cohort buys into or escapes another's fees.
    #[inline]
    pub fn accrued_fee_assets(&self, now: i64) -> u64 {
        // Saturating: degrade to "everything is fees", never to a small
        // wrong number. Fractional numéraire stays carried in capital-time
        // until it becomes a whole collectible unit; crystallization frequency
        // therefore cannot manufacture rounding dust.
        let numerator = self
            .fee_capital_time_at(now)
            .saturating_mul(self.streaming_bps());
        narrow(numerator / FEE_DENOM)
    }

    /// Fold the elapsed AUM·time segment and stamp the clock. MUST run before
    /// any write to `aum` so each segment is billed at the AUM that held it.
    #[inline]
    pub fn accrue_capital_time(&mut self, now: i64) {
        self.fee_capital_time = U128::from(self.fee_capital_time_at(now));
        self.last_fee_accrual_at = I64::from(now);
    }

    /// Watermark basis: per-share NAV net of the streaming liability and folded
    /// perf fees, in `nav_per_share`'s virtual-offset form.
    #[inline]
    fn nav_for_watermark(&self, eff_supply: u64, now: i64) -> u128 {
        let aum_net = u64::from(self.aum)
            .saturating_sub(self.accrued_fee_assets(now))
            .saturating_sub(u64::from(self.perf_fee_accrued));
        (aum_net as u128)
            .saturating_add(crate::constants::VIRTUAL_ASSETS)
            .saturating_mul(NAV_SCALE)
            / (eff_supply as u128).saturating_add(crate::constants::VIRTUAL_SHARES)
    }

    /// Fee owed on the unfolded NAV rise above the HWM at the effective
    /// supply. Zero with no holders: that NAV is a virtual-offset artifact.
    #[inline]
    fn perf_pending(&self, eff_supply: u64, now: i64) -> u64 {
        if eff_supply == 0 {
            return 0;
        }
        let delta = self
            .nav_for_watermark(eff_supply, now)
            .saturating_sub(u128::from(self.hwm_nav));
        // Divide by NAV_SCALE before ×bps: `delta·supply·bps` can overflow
        // u128. The early floor costs < 1 numéraire unit.
        let perf =
            delta.saturating_mul(eff_supply as u128) / NAV_SCALE * self.performance_bps() / 10_000;
        narrow(perf)
    }

    /// Fold both fee integrals (streaming + performance, ratcheting the HWM).
    /// MUST run with the PRE-change supply in every instruction that moves
    /// supply or the redeem accumulators, so each gain is billed at the supply
    /// that held it. `Redeem`/`CancelRedeem` are exempt: `eff_supply` is
    /// unchanged there.
    #[inline]
    pub fn accrue_fees(&mut self, share_supply: u64, now: i64) {
        let eff_supply = self.eff_supply(share_supply);
        // No holders → there is no cohort to charge or dilute. Forgive fee
        // integrals that can no longer be consumed, then re-arm the HWM from
        // that cleared basis. Re-arming before the clear would make the next
        // cohort's principal look like a performance gain.
        if eff_supply == 0 {
            self.fee_capital_time = U128::ZERO;
            self.perf_fee_accrued = U64::ZERO;
            self.last_fee_accrual_at = I64::from(now);
            self.hwm_nav = U128::from(
                crate::nav::nav_per_share(0, self.eff_aum(0, now)).unwrap_or(BOOTSTRAP_HWM_NAV),
            );
            return;
        }
        // Measure the NAV rise before forgiving anything above the payable
        // cap. Otherwise the write-down itself raises fee-net NAV and can be
        // re-booked immediately as a performance fee.
        self.accrue_capital_time(now);
        let pending = self.perf_pending(eff_supply, now);
        self.perf_fee_accrued = U64::from(u64::from(self.perf_fee_accrued).saturating_add(pending));
        self.write_down_unpayable_fees(eff_supply, now);
        // Preserve the forgiveness adjustment even below the previous peak.
        self.hwm_nav =
            U128::from(u128::from(self.hwm_nav).max(self.nav_for_watermark(eff_supply, now)));
    }

    /// Exclude issuance-driven NAV increases from performance fees without
    /// lowering the HWM. Accrue pre-entry fees first, then apply the deposit or
    /// fee debit and pass the post-mint supply. Nets folded fees and includes
    /// pending redeem shares, so entry headroom and rounding are not returns.
    /// For fee mints, independent recipient share floors would otherwise leave
    /// a small apparent gain that a later performance-rate change could reprice.
    #[inline]
    pub fn ratchet_hwm_after_mint(&mut self, share_supply: u64, now: i64) {
        let eff_supply = self.eff_supply(share_supply);
        if eff_supply > 0 {
            self.hwm_nav =
                U128::from(u128::from(self.hwm_nav).max(self.nav_for_watermark(eff_supply, now)));
        }
    }

    #[inline]
    pub fn fee_payable_cap(&self) -> u64 {
        self.accrual_base() / 2
    }

    /// Forgive folded fee debt above the amount the live book can pay without
    /// surrendering more than half of gross AUM. Streaming fees have priority:
    /// trim their capital-time only when they alone exceed the cap, then fit
    /// folded performance fees into the remaining room.
    ///
    /// The write-down is sticky. A later AUM recovery cannot resurrect fees
    /// forgiven during a drawdown. `now` must already be folded into
    /// `fee_capital_time`; callers supply the effective cohort size so any
    /// forgiveness lifts the HWM by the waived claim per share.
    #[inline]
    pub fn write_down_unpayable_fees(&mut self, eff_supply: u64, now: i64) {
        let cap = self.fee_payable_cap();
        let bps = self.streaming_bps();
        let prior_capital_time = u128::from(self.fee_capital_time);
        let prior_perf = u64::from(self.perf_fee_accrued);
        let prior_streaming = self.accrued_fee_assets(now);

        // Compare the exact fractional claim to the cap. In particular, a
        // zero-cap book must forgive sub-unit capital-time instead of letting
        // it resurrect after a later AUM recovery.
        if prior_capital_time.saturating_mul(bps) > u128::from(cap).saturating_mul(FEE_DENOM) {
            self.fee_capital_time = U128::from(u128::from(cap).saturating_mul(FEE_DENOM) / bps);
        }

        let room = cap.saturating_sub(self.accrued_fee_assets(now));
        self.perf_fee_accrued = U64::from(u64::from(self.perf_fee_accrued).min(room));

        if u128::from(self.fee_capital_time) != prior_capital_time
            || u64::from(self.perf_fee_accrued) != prior_perf
        {
            // Lift the old peak by the full waived claim, even if trough NAV clamps to zero.
            let forgiven = u128::from(prior_streaming - self.accrued_fee_assets(now))
                + u128::from(prior_perf - u64::from(self.perf_fee_accrued));
            let nav_lift = (forgiven * NAV_SCALE)
                .div_ceil(u128::from(eff_supply) + crate::constants::VIRTUAL_SHARES);
            self.hwm_nav = U128::from(
                u128::from(self.hwm_nav)
                    .saturating_add(nav_lift)
                    .max(self.nav_for_watermark(eff_supply, now)),
            );
        }
    }

    /// Economic fee liability as of `now`: streaming plus folded and pending
    /// performance fees. Folded debt is written down to
    /// [`Self::fee_payable_cap`] after [`Self::accrue_fees`] and after fresh AUM
    /// validation; a current, not-yet-folded NAV rise remains visible here so a
    /// cohort cannot enter or exit around it.
    #[inline]
    pub fn raw_fee_liability(&self, share_supply: u64, now: i64) -> u64 {
        let eff_supply = self.eff_supply(share_supply);
        self.accrued_fee_assets(now)
            .saturating_add(u64::from(self.perf_fee_accrued))
            .saturating_add(self.perf_pending(eff_supply, now))
    }

    /// Fee assets one crystallization would hand out, split between curator
    /// and protocol. Pure: [`Self::debit_fees`] removes what is actually paid.
    /// The pro-rata cap is defense in depth; [`Self::accrue_fees`] normally
    /// reconciles the aggregate folded claim to [`Self::fee_payable_cap`]
    /// before this runs.
    #[inline]
    pub fn fee_breakdown(&self) -> FeeBreakdown {
        let capital_time = u128::from(self.fee_capital_time);
        let streaming = capital_time.saturating_mul(self.streaming_bps()) / FEE_DENOM;
        let curator_management =
            capital_time.saturating_mul(u128::from(self.management_fee_bps.get())) / FEE_DENOM;
        let perf = u128::from(u64::from(self.perf_fee_accrued));

        let owed = streaming.saturating_add(perf);
        let cap = u128::from(self.fee_payable_cap());
        // Pro-rata and floor-monotone, so each remainder stays non-negative.
        let (take_streaming, take_curator_management, take_perf) = if owed > cap {
            let cut = |x: u128| x.saturating_mul(cap) / owed;
            (cut(streaming), cut(curator_management), cut(perf))
        } else {
            (streaming, curator_management, perf)
        };

        let curator_performance = take_perf
            .saturating_mul(u128::from(self.performance_fee_bps.get()))
            .checked_div(self.performance_bps())
            .unwrap_or(0);
        FeeBreakdown {
            curator_management: narrow(take_curator_management),
            protocol_management: narrow(take_streaming.saturating_sub(take_curator_management)),
            curator_performance: narrow(curator_performance),
            // Assign division dust to the protocol so the split conserves the
            // exact performance liability removed from the accumulator.
            protocol_performance: narrow(take_perf.saturating_sub(curator_performance)),
        }
    }

    /// Debit the folded accumulators by exactly `paid`, a (possibly partial)
    /// [`Self::fee_breakdown`]. A recipient's entries may be zeroed to leave
    /// its claim in the integrals for a later crystallization.
    #[inline]
    pub fn debit_fees(&mut self, paid: FeeBreakdown) {
        let bps = self.streaming_bps();
        let capital_time = u128::from(self.fee_capital_time);
        let streaming = u128::from(paid.curator_management)
            .saturating_add(u128::from(paid.protocol_management));
        // Consume the least capital-time that fully covers the whole units
        // paid. The remaining fraction stays below one unit and carries into a
        // later crystallization.
        let consumed = if bps == 0 {
            capital_time // No claim accrued during the fee-free interval.
        } else {
            streaming.saturating_mul(FEE_DENOM).div_ceil(bps)
        };
        self.fee_capital_time = U128::from(capital_time.saturating_sub(consumed));
        self.perf_fee_accrued = U64::from(
            u64::from(self.perf_fee_accrued)
                .saturating_sub(paid.curator_performance)
                .saturating_sub(paid.protocol_performance),
        );
    }

    /// Take everything collectible.
    #[inline]
    pub fn take_fees(&mut self) -> FeeBreakdown {
        let fees = self.fee_breakdown();
        self.debit_fees(fees);
        fees
    }

    /// Convert each recipient's total to shares independently and debit only
    /// what mints (L-06). A recipient whose total floors to zero shares keeps
    /// its claim in the integrals instead of failing the other's collection.
    /// Errors only when neither recipient can mint, so the integrals are never
    /// touched for nothing.
    // ponytail: the retained dust is re-split by rate next time, so up to
    // `pb/(cb+pb)` of a sub-share amount can drift to the other recipient per
    // event (zero while the protocol rates are 0). Per-recipient carry fields
    // if that ever matters.
    #[inline]
    pub fn settle_fee_shares(
        &mut self,
        mut fees: FeeBreakdown,
        eff_supply: u64,
        fee_basis: u64,
    ) -> Result<(FeeBreakdown, u64, u64), VaultError> {
        let curator_shares =
            crate::nav::shares_for_deposit(fees.curator_total(), eff_supply, fee_basis)?;
        let protocol_shares =
            crate::nav::shares_for_deposit(fees.protocol_total(), eff_supply, fee_basis)?;
        if curator_shares == 0 && protocol_shares == 0 {
            if fees.total() > 0 {
                return Err(VaultError::FeeAmountTooSmall);
            }
        } else if curator_shares == 0 {
            fees.curator_management = 0;
            fees.curator_performance = 0;
        } else if protocol_shares == 0 {
            fees.protocol_management = 0;
            fees.protocol_performance = 0;
        }
        self.debit_fees(fees);
        Ok((fees, curator_shares, protocol_shares))
    }

    /// Charge `slippage` against the swap leaky bucket, which refills linearly
    /// at one current-AUM capacity per hour. Excess debt after an AUM shrink
    /// can take longer to clear; window boundaries never reset spent slippage.
    /// `MAX_SWAP_SLIPPAGE_BPS` bounds one hop, this bounds the sum.
    #[inline]
    pub fn charge_swap_slippage(&mut self, slippage: u64, now: i64) -> Result<(), VaultError> {
        let (spent, at) = refill_slippage_bucket(
            self.aum.get(),
            self.swap_slippage_spent.get(),
            self.swap_slippage_last_refill_at.get(),
            slippage,
            now,
            MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS,
        )?;
        self.swap_slippage_spent = U64::from(spent);
        self.swap_slippage_last_refill_at = I64::from(at);
        Ok(())
    }

    /// [`Self::charge_swap_slippage`] for the unlisted-source
    /// (seized-collateral) arm — a separate bucket because its floor is looser.
    #[inline]
    pub fn charge_liquidation_slippage(
        &mut self,
        slippage: u64,
        now: i64,
    ) -> Result<(), VaultError> {
        let (spent, at) = refill_slippage_bucket(
            self.aum.get(),
            self.liq_slippage_spent.get(),
            self.liq_slippage_last_refill_at.get(),
            slippage,
            now,
            MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS,
        )?;
        self.liq_slippage_spent = U64::from(spent);
        self.liq_slippage_last_refill_at = I64::from(at);
        Ok(())
    }

    /// Live supply plus the shares a queued redeem already burned. The
    /// saturating add would *hide* pending redeems if it ever clamped;
    /// [`Self::require_mint_headroom`] is what keeps that unreachable.
    #[inline(always)]
    pub fn eff_supply(&self, share_supply: u64) -> u64 {
        share_supply.saturating_add(self.pending_redeem_shares.get())
    }

    /// Refuse a mint that would push `share_supply + pending_redeem_shares`
    /// past `u64::MAX`. Token-2022 bounds `Mint.supply` alone, so minting into
    /// the room a queued redeem freed succeeds while every effective-supply
    /// reader clamps — the redeems vanish from the denominator and the next
    /// fill is overpaid. Those readers can't error, so this is the only gate:
    /// call it before any `MintTo` not matched by a `pending_redeem_shares`
    /// debit.
    #[inline]
    pub fn require_mint_headroom(&self, share_supply: u64, amount: u64) -> Result<(), VaultError> {
        share_supply
            .checked_add(self.pending_redeem_shares.get())
            .and_then(|eff| eff.checked_add(amount))
            .map(|_| ())
            .ok_or(VaultError::AumOverflow)
    }

    #[inline(always)]
    pub fn eff_aum(&self, share_supply: u64, now: i64) -> u64 {
        self.aum.get().saturating_sub(
            self.raw_fee_liability(share_supply, now)
                .min(self.fee_payable_cap()),
        )
    }

    /// Refuse instant redemption when permissioned opaque marks can move NAV by
    /// more than its exit fee can absorb. This is a runtime gate, not only a
    /// configuration check: old vaults and newly occupied sink slots must fail
    /// closed too. Issuance separately prices in mark headroom because an exit
    /// fee cannot protect the independently priced queued redeem path.
    ///
    /// Bounds the total mark-up that distinct opaque NAV inputs can compose
    /// around `validate_aum` in one transaction. Each mark's cap is based on the
    /// then-current AUM, so capacities compound rather than merely add. This is
    /// deliberately not described as protection for fee-free queued exits.
    #[inline]
    pub fn require_instant_mark_fee_safe(&self) -> Result<(), ProgramError> {
        let unsafe_direct = || ProgramError::from(VaultError::InstantRedeemDisabled);

        let fee_bps = self.instant_redeem_fee_bps.get();
        // A disabled instant exit is safe regardless of the strategy's mark
        // shape; queued issuance is protected separately by entry headroom.
        if fee_bps == u16::MAX {
            return Ok(());
        }

        // A manual price drifts in bps of the token *price*, but Vault stores no
        // per-row custody balance. With a leveraged/offset book there is no
        // sound conversion from that price move to bps of net AUM, so instant
        // redemption is disabled whenever such a row affects NAV.
        if self
            .assets
            .iter()
            .any(|a| a.configured() && a.price_in_aum() && a.manual_price())
        {
            return Err(unsafe_direct());
        }

        // Fluid's position mark nets collateral and debt. A third-party
        // liquidation can lower that net value without invalidating our mark,
        // so its drift cap cannot protect an instant exit. A lender-only slot
        // is Earn-only: Fluid Vaults is refused at grant and at routing, so its
        // mark holds unliquidatable Earn receipts and the drift cap below holds.
        if self.positions.iter().any(|p| {
            p.enabled()
                && p.required_in_aum()
                && p.protocol == Protocol::JupiterLend as u8
                && !p.lender_only()
        }) {
            return Err(unsafe_direct());
        }

        // Larger liability reports intentionally bypass drift: delaying a real
        // debt increase would overstate NAV. That makes a pending increase
        // impossible to fee-bound, so opaque DEBT books cannot redeem instantly.
        if self
            .positions
            .iter()
            .any(|p| p.enabled() && p.required_in_aum() && p.debt())
        {
            return Err(unsafe_direct());
        }

        // Growth factor in bps, rounded up after every multiplication so the
        // integer calculation can never understate composed capacity.
        let mut growth_bps = 10_000u128;
        let mut compound = |cap_bps: u64| -> Result<(), ProgramError> {
            growth_bps = ceil_compound_bps(growth_bps, cap_bps).ok_or_else(unsafe_direct)?;
            Ok(())
        };

        for position in self
            .positions
            .iter()
            .filter(|p| p.enabled() && p.required_in_aum())
        {
            let cap = drift_budget(position.max_drift_bps.get()).ok_or_else(unsafe_direct)?;
            compound(cap)?;
        }

        // A sink marked earlier in the same transaction is already nonzero by
        // the time `redeem_direct` reaches this gate. Charge every occupant the
        // worse of its first-mark and steady-state budgets so that committing a
        // bootstrap cannot reclassify the slot down to the default drift cap.
        // A waived bootstrap cap remains fail-closed even after first mark.
        for _sink in self.sinks.iter().filter(|s| !s.is_free()) {
            let bootstrap_cap =
                drift_budget(self.sink_bootstrap_cap_bps.get()).ok_or_else(unsafe_direct)?;
            compound(bootstrap_cap.max(MAX_DRIFT_BPS))?;
        }

        let capacity_bps = growth_bps.checked_sub(10_000).ok_or_else(unsafe_direct)?;
        if capacity_bps > u128::from(fee_bps) {
            return Err(unsafe_direct());
        }
        Ok(())
    }
}

/// `value × (1 + cap_bps / 10_000)`, rounded up. `None` is an arithmetic
/// overflow, which every caller maps to a fail-closed program error.
#[inline]
fn ceil_compound_bps(value: u128, cap_bps: u64) -> Option<u128> {
    value
        .checked_mul(10_000u128.saturating_add(u128::from(cap_bps)))?
        .checked_add(9_999)?
        .checked_div(10_000)
}

/// One pausable flow — each variant is a distinct bit in `Vault.paused`.
#[repr(u8)]
#[derive(Clone, Copy)]
pub enum Pause {
    /// Blocks `Subscribe`, `SubscribeDirect`, `Fulfill`, and `CrystallizeFees`
    /// — every path that mints shares.
    Subscribe = 1 << 0,
    /// Blocks `Redeem`, `RedeemDirect`, and `FulfillRedeem`. Claims and
    /// cancels of already-escrowed funds stay open.
    Redeem = 1 << 1,
    /// Blocks `ProtocolCPI` and `PriceOpaque` — every mark-moving and
    /// fund-moving delegate path.
    Strategy = 1 << 2,
}

impl Pause {
    /// Every valid bit — `SetPaused` / `UpdateVault` mask their input against this.
    pub const ALL: u8 = Self::Subscribe as u8 | Self::Redeem as u8 | Self::Strategy as u8;
    /// Both share flows. An outright curator write that lowers a position
    /// mark — `PriceOpaque` or a position reset — needs both bits set, so
    /// that no share flow settles at the new mark before the curator checks it.
    pub const SHARE_FLOWS: u8 = Self::Subscribe as u8 | Self::Redeem as u8;
}

impl Account for Vault {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 1;
}

check_account_layout!(Vault);

// Pin the zero-copy ABI; vaults on any older layout require migration.
const _: () = assert!(core::mem::size_of::<Vault>() == 2_333);
const _: () = assert!(Vault::LEN == 2_334);
const _: () = assert!(core::mem::offset_of!(Vault, max_aum_staleness_seconds) == 2_167);
const _: () = assert!(core::mem::size_of::<SinkSlot>() == 57);
const _: () = assert!(core::mem::offset_of!(SinkSlot, balance) == 40);
const _: () = assert!(core::mem::offset_of!(Vault, nest_receipt_mint) == 2_171);
const _: () = assert!(core::mem::offset_of!(Vault, queued_redeem_fee_bps) == 2_203);
const _: () = assert!(core::mem::offset_of!(Vault, _reserved) == 2_205);
const _: () = assert!(core::mem::size_of::<AssetConfig>() == 181);
const _: () = assert!(core::mem::offset_of!(Vault, management_fee_bps) == 106);
const _: () = assert!(core::mem::offset_of!(Vault, performance_fee_bps) == 2_159);
const _: () = assert!(core::mem::offset_of!(Vault, curator_fee_account_bump) == 2_161);
const _: () = assert!(core::mem::offset_of!(Vault, protocol_fee_account_bump) == 2_162);

// The row arrays are spelled with literal lengths because codama's IDL derive
// cannot resolve a const there. This is the only thing keeping the literals and
// the constants in step.
const _: () = assert!(MAX_ALLOWED_ASSETS == 8 && MAX_POSITIONS == 8 && MAX_SINKS == 4);

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            constants::BOOTSTRAP_HWM_NAV,
            nav::{assets_for_redeem, shares_for_deposit},
            state::{
                ASSET_FLAG_MANUAL_PRICE, ASSET_FLAG_PRICE_IN_AUM, POSITION_FLAG_DEBT,
                POSITION_FLAG_ENABLED, POSITION_FLAG_LENDER_ONLY, POSITION_FLAG_REQUIRED_IN_AUM,
            },
        },
    };

    fn vault(aum: u64) -> Vault {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        v.aum = U64::from(aum);
        v.hwm_nav = U128::from(BOOTSTRAP_HWM_NAV);
        // A representative 10% curator rate; the protocol rate is additive.
        v.performance_fee_bps = U16::from(1_000);
        v
    }

    fn fee_shares(fees: FeeBreakdown, eff_supply: u64, fee_net_aum: u64) -> u64 {
        let convert = |assets| {
            let shares = shares_for_deposit(assets, eff_supply, fee_net_aum).unwrap();
            assert!(
                assets == 0 || shares > 0,
                "positive fee assets must not be discarded as share dust"
            );
            shares
        };
        convert(fees.curator_total()).saturating_add(convert(fees.protocol_total()))
    }

    fn crystallize(v: &mut Vault, supply: u64) -> u64 {
        let eff_supply = v.eff_supply(supply);
        let fee_net_aum = v.eff_aum(supply, 0);
        assert!(fee_net_aum > 0, "fixture must remain solvent");
        let fees = v.take_fees();
        let supply = supply.saturating_add(fee_shares(fees, eff_supply, fee_net_aum));
        v.ratchet_hwm_after_mint(supply, 0);
        supply
    }

    /// Mirror the fee-accounting half of `CrystallizeFees` for cohort tests.
    /// Each recipient converts its total fee assets against the same pre-mint
    /// basis, matching the two independent on-chain share calculations.
    fn crystallize_booked(v: &mut Vault, supply: u64, now: i64) -> (u64, u64) {
        v.accrue_fees(supply, now);
        let eff_supply = v.eff_supply(supply);
        let fee_net_aum = v.eff_aum(supply, now);
        assert!(fee_net_aum > 0, "fixture must remain solvent");
        let fees = v.take_fees();
        let shares = fee_shares(fees, eff_supply, fee_net_aum);
        let supply = supply.saturating_add(shares);
        v.ratchet_hwm_after_mint(supply, now);
        (supply, shares)
    }

    const UNIT: u64 = 1_000_000;
    const BASE: u64 = 1_000 * UNIT;

    /// L-06: a curator entitlement that floors to zero shares must not block
    /// a representable protocol entitlement, and must stay in the integrals.
    #[test]
    fn fee_dust_of_one_recipient_does_not_block_the_other() {
        let mut v = vault(BASE);
        let supply = BASE / 4; // ~4 numéraire units per share: 1 unit is dust
        v.perf_fee_accrued = U64::from(4_001u64);
        // Protocol rates are 0 today, so hand-split the folded liability the
        // way a live protocol rate would.
        let fees = FeeBreakdown {
            curator_performance: 1,
            protocol_performance: 4_000,
            ..FeeBreakdown::default()
        };

        let eff_supply = v.eff_supply(supply);
        let basis = v.eff_aum(supply, 0);
        let (paid, curator, protocol) = v.settle_fee_shares(fees, eff_supply, basis).unwrap();
        assert_eq!(curator, 0);
        assert!(protocol > 0);
        assert_eq!(paid.curator_total(), 0);
        assert_eq!(paid.protocol_total(), 4_000);
        assert_eq!(v.perf_fee_accrued.get(), 1, "curator dust must stay owed");

        // Dust alone still refuses to mint and leaves the integrals untouched.
        let fees = v.fee_breakdown();
        assert_eq!(
            v.settle_fee_shares(fees, eff_supply, basis),
            Err(VaultError::FeeAmountTooSmall)
        );
        assert_eq!(v.perf_fee_accrued.get(), 1);
    }

    #[test]
    fn sink_claims_need_curator_capacity() {
        let mut v = vault(BASE);
        let seized = Address::new_from_array([9; 32]);

        // Off by default, like `max_flash_liability == 0`.
        assert!(matches!(
            v.find_or_claim_sink(&seized),
            Err(VaultError::SinkCapacityExhausted)
        ));

        v.sink_capacity = 1;
        // The vacancy marker itself is never claimable, at any capacity —
        // a zero-mint occupant would still read as free.
        assert!(matches!(
            v.find_or_claim_sink(&Address::default()),
            Err(VaultError::DisallowedAsset)
        ));
        assert!(v.find_or_claim_sink(&seized).is_ok());
        // Same mint reuses its slot; a second mint has nowhere to go.
        assert!(v.find_or_claim_sink(&seized).is_ok());
        assert!(matches!(
            v.find_or_claim_sink(&Address::new_from_array([8; 32])),
            Err(VaultError::SinkCapacityExhausted)
        ));

        // Lowering capacity stops new claims without orphaning collateral the
        // vault already holds — that balance is real either way.
        v.sink_capacity = 0;
        assert!(v.find_or_claim_sink(&seized).is_ok());
    }

    #[test]
    fn nest_receipts_cannot_claim_or_reuse_sinks() {
        let mut v = vault(BASE);
        v.sink_capacity = 2;
        let receipt = Address::new_from_array([7; 32]);
        let collateral = Address::new_from_array([8; 32]);
        v.positions[0].protocol = Protocol::Nest as u8;
        v.positions[0].flags = POSITION_FLAG_ENABLED | POSITION_FLAG_REQUIRED_IN_AUM;

        // A pre-upgrade position has no mint binding: fail closed until bound.
        assert!(matches!(
            v.find_or_claim_sink(&receipt),
            Err(VaultError::DisallowedAsset)
        ));
        assert!(matches!(
            v.find_or_claim_sink(&collateral),
            Err(VaultError::DisallowedAsset)
        ));
        v.nest_receipt_mint = receipt;
        assert!(matches!(
            v.find_or_claim_sink(&receipt),
            Err(VaultError::DisallowedAsset)
        ));
        assert!(v.sinks[0].is_free());
        assert!(v.find_or_claim_sink(&collateral).is_ok());

        // Existing overlap is rejected before reuse, including written-off slots.
        v.sinks[1].mint = receipt;
        assert!(matches!(
            v.find_or_claim_sink(&receipt),
            Err(VaultError::DisallowedAsset)
        ));
        // The receipt remains reserved even before a position is added.
        v.positions[0] = PositionConfig::zero();
        assert_eq!(
            v.require_sink_mint(&receipt),
            Err(VaultError::DisallowedAsset)
        );
    }

    #[test]
    fn vault_shares_can_never_be_claimed_as_backing() {
        let mut v = vault(BASE);
        v.sink_capacity = 1;
        v.share_mint = Address::new_from_array([6; 32]);
        let share_mint = v.share_mint;

        assert!(matches!(
            v.find_or_claim_sink(&share_mint),
            Err(VaultError::DisallowedAsset)
        ));
        assert!(v.sinks[0].is_free(), "a rejected share mint claimed a sink");

        let listed_mint = Address::new_from_array([5; 32]);
        v.assets[0].mint = listed_mint;
        assert!(matches!(
            v.find_or_claim_sink(&listed_mint),
            Err(VaultError::DisallowedAsset)
        ));
        assert!(v.sinks[0].is_free(), "a listed asset claimed a sink");
    }

    #[test]
    fn write_off_keeps_collateral_bound_for_remark() {
        let mut v = vault(BASE);
        v.sink_capacity = MAX_SINKS as u8;
        let seized = Address::new_from_array([7; 32]);

        let sink = v.find_or_claim_sink(&seized).unwrap();
        assert_eq!(sink.balance.get(), 0);
        sink.value = U64::from(500 * UNIT);
        sink.balance = U64::from(10u64);
        sink.set_priced();

        v.sinks[0].write_off(1_000);

        assert!(v.sinks[0].value.get() == 0, "the mark must leave AUM");
        assert!(v.sinks[0].holds(&seized), "the mint binding must survive");
        assert_eq!(v.sinks[0].balance.get(), 10);
        assert!(
            v.sinks[0].priced(),
            "written off at 0, so validate_aum credits nothing instead of \
             failing closed on an unpriced occupant"
        );
        assert!(v.find_sink_mut(&seized).is_some());

        v.sinks[0].record_outflow(5);
        assert_eq!(v.sinks[0].balance.get(), 10, "retain the pricing basis");
        assert!(!v.sinks[0].priced());

        // A successful full sale after re-marking is what frees the slot.
        v.sinks[0].record_outflow(0);
        assert!(v.sinks[0].is_free());
        assert_eq!(v.sinks[0].balance.get(), 0);
    }

    /// A mint is bounded by the *effective* supply, not `Mint.supply`.
    #[test]
    fn mint_headroom_counts_burned_redeem_shares() {
        let mut v = vault(BASE);
        v.pending_redeem_shares = U64::from(1_000u64);
        let supply = u64::MAX - 1_000;
        // Token-2022 would allow this — `supply + 1_000` still fits.
        assert_eq!(
            v.require_mint_headroom(supply, 1_000),
            Err(VaultError::AumOverflow)
        );
        assert_eq!(v.require_mint_headroom(supply, 0), Ok(()));
        // Without a queued redeem the same mint is fine.
        v.pending_redeem_shares = U64::ZERO;
        assert_eq!(v.require_mint_headroom(supply, 1_000), Ok(()));
        assert_eq!(v.eff_supply(supply), supply);
    }

    #[test]
    fn crystallization_does_not_create_a_phantom_drawdown() {
        const SUPPLY: u64 = BASE;
        let mut v = vault(BASE);

        // Flat: nothing to charge, HWM unmoved.
        v.accrue_fees(SUPPLY, 0);
        assert_eq!(u64::from(v.perf_fee_accrued), 0);

        // NAV rises 10% → a fee accrues and the HWM ratchets.
        v.aum = U64::from(BASE + 100 * UNIT);
        v.accrue_fees(SUPPLY, 0);
        let perf = u64::from(v.perf_fee_accrued);
        assert!(perf > 0, "a NAV rise must accrue a performance fee");
        let hwm = u128::from(v.hwm_nav);

        // Crystallize, then re-read the basis at the new supply.
        let supply = crystallize(&mut v, SUPPLY);
        assert!(supply > SUPPLY, "fee shares were minted");
        let nav_after = v.nav_for_watermark(supply, 0);

        assert!(
            nav_after >= hwm,
            "crystallizing must not push the watermark NAV below the HWM: \
             {nav_after} < {hwm} (drawdown of {})",
            hwm - nav_after
        );
        // Only floor-rounding on the fee-share mint separates them.
        assert!(
            nav_after - hwm < NAV_SCALE / 1_000_000,
            "gap must be rounding dust, got {}",
            nav_after - hwm
        );
        v.accrue_fees(supply, 0);
        assert_eq!(
            u64::from(v.perf_fee_accrued),
            0,
            "fee-share mint rounding was charged as new performance"
        );

        // Ground above the HWM is still chargeable, not absorbed by a deficit.
        v.aum = U64::from(BASE + 200 * UNIT);
        v.accrue_fees(supply, 0);
        assert!(
            u64::from(v.perf_fee_accrued) > 0,
            "the next real gain must still be charged"
        );
    }

    #[test]
    fn recipient_share_rounding_cannot_be_repriced_as_performance() {
        const SUPPLY: u64 = 100_000_000;
        let mut v = vault(1_000_000_000);
        v.performance_fee_bps = U16::from(750u16);

        v.accrue_fees(SUPPLY, 0);
        assert!(
            u64::from(v.perf_fee_accrued) > 0,
            "fixture must book a real gain"
        );
        let post_fee_supply = crystallize(&mut v, SUPPLY);
        assert!(post_fee_supply > SUPPLY);

        // With separate curator/protocol share floors, this fixture leaves a
        // measurable NAV rounding gain unless crystallization ratchets over it.
        v.performance_fee_bps = U16::from(2_000u16);
        v.accrue_fees(post_fee_supply, 0);
        assert_eq!(
            u64::from(v.perf_fee_accrued),
            0,
            "fee-share rounding was repriced under the new performance rate"
        );
    }

    #[test]
    fn subscription_hwm_uses_fee_net_effective_supply_and_preserves_drawdowns() {
        for aum in [BASE / 2, BASE, BASE + 100 * UNIT] {
            for pending in [0, BASE / 2] {
                let mut v = vault(aum);
                let supply = BASE - pending;
                v.pending_redeem_shares = U64::from(pending);
                v.management_fee_bps = U16::from(200u16);
                let now = SECONDS_PER_YEAR as i64;
                v.accrue_fees(supply, now);
                let prior_hwm = u128::from(v.hwm_nav);
                let fees = v.raw_fee_liability(supply, now);
                let shares = shares_for_deposit(
                    BASE,
                    v.eff_supply(supply),
                    v.eff_aum(supply, now) + BASE / 20,
                )
                .unwrap();

                v.aum = U64::from(aum + BASE);
                let post_supply = supply + shares;
                v.ratchet_hwm_after_mint(post_supply, now);
                let post_nav =
                    crate::nav::nav_per_share(post_supply + pending, aum + BASE - fees).unwrap();
                assert_eq!(u128::from(v.hwm_nav), prior_hwm.max(post_nav));
                if aum == BASE / 2 {
                    assert!(post_nav < prior_hwm);
                }
                v.accrue_fees(post_supply, now);
                assert_eq!(v.raw_fee_liability(post_supply, now), fees);
            }
        }
    }

    #[test]
    fn hwm_is_monotonic_across_folds() {
        const SUPPLY: u64 = BASE;
        let mut v = vault(BASE);
        let mut hwm = u128::from(v.hwm_nav);

        for mult in [1_050u64, 1_100, 1_100, 2_000, 1_500, 10_000] {
            let aum = BASE / 1_000 * mult;
            v.aum = U64::from(aum);
            v.accrue_fees(SUPPLY, 0);
            let next = u128::from(v.hwm_nav);
            assert!(next >= hwm, "HWM fell from {hwm} to {next} at aum {aum}");
            hwm = next;
        }
    }

    #[test]
    fn sub_unit_gain_is_not_recharged_to_a_later_depositor() {
        let mut v = vault(1);
        let old_supply = 1;

        // Real gain, too small for one numeraire unit at this supply.
        v.aum = U64::from(10u64);
        v.accrue_fees(old_supply, 0);
        assert_eq!(u64::from(v.perf_fee_accrued), 0);
        assert!(
            u128::from(v.hwm_nav) > BOOTSTRAP_HWM_NAV,
            "the fee dust must still ratchet the cohort watermark"
        );

        // A stale HWM would multiply the forgiven dust by this larger supply.
        let deposit = BASE;
        let shares = shares_for_deposit(deposit, old_supply, v.eff_aum(old_supply, 0)).unwrap();
        v.aum = U64::from(u64::from(v.aum).saturating_add(deposit));
        v.accrue_fees(old_supply.saturating_add(shares), 0);
        assert_eq!(
            u64::from(v.perf_fee_accrued),
            0,
            "later depositor was charged the prior cohort's sub-unit gain"
        );
    }

    #[test]
    fn empty_supply_rearms_the_watermark() {
        let mut v = vault(BASE);
        v.accrue_fees(BASE, 0);

        // Burn all but a dust share: AUM unchanged, NAV/share ~10⁶× higher.
        let dust = UNIT / 1_000;
        v.accrue_fees(dust, 0);
        let poisoned = u128::from(v.hwm_nav);
        assert!(
            poisoned > BOOTSTRAP_HWM_NAV * 1_000,
            "a burn-driven spike should have ratcheted the HWM: {poisoned}"
        );

        crystallize(&mut v, dust);
        // A final exit can leave fee integrals with no cohort to dilute. They
        // must not strand the next entry, and the HWM must be re-armed only
        // after their forgiveness changes fee-net AUM.
        v.fee_capital_time = U128::from(1u128);
        v.perf_fee_accrued = U64::from(UNIT);
        v.accrue_fees(0, 0);
        assert_eq!(v.fee_capital_time.get(), 0);
        assert_eq!(v.perf_fee_accrued.get(), 0);
        let entry = v.nav_for_watermark(0, 0);
        assert_eq!(
            u128::from(v.hwm_nav),
            entry,
            "HWM not re-armed to entry NAV"
        );
        assert!(
            entry > BOOTSTRAP_HWM_NAV,
            "a funded book does not re-arm to 1.0"
        );

        let deposit = BASE;
        let shares = shares_for_deposit(deposit, 0, v.eff_aum(0, 0)).expect("entry share math");
        v.aum = U64::from(u64::from(v.aum) + deposit);
        v.accrue_fees(shares, 0);
        assert!(
            u64::from(v.perf_fee_accrued) <= deposit / 10_000,
            "principal charged as performance: {}",
            u64::from(v.perf_fee_accrued)
        );

        // ...and a genuine rise on top of it still is.
        v.aum = U64::from(u64::from(v.aum) * 3 / 2);
        v.accrue_fees(shares, 0);
        assert!(
            u64::from(v.perf_fee_accrued) > deposit / 100,
            "the new cohort's gain must still be chargeable"
        );
    }

    #[test]
    fn reentry_over_uncollected_fees_is_not_charged_as_performance() {
        const YEAR: i64 = SECONDS_PER_YEAR as i64;
        let now = 5 * YEAR;
        let mut v = vault(BASE);
        v.management_fee_bps = U16::from(1_000u16); // 10% + protocol management

        // Five years of fee booked, then the cohort exits with a small backed
        // residual. An exactly insolvent book is not an ordinary entry path.
        v.accrue_fees(BASE, now);
        v.aum = U64::from(v.accrued_fee_assets(now).saturating_add(UNIT));
        v.accrue_fees(0, now);

        // Re-entry, priced exactly as `deposit_basis` prices it.
        let deposit = BASE;
        let shares = shares_for_deposit(deposit, 0, v.eff_aum(0, now)).expect("entry share math");
        v.aum = U64::from(u64::from(v.aum) + deposit);
        v.accrue_fees(shares, now);
        assert!(
            u64::from(v.perf_fee_accrued) <= deposit / 10_000,
            "re-entry principal charged beyond virtual-offset dust"
        );

        // ...and a genuine rise on top of it still is.
        v.aum = U64::from(u64::from(v.aum) * 2);
        v.accrue_fees(shares, now);
        assert!(
            u64::from(v.perf_fee_accrued) > deposit / 100,
            "the new cohort's gain must still be chargeable"
        );
    }

    #[test]
    fn crystallize_and_deposit_are_cohort_order_independent() {
        const AUM: u64 = 100 * UNIT;
        const SUPPLY: u64 = 100 * UNIT;
        const DEBT: u64 = 40 * UNIT;
        const DEPOSIT: u64 = 100 * UNIT;
        // Loose enough for virtual-offset and floor dust, tiny next to the
        // pre-fix $20 cohort transfer this test detects.
        const DUST: u64 = 10_000;

        let setup = || {
            let mut v = vault(AUM);
            v.performance_fee_bps = U16::ZERO;
            v.perf_fee_accrued = U64::from(DEBT);
            v
        };

        // Deposit first: the historical debt remains inside the payable cap.
        let mut deposit_first = setup();
        assert_eq!(deposit_first.raw_fee_liability(SUPPLY, 0), DEBT);
        assert_eq!(deposit_first.fee_payable_cap(), AUM / 2);
        let entrant_first =
            shares_for_deposit(DEPOSIT, SUPPLY, deposit_first.eff_aum(SUPPLY, 0)).unwrap();
        deposit_first.accrue_fees(SUPPLY, 0);
        deposit_first.aum = U64::from(AUM + DEPOSIT);
        let pre_fee_supply = SUPPLY + entrant_first;
        let (deposit_first_supply, deposit_first_fee_shares) =
            crystallize_booked(&mut deposit_first, pre_fee_supply, 0);
        assert_eq!(deposit_first.raw_fee_liability(deposit_first_supply, 0), 0);

        // Crystallize first: the whole in-cap debt is converted before entry.
        let mut crystal_first = setup();
        let (supply_after_first_fee, first_fee_shares) =
            crystallize_booked(&mut crystal_first, SUPPLY, 0);
        assert_eq!(
            crystal_first.raw_fee_liability(supply_after_first_fee, 0),
            0
        );
        let entrant_second = shares_for_deposit(
            DEPOSIT,
            supply_after_first_fee,
            crystal_first.eff_aum(supply_after_first_fee, 0),
        )
        .unwrap();
        crystal_first.accrue_fees(supply_after_first_fee, 0);
        crystal_first.aum = U64::from(AUM + DEPOSIT);
        let supply_before_second_fee = supply_after_first_fee + entrant_second;
        let (crystal_first_supply, second_fee_shares) =
            crystallize_booked(&mut crystal_first, supply_before_second_fee, 0);
        let crystal_first_fee_shares = first_fee_shares + second_fee_shares;
        assert_eq!(crystal_first.raw_fee_liability(crystal_first_supply, 0), 0);

        let values = |v: &Vault, supply: u64, entrant: u64, fees: u64| {
            let net = v.eff_aum(supply, 0);
            [
                assets_for_redeem(SUPPLY, supply, net).unwrap(),
                assets_for_redeem(entrant, supply, net).unwrap(),
                assets_for_redeem(fees, supply, net).unwrap(),
            ]
        };
        let deposit_first_values = values(
            &deposit_first,
            deposit_first_supply,
            entrant_first,
            deposit_first_fee_shares,
        );
        let crystal_first_values = values(
            &crystal_first,
            crystal_first_supply,
            entrant_second,
            crystal_first_fee_shares,
        );

        for (deposit_first, crystal_first) in
            deposit_first_values.into_iter().zip(crystal_first_values)
        {
            assert!(
                deposit_first.abs_diff(crystal_first) <= DUST,
                "cohort value depends on ordering: {deposit_first} vs {crystal_first}",
            );
        }
        assert!(deposit_first_values[0].abs_diff(AUM - DEBT) <= DUST);
        assert!(deposit_first_values[1].abs_diff(DEPOSIT) <= DUST);
        assert!(deposit_first_values[2].abs_diff(DEBT) <= DUST);
    }

    #[test]
    fn per_second_crystallization_cannot_grind_the_streaming_fee_away() {
        const SUPPLY: u64 = BASE;
        // Small enough that one second of accrual is worth well under one
        // numéraire unit at 25 bps, but large enough for
        // the full window to accrue several whole units.
        let mut v = vault(BASE);
        v.management_fee_bps = U16::from(25u16);

        let mut ground = 0u64;
        for t in 1..=600i64 {
            v.accrue_fees(SUPPLY, t);
            ground += v.take_fees().total();
        }

        // The honest baseline: leave it alone and collect once at the end.
        let mut honest = vault(BASE);
        honest.management_fee_bps = U16::from(25u16);
        honest.accrue_fees(SUPPLY, 600);
        let collected = honest.take_fees().total();

        assert!(collected > 1, "setup: 600s must accrue several fee units");
        assert_eq!(
            ground, collected,
            "crystallization cadence changed the fee: {ground} across 600 calls \
             vs {collected} in one"
        );
    }

    #[test]
    fn zero_protocol_rates_leave_management_and_performance_fees_with_curator() {
        const AUM: u64 = 1_000_000_000;
        let mut v = vault(AUM);
        v.management_fee_bps = U16::from(75u16);
        v.performance_fee_bps = U16::from(750u16);
        v.fee_capital_time = U128::from(u128::from(AUM) * SECONDS_PER_YEAR);
        // The entire booked claim, including its last unit, goes to the curator.
        v.perf_fee_accrued = U64::from(100_000_001u64);

        let fees = v.take_fees();
        assert_eq!(
            fees,
            FeeBreakdown {
                curator_management: 7_500_000,
                protocol_management: 0,
                curator_performance: 100_000_001,
                protocol_performance: 0,
            }
        );
        assert_eq!(fees.total(), 107_500_001);
        assert_eq!(v.fee_capital_time.get(), 0);
        assert_eq!(v.perf_fee_accrued.get(), 0);
    }

    #[test]
    fn fee_rates_can_be_zero_independently() {
        for (management, performance) in [(0u16, 0u16), (0, 1_000), (100, 0), (100, 1_000)] {
            let mut v = vault(2 * BASE);
            v.management_fee_bps = U16::from(management);
            v.performance_fee_bps = U16::from(performance);
            let now = SECONDS_PER_YEAR as i64;
            v.accrue_fees(BASE, now);
            let liability = v.raw_fee_liability(BASE, now);
            let fees = v.take_fees();
            assert_eq!(fees.curator_management > 0, management > 0);
            assert_eq!(fees.curator_performance > 0, performance > 0);
            assert_eq!(fees.protocol_total(), 0);
            assert_eq!(fees.total(), liability);
            assert_eq!(v.fee_capital_time.get(), 0);
            assert_eq!(v.perf_fee_accrued.get(), 0);
            if management == 0 && performance == 0 {
                assert_eq!(fees.total(), 0);
                assert_eq!(v.eff_aum(BASE, now), 2 * BASE);
            }
        }
    }

    #[test]
    fn fee_liability_above_the_payment_cap_is_written_down() {
        const SUPPLY: u64 = BASE;
        const YEAR: i64 = SECONDS_PER_YEAR as i64;
        let now = 5 * YEAR;

        let mut v = vault(BASE);
        v.management_fee_bps = U16::from(1_000u16); // 10% + protocol management

        // Five years uncrystallized, then a 95% drawdown: the previously
        // folded liability is much larger than the live book.
        v.accrue_fees(SUPPLY, now);
        v.aum = U64::from(BASE / 20);
        let stranded = v.raw_fee_liability(SUPPLY, now);
        assert!(
            stranded > u64::from(v.aum),
            "setup: debt must strand the book"
        );

        // Any fold — including permissionless AUM validation — makes the
        // write-down sticky at the live payable cap.
        v.accrue_capital_time(now);
        v.write_down_unpayable_fees(v.eff_supply(SUPPLY), now);
        let cap = v.fee_payable_cap();
        assert!(v.raw_fee_liability(SUPPLY, now) <= cap);
        assert!(v.eff_aum(SUPPLY, now) >= u64::from(v.aum) - cap);

        // One crystallization drains every whole collectible unit. A
        // sub-numeraire streaming fraction remains carried, but no senior
        // liability survives to freeze a later flow or resurrect after an AUM
        // recovery.
        let took = v.take_fees().total();
        assert!(took > 0 && took <= cap, "collected {took}, cap {cap}");
        assert_eq!(v.accrued_fee_assets(now), 0);
        assert_eq!(v.perf_fee_accrued.get(), 0);
        assert_eq!(v.raw_fee_liability(SUPPLY, now), 0);
    }

    #[test]
    fn share_quotes_apply_the_fee_cap_before_reconciliation() {
        const AUM: u64 = 30 * UNIT;
        const SUPPLY: u64 = 125 * UNIT / 10;
        const DEPOSIT: u64 = 30 * UNIT;

        let mut v = vault(AUM);
        v.perf_fee_accrued = U64::from(20 * UNIT);
        assert!(v.raw_fee_liability(SUPPLY, 0) > v.fee_payable_cap());

        let quoted_aum = v.eff_aum(SUPPLY, 0);
        assert_eq!(quoted_aum, AUM / 2);
        let deposit_quote = shares_for_deposit(DEPOSIT, SUPPLY, quoted_aum).unwrap();
        let redeem_quote = assets_for_redeem(SUPPLY, SUPPLY, quoted_aum).unwrap();

        v.accrue_fees(SUPPLY, 0);
        assert_eq!(quoted_aum, v.eff_aum(SUPPLY, 0));
        assert_eq!(
            deposit_quote,
            shares_for_deposit(DEPOSIT, SUPPLY, v.eff_aum(SUPPLY, 0)).unwrap()
        );
        assert_eq!(
            redeem_quote,
            assets_for_redeem(SUPPLY, SUPPLY, v.eff_aum(SUPPLY, 0)).unwrap()
        );
    }

    #[test]
    fn reversed_performance_gain_is_written_down_to_the_live_cap() {
        const SUPPLY: u64 = BASE;
        let mut v = vault(BASE);
        v.performance_fee_bps = U16::from(5_000u16);
        v.accrue_fees(SUPPLY, 0);

        // A 3x mark books roughly one AUM of performance fees at 50%.
        v.aum = U64::from(3 * BASE);
        v.accrue_fees(SUPPLY, 0);
        let peak_fee = u64::from(v.perf_fee_accrued);
        assert!(peak_fee > BASE / 2, "setup: peak fee {peak_fee}");

        // The gain reverses before crystallization. The old claim cannot stay
        // senior to the whole recovered principal.
        v.aum = U64::from(BASE);
        v.accrue_fees(SUPPLY, 0);
        assert!(u64::from(v.perf_fee_accrued) < peak_fee);
        assert!(v.raw_fee_liability(SUPPLY, 0) <= v.fee_payable_cap());
        assert!(v.eff_aum(SUPPLY, 0) >= BASE / 2);

        let written_down = u64::from(v.perf_fee_accrued);
        v.accrue_fees(SUPPLY, 0);
        assert_eq!(
            u64::from(v.perf_fee_accrued),
            written_down,
            "forgiven debt was re-booked as a fresh performance gain"
        );
    }

    #[test]
    fn forgiven_performance_fees_are_not_rebooked_on_gross_recovery() {
        for pending in [0, BASE / 4] {
            let supply = BASE - pending;
            let mut v = vault(BASE);
            v.pending_redeem_shares = U64::from(pending);
            v.performance_fee_bps = U16::from(5_000u16);
            v.accrue_fees(supply, 0);
            v.aum = U64::from(3 * BASE);
            v.accrue_fees(supply, 0);

            // Repeated recoveries, including debt exceeding the entire trough book.
            for trough in [6 * BASE / 5, BASE / 10, 0] {
                let peak_fee = u64::from(v.perf_fee_accrued);
                v.aum = U64::from(trough);
                v.accrue_fees(supply, 0);
                let retained = u64::from(v.perf_fee_accrued);
                assert!(retained < peak_fee);
                assert_eq!(retained, trough / 2);

                v.aum = U64::from(3 * BASE);
                assert_eq!(v.perf_pending(BASE, 0), 0);
                v.accrue_fees(supply, 0);
                v.accrue_fees(supply, 0);
                assert_eq!(u64::from(v.perf_fee_accrued), retained);
            }

            // A new economic gain above the old gross peak still earns fees.
            v.aum = U64::from(4 * BASE);
            v.accrue_fees(supply, 0);
            assert!(u64::from(v.perf_fee_accrued) > BASE / 4);
            let retained = u64::from(v.perf_fee_accrued);
            assert_eq!(v.take_fees().total(), retained);
        }
    }

    #[test]
    fn forgiven_streaming_fees_are_not_rebooked_on_gross_recovery() {
        let mut v = vault(4 * BASE);
        v.management_fee_bps = U16::from(1_000u16);
        v.performance_fee_bps = U16::ZERO;
        v.fee_capital_time = U128::from(8 * u128::from(BASE) * SECONDS_PER_YEAR);
        v.accrue_fees(BASE, 0);
        v.performance_fee_bps = U16::from(5_000u16);

        for trough in [BASE, BASE / 10, 0] {
            let peak_fee = v.accrued_fee_assets(0);
            v.aum = U64::from(trough);
            v.accrue_fees(BASE, 0);
            let retained = v.accrued_fee_assets(0);
            assert!(retained < peak_fee);
            assert_eq!(retained, trough / 2);

            v.aum = U64::from(4 * BASE);
            assert_eq!(v.perf_pending(BASE, 0), 0);
            v.accrue_fees(BASE, 0);
            assert_eq!(u64::from(v.perf_fee_accrued), 0);
            assert_eq!(v.accrued_fee_assets(0), retained);
        }

        v.aum = U64::from(5 * BASE);
        v.accrue_fees(BASE, 0);
        assert!(u64::from(v.perf_fee_accrued) > BASE / 4);
    }

    #[test]
    fn fee_write_down_keeps_streaming_senior_to_performance() {
        const DENOM: u128 = 10_000 * SECONDS_PER_YEAR;
        const STREAMING: u64 = 400 * UNIT;
        const PERF: u64 = 400 * UNIT;
        const SUPPLY: u64 = BASE;

        let mut v = vault(BASE);
        v.management_fee_bps = U16::from(25u16);
        v.performance_fee_bps = U16::ZERO;
        v.fee_capital_time = U128::from(u128::from(STREAMING) * DENOM / v.streaming_bps());
        v.perf_fee_accrued = U64::from(PERF);
        v.write_down_unpayable_fees(SUPPLY, 0);

        assert_eq!(v.accrued_fee_assets(0), STREAMING);
        assert_eq!(u64::from(v.perf_fee_accrued), BASE / 2 - STREAMING);
        assert_eq!(v.raw_fee_liability(SUPPLY, 0), BASE / 2);
    }

    #[test]
    fn fee_write_down_ratchets_hwm_over_the_forgiven_claim() {
        const SUPPLY: u64 = BASE;
        let mut v = vault(4 * BASE);
        let prior_hwm = u128::from(v.hwm_nav);
        v.perf_fee_accrued = U64::from(3 * BASE);

        v.write_down_unpayable_fees(SUPPLY, 0);

        let post_write_down_nav = v.nav_for_watermark(SUPPLY, 0);
        assert!(post_write_down_nav > prior_hwm);
        // The forgiveness adjustment rounds up by at most one NAV quantum.
        assert!(u128::from(v.hwm_nav).abs_diff(post_write_down_nav) <= 1);
        assert_eq!(u64::from(v.perf_fee_accrued), v.fee_payable_cap());
    }

    // The cumulative liquidation budget is the only thing bounding the SUM of
    // liquidation shortfall; an unvalidated book must not be a way around it.
    #[test]
    fn liquidation_budget_is_not_bypassed_by_an_unvalidated_book() {
        let mut v = vault(0);
        assert!(
            v.charge_liquidation_slippage(1, 0).is_err(),
            "aum == 0 must reject any shortfall, not waive the budget"
        );
        assert!(
            v.charge_liquidation_slippage(0, 0).is_ok(),
            "a lossless liquidation still passes"
        );

        // With a book: hops accumulate against the bucket capacity...
        let mut v = vault(BASE);
        let budget = ((BASE as u128) * MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS / 10_000) as u64;
        for _ in 0..4 {
            v.charge_liquidation_slippage(budget / 4, 0)
                .expect("in-budget hop");
        }
        assert!(
            v.charge_liquidation_slippage(1, 0).is_err(),
            "the cumulative ceiling must bind once spent"
        );
        // ...and a full refill period restores capacity.
        v.charge_liquidation_slippage(budget, SWAP_SLIPPAGE_WINDOW_SECONDS)
            .expect("a full hour refills the bucket");
    }

    #[test]
    fn slippage_budgets_cannot_double_spend_across_a_window_boundary() {
        let mut v = vault(BASE);
        let swap_budget = ((BASE as u128) * MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS / 10_000) as u64;
        let liq_budget = ((BASE as u128) * MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS / 10_000) as u64;
        let boundary = SWAP_SLIPPAGE_WINDOW_SECONDS;

        // Spend a full capacity one second before the old fixed window ended:
        // that reset admitted a second full charge, a leaky bucket does not.
        v.charge_swap_slippage(0, 0).unwrap();
        v.charge_swap_slippage(swap_budget, boundary - 1).unwrap();
        assert!(
            v.charge_swap_slippage(swap_budget, boundary).is_err(),
            "adjacent swaps spent two full hourly budgets"
        );

        v.charge_liquidation_slippage(0, 0).unwrap();
        v.charge_liquidation_slippage(liq_budget, boundary - 1)
            .unwrap();
        assert!(
            v.charge_liquidation_slippage(liq_budget, boundary).is_err(),
            "adjacent liquidations spent two full hourly budgets"
        );

        // Capacity returns continuously, measured from the charge.
        v.charge_swap_slippage(swap_budget / 2, boundary - 1 + boundary / 2)
            .expect("half the swap bucket should refill in half an hour");
        v.charge_swap_slippage(swap_budget / 2, boundary - 1 + boundary)
            .expect("the remaining swap capacity should refill continuously");
    }

    // H-01: resizing capacity must not forgive spent slippage in either bucket.
    #[test]
    fn slippage_history_survives_aum_shrink_and_recovery() {
        let mut v = vault(1_000);
        for _ in 0..2 {
            v.charge_swap_slippage(10, 0).unwrap();
            v.charge_liquidation_slippage(50, 0).unwrap();
        }

        for aum in [400, 0, 1_000] {
            v.aum = U64::from(aum);
            v.charge_swap_slippage(0, 0).unwrap();
            v.charge_liquidation_slippage(0, 0).unwrap();
            assert!(v.charge_swap_slippage(1, 0).is_err());
            assert!(v.charge_liquidation_slippage(1, 0).is_err());
            assert_eq!(
                (
                    v.swap_slippage_spent.get(),
                    v.liq_slippage_spent.get(),
                    v.swap_slippage_last_refill_at.get(),
                    v.liq_slippage_last_refill_at.get(),
                ),
                (20, 100, 0, 0)
            );
        }

        let half_hour = SWAP_SLIPPAGE_WINDOW_SECONDS / 2;
        v.charge_swap_slippage(10, half_hour).unwrap();
        v.charge_liquidation_slippage(50, half_hour).unwrap();
        assert!(v.charge_swap_slippage(1, half_hour).is_err());
        assert!(v.charge_liquidation_slippage(1, half_hour).is_err());
    }

    // M-04: retained debt must still decay after a lasting AUM shrink.
    #[test]
    fn slippage_bucket_repays_excess_debt_before_allowing_new_losses() {
        let hour = SWAP_SLIPPAGE_WINDOW_SECONDS;
        for bps in [
            MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS,
            MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS,
        ] {
            let budget = (1_000 * bps / 10_000) as u64;
            let small = (400 * bps / 10_000) as u64;
            let refill =
                |spent, at, charge, now| refill_slippage_bucket(400, spent, at, charge, now, bps);
            let (spent, at) = refill(budget, 0, 0, hour / 2).unwrap();
            assert_eq!((spent, at), (budget - small / 2, hour / 2));
            assert_eq!(
                refill(spent, at, 1, hour),
                Err(VaultError::SwapSlippageBudgetExceeded)
            );
            // Backward time cannot forgive debt or move the refill timestamp.
            assert_eq!(refill(spent, at, 0, 0).unwrap(), (spent, at));
            // Two hours repay enough debt to admit half the smaller capacity.
            assert_eq!(
                refill(spent, at, small / 2, 2 * hour).unwrap(),
                (small, 2 * hour)
            );
            // No intermediate zero-loss calls are required for full recovery.
            assert_eq!(
                refill(budget, 0, small, 5 * hour / 2).unwrap(),
                (small, 5 * hour / 2)
            );
            assert_eq!(
                refill_slippage_bucket(0, budget, 0, 0, hour, bps).unwrap(),
                (budget, hour)
            );
        }
    }

    #[test]
    fn entry_nav_after_a_strand_is_deposit_size_independent() {
        let v = vault(BASE);
        let entry = v.nav_for_watermark(0, 0);
        for deposit in [UNIT, BASE, 1_000 * BASE] {
            let shares = shares_for_deposit(deposit, 0, BASE).unwrap();
            let mut w = vault(BASE + deposit);
            let landed = w.nav_for_watermark(shares, 0);
            let drift_bps = (landed.abs_diff(entry)).saturating_mul(10_000) / entry;
            assert!(
                drift_bps <= 10,
                "entry NAV moved {drift_bps} bps at {deposit}"
            );

            w.hwm_nav = U128::from(entry);
            w.accrue_fees(shares, 0);
            assert!(
                u64::from(w.perf_fee_accrued) <= deposit / 10_000,
                "principal charged as gain at deposit {deposit}"
            );
        }
    }

    #[test]
    fn instant_mark_fee_gate_compounds_distinct_required_positions() {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        for position in &mut v.positions[..2] {
            position.flags = POSITION_FLAG_ENABLED | POSITION_FLAG_REQUIRED_IN_AUM;
            // Stored zero is the ordinary MAX_DRIFT_BPS default.
            position.max_drift_bps = U16::ZERO;
        }

        // Two independent 200 bps marks compose to 1.02^2 - 1 = 404 bps.
        v.instant_redeem_fee_bps = U16::from(403u16);
        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
        v.instant_redeem_fee_bps = U16::from(404u16);
        assert!(v.require_instant_mark_fee_safe().is_ok());

        // The disabled marker skips only the finite-fee comparison.
        v.instant_redeem_fee_bps = U16::from(u16::MAX);
        assert!(v.require_instant_mark_fee_safe().is_ok());
    }

    #[test]
    fn instant_mark_fee_gate_rejects_unbounded_required_position() {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        v.instant_redeem_fee_bps = U16::from(1_000u16);
        v.positions[0].flags = POSITION_FLAG_ENABLED | POSITION_FLAG_REQUIRED_IN_AUM;
        // window_bps preserves the explicit u16::MAX waiver.
        v.positions[0].max_drift_bps = U16::from(u16::MAX);

        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
    }

    #[test]
    fn instant_mark_fee_gate_rejects_net_fluid_mark() {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        v.instant_redeem_fee_bps = U16::from(10_000u16);
        v.positions[0].protocol = Protocol::JupiterLend as u8;
        v.positions[0].flags = POSITION_FLAG_ENABLED;
        v.positions[0].last_value = U64::from(600u64);
        assert!(v.require_instant_mark_fee_safe().is_ok());

        v.positions[0].flags |= POSITION_FLAG_REQUIRED_IN_AUM;

        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );

        v.positions[0].flags &= !POSITION_FLAG_REQUIRED_IN_AUM;
        assert!(v.require_instant_mark_fee_safe().is_ok());
    }

    #[test]
    fn instant_mark_fee_gate_admits_earn_only_mark_within_its_drift_cap() {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        v.instant_redeem_fee_bps = U16::from(50u16);
        v.positions[0].protocol = Protocol::JupiterLend as u8;
        v.positions[0].flags =
            POSITION_FLAG_ENABLED | POSITION_FLAG_REQUIRED_IN_AUM | POSITION_FLAG_LENDER_ONLY;
        v.positions[0].max_drift_bps = U16::from(50u16);
        assert!(v.require_instant_mark_fee_safe().is_ok());

        // Earn-only lifts the Fluid block, not the aggregate drift bound.
        v.positions[0].max_drift_bps = U16::from(51u16);
        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
    }

    #[test]
    fn instant_mark_fee_gate_compounds_each_sink_bootstrap() {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        for (i, sink) in v.sinks[..2].iter_mut().enumerate() {
            sink.mint = Address::new_from_array([i as u8 + 1; 32]);
        }

        // mark -> validate can be interleaved between zero sinks, so the second
        // aggregate ceiling reads the AUM raised by the first: 404 bps total.
        v.instant_redeem_fee_bps = U16::from(403u16);
        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
        v.instant_redeem_fee_bps = U16::from(404u16);
        assert!(v.require_instant_mark_fee_safe().is_ok());

        // A nonzero walk composes identically with a remaining bootstrap.
        v.sinks[0].value = U64::from(1u64);
        v.instant_redeem_fee_bps = U16::from(403u16);
        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
        v.instant_redeem_fee_bps = U16::from(404u16);
        assert!(v.require_instant_mark_fee_safe().is_ok());
    }

    #[test]
    fn instant_mark_fee_gate_keeps_the_bootstrap_budget_after_first_mark() {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        v.sinks[0].mint = Address::new_from_array([1; 32]);
        v.sinks[0].value = U64::from(1u64);
        v.sink_bootstrap_cap_bps = U16::from(500u16);

        v.instant_redeem_fee_bps = U16::from(499u16);
        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
        v.instant_redeem_fee_bps = U16::from(500u16);
        assert!(v.require_instant_mark_fee_safe().is_ok());

        // Marking a sink must not turn an unbounded bootstrap cap into 200 bps.
        v.sink_bootstrap_cap_bps = U16::from(u16::MAX);
        v.instant_redeem_fee_bps = U16::from(10_000u16);
        assert_eq!(
            v.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );
    }

    #[test]
    fn instant_mark_fee_gate_rejects_manual_and_debt_books_when_enabled() {
        let mut manual: Vault = unsafe { core::mem::zeroed() };
        manual.instant_redeem_fee_bps = U16::from(10_000u16);
        manual.assets[0].mint = Address::new_from_array([1; 32]);
        manual.assets[0].flags = ASSET_FLAG_PRICE_IN_AUM | ASSET_FLAG_MANUAL_PRICE;
        assert_eq!(
            manual.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );

        let mut debt: Vault = unsafe { core::mem::zeroed() };
        debt.instant_redeem_fee_bps = U16::from(10_000u16);
        debt.positions[0].flags =
            POSITION_FLAG_ENABLED | POSITION_FLAG_REQUIRED_IN_AUM | POSITION_FLAG_DEBT;
        assert_eq!(
            debt.require_instant_mark_fee_safe(),
            Err(VaultError::InstantRedeemDisabled.into()),
        );

        manual.instant_redeem_fee_bps = U16::from(u16::MAX);
        debt.instant_redeem_fee_bps = U16::from(u16::MAX);
        assert!(manual.require_instant_mark_fee_safe().is_ok());
        assert!(debt.require_instant_mark_fee_safe().is_ok());
    }
}
