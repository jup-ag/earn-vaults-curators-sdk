use {
    codama_macros::CodamaType,
    solana_address::{Address, address_eq},
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI64, PodU64},
};

/// Maximum distinct seized-collateral mints held by one vault.
pub const MAX_SINKS: usize = 4;

/// Set when the sink has a current attested mark.
pub const SINK_FLAG_PRICED: u8 = 1 << 0;

/// Seized collateral in custody, keyed by mint and awaiting liquidation.
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct SinkSlot {
    /// Seized mint; all-zero means the slot is free.
    pub mint: Address,
    /// Numéraire mark for the full custody balance of [`Self::mint`].
    #[codama(type = number(u64))]
    pub value: PodU64,
    /// Last attestation time for [`Self::value`].
    #[codama(type = number(i64))]
    pub last_priced_at: PodI64,
    /// [`SINK_FLAG_PRICED`].
    pub flags: u8,
}

impl SinkSlot {
    #[inline]
    pub const fn zero() -> Self {
        Self {
            mint: Address::new_from_array([0; 32]),
            value: PodU64::ZERO,
            last_priced_at: PodI64::ZERO,
            flags: 0,
        }
    }

    /// Whether the current mark is attested.
    #[inline]
    pub const fn priced(&self) -> bool {
        self.flags & SINK_FLAG_PRICED != 0
    }

    #[inline]
    pub fn set_priced(&mut self) {
        self.flags |= SINK_FLAG_PRICED;
    }

    /// Drop the attestation without touching the mark.
    #[inline]
    pub fn dirty(&mut self) {
        self.flags &= !SINK_FLAG_PRICED;
    }

    /// Whether the slot is available.
    #[inline]
    pub fn is_free(&self) -> bool {
        address_eq(&self.mint, &Address::new_from_array([0; 32]))
    }

    /// Whether this occupied slot carries `mint`.
    #[inline]
    pub fn holds(&self, mint: &Address) -> bool {
        !self.is_free() && address_eq(&self.mint, mint)
    }

    /// Reset the slot to free. Only correct once the balance it stood for has
    /// left custody — the mint binding *is* the vault's handle on that balance,
    /// so dropping it while tokens remain strands them (see [`Self::write_off`]).
    #[inline]
    pub fn release(&mut self) {
        *self = Self::zero();
    }

    /// Write the mark down to nothing while keeping the slot bound to its mint.
    ///
    /// The curator's escape hatch: collateral nobody will buy, or a mark the
    /// drift guard has anchored wrong. The value leaves AUM, but the *balance*
    /// is still in custody, so the binding has to survive — it is what the
    /// liquidation arm resolves `src_mint` through, and what `price_opaque`
    /// needs to re-mark if the write-off turns out to have been premature.
    ///
    /// Attested at 0 rather than dirtied, so `validate_aum` credits nothing
    /// instead of failing closed on an unpriced occupant. A subsequent sale
    /// takes `new_value` to 0 and frees the slot through [`Self::release`].
    #[inline]
    pub fn write_off(&mut self, now: i64) {
        self.value = PodU64::ZERO;
        self.last_priced_at = PodI64::from(now);
        self.set_priced();
    }
}
