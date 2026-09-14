use {
    crate::state::Protocol,
    codama_macros::CodamaType,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::pod::{PodI64, PodU16, PodU64},
};

pub const POSITION_FLAG_REQUIRED_IN_AUM: u8 = 1 << 0;
pub const POSITION_FLAG_DEBT: u8 = 1 << 2;
pub const POSITION_FLAG_ENABLED: u8 = 1 << 3;
pub const POSITION_FLAG_PRICED: u8 = 1 << 4;
pub const POSITION_FLAG_INITIALIZED: u8 = 1 << 5;
pub const POSITION_FLAG_LENDER_ONLY: u8 = 1 << 6;
pub const POSITION_FLAG_CURATOR_MASK: u8 = POSITION_FLAG_REQUIRED_IN_AUM
    | POSITION_FLAG_DEBT
    | POSITION_FLAG_ENABLED
    | POSITION_FLAG_LENDER_ONLY;

/// One opaque position. Slots in `Vault.positions` are **free** — a position
/// names its own protocol rather than being addressed by it, so a vault spends
/// slots only on the integrations it uses. Keying the array by the [`Protocol`]
/// discriminant instead would force it as wide as the enum and make every new
/// integration a `Vault::LEN` change; that trade is backwards, because
/// over-allocating costs rent while under-allocating costs a migration.
///
/// At most one *enabled* position per protocol — enforced at
/// `initialize_vault`, since every settlement fold and dirty resolves a
/// protocol to a slot by scanning ([`Vault::find_position_mut`]).
///
/// [`Vault::find_position_mut`]: crate::state::Vault::find_position_mut
#[derive(SchemaRead, SchemaWrite, Clone, Copy, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct PositionConfig {
    /// Protocol this position belongs to.
    pub protocol: u8,
    /// Bitmask of `POSITION_FLAG_*` values. `0` = unconfigured slot.
    pub flags: u8,
    /// Optional per-position staleness override. `0` means global default.
    #[codama(type = number(u64))]
    pub max_staleness_seconds: PodU64,
    /// Optional per-position drift override. `0` means global default.
    #[codama(type = number(u16))]
    pub max_drift_bps: PodU16,
    /// Optional haircut for stale/defaulted/illiquid marks. `0` means none.
    #[codama(type = number(u16))]
    pub haircut_bps: PodU16,
    #[codama(type = number(u64))]
    pub last_value: PodU64,
    #[codama(type = number(i64))]
    pub last_priced_at: PodI64,
}

impl PositionConfig {
    #[inline]
    pub const fn zero() -> Self {
        Self {
            protocol: 0,
            flags: 0,
            max_staleness_seconds: PodU64::ZERO,
            max_drift_bps: PodU16::ZERO,
            haircut_bps: PodU16::ZERO,
            last_value: PodU64::ZERO,
            last_priced_at: PodI64::ZERO,
        }
    }

    #[inline]
    pub const fn enabled(&self) -> bool {
        self.flags & POSITION_FLAG_ENABLED != 0
    }

    #[inline]
    pub const fn required_in_aum(&self) -> bool {
        self.flags & POSITION_FLAG_REQUIRED_IN_AUM != 0
    }

    #[inline]
    pub const fn debt(&self) -> bool {
        self.flags & POSITION_FLAG_DEBT != 0
    }

    /// See [`POSITION_FLAG_LENDER_ONLY`] — the protocol's borrower bits can
    /// never be granted on this vault.
    #[inline]
    pub const fn lender_only(&self) -> bool {
        self.flags & POSITION_FLAG_LENDER_ONLY != 0
    }

    /// Whether a real mark has been posted (and not since invalidated). See
    /// [`POSITION_FLAG_PRICED`].
    #[inline]
    pub const fn priced(&self) -> bool {
        self.flags & POSITION_FLAG_PRICED != 0
    }

    /// Whether this slot has ever been priced *or* touched by a protocol CPI.
    /// See [`POSITION_FLAG_INITIALIZED`].
    #[inline]
    pub const fn initialized(&self) -> bool {
        self.flags & POSITION_FLAG_INITIALIZED != 0
    }

    /// Reject unsupported flag combinations for `protocol`.
    #[inline]
    pub fn validate_flags(flags: u8, protocol: Protocol) -> Result<(), ProgramError> {
        if flags & POSITION_FLAG_REQUIRED_IN_AUM != 0 && flags & POSITION_FLAG_ENABLED == 0 {
            return Err(ProgramError::InvalidArgument);
        }
        // Debt must count in AUM and cannot use a protocol that folds marks.
        if flags & POSITION_FLAG_DEBT != 0
            && (flags & POSITION_FLAG_REQUIRED_IN_AUM == 0 || protocol.folds_marks())
        {
            return Err(ProgramError::InvalidArgument);
        }
        if flags & POSITION_FLAG_LENDER_ONLY != 0
            && (protocol != Protocol::Offerbook || flags & POSITION_FLAG_ENABLED == 0)
        {
            return Err(ProgramError::InvalidArgument);
        }
        Ok(())
    }
}
