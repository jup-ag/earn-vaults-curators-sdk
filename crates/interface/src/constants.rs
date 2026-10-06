use {
    crate::utils::find_program_address_const,
    solana_address::Address,
    zeropod::pod::{PodI64, PodU32, PodU64},
};

pub const SEED_EVENT_AUTHORITY: &[u8] = b"__event_authority";

pub const EVENT_PDA: (Address, u8) =
    find_program_address_const(&[SEED_EVENT_AUTHORITY], &crate::ID);

/// Maximum forwarded account entries per protocol CPI (excluding Yield's four
/// routing accounts). Fluid `operate` uses 35 fixed slots, leaving 20 remaining
/// accounts; see README.md, "Fluid routed account limit (I-04)".
pub const MAX_ACTION_ACCOUNTS: usize = 55;

/// Protocol's annual management fee on vault AUM, in bps. This is additive to
/// the per-vault curator management fee in
/// [`Vault.management_fee_bps`](crate::state::Vault). Currently disabled. Treat this as
/// immutable after deployment unless all outstanding fee capital-time is
/// settled at an explicit migration boundary.
pub const PROTOCOL_MANAGEMENT_FEE_BPS: u16 = 0;

/// Protocol's performance fee on net-new NAV gains above the vault's
/// high-water mark, in bps. This is additive to the per-vault curator
/// performance fee in [`Vault.performance_fee_bps`](crate::state::Vault). Currently disabled.
/// Treat this as immutable after deployment unless all accrued and unbooked
/// high-water-mark gains are settled at an explicit migration boundary.
pub const PROTOCOL_PERFORMANCE_FEE_BPS: u16 = 0;

/// Cap on the per-vault HWM `Vault.performance_fee_bps`. The charge is booked
/// only on NAV gains above the high-water mark; folded fee debt is separately
/// written down to `Vault::fee_payable_cap` as the live book changes. 50%.
pub const MAX_PERFORMANCE_FEE_BPS: u16 = 5_000;

const _: () =
    assert!(MAX_PERFORMANCE_FEE_BPS as u32 + PROTOCOL_PERFORMANCE_FEE_BPS as u32 <= 10_000);

/// Authorized claimant for the protocol's crystallized-fee share PDA.
/// ⚠️ Dev placeholder — replace with the treasury multisig before deployment.
pub const PROTOCOL_TREASURY: Address =
    Address::from_str_const("GTmMKk9qdVzAsxrnLeXSgsAjf7UbWJph7TqfYZFnA6k6");

/// Max age of `Vault.last_priced_at` when `fulfill` reads `Vault.aum`;
/// `validate_aum` holds each priced slot to the same bound.
const MAX_STALENESS: i64 = 60;
pub const MAX_STALENESS_SECONDS: PodI64 = PodI64::new_from_array(MAX_STALENESS.to_le_bytes());
/// The same window in the width an asset row stores it — see
/// [`crate::state::AssetConfig::max_staleness_seconds`].
pub const MAX_STALENESS_SECONDS_U32: PodU32 =
    PodU32::new_from_array((MAX_STALENESS as u32).to_le_bytes());

/// Ceiling on every curator-set staleness override — asset rows, position
/// slots, and the sink window alike. Bounds how far a curator can widen the
/// stale-price sandwich on anything the vault prices off a cached reading.
pub const MAX_PARAM_STALENESS_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Tighter ceiling for asset rows: settlement and recorded AUM still price each
/// row from its cached spot reading. Instant subscribe/redeem share one
/// canonical row, while the week above stays for positions and sinks —
/// illiquid marks posted by a delegate. Bounded on write *and* clamped on read,
/// including for older rows.
pub const MAX_ASSET_STALENESS_SECONDS: i64 = 600;

/// Stored in a bps window (position drift, sink bootstrap cap) the curator
/// explicitly waived — `Some(0)` at the argument layer, where `None` means "the
/// global default" and a plain `0` would otherwise be indistinguishable from
/// it. Outside the `0..=10_000` band every real value lives in, so a read site
/// tells it apart from a configured cap. Staleness has no such sentinel: its
/// waiver is [`MAX_PARAM_STALENESS_SECONDS`] itself, the ceiling being the point.
pub const UNBOUNDED_BPS: u16 = u16::MAX;

/// Freshness window an asset row falls back to when its `max_staleness_seconds`
/// is `0` — the age a cached reading may reach at `validate_aum` and, because
/// every settlement re-gates that cache, at every site that values off it.
pub const DEFAULT_ASSET_STALENESS_SECONDS: i64 = 15;

/// Settlement-grade freshness: the age past which a cached reading may not
/// *size a money-moving settlement*: direct and queued share issuance, direct
/// and queued redeem payouts, listed or unlisted swap defenses, and any
/// protocol leg folded into a mark. A fold clears the mark's priced attestation,
/// but retains its objective delta and can be re-attested unchanged at zero
/// drift, so that delta must still use a recent reading. The row window
/// tolerates keeper lag for valuation; a settlement executed at a price the
/// market has already left can make its offsetting book entry agree only with
/// that stale cache. A short window requires a near-contemporaneous
/// `refresh_prices`, keeping the settlement and its accounting on a recent
/// reading.
pub const SETTLEMENT_MAX_STALENESS_SECONDS: i64 = 15;

/// Pyth confidence cap in bps of price when an asset leaves
/// `max_confidence_bps` at `0` — a blown-out band (depeg/halt) can't set AUM.
pub const DEFAULT_MAX_CONFIDENCE_BPS: u16 = 100;

/// Per-hop floor on a routed swap's output vs. its oracle-fair value, in bps.
/// Oracle-derived, never from the caller's quote, so a SWAP delegate can't
/// route at a self-favoring rate.
pub const MAX_SWAP_SLIPPAGE_BPS: u64 = 100;

/// Same, for an unlisted (`LIQUIDATE`) swap: base received must clear the
/// retired sink mark less this. Looser — seized collateral executes worse.
pub const MAX_LIQ_SLIPPAGE_BPS: u64 = 500;

/// Refill duration for both slippage buckets, in seconds. 1h.
pub const SWAP_SLIPPAGE_WINDOW_SECONDS: i64 = 60 * 60;

/// Swap slippage bucket capacity, in bps of AUM, refilling linearly over
/// [`SWAP_SLIPPAGE_WINDOW_SECONDS`] — caps burst loss across hops that each
/// clear [`MAX_SWAP_SLIPPAGE_BPS`], until the curator can `SetPaused`. Only
/// meaningful under the settlement freshness gate
/// ([`SETTLEMENT_MAX_STALENESS_SECONDS`]): struck off a lagged cache, both the
/// floor and this bucket record nothing.
pub const MAX_SWAP_SLIPPAGE_PER_WINDOW_BPS: u128 = 200;

/// Same bucket for liquidations; higher because they're episodic and larger
/// per event, sized to fit one honest default-liquidation.
pub const MAX_LIQ_SLIPPAGE_PER_WINDOW_BPS: u128 = 1_000;

/// `price_opaque` drift budget in bps per `MAX_STALENESS_SECONDS`, pro-rated by
/// elapsed time (`elapsed == 0` → no budget) against the smaller of the prior
/// mark and last validated net AUM — blocks walking a mark for stale-NAV arb.
pub const MAX_DRIFT_BPS: u64 = 200; // 2% per staleness window
pub const MAX_TIMELOCK_DELAY: PodU64 = PodU64::new_from_array((30u64 * 24 * 60 * 60).to_le_bytes());

/// `Vault.timelock_delay` when the curator passes `0` at init: a non-zero
/// Redeem → ClaimRedeem cool-down is the baseline posture (recall time). 1 day.
pub const DEFAULT_TIMELOCK_DELAY: PodU64 = PodU64::new_from_array((24_u64 * 60 * 60).to_le_bytes());

pub const MAX_INSTANT_REDEEM_FEE_BPS: u16 = 1_000;
pub const MIN_INSTANT_REDEEM_FEE_BPS: u16 = 50;

/// Cap on the annual streaming `Vault.management_fee_bps` — bounds curator
/// dilution via permissionless `crystallize_fees`. 10%/yr, non-confiscatory.
pub const MAX_MANAGEMENT_FEE_BPS: u16 = 1_000;

const _: () = assert!(MAX_MANAGEMENT_FEE_BPS as u32 + PROTOCOL_MANAGEMENT_FEE_BPS as u32 <= 10_000);

/// ERC-4626 virtual offsets: `shares = deposit * (supply + VS) / (aum + VA)`.
/// Magnitude prices a donation attack at ~1000x; `VS == VA` pins bootstrap to
/// 1 raw share per numéraire unit ([`SHARE_DECIMALS`], `hwm_nav`). Lock pre-launch.
pub const VIRTUAL_SHARES: u128 = 1_000;
pub const VIRTUAL_ASSETS: u128 = 1_000;

/// Performance-fee HWM of a holder-less vault (`VS == VA` ⇒ 1.0/share), written
/// at init and re-armed by `accrue_fees` at zero supply so the next cohort's
/// mark is reachable. Only exact when the emptied book holds no assets either.
pub const BOOTSTRAP_HWM_NAV: u128 = NAV_SCALE * VIRTUAL_ASSETS / VIRTUAL_SHARES;

/// Per-share NAV scale: `nav_q18 / NAV_SCALE` is base-asset units per share.
pub const NAV_SCALE: u128 = 1_000_000_000_000_000_000;

/// Decimals of the AUM/NAV numéraire (USD).
pub const NUMERAIRE_DECIMALS: u8 = 6;

/// Ceiling on a curator-declared `AssetConfig::report_decimals`. Past
/// `MAX_POW10` (38) the oracle converters floor every value to zero anyway, so
/// this is the point where a declared scale stops meaning anything.
pub const MAX_ORACLE_REPORT_DECIMALS: u8 = 38;

/// Exponent of a hand-posted price (zero `oracle`): numéraire micro-units per whole token, so 1 USD is `1_000_000`.
pub const MANUAL_PRICE_EXPO: i32 = -(NUMERAIRE_DECIMALS as i32);

/// Cap on an onboarded mint's decimals (real mints are ≤ 9) — bounds the
/// oracle converters' `10^|exponent + NUMERAIRE_DECIMALS − decimals|` so a
/// pathological mint can't `AumOverflow` every pricing call for its slot.
pub const MAX_ASSET_DECIMALS: u8 = 18;

/// Share-mint decimals: shares bootstrap 1:1 with numéraire units (VS == VA).
/// Re-derive as `NUMERAIRE_DECIMALS + log10(VS/VA)` if that ratio changes.
pub const SHARE_DECIMALS: u8 = NUMERAIRE_DECIMALS;

/// Seconds in a year (365d) — denominator for the streaming management fee.
pub const SECONDS_PER_YEAR: u128 = 365 * 24 * 60 * 60;

/// Largest single signed report `refresh_prices` forwards to the verifier. The
/// DON's compressed v3 payload plus its f+1 signatures sits well under this, and
/// the ceiling keeps the CPI buffer on the program's stack. The binding limit in
/// practice is the transaction size, not this.
pub const MAX_SIGNED_REPORT_LEN: usize = 1024;

/// Chainlink Data Streams **Verifier** program.
pub const CHAINLINK_VERIFIER_ID: Address =
    Address::from_str_const("Gt9S41PtjR58CbG9JhJ3J6vxesqrNAswbWYbLNTMZA3c");

/// Chainlink **CCIP Router** program on Solana mainnet (`ccip_send`).
pub const CCIP_ROUTER_ID: Address =
    Address::from_str_const("Ccip842gzYHhvdDkSyi2YVCoAWPbYJoApMFzSxQroE9C");

/// Circle **CCTP v2 TokenMessengerMinter** program on Solana mainnet
/// (`deposit_for_burn_with_hook`) — the burn leg of a Nest Solana mint.
pub const CCTP_TOKEN_MESSENGER_MINTER_V2_ID: Address =
    Address::from_str_const("CCTPV2vPZJS2u2BBsUoscuikbYjnpFmbFsvVuJdgUMQe");

/// Nest's LayerZero OFT program on Solana mainnet (`send`).
pub const NEST_OFT_ID: Address =
    Address::from_str_const("ChEfPd3RzLeYiRwp1K9evimmaFSd6DV1S4Mv5q5Aj1th");

pub const HOOK_ID: Address = Address::from_str_const("JUPHookQEq12nYVaRYDkaznwomoZGS7PwTS3CMMiyUa");
pub const HOOK_IX_EXECUTE: [u8; 8] = [105, 37, 101, 197, 75, 251, 102, 26];
pub const HOOK_IX_INIT_EXTRA_ACCOUNT_META_LIST: [u8; 8] = [235, 11, 148, 19, 61, 141, 235, 14];
pub const HOOK_SEED_EXTRA_ACCOUNT_METAS: &[u8] = b"extra-account-metas";
