use {
    crate::{
        check_account_layout,
        constants::{
            BOOTSTRAP_HWM_NAV, MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS, MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS,
            NAV_SCALE, PROTOCOL_MANAGEMENT_FEE_BPS, PROTOCOL_PERFORMANCE_FEE_BPS, SECONDS_PER_YEAR,
            SWAP_SLIPPAGE_WINDOW_SECONDS,
        },
        errors::VaultError,
        state::{
            Account, AssetConfig, CcipLane, LoanLimits, MAX_ALLOWED_ASSETS, MAX_POLICIES,
            MAX_POSITIONS, MAX_SINKS, NestRoute, PositionConfig, Protocol, ProtocolPolicy,
            SinkSlot,
        },
    },
    codama_macros::CodamaAccount,
    solana_address::{Address, address_eq},
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI64, PodU16, PodU64, PodU128},
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
    pub created_at: PodI64,
    pub authority_bump: u8,

    // Configuration
    pub paused: u8,
    #[codama(type = number(u16))]
    pub management_fee_bps: PodU16,
    #[codama(type = number(u16))]
    pub instant_redeem_fee_bps: PodU16,
    /// Haircut applied to occupied sink marks by `validate_aum`.
    #[codama(type = number(u16))]
    pub sink_haircut_bps: PodU16,
    /// Sink-mark freshness window; `0` uses `MAX_STALENESS_SECONDS`.
    #[codama(type = number(u64))]
    pub sink_max_staleness_seconds: PodU64,
    /// Minimum seconds between `Redeem` and `ClaimRedeem`, giving the manager
    /// time to recall capital. `0` disables; capped at `MAX_TIMELOCK_DELAY`.
    #[codama(type = number(u64))]
    pub timelock_delay: PodU64,
    /// Transient liability cap in numéraire micro-units; `0` disables it.
    #[codama(type = number(u64))]
    pub max_flash_liability: PodU64,

    // Live state
    #[codama(type = number(u64))]
    pub aum: PodU64,
    #[codama(type = number(i64))]
    pub last_priced_at: PodI64,
    #[codama(type = number(u64))]
    pub last_priced_slot: PodU64,
    #[codama(type = number(u64))]
    pub pending_redeem_shares: PodU64,
    /// Raw principal temporarily lent to custody by a `FLASH` delegate.
    #[codama(type = number(u64))]
    pub flash_liability: PodU64,
    /// Performance-fee high-water mark: highest `nav_for_watermark` ever
    /// folded, ratcheted on every supply-changing flow. Never lowered.
    #[codama(type = number(u128))]
    pub hwm_nav: PodU128,
    #[codama(type = number(u128))]
    pub fee_capital_time: PodU128,
    #[codama(type = number(i64))]
    pub last_fee_accrual_at: PodI64,
    #[codama(type = number(u64))]
    pub perf_fee_accrued: PodU64,
    /// Currently consumed swap-slippage bucket capacity, in numeraire.
    #[codama(type = number(u64))]
    pub swap_slippage_spent: PodU64,
    /// Timestamp of the last successful swap-slippage bucket refill.
    #[codama(type = number(i64))]
    pub swap_slippage_last_refill_at: PodI64,
    /// Currently consumed liquidation-slippage bucket capacity, in numeraire.
    /// Shared liquidation shortfall across all sinks in the current window.
    #[codama(type = number(u64))]
    pub liq_slippage_spent: PodU64,
    /// Timestamp of the last successful liquidation-slippage bucket refill.
    #[codama(type = number(i64))]
    pub liq_slippage_last_refill_at: PodI64,

    // Rows
    pub assets: [AssetConfig; 8],
    /// Free slots; each position names its own protocol.
    pub positions: [PositionConfig; 8],
    /// Seized collateral awaiting sale, keyed by mint. See [`SinkSlot`].
    pub sinks: [SinkSlot; 4],
    #[codama(type = number(u16))]
    pub sink_bootstrap_cap_bps: PodU16,
    pub sink_capacity: u8,
    /// HWM performance fee on NAV gains, in bps, minted to the curator by
    /// `crystallize_fees`.
    #[codama(type = number(u16))]
    pub performance_fee_bps: PodU16,
    pub curator_fee_account_bump: u8,
    pub protocol_fee_account_bump: u8,
    /// Curator policy per routed protocol (Offerbook loan limits, the CCIP
    /// lane, …), tagged by [`Protocol`] and decoded by the owning adapter. See
    /// [`ProtocolPolicy`]; typed reads via [`Vault::offerbook_limits`],
    /// [`Vault::ccip_lane`] and [`Vault::nest_route`].
    pub policies: [ProtocolPolicy; 8],
    /// Confidence discount `validate_aum` left in `aum`: `Σ(mid − floor)` over
    /// the spot rows, in bps of `aum`, rounded up. Every share mint adds it
    /// back so a band that closes on the next report pays the holders who
    /// carried it, not the new shares (L-04). `u16::MAX` = not representable;
    /// mints fail closed.
    #[codama(type = number(u16))]
    pub aum_conf_bps: PodU16,
    /// Live `Delegate` PDAs. `grant_revoke_delegate` counts creates and
    /// closes; `close_vault` refuses while any remain, so no grant can outlive
    /// the vault and rebind to a same-address recreation (I-01).
    pub delegate_count: u8,
    _buffer: [u8; 3],
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
    let elapsed = now
        .saturating_sub(last_refill_at)
        .clamp(0, SWAP_SLIPPAGE_WINDOW_SECONDS) as u128;
    let refilled = capacity.saturating_mul(elapsed) / SWAP_SLIPPAGE_WINDOW_SECONDS as u128;
    let decayed = u128::from(prior_spent)
        .min(capacity)
        .saturating_sub(refilled);
    let updated = decayed.saturating_add(u128::from(charge));
    if updated > capacity {
        return Err(VaultError::SwapSlippageBudgetExceeded);
    }
    Ok((narrow(updated), now.max(last_refill_at)))
}

impl Vault {
    /// The enabled, AUM-counted position tracking `protocol`. A scan, because
    /// slots are free: `initialize_vault` admits at most one enabled slot per
    /// protocol, which is what makes "first match" unambiguous.
    #[inline]
    /// The stored policy for `protocol`, if the curator set one.
    pub fn policy(&self, protocol: Protocol) -> Option<&ProtocolPolicy> {
        self.policies
            .iter()
            .find(|p| !p.is_free() && p.protocol == protocol as u8)
    }

    /// Store, replace, or (for a disabled all-zero payload) free `policy`'s
    /// slot. One slot per protocol; `InvalidArgument` when every slot is taken
    /// by another protocol.
    pub fn set_policy(&mut self, policy: ProtocolPolicy) -> Result<(), ProgramError> {
        let tag = policy.protocol;
        let slot = match self
            .policies
            .iter()
            .position(|p| !p.is_free() && p.protocol == tag)
        {
            Some(i) => i,
            None if policy.is_disabled() => return Ok(()),
            None => self
                .policies
                .iter()
                .position(ProtocolPolicy::is_free)
                .ok_or(ProgramError::InvalidArgument)?,
        };
        self.policies[slot] = if policy.is_disabled() {
            ProtocolPolicy::zero()
        } else {
            policy
        };
        Ok(())
    }

    /// Offerbook loan policy; the all-zero value means loans are closed.
    #[inline]
    pub fn offerbook_limits(&self) -> LoanLimits {
        self.policy(Protocol::Offerbook)
            .map_or(LoanLimits::zero(), ProtocolPolicy::decode)
    }

    /// CCIP destination lane; the all-zero value means bridging is off.
    #[inline]
    pub fn ccip_lane(&self) -> CcipLane {
        self.policy(Protocol::Ccip)
            .map_or(CcipLane::zero(), ProtocolPolicy::decode)
    }

    /// Nest composer / relayer / OFT route; the all-zero value means Nest is off.
    #[inline]
    pub fn nest_route(&self) -> NestRoute {
        self.policy(Protocol::Nest)
            .map_or(NestRoute::zero(), ProtocolPolicy::decode)
    }

    pub fn find_position_mut(&mut self, protocol: u8) -> Option<&mut PositionConfig> {
        self.positions
            .iter_mut()
            .find(|p| p.protocol == protocol && p.enabled() && p.required_in_aum())
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
        // The zero mint is the vacancy marker (`SinkSlot::is_free`): stamping
        // it into a slot would leave an occupant that still reads as free. No
        // initialized mint has the zero address, so this is the encoding's
        // invariant made explicit rather than a reachable settlement path.
        if address_eq(mint, &Address::default())
            || address_eq(mint, &self.share_mint)
            || self
                .assets
                .iter()
                .any(|asset| asset.configured() && address_eq(&asset.mint, mint))
        {
            return Err(VaultError::DisallowedAsset);
        }
        if let Some(i) = self.sinks.iter().position(|s| s.holds(mint)) {
            return Ok(&mut self.sinks[i]);
        }
        let capacity = (self.sink_capacity as usize).min(MAX_SINKS);
        let i = self.sinks[..capacity]
            .iter()
            .position(|s| s.is_free())
            .ok_or(VaultError::SinkCapacityExhausted)?;
        self.sinks[i].mint = *mint;
        Ok(&mut self.sinks[i])
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
        const DENOM: u128 = 10_000 * SECONDS_PER_YEAR;
        // Saturating: degrade to "everything is fees", never to a small
        // wrong number. Fractional numéraire stays carried in capital-time
        // until it becomes a whole collectible unit; crystallization frequency
        // therefore cannot manufacture rounding dust.
        let numerator = self
            .fee_capital_time_at(now)
            .saturating_mul(self.streaming_bps());
        let assets = numerator / DENOM;
        u64::try_from(assets).unwrap_or(u64::MAX)
    }

    #[inline]
    fn fold_capital_time(&mut self, now: i64) {
        self.fee_capital_time = PodU128::from(self.fee_capital_time_at(now));
        self.last_fee_accrual_at = PodI64::from(now);
    }

    /// Fold the elapsed AUM·time segment and stamp the clock. MUST run before
    /// any write to `aum` so each segment is billed at the AUM that held it.
    #[inline]
    pub fn accrue_capital_time(&mut self, now: i64) {
        self.fold_capital_time(now);
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
        u64::try_from(perf).unwrap_or(u64::MAX)
    }

    /// Fold both fee integrals (streaming + performance, ratcheting the HWM).
    /// MUST run with the PRE-change supply in every instruction that moves
    /// supply or the redeem accumulators, so each gain is billed at the supply
    /// that held it. `Redeem`/`CancelRedeem` are exempt: `eff_supply` is
    /// unchanged there.
    #[inline]
    pub fn accrue_fees(&mut self, share_supply: u64, now: i64) {
        let eff_supply = share_supply.saturating_add(u64::from(self.pending_redeem_shares));
        // No holders → there is no cohort to charge or dilute. Forgive fee
        // integrals that can no longer be consumed, then re-arm the HWM from
        // that cleared basis. Re-arming before the clear would make the next
        // cohort's principal look like a performance gain.
        if eff_supply == 0 {
            self.fee_capital_time = PodU128::ZERO;
            self.perf_fee_accrued = PodU64::ZERO;
            self.last_fee_accrual_at = PodI64::from(now);
            self.hwm_nav = PodU128::from(
                crate::nav::nav_per_share(0, self.eff_aum(0, now)).unwrap_or(BOOTSTRAP_HWM_NAV),
            );
            return;
        }
        // Measure the NAV rise before forgiving anything above the payable
        // cap. Otherwise the write-down itself raises fee-net NAV and can be
        // re-booked immediately as a performance fee.
        self.fold_capital_time(now);
        let prior_hwm = u128::from(self.hwm_nav);
        let nav = self.nav_for_watermark(eff_supply, now);
        if nav > prior_hwm {
            let pending = self.perf_pending(eff_supply, now);
            if pending > 0 {
                self.perf_fee_accrued =
                    PodU64::from(u64::from(self.perf_fee_accrued).saturating_add(pending));
            }
        }
        self.write_down_unpayable_fees(eff_supply, now);
        // Ratchet from the post-write-down basis even when no fee was booked:
        // forgiven debt is not a gain that a later fold may charge again.
        self.hwm_nav = PodU128::from(prior_hwm.max(self.nav_for_watermark(eff_supply, now)));
    }

    /// Ratchet the HWM over a non-economic NAV increase after fee shares are
    /// minted. `share_supply` must be the post-mint supply and the fee asset
    /// liabilities must already be debited. Independent recipient share floors
    /// can otherwise leave a small apparent gain that a later performance-rate
    /// change could reprice. This only waives that rounding benefit; it never
    /// lowers the existing watermark.
    #[inline]
    pub fn ratchet_hwm_after_fee_mint(&mut self, share_supply: u64, now: i64) {
        let eff_supply = self.eff_supply(share_supply);
        if eff_supply > 0 {
            self.hwm_nav = PodU128::from(
                u128::from(self.hwm_nav).max(self.nav_for_watermark(eff_supply, now)),
            );
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
    /// forgiveness can ratchet the HWM on the resulting fee-net basis.
    #[inline]
    pub fn write_down_unpayable_fees(&mut self, eff_supply: u64, now: i64) {
        const DENOM: u128 = 10_000 * SECONDS_PER_YEAR;
        let cap = self.fee_payable_cap();
        let bps = self.streaming_bps();
        let prior_capital_time = u128::from(self.fee_capital_time);
        let prior_perf = u64::from(self.perf_fee_accrued);

        // Compare the exact fractional claim to the cap. In particular, a
        // zero-cap book must forgive sub-unit capital-time instead of letting
        // it resurrect after a later AUM recovery.
        if prior_capital_time.saturating_mul(bps) > u128::from(cap).saturating_mul(DENOM) {
            self.fee_capital_time = PodU128::from(u128::from(cap).saturating_mul(DENOM) / bps);
        }

        let room = cap.saturating_sub(self.accrued_fee_assets(now));
        self.perf_fee_accrued = PodU64::from(u64::from(self.perf_fee_accrued).min(room));

        if u128::from(self.fee_capital_time) != prior_capital_time
            || u64::from(self.perf_fee_accrued) != prior_perf
        {
            // Forgiveness raises fee-net NAV; ratchet to that post-write-down
            // basis so the waived claim cannot be booked again as performance.
            self.hwm_nav = PodU128::from(
                u128::from(self.hwm_nav).max(self.nav_for_watermark(eff_supply, now)),
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
        let eff_supply = share_supply.saturating_add(u64::from(self.pending_redeem_shares));
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
        const DENOM: u128 = 10_000 * SECONDS_PER_YEAR;
        let capital_time = u128::from(self.fee_capital_time);
        let streaming = capital_time.saturating_mul(self.streaming_bps()) / DENOM;
        let curator_management =
            capital_time.saturating_mul(u128::from(self.management_fee_bps.get())) / DENOM;
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
        const DENOM: u128 = 10_000 * SECONDS_PER_YEAR;
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
            streaming.saturating_mul(DENOM).div_ceil(bps)
        };
        self.fee_capital_time = PodU128::from(capital_time.saturating_sub(consumed));
        self.perf_fee_accrued = PodU64::from(
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

    /// Charge `slippage` against the swap leaky bucket, which decays linearly
    /// over an hour (a fixed window would grant two budgets in adjacent slots).
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
        self.swap_slippage_spent = PodU64::from(spent);
        self.swap_slippage_last_refill_at = PodI64::from(at);
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
        self.liq_slippage_spent = PodU64::from(spent);
        self.liq_slippage_last_refill_at = PodI64::from(at);
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
        self.aum
            .get()
            .saturating_sub(self.raw_fee_liability(share_supply, now))
    }
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
}

impl Account for Vault {
    const LEN: usize = 1 + core::mem::size_of::<Self>();
    const DISCRIMINATOR: u8 = 1;
}

check_account_layout!(Vault);
const _: () = assert!(core::mem::size_of::<[ProtocolPolicy; MAX_POLICIES]>() == 8 * 64);

// Pin the deployed zero-copy ABI. The generic layout check above only proves
// that `LEN` matches the current Rust type; these constants catch accidental
// field insertion or reordering before it can strand existing vault accounts.
const _: () = assert!(core::mem::size_of::<Vault>() == 2_209);
const _: () = assert!(Vault::LEN == 2_210);
const _: () = assert!(core::mem::offset_of!(Vault, management_fee_bps) == 106);
const _: () = assert!(core::mem::offset_of!(Vault, performance_fee_bps) == 1_687);
const _: () = assert!(core::mem::offset_of!(Vault, curator_fee_account_bump) == 1_689);
const _: () = assert!(core::mem::offset_of!(Vault, protocol_fee_account_bump) == 1_690);

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
        },
    };

    fn vault(aum: u64) -> Vault {
        let mut v: Vault = unsafe { core::mem::zeroed() };
        v.aum = PodU64::from(aum);
        v.hwm_nav = PodU128::from(BOOTSTRAP_HWM_NAV);
        // A representative 10% curator rate; the protocol rate is additive.
        v.performance_fee_bps = PodU16::from(1_000);
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
        v.ratchet_hwm_after_fee_mint(supply, 0);
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
        v.ratchet_hwm_after_fee_mint(supply, now);
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
        v.perf_fee_accrued = PodU64::from(4_001u64);
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
        sink.value = PodU64::from(500 * UNIT);
        sink.set_priced();

        v.sinks[0].write_off(1_000);

        assert!(v.sinks[0].value.is_zero(), "the mark must leave AUM");
        assert!(v.sinks[0].holds(&seized), "the mint binding must survive");
        assert!(
            v.sinks[0].priced(),
            "written off at 0, so validate_aum credits nothing instead of \
             failing closed on an unpriced occupant"
        );
        assert!(v.find_sink_mut(&seized).is_some());

        // A successful full sale after re-marking is what frees the slot.
        v.sinks[0].release();
        assert!(v.sinks[0].is_free());
    }

    /// A mint is bounded by the *effective* supply, not `Mint.supply`.
    #[test]
    fn mint_headroom_counts_burned_redeem_shares() {
        let mut v = vault(BASE);
        v.pending_redeem_shares = PodU64::from(1_000u64);
        let supply = u64::MAX - 1_000;
        // Token-2022 would allow this — `supply + 1_000` still fits.
        assert_eq!(
            v.require_mint_headroom(supply, 1_000),
            Err(VaultError::AumOverflow)
        );
        assert_eq!(v.require_mint_headroom(supply, 0), Ok(()));
        // Without a queued redeem the same mint is fine.
        v.pending_redeem_shares = PodU64::ZERO;
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
        v.aum = PodU64::from(BASE + 100 * UNIT);
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
        v.aum = PodU64::from(BASE + 200 * UNIT);
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
        v.performance_fee_bps = PodU16::from(750u16);

        v.accrue_fees(SUPPLY, 0);
        assert!(
            u64::from(v.perf_fee_accrued) > 0,
            "fixture must book a real gain"
        );
        let post_fee_supply = crystallize(&mut v, SUPPLY);
        assert!(post_fee_supply > SUPPLY);

        // With separate curator/protocol share floors, this fixture leaves a
        // measurable NAV rounding gain unless crystallization ratchets over it.
        v.performance_fee_bps = PodU16::from(2_000u16);
        v.accrue_fees(post_fee_supply, 0);
        assert_eq!(
            u64::from(v.perf_fee_accrued),
            0,
            "fee-share rounding was repriced under the new performance rate"
        );
    }

    #[test]
    fn hwm_is_monotonic_across_folds() {
        const SUPPLY: u64 = BASE;
        let mut v = vault(BASE);
        let mut hwm = u128::from(v.hwm_nav);

        for mult in [1_050u64, 1_100, 1_100, 2_000, 1_500, 10_000] {
            let aum = BASE / 1_000 * mult;
            v.aum = PodU64::from(aum);
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
        v.aum = PodU64::from(10u64);
        v.accrue_fees(old_supply, 0);
        assert_eq!(u64::from(v.perf_fee_accrued), 0);
        assert!(
            u128::from(v.hwm_nav) > BOOTSTRAP_HWM_NAV,
            "the fee dust must still ratchet the cohort watermark"
        );

        // A stale HWM would multiply the forgiven dust by this larger supply.
        let deposit = BASE;
        let shares = shares_for_deposit(deposit, old_supply, v.eff_aum(old_supply, 0)).unwrap();
        v.aum = PodU64::from(u64::from(v.aum).saturating_add(deposit));
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
        v.fee_capital_time = PodU128::from(1u128);
        v.perf_fee_accrued = PodU64::from(UNIT);
        v.accrue_fees(0, 0);
        assert!(v.fee_capital_time.is_zero());
        assert!(v.perf_fee_accrued.is_zero());
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
        v.aum = PodU64::from(u64::from(v.aum) + deposit);
        v.accrue_fees(shares, 0);
        assert!(
            u64::from(v.perf_fee_accrued) <= deposit / 10_000,
            "principal charged as performance: {}",
            u64::from(v.perf_fee_accrued)
        );

        // ...and a genuine rise on top of it still is.
        v.aum = PodU64::from(u64::from(v.aum) * 3 / 2);
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
        v.management_fee_bps = PodU16::from(1_000u16); // 10% + protocol management

        // Five years of fee booked, then the cohort exits with a small backed
        // residual. An exactly insolvent book is not an ordinary entry path.
        v.accrue_fees(BASE, now);
        v.aum = PodU64::from(v.accrued_fee_assets(now).saturating_add(UNIT));
        v.accrue_fees(0, now);

        // Re-entry, priced exactly as `deposit_basis` prices it.
        let deposit = BASE;
        let shares = shares_for_deposit(deposit, 0, v.eff_aum(0, now)).expect("entry share math");
        v.aum = PodU64::from(u64::from(v.aum) + deposit);
        v.accrue_fees(shares, now);
        assert!(
            u64::from(v.perf_fee_accrued) <= deposit / 10_000,
            "re-entry principal charged beyond virtual-offset dust"
        );

        // ...and a genuine rise on top of it still is.
        v.aum = PodU64::from(u64::from(v.aum) * 2);
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
            v.performance_fee_bps = PodU16::ZERO;
            v.perf_fee_accrued = PodU64::from(DEBT);
            v
        };

        // Deposit first: the historical debt remains inside the payable cap.
        let mut deposit_first = setup();
        assert_eq!(deposit_first.raw_fee_liability(SUPPLY, 0), DEBT);
        assert_eq!(deposit_first.fee_payable_cap(), AUM / 2);
        let entrant_first =
            shares_for_deposit(DEPOSIT, SUPPLY, deposit_first.eff_aum(SUPPLY, 0)).unwrap();
        deposit_first.accrue_fees(SUPPLY, 0);
        deposit_first.aum = PodU64::from(AUM + DEPOSIT);
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
        crystal_first.aum = PodU64::from(AUM + DEPOSIT);
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
        v.management_fee_bps = PodU16::from(25u16);

        let mut ground = 0u64;
        for t in 1..=600i64 {
            v.accrue_fees(SUPPLY, t);
            ground += v.take_fees().total();
        }

        // The honest baseline: leave it alone and collect once at the end.
        let mut honest = vault(BASE);
        honest.management_fee_bps = PodU16::from(25u16);
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
        v.management_fee_bps = PodU16::from(75u16);
        v.performance_fee_bps = PodU16::from(750u16);
        v.fee_capital_time = PodU128::from(u128::from(AUM) * SECONDS_PER_YEAR);
        // The entire booked claim, including its last unit, goes to the curator.
        v.perf_fee_accrued = PodU64::from(100_000_001u64);

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
        assert!(v.fee_capital_time.is_zero());
        assert!(v.perf_fee_accrued.is_zero());
    }

    #[test]
    fn fee_rates_can_be_zero_independently() {
        for (management, performance) in [(0u16, 0u16), (0, 1_000), (100, 0), (100, 1_000)] {
            let mut v = vault(2 * BASE);
            v.management_fee_bps = PodU16::from(management);
            v.performance_fee_bps = PodU16::from(performance);
            let now = SECONDS_PER_YEAR as i64;
            v.accrue_fees(BASE, now);
            let liability = v.raw_fee_liability(BASE, now);
            let fees = v.take_fees();
            assert_eq!(fees.curator_management > 0, management > 0);
            assert_eq!(fees.curator_performance > 0, performance > 0);
            assert_eq!(fees.protocol_total(), 0);
            assert_eq!(fees.total(), liability);
            assert!(v.fee_capital_time.is_zero());
            assert!(v.perf_fee_accrued.is_zero());
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
        v.management_fee_bps = PodU16::from(1_000u16); // 10% + protocol management

        // Five years uncrystallized, then a 95% drawdown: the previously
        // folded liability is much larger than the live book.
        v.accrue_fees(SUPPLY, now);
        v.aum = PodU64::from(BASE / 20);
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
        assert!(v.perf_fee_accrued.is_zero());
        assert_eq!(v.raw_fee_liability(SUPPLY, now), 0);
    }

    #[test]
    fn reversed_performance_gain_is_written_down_to_the_live_cap() {
        const SUPPLY: u64 = BASE;
        let mut v = vault(BASE);
        v.performance_fee_bps = PodU16::from(5_000u16);
        v.accrue_fees(SUPPLY, 0);

        // A 3x mark books roughly one AUM of performance fees at 50%.
        v.aum = PodU64::from(3 * BASE);
        v.accrue_fees(SUPPLY, 0);
        let peak_fee = u64::from(v.perf_fee_accrued);
        assert!(peak_fee > BASE / 2, "setup: peak fee {peak_fee}");

        // The gain reverses before crystallization. The old claim cannot stay
        // senior to the whole recovered principal.
        v.aum = PodU64::from(BASE);
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
    fn fee_write_down_keeps_streaming_senior_to_performance() {
        const DENOM: u128 = 10_000 * SECONDS_PER_YEAR;
        const STREAMING: u64 = 400 * UNIT;
        const PERF: u64 = 400 * UNIT;
        const SUPPLY: u64 = BASE;

        let mut v = vault(BASE);
        v.management_fee_bps = PodU16::from(25u16);
        v.performance_fee_bps = PodU16::ZERO;
        v.fee_capital_time = PodU128::from(u128::from(STREAMING) * DENOM / v.streaming_bps());
        v.perf_fee_accrued = PodU64::from(PERF);
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
        v.perf_fee_accrued = PodU64::from(3 * BASE);

        v.write_down_unpayable_fees(SUPPLY, 0);

        let post_write_down_nav = v.nav_for_watermark(SUPPLY, 0);
        assert!(post_write_down_nav > prior_hwm);
        assert_eq!(u128::from(v.hwm_nav), post_write_down_nav);
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

    // M-04: usage is stored as an absolute amount while capacity follows AUM.
    // A shrink must not leave the bucket permanently over-full.
    #[test]
    fn slippage_bucket_recovers_after_aum_shrinks() {
        let mut v = vault(BASE);
        let budget = ((BASE as u128) * MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS / 10_000) as u64;
        v.charge_swap_slippage(budget, 0).unwrap();

        // AUM drops to 40%: the resized bucket is fully spent, not over-full.
        v.aum = PodU64::from(BASE * 2 / 5);
        let small = ((BASE as u128 * 2 / 5) * MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS / 10_000) as u64;
        v.charge_swap_slippage(0, 0)
            .expect("a zero-slippage swap must pass right after the shrink");
        assert!(v.charge_swap_slippage(1, 0).is_err());
        v.charge_swap_slippage(small / 2, SWAP_SLIPPAGE_WINDOW_SECONDS / 2)
            .expect("capacity refills against the resized bucket");
        v.charge_swap_slippage(
            small,
            SWAP_SLIPPAGE_WINDOW_SECONDS / 2 + SWAP_SLIPPAGE_WINDOW_SECONDS,
        )
        .expect("a full hour restores the whole resized bucket");
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

            w.hwm_nav = PodU128::from(entry);
            w.accrue_fees(shares, 0);
            assert!(
                u64::from(w.perf_fee_accrued) <= deposit / 10_000,
                "principal charged as gain at deposit {deposit}"
            );
        }
    }
}
