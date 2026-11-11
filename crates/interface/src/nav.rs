//! Pure NAV math — share ⇄ asset conversion with the ERC-4626 virtual offset.
//! One formula for every deposit including the first: the
//! `(VIRTUAL_SHARES, VIRTUAL_ASSETS)` offsets fix a 1:1 bootstrap rate and make
//! a donation attack cost ~`VS`× the target.

use crate::{
    constants::{MAX_ENTRY_PREMIUM_BPS, NAV_SCALE, VIRTUAL_ASSETS, VIRTUAL_SHARES},
    errors::VaultError,
};

/// `floor(a · b / c)` in u128, narrowed back to u64.
#[inline(always)]
fn mul_div(a: u64, b: u128, c: u128) -> Result<u64, VaultError> {
    let numerator = (a as u128).checked_mul(b).ok_or(VaultError::AumOverflow)?;
    u64::try_from(numerator / c).map_err(|_| VaultError::AumOverflow)
}

/// Price a deposit at `aum + headroom`, refusing it when paying that reserve
/// would lift the share price by more than [`MAX_ENTRY_PREMIUM_BPS`]. The
/// entrant gives up `deposit · headroom / (aum + headroom + deposit)` to the
/// existing holders, so NAV rises by that premium over `aum`. A small enough
/// deposit always prices, whatever reserve the book carries.
#[inline]
pub fn deposit_pricing_aum(aum: u64, headroom: u64, deposit: u64) -> Result<u64, VaultError> {
    let entry_aum = aum.checked_add(headroom).ok_or(VaultError::AumOverflow)?;
    let premium = mul_div(
        deposit,
        u128::from(headroom),
        (u128::from(entry_aum) + u128::from(deposit)).max(1),
    )?;
    // Reject rather than clamp: the full reserve protects against stale-price arbitrage.
    if u128::from(premium) * 10_000 > u128::from(aum) * u128::from(MAX_ENTRY_PREMIUM_BPS) {
        return Err(VaultError::EntryPremiumExceeded);
    }
    Ok(entry_aum)
}

/// `shares = deposit · (supply + VS) / (aum + VA)`.
#[inline]
pub fn shares_for_deposit(deposit: u64, supply: u64, aum: u64) -> Result<u64, VaultError> {
    mul_div(
        deposit,
        (supply as u128).saturating_add(VIRTUAL_SHARES),
        (aum as u128).saturating_add(VIRTUAL_ASSETS),
    )
}

/// `assets = shares · (aum + VA) / (supply + VS)`. Inverse of
/// [`shares_for_deposit`], floored the same way, so a round trip never profits.
#[inline]
pub fn assets_for_redeem(shares: u64, supply: u64, aum: u64) -> Result<u64, VaultError> {
    mul_div(
        shares,
        (aum as u128).saturating_add(VIRTUAL_ASSETS),
        (supply as u128).saturating_add(VIRTUAL_SHARES),
    )
}

/// Numéraire owed to a queued redeem. `pending_shares` goes back into `supply`
/// so concurrent redeems all price against the pre-burn supply. A fill debits
/// `aum` by what it pays, in the same instruction that prices it, so the next
/// redeem already sees the reduced book — no separate owed-out term.
#[inline]
pub fn redeem_owed(
    shares: u64,
    supply: u64,
    pending_shares: u64,
    aum: u64,
) -> Result<u64, VaultError> {
    // Checked, not saturating: a clamp would drop the queued redeems out of
    // the denominator and overpay this one. `require_mint_headroom` keeps this
    // unreachable.
    let eff_supply = supply
        .checked_add(pending_shares)
        .ok_or(VaultError::AumOverflow)?;
    assets_for_redeem(shares, eff_supply, aum)
}

/// Per-share NAV, `(aum + VA) · NAV_SCALE / (supply + VS)` — the same
/// virtual-offset terms as the two conversions, so all three stay consistent.
#[inline]
pub fn nav_per_share(supply: u64, aum: u64) -> Result<u128, VaultError> {
    let numerator = (aum as u128)
        .saturating_add(VIRTUAL_ASSETS)
        .checked_mul(NAV_SCALE)
        .ok_or(VaultError::AumOverflow)?;
    Ok(numerator / (supply as u128).saturating_add(VIRTUAL_SHARES))
}

// Kani proofs — `just kani`. Compiled only under `--cfg kani`.
#[cfg(kani)]
mod proofs {
    use super::*;

    /// Total for any inputs: overflow is checked and `supply + VS` is nonzero.
    #[kani::proof]
    fn shares_for_deposit_never_panics() {
        let _ = shares_for_deposit(kani::any(), kani::any(), kani::any());
    }

    #[kani::proof]
    fn assets_for_redeem_never_panics() {
        let _ = assets_for_redeem(kani::any(), kani::any(), kani::any());
    }

    #[kani::proof]
    fn redeem_owed_never_panics() {
        let _ = redeem_owed(kani::any(), kani::any(), kani::any(), kani::any());
    }

    #[kani::proof]
    fn nav_per_share_never_panics() {
        let _ = nav_per_share(kani::any(), kani::any());
    }

    /// Mint-then-redeem never pays out more than was deposited: the double
    /// floor plus the offset always rounds in the vault's favor.
    #[kani::proof]
    fn round_trip_never_profits() {
        let deposit: u64 = kani::any();
        let supply: u64 = kani::any();
        let aum: u64 = kani::any();
        if let Ok(shares) = shares_for_deposit(deposit, supply, aum) {
            // Skip states the vault's own u64 accounting couldn't hold.
            if let (Some(s2), Some(a2)) = (supply.checked_add(shares), aum.checked_add(deposit)) {
                if let Ok(assets_back) = assets_for_redeem(shares, s2, a2) {
                    assert!(assets_back <= deposit);
                }
            }
        }
    }

    /// A deposit never dilutes existing holders below their pro-rata value.
    #[kani::proof]
    fn deposit_never_dilutes_existing_holders() {
        let deposit: u64 = kani::any();
        let supply: u64 = kani::any();
        let aum: u64 = kani::any();
        if let Ok(shares) = shares_for_deposit(deposit, supply, aum) {
            if let (Some(s2), Some(a2)) = (supply.checked_add(shares), aum.checked_add(deposit)) {
                // What the pre-existing supply can redeem: after ≥ before.
                if let (Ok(before), Ok(after)) = (
                    assets_for_redeem(supply, supply, aum),
                    assets_for_redeem(supply, s2, a2),
                ) {
                    assert!(after >= before.saturating_sub(1)); // ≤1 unit rounding dust
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deposit_premium_boundary_and_extremes() {
        const AUM: u64 = 1_000_000;
        let exceeded = Err(VaultError::EntryPremiumExceeded);
        // A reserve within the limit prices any size.
        assert_eq!(deposit_pricing_aum(AUM, 10_000, u64::MAX), Ok(1_010_000));
        // Just above it, a book-sized deposit still prices and a huge one is refused.
        assert_eq!(deposit_pricing_aum(AUM, 10_100, AUM), Ok(1_010_100));
        assert_eq!(deposit_pricing_aum(AUM, 10_100, 1 << 40), exceeded);
        // A reserve of half the book never closes entry; it shrinks what fits.
        assert_eq!(deposit_pricing_aum(AUM, AUM / 2, 20_000), Ok(1_500_000));
        assert_eq!(deposit_pricing_aum(AUM, AUM / 2, 40_000), exceeded);
        assert_eq!(
            deposit_pricing_aum(AUM, u64::MAX / 2, 1),
            Ok(AUM + u64::MAX / 2)
        );
        assert_eq!(deposit_pricing_aum(0, 0, 0), Ok(0));
        assert_eq!(
            deposit_pricing_aum(u64::MAX, 1, 0),
            Err(VaultError::AumOverflow)
        );
    }

    /// No admitted deposit lifts the share price past the limit, whatever the
    /// reserve, book or deposit size.
    #[test]
    fn admitted_deposits_bound_the_share_price_move() {
        let mut x = 0x9E3779B97F4A7C15u64;
        let mut rng = move |bits: u32| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x >> (64 - 1 - x % u64::from(bits)) as u32
        };
        let mut admitted = 0;
        for _ in 0..200_000 {
            let aum = 1_000_000 + rng(40);
            let supply = 1_000_000 + rng(40);
            let (headroom, deposit) = (rng(44), rng(48));
            let Ok(entry_aum) = deposit_pricing_aum(aum, headroom, deposit) else {
                continue;
            };
            let Ok(shares) = shares_for_deposit(deposit, supply, entry_aum) else {
                continue;
            };
            admitted += 1;
            let before = (
                u128::from(aum) + VIRTUAL_ASSETS,
                u128::from(supply) + VIRTUAL_SHARES,
            );
            let after = (
                before.0 + u128::from(deposit),
                before.1 + u128::from(shares),
            );
            // after.0 / after.1 <= before.0 / before.1 · (1 + limit), plus 1 bp of flooring.
            assert!(
                after.0 * before.1 * 10_000
                    <= before.0 * after.1 * u128::from(10_000 + MAX_ENTRY_PREMIUM_BPS + 1),
                "aum={aum} supply={supply} headroom={headroom} deposit={deposit}"
            );
        }
        assert!(admitted > 20_000, "sweep admitted only {admitted}");
    }

    #[test]
    fn bootstrap_mint_yields_virtual_share_multiple() {
        let shares = shares_for_deposit(1_000, 0, 0).unwrap();
        assert_eq!(shares as u128, 1_000 * VIRTUAL_SHARES / VIRTUAL_ASSETS);
    }

    // Use a deterministic xorshift sweep to avoid a property-test dependency;
    // the Kani round-trip/dilution proofs do not terminate on nonlinear u128
    // division.
    #[test]
    fn round_trip_and_dilution_randomized_sweep() {
        let mut x = 0x243F6A8885A308D3u64;
        let mut rng = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let edges = [0u64, 1, 2, 999, 1_000, 1_001, u64::MAX - 1, u64::MAX];
        let check = |deposit: u64, supply: u64, aum: u64| {
            let Ok(shares) = shares_for_deposit(deposit, supply, aum) else {
                return;
            };
            let (Some(s2), Some(a2)) = (supply.checked_add(shares), aum.checked_add(deposit))
            else {
                return;
            };
            if let Ok(back) = assets_for_redeem(shares, s2, a2) {
                assert!(
                    back <= deposit,
                    "round-trip profit: d={deposit} s={supply} a={aum} -> {back}"
                );
            }
            if let (Ok(before), Ok(after)) = (
                assets_for_redeem(supply, supply, aum),
                assets_for_redeem(supply, s2, a2),
            ) {
                assert!(
                    after >= before.saturating_sub(1),
                    "dilution: d={deposit} s={supply} a={aum} {before}->{after}"
                );
            }
        };
        for &d in &edges {
            for &s in &edges {
                for &a in &edges {
                    check(d, s, a);
                }
            }
        }
        for _ in 0..2_000_000 {
            check(rng(), rng(), rng());
            check(rng() % 1_000_000, rng() % 1_000_000, rng() % 1_000_000);
        }
    }

    /// Clamping `supply + pending_shares` would pay this redeemer as if they
    /// were the only claim on the book. Fail closed instead.
    #[test]
    fn redeem_owed_rejects_pending_hidden_by_supply_saturation() {
        let supply = u64::MAX - 1_000;
        let hidden = redeem_owed(1_000, supply, 2_000, u64::MAX);
        assert_eq!(hidden, Err(VaultError::AumOverflow));
        // One share short still prices, and pays less than the clamp would.
        let ok = redeem_owed(1_000, supply, 1_000, u64::MAX).unwrap();
        let clamped = assets_for_redeem(1_000, u64::MAX, u64::MAX).unwrap();
        assert!(ok <= clamped);
    }

    #[test]
    fn steady_state_mint_and_redeem_round_trip() {
        let supply = 10_000_000_000u64;
        let aum = 1_000u64;
        let shares = shares_for_deposit(100, supply, aum).unwrap();
        let assets_back = assets_for_redeem(shares, supply + shares, aum + 100).unwrap();
        assert!(assets_back <= 100);
    }

    #[test]
    fn first_depositor_inflation_attack_is_mitigated() {
        let attacker_shares = shares_for_deposit(1, 0, 0).unwrap();
        assert!(attacker_shares > 0);
        let donated_aum = 1_000_000_000u64;
        let supply = attacker_shares;
        // Rounding the victim to zero costs ~VS× their deposit.
        let victim_deposit = (donated_aum as u128 / VIRTUAL_SHARES) as u64 + 1;
        let victim_shares = shares_for_deposit(victim_deposit, supply, donated_aum).unwrap();
        assert!(victim_shares > 0);
    }

    #[test]
    fn nav_bootstrap_is_unit() {
        // Empty vault: 1 share = 1 numéraire unit.
        assert_eq!(
            nav_per_share(0, 0).unwrap(),
            NAV_SCALE * VIRTUAL_ASSETS / VIRTUAL_SHARES
        );
    }

    #[test]
    fn nav_steady_state_after_deposit_is_unchanged() {
        // NAV must sit where a second depositor would buy in.
        let deposit = 1_000u64;
        let shares = shares_for_deposit(deposit, 0, 0).unwrap();
        let nav = nav_per_share(shares, deposit).unwrap();
        assert_eq!(nav, NAV_SCALE * VIRTUAL_ASSETS / VIRTUAL_SHARES);
    }

    #[test]
    fn inflation_attack_yields_no_profit_and_victim_keeps_funds() {
        // Attacker seeds with 1 unit, donates a large AUM (worst case: picked
        // up by `validate_aum`), then a realistic victim deposits. The victim
        // must redeem back ~their whole deposit, the attacker no more than
        // deposit + donation.
        let attacker_deposit = 1u64;
        let attacker_shares = shares_for_deposit(attacker_deposit, 0, 0).unwrap();
        assert!(attacker_shares > 0);

        let donation = 10_000_000_000u64; // $10k of "free" AUM, no shares minted
        let aum_after_donation = attacker_deposit + donation;

        let victim_deposit = 5_000_000_000u64; // $5k, realistic
        let victim_shares =
            shares_for_deposit(victim_deposit, attacker_shares, aum_after_donation).unwrap();
        assert!(victim_shares > 0, "victim's deposit rounded to zero shares");

        // 1. Victim redeems immediately: loss is rounding dust, not the donation.
        let supply = attacker_shares + victim_shares;
        let aum = aum_after_donation + victim_deposit;
        let victim_out = assets_for_redeem(victim_shares, supply, aum).unwrap();
        assert!(victim_out <= victim_deposit); // vault never pays out more than in
        let loss = victim_deposit - victim_out;
        assert!(
            loss * 1_000 < victim_deposit, // < 0.1% lost to rounding
            "victim lost {loss} of {victim_deposit} — attack succeeded"
        );

        // 2. Attacker can't even recover the donation, let alone skim.
        let attacker_out = assets_for_redeem(attacker_shares, supply, aum).unwrap();
        assert!(
            attacker_out <= attacker_deposit + donation,
            "attacker extracted a profit"
        );
    }

    // Two holders each own half and both redeem: whoever is fulfilled first
    // gets their half, not the whole AUM (which `supply + this_escrow` paid).
    #[test]
    fn concurrent_full_redeems_each_get_their_half() {
        // supply 1e9, aum 1e9 (offsets negligible), Alice + Bob 500M each.
        let (mut supply, mut aum) = (1_000_000_000u64, 1_000_000_000u64);
        let mut pending_shares = 0u64;
        let (alice, bob) = (500_000_000u64, 500_000_000u64);

        // Both redeem: shares burn now (supply drops), pending tracks them.
        supply -= alice;
        pending_shares += alice;
        supply -= bob;
        pending_shares += bob;
        assert_eq!(supply, 0);
        assert_eq!(pending_shares, 1_000_000_000);

        // Alice priced against the same aum snapshot (no revalidate). Her fill
        // releases the shares and debits the payout in one instruction.
        let owed_a = redeem_owed(alice, supply, pending_shares, aum).unwrap();
        pending_shares -= alice;
        aum -= owed_a;

        // Bob priced against the same snapshot, now reduced by Alice's payout.
        let owed_b = redeem_owed(bob, supply, pending_shares, aum).unwrap();

        // Each got ~half, neither drained the other. The few-hundred shortfall
        // is the offset bias, which favors the vault.
        assert!(owed_a <= 500_000_000, "alice over her half: {owed_a}");
        assert!(owed_a >= 499_990_000, "alice under-paid: {owed_a}");
        assert!(owed_b <= 500_000_000, "bob over his half: {owed_b}");
        assert!(owed_b >= 499_990_000, "bob under-paid: {owed_b}");
        assert!(owed_a + owed_b <= 1_000_000_000, "paid out more than AUM");
    }

    // Illiquid: Alice settles only 300M. Her unpaid shares stay in
    // `pending_shares` — a partial fill is not a fixed numéraire claim on the
    // whole request — so Bob still sees only his own half.
    #[test]
    fn concurrent_partial_redeem_releases_only_paid_shares() {
        let (mut supply, mut aum) = (1_000_000_000u64, 1_000_000_000u64);
        let mut pending_shares = 0u64;
        let (alice, bob) = (500_000_000u64, 500_000_000u64);

        supply -= alice;
        pending_shares += alice;
        supply -= bob;
        pending_shares += bob;

        // Alice's fill prices and releases only the shares it actually pays.
        let alice_settled = 300_000_000u64;
        let paid_a = redeem_owed(alice_settled, supply, pending_shares, aum).unwrap();
        pending_shares -= alice_settled;
        aum -= paid_a;

        // Bob: 700M AUM over 700M effective shares (his 500M + Alice's 200M).
        let owed_b = redeem_owed(bob, supply, pending_shares, aum).unwrap();
        assert!(owed_b <= 500_000_000, "bob over his half: {owed_b}");
        assert!(owed_b >= 499_990_000, "bob under-paid: {owed_b}");
    }

    // A lone redeemer prices exactly as the old single-escrow formula did.
    #[test]
    fn single_redeem_matches_legacy_pricing() {
        let s0 = 10_000_000_000u64;
        let aum = 9_000_000_000u64;
        let shares = 2_000_000_000u64;
        let supply_post_burn = s0 - shares;
        let new = redeem_owed(shares, supply_post_burn, shares, aum).unwrap();
        let legacy = assets_for_redeem(shares, supply_post_burn + shares, aum).unwrap();
        assert_eq!(new, legacy);
    }

    // A redeem burns shares while its assets stay put, so the raw per-share
    // NAV spikes — `crystallize_fees` would over-mint perf fees and ratchet the
    // HWM on it. Netting the pending accumulators back in keeps it flat.
    #[test]
    fn crystallize_nav_unchanged_by_pending_redeem() {
        let (supply0, aum) = (1_000_000_000u64, 1_000_000_000u64);
        let nav_before = nav_per_share(supply0, aum).unwrap();

        // A holder redeems 90%: burned now, unpriced (reserve 0).
        let r = 900_000_000u64;
        let supply_post = supply0 - r;
        let (pending_shares, pending_value) = (r, 0u64);

        let raw = nav_per_share(supply_post, aum).unwrap();
        let netted = nav_per_share(
            supply_post.saturating_add(pending_shares),
            aum.saturating_sub(pending_value),
        )
        .unwrap();

        assert!(raw > nav_before * 2, "raw NAV should spike (the bug)");
        assert_eq!(
            netted, nav_before,
            "netted NAV must be flat across a pending redeem"
        );
    }

    // Retaining the exit fee without minting shares transfers its backing to holders.
    #[test]
    fn instant_redeem_fee_conserves_value() {
        let (supply, aum) = (1_000_000_000u64, 1_000_000_000u64);
        let (redeemer, holder) = (400_000_000u64, 600_000_000u64);
        let holder_before = assets_for_redeem(holder, supply, aum).unwrap();
        let gross = redeem_owed(redeemer, supply, 0, aum).unwrap();
        let fee = gross / 100;
        let net = gross - fee;
        let (supply_post, aum_post) = (supply - redeemer, aum - net);
        assert_eq!(supply_post, holder);
        assert_eq!(net + aum_post, aum);
        assert_eq!(aum_post, holder_before + fee);
        let holder_after = assets_for_redeem(holder, supply_post, aum_post).unwrap();
        assert!(
            holder_after >= holder_before + fee - 10,
            "virtual-offset dust only"
        );
    }

    #[test]
    fn nav_doubles_when_aum_doubles() {
        // The +VA term doesn't double, so NAV lands short of 2× by exactly
        // `VA·NAV_SCALE/(supply+VS)` — bound by that, not 1 ULP.
        let deposit = (1_000_000_000_000_000_000u128 / VIRTUAL_SHARES) as u64;
        let shares = shares_for_deposit(deposit, 0, 0).unwrap();
        let nav_before = nav_per_share(shares, deposit).unwrap();
        // Vault earns 100%: same supply, double AUM.
        let nav_after = nav_per_share(shares, deposit * 2).unwrap();
        let offset_bias = VIRTUAL_ASSETS * NAV_SCALE / (shares as u128 + VIRTUAL_SHARES);
        assert!(nav_after >= 2 * nav_before - offset_bias - 1);
        assert!(nav_after <= 2 * nav_before);
    }
}
