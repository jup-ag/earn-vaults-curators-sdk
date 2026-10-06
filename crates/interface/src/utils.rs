#[macro_export]
macro_rules! check_account_layout {
    ($acc:ident) => {
        const _: () = assert!(core::mem::align_of::<$acc>() == 1);
        const _: () = assert!(1 + core::mem::size_of::<$acc>() == $acc::LEN);
    };
}

#[cfg(any(target_os = "solana", target_arch = "bpf"))]
use solana_address::{PDA_MARKER, address_eq};
use {solana_address::Address, solana_program_error::ProgramError};

/// Whether a Token-2022 account carries the `ImmutableOwner` extension.
///
/// Token accounts store their account-type byte after the 165-byte base
/// layout, followed by TLV entries (`type: u16`, `len: u16`, value). Extension
/// type 7 is `ImmutableOwner` and has an empty value.
pub fn token_account_has_immutable_owner(data: &[u8]) -> bool {
    const ACCOUNT_BASE_LEN: usize = 165;
    const ACCOUNT_TYPE_ACCOUNT: u8 = 2;
    const IMMUTABLE_OWNER: u16 = 7;

    if data.get(ACCOUNT_BASE_LEN).copied() != Some(ACCOUNT_TYPE_ACCOUNT) {
        return false;
    }

    let mut i = ACCOUNT_BASE_LEN + 1;
    while let Some(header) = data.get(i..i + 4) {
        let extension = u16::from_le_bytes([header[0], header[1]]);
        let len = u16::from_le_bytes([header[2], header[3]]) as usize;
        if extension == 0 {
            return false;
        }
        if extension == IMMUTABLE_OWNER {
            return len == 0;
        }
        i += 4 + len;
    }
    false
}

/// Compile-time PDA derivation (pure-Rust sha256 via `const_crypto`) — used
/// for constants like `EVENT_PDA`. Too expensive for on-chain runtime use;
/// the syscall forms below serve that.
pub const fn find_program_address_const(
    seeds: &[&[u8]],
    program_id: &solana_address::Address,
) -> (solana_address::Address, u8) {
    let (bytes, bump) = const_crypto::ed25519::derive_program_address(seeds, program_id.as_array());
    (solana_address::Address::new_from_array(bytes), bump)
}

// ---------------------------------------------------------------------------
// Runtime, syscall-level PDA derivation — the single home of the hand-rolled
// sha256/curve-validate machinery, shared by the program (`utils::pda`
// re-exports it) and the hook. Hashing `[seeds..., bump, program_id,
// PDA_MARKER]` with `sol_sha256` + `sol_curve_validate_point` costs ~544 CU on
// the typical first-try (bump 255) hit vs ~1,500 CU for the
// `find_program_address` syscall.
//
// SBF ABI note (relied on by every syscall below): `&[u8]` has layout
// `(*const u8, u64)`, identical to `SolBytes`, so a slice-of-slices passes
// straight to `sol_sha256` without intermediate copies.
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "solana", target_arch = "bpf"))]
unsafe extern "C" {
    fn sol_sha256(vals: *const u8, val_len: u64, hash_result: *mut u8) -> u64;
    fn sol_curve_validate_point(curve_id: u64, point_addr: *const u8, result: *mut u8) -> u64;
}

/// Max slice entries we pass to `sol_sha256`: 16 seeds + bump + program_id +
/// PDA_MARKER on the find path, or 17 bump-inclusive seeds + program_id +
/// PDA_MARKER on the verify path.
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
const MAX_PDA_SLICES: usize = 19;

/// Bump-search core shared by [`try_find_program_address`] and
/// [`verify_canonical_program_address`]: walk `bump` from 255 down to 0 and
/// return the first digest that is off-curve (a valid PDA), with its bump.
///
/// Caller must have checked `seeds.len() <= 16`.
#[cfg(any(target_os = "solana", target_arch = "bpf"))]
#[inline(always)]
fn find_program_address_impl(
    seeds: &[&[u8]],
    program_id: &Address,
) -> Result<([u8; 32], u8), ProgramError> {
    const CURVE25519_EDWARDS: u64 = 0;
    let n = seeds.len();

    // Build the input array: [seeds..., bump, program_id, PDA_MARKER].
    // Max 16 seeds + bump + program_id + marker = 19 entries.
    let mut slices = core::mem::MaybeUninit::<[&[u8]; MAX_PDA_SLICES]>::uninit();
    let sptr = slices.as_mut_ptr() as *mut &[u8];

    let mut i = 0;
    while i < n {
        // SAFETY: `i < n <= 16` so `sptr.add(i)` is within the 19-slot array.
        unsafe { sptr.add(i).write(seeds[i]) };
        i += 1;
    }
    // SAFETY: Slots `n+1` and `n+2` are within bounds (n <= 16, array has 19).
    unsafe {
        sptr.add(n + 1).write(program_id.as_ref());
        sptr.add(n + 2).write(PDA_MARKER.as_slice());
    }

    // The bump slot points into `bump_arr`; only the byte changes per iteration.
    let mut bump_arr = [u8::MAX];
    let bump_ptr = bump_arr.as_mut_ptr();
    // SAFETY: Slot `n` is within bounds. The stored `&[u8]` aliases `bump_arr`
    // but is never read through Rust code; it is only consumed by `sol_sha256`
    // as a raw `(*const u8, u64)` pair (SolBytes), so the mutation through
    // `bump_ptr` below is invisible to any Rust reference.
    unsafe { sptr.add(n).write(core::slice::from_raw_parts(bump_ptr, 1)) };

    // SAFETY: All `n + 3` elements initialized above.
    let input = unsafe { core::slice::from_raw_parts(sptr, n + 3) };
    let mut hash = core::mem::MaybeUninit::<[u8; 32]>::uninit();

    // u64 counter avoids per-iteration zero-extension on SBF.
    let mut bump: u64 = u8::MAX as u64;
    loop {
        // SAFETY: `bump_ptr` points to `bump_arr[0]`. Writing a u8 is always valid.
        unsafe { bump_ptr.write(bump as u8) };

        // SAFETY: See the module-level SBF ABI note.
        unsafe {
            sol_sha256(
                input as *const _ as *const u8,
                input.len() as u64,
                hash.as_mut_ptr() as *mut u8,
            );
        }

        // SAFETY: `hash` was written by `sol_sha256`. Returns 0 if on curve,
        // non-zero if off curve (a valid PDA).
        let on_curve = unsafe {
            sol_curve_validate_point(
                CURVE25519_EDWARDS,
                hash.as_ptr() as *const u8,
                core::ptr::null_mut(),
            )
        };
        if on_curve != 0 {
            // SAFETY: `hash` fully initialized by `sol_sha256` above.
            return Ok((unsafe { hash.assume_init() }, bump as u8));
        }

        if bump == 0 {
            return Err(ProgramError::InvalidSeeds);
        }
        bump -= 1;
    }
}

/// Find a valid program derived address and its bump seed.
#[inline]
pub fn try_find_program_address(
    seeds: &[&[u8]],
    program_id: &Address,
) -> Result<(Address, u8), ProgramError> {
    if seeds.len() > 16 {
        return Err(ProgramError::InvalidSeeds);
    }

    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    {
        let (hash, bump) = find_program_address_impl(seeds, program_id)?;
        Ok((Address::new_from_array(hash), bump))
    }

    #[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
    {
        let _ = (seeds, program_id);
        Err(ProgramError::InvalidArgument)
    }
}

/// Verify `expected` is the canonical PDA for `seeds` and return its bump.
///
/// The verification form of [`try_find_program_address`] — avoids
/// materializing the derived `Address` only to compare it at the callsite.
#[inline]
pub fn verify_canonical_program_address(
    seeds: &[&[u8]],
    program_id: &Address,
    expected: &Address,
) -> Result<u8, ProgramError> {
    if seeds.len() > 16 {
        return Err(ProgramError::InvalidSeeds);
    }

    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    {
        let (hash, bump) = find_program_address_impl(seeds, program_id)?;
        // SAFETY: `Address` is `[u8; 32]`.
        if !address_eq(unsafe { &*(hash.as_ptr() as *const Address) }, expected) {
            return Err(ProgramError::InvalidSeeds);
        }
        Ok(bump)
    }

    #[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
    {
        let _ = (seeds, program_id, expected);
        Err(ProgramError::InvalidArgument)
    }
}

/// Verify that `expected` matches `sha256(seeds || program_id ||
/// "ProgramDerivedAddress")` — one hash, no bump search.
///
/// The seeds slice must already include the bump byte.
#[inline]
pub fn verify_program_address(
    seeds: &[&[u8]],
    program_id: &Address,
    expected: &Address,
) -> Result<(), ProgramError> {
    // seeds includes bump: max 17, so [seeds..., program_id, PDA_MARKER] fits
    // the 19-slot array.
    if seeds.len() > 17 {
        return Err(ProgramError::InvalidSeeds);
    }

    #[cfg(any(target_os = "solana", target_arch = "bpf"))]
    {
        let n = seeds.len();

        let mut slices = core::mem::MaybeUninit::<[&[u8]; MAX_PDA_SLICES]>::uninit();
        let sptr = slices.as_mut_ptr() as *mut &[u8];

        let mut i = 0;
        while i < n {
            // SAFETY: `i < n <= 17` so `sptr.add(i)` is within the 19-slot array.
            unsafe { sptr.add(i).write(seeds[i]) };
            i += 1;
        }
        // SAFETY: Slots `n` and `n+1` are within bounds (n <= 17, array has 19).
        unsafe {
            sptr.add(n).write(program_id.as_ref());
            sptr.add(n + 1).write(PDA_MARKER.as_slice());
        }

        // SAFETY: All `n + 2` elements initialized above.
        let input = unsafe { core::slice::from_raw_parts(sptr, n + 2) };
        let mut hash = core::mem::MaybeUninit::<[u8; 32]>::uninit();

        // SAFETY: See the module-level SBF ABI note.
        unsafe {
            sol_sha256(
                input as *const _ as *const u8,
                input.len() as u64,
                hash.as_mut_ptr() as *mut u8,
            );
        }

        // SAFETY: `hash` is fully initialized by `sol_sha256`, and `Address`
        // is `[u8; 32]`.
        if address_eq(unsafe { &*(hash.as_ptr() as *const Address) }, expected) {
            Ok(())
        } else {
            Err(ProgramError::InvalidSeeds)
        }
    }

    #[cfg(not(any(target_os = "solana", target_arch = "bpf")))]
    {
        let _ = (seeds, program_id, expected);
        Err(ProgramError::InvalidArgument)
    }
}
