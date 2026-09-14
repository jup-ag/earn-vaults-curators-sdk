mod delegate;
mod escrow;
mod policy;
mod vault;

use {
    crate::ID,
    solana_account_view::AccountView,
    solana_address::address_eq,
    solana_program_error::ProgramError,
    wincode::{SchemaRead, SchemaWrite, ZeroCopy, config::DefaultConfig},
};
pub use {delegate::*, escrow::*, policy::*, vault::*};

pub trait Account:
    ZeroCopy
    + for<'de> SchemaRead<'de, DefaultConfig, Dst = Self>
    + SchemaWrite<DefaultConfig, Src = Self>
    + Sized
{
    const LEN: usize;
    const DISCRIMINATOR: u8;

    /// Return `Self` from the given account, after verifying the program owns
    /// it and its data starts with [`Self::DISCRIMINATOR`] (length is checked
    /// by [`Account::read`]).
    ///
    /// # Safety
    ///
    /// The account data is borrowed without runtime borrow tracking
    /// (`borrow_unchecked`): the caller must ensure no mutable borrow of this
    /// account's data — including through a duplicate `AccountView` of the
    /// same account — exists while the returned reference is alive.
    #[inline(always)]
    unsafe fn from_account_unchecked(account: &AccountView) -> Result<&Self, ProgramError> {
        if !address_eq(account.owner(), &ID) {
            return Err(ProgramError::InvalidAccountOwner);
        }

        let bytes = unsafe { account.borrow_unchecked() };
        <Self as Account>::read(bytes)
    }

    /// Return `&mut Self` from the given account, after verifying the program
    /// owns it and its data starts with [`Self::DISCRIMINATOR`] (length is
    /// checked by [`Account::read_mut`]).
    ///
    /// # Safety
    ///
    /// The account data is borrowed mutably without runtime borrow tracking
    /// (`borrow_unchecked_mut`): the caller must ensure no other borrow of this
    /// account's data — shared or mutable, including through a duplicate
    /// `AccountView` of the same account — exists while the returned reference
    /// is alive.
    #[inline(always)]
    unsafe fn from_account_unchecked_mut(
        account: &mut AccountView,
    ) -> Result<&mut Self, ProgramError> {
        if !address_eq(account.owner(), &ID) {
            return Err(ProgramError::InvalidAccountOwner);
        }

        let bytes = unsafe { account.borrow_unchecked_mut() };
        <Self as Account>::read_mut(bytes)
    }

    #[inline(always)]
    fn read_mut(data: &mut [u8]) -> Result<&mut Self, ProgramError> {
        let (dis, data) = data
            .split_first_mut()
            .ok_or(ProgramError::InvalidAccountData)?;

        if *dis != Self::DISCRIMINATOR {
            return Err(ProgramError::InvalidAccountData);
        }

        <Self as ZeroCopy>::from_bytes_mut(data).map_err(|_| ProgramError::InvalidAccountData)
    }

    #[inline(always)]
    fn read(data: &[u8]) -> Result<&Self, ProgramError> {
        let (dis, data) = data.split_first().ok_or(ProgramError::InvalidAccountData)?;

        if *dis != Self::DISCRIMINATOR {
            return Err(ProgramError::InvalidAccountData);
        }

        <Self as ZeroCopy>::from_bytes(data).map_err(|_| ProgramError::InvalidAccountData)
    }
}

pub const MAX_ALLOWED_ASSETS: usize = 8;
pub const MAX_POSITIONS: u8 = 8;

pub const REQUEST_TYPE_SUBSCRIBE: u8 = 0;
pub const REQUEST_TYPE_REDEEM: u8 = 1;

/// Which integration a position tracks. `Vault.positions` is indexed by this
/// discriminant, so variants are append-only: reordering remaps on-chain rows.
#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, codama_macros::CodamaType)]
pub enum Protocol {
    /// Vault-held tokens priced opaquely / by oracle (no owning program).
    SpotTokens,
    /// jup-ag Offerbook P2P lending book.
    Offerbook,
    /// Jupiter Lend (Fluid) lend/borrow loop — vaults + liquidity programs.
    JupiterLend,
}

impl Protocol {
    #[inline]
    pub const fn bit(self) -> u8 {
        1 << (self as u8)
    }

    /// Parse a raw discriminant; `None` if unknown. Keep in sync with the
    /// variants above.
    #[inline]
    pub const fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::SpotTokens),
            1 => Some(Self::Offerbook),
            2 => Some(Self::JupiterLend),
            _ => None,
        }
    }

    /// Whether a settlement auto-folds priced custody deltas into this
    /// protocol's mark (out-flow ⇒ mark up), which makes the mark asset-side
    /// and so incompatible with a [`POSITION_FLAG_DEBT`] slot — `validate_aum`
    /// subtracts that one, moving AUM by `2Δ` the wrong way. Exhaustive: a new
    /// variant must say whether it folds.
    #[inline]
    pub const fn folds_marks(self) -> bool {
        match self {
            // Never a fold target — `price_opaque` is the only writer.
            Self::SpotTokens => false,
            Self::Offerbook | Self::JupiterLend => true,
        }
    }
}
