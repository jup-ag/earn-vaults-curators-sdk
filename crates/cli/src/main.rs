//! `earn-vaults` — initialize and operate Earn Vaults on a live Solana
//! cluster. Commands use the Rust SDK instruction builders over JSON-RPC;
//! protocol routes are supplied to `protocol-cpi` as JSON files.
//!
//! Flow:  init-vault → grant-delegate → init-accounts → subscribe → validate-aum
//!        → protocol-cpi (deploy) → protocol-cpi (unwind) → validate-aum
//!        → redeem → fulfill-redeem → claim-redeem
//!
//! State (vault address, derived PDAs, token accounts) is persisted to a JSON
//! file between invocations (default ./vault-state.json).

use {
    anyhow::{Context, Result, anyhow, bail},
    base64::{Engine, engine::general_purpose::STANDARD as B64},
    clap::{Parser, Subcommand},
    earn_vaults_cli::{
        TOKEN_2022, addr, create_ata, create_ata_ix, fetch_snapshot, load_kp, mint_token_program,
        rpc, send_opts, set_fees,
    },
    earn_vaults_sdk::{
        ID as PROGRAM_ID,
        arguments::{PositionAdd, PositionInit},
        constants::{CCIP_ROUTER_ID, CCTP_TOKEN_MESSENGER_MINTER_V2_ID, NEST_OFT_ID},
        instructions::{
            AcceptCurator, AssetAddFields, AssetUpdateFields, CancelPartialRedeem, CancelRedeem,
            ClaimFees, ClaimRedeem, CrystallizeFees, FulfillRedeem, GrantRevokeDelegate,
            InitializeExtraAccountMetas, InitializeVault, PositionUpdateFields, PriceOpaque,
            ProtocolCpi, Redeem, RedeemDirect, RefreshPrices, SetProtocolPolicy, SubscribeDirect,
            UpdateShareMeta, UpdateVault, ValidateAum,
        },
        pda::{
            derive_associated_token_account, derive_share_mint, derive_vault,
            derive_vault_authority,
        },
        state::{CcipLane, CollateralTerms, GENERAL_COLLATERAL, LoanLimits, NestRoute, Protocol},
        zeropod::pod::{PodOption, PodU16, PodU32},
    },
    serde::{Deserialize, Serialize},
    serde_json::{Value, json},
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    solana_signer::Signer,
    std::{
        collections::BTreeMap,
        path::PathBuf,
        thread,
        time::{Duration, SystemTime},
    },
};

mod fluid;

// Default mainnet base-asset addresses.
const DEFAULT_BASE_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
/// Default base/USD `PriceUpdateV2` account. The program pins the embedded feed
/// id rather than this account address.
const DEFAULT_BASE_ORACLE: &str = "6HAuqASbHEh4w4REJEUUUCginTLfj1kwCh215ZLtMkrT";
/// Default base/USD Pyth feed id, base58 of `eaa020…c94a`.
const DEFAULT_BASE_FEED_ID: &str = "Gnt27xtC473ZT2Mw5u8wZ68Z3gULkSTb5DuxJy7eJotD";
const JUPITER: &str = "JUP6LkbZbjS1jKKwapdHNy74zcZ3tLUZoi5QNyVTaV4";
const JUPITER_SWAP_BIT: u64 = 1 << 0;

#[derive(Parser)]
#[command(name = "earn-vaults", about = "Operate Earn Vaults on a live cluster")]
struct Cli {
    /// RPC endpoint.
    #[arg(
        long,
        env = "SOLANA_RPC_URL",
        default_value = "https://api.mainnet-beta.solana.com",
        global = true
    )]
    rpc: String,
    /// State file (persists derived addresses between calls).
    #[arg(long, default_value = "vault-state.json", global = true)]
    state: PathBuf,
    /// Priority fee in micro-lamports per compute unit (0 = don't bid). Every
    /// transaction gets it; mainnet needs a nonzero bid to land promptly under
    /// load.
    #[arg(long, default_value_t = 0, global = true)]
    priority_fee: u64,
    /// Explicit compute-unit ceiling (0 = the runtime default, 200k per
    /// instruction). Raise it for a `protocol-cpi` whose routed CPI is deep, or
    /// for a `subscribe` that bundles `validate-aum` over several legs.
    #[arg(long, default_value_t = 0, global = true)]
    compute_limit: u32,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
#[expect(
    clippy::large_enum_variant,
    reason = "one parsed CLI command; boxing individual options adds needless indirection"
)]
enum Cmd {
    /// Print a live vault snapshot without a state file or keypair.
    Show {
        vault: Address,
        /// Investor pubkey to include share balance and redeem value.
        #[arg(long)]
        user: Option<Address>,
    },
    /// InitializeVault: one base (deposit) asset, an optional priced "other"
    /// asset, and any number of opaque positions. Mints may be classic SPL or
    /// Token-2022 — the program CPIs whichever owns each mint. Records vault +
    /// PDAs to state.
    ///
    /// For an INF loop: `--base-mint <INF> --base-oracle <INF/USD>` (deposit),
    /// `--other-mint <wSOL> --other-oracle <SOL/USD>` (the debt asset, priced
    /// into AUM but not deposit-enabled), and
    /// `--position 2:2:9:60:300` (slot 2, JupiterLend protocol, flags
    /// ENABLED|REQUIRED_IN_AUM, 60s staleness, 3% drift).
    InitVault {
        #[arg(long)]
        curator: PathBuf,
        /// Base (deposit/withdraw) mint.
        #[arg(long, default_value = DEFAULT_BASE_MINT)]
        base_mint: String,
        /// Base oracle **account** (a Pyth `PriceUpdateV2`) — stored in state and
        /// passed as a `validate_aum` price leg (the only instruction that reads
        /// a feed — everything else prices off the cached asset row).
        #[arg(long, default_value = DEFAULT_BASE_ORACLE)]
        base_oracle: String,
        /// Base **feed id** (base58 of the 32-byte Pyth feed id) — what the vault
        /// stores + pins on. The built-in mint, oracle, and feed values form a
        /// matched mainnet set; override them together.
        #[arg(long, default_value = DEFAULT_BASE_FEED_ID)]
        base_feed_id: String,
        /// Priced "other" mint (into AUM, not deposit-enabled) — e.g. the wSOL
        /// debt asset for a loop. Omit for a base-only vault (e.g. the
        /// offerbook-mm runbook, where seized collateral rides the opaque
        /// position mark instead of the asset list).
        #[arg(long, requires_all = ["other_oracle", "other_feed_id"])]
        other_mint: Option<String>,
        /// Other oracle **account** (a `validate_aum` price leg).
        #[arg(long, requires_all = ["other_mint", "other_feed_id"])]
        other_oracle: Option<String>,
        /// Other **feed id** — what the vault pins on.
        #[arg(long, requires_all = ["other_mint", "other_oracle"])]
        other_feed_id: Option<String>,
        /// Opaque position, repeatable.
        /// `SLOT:PROTOCOL:FLAGS:STALENESS:DRIFT` (decimal). SLOT is the
        /// free index in `Vault.positions` (0..7). Protocol values: SpotTokens
        /// 0, Offerbook 1, JupiterLend 2 — at most one *enabled* slot each.
        /// Flag values: REQUIRED_IN_AUM 1, DEBT 4, ENABLED 8.
        /// STALENESS and DRIFT are three-way: leave the field EMPTY
        /// (`2:2:9:::50`) for the program default (60s / 2%), pass `0` for the
        /// loosest setting (the 7-day staleness ceiling / no drift budget), or
        /// pass the value.
        #[arg(long = "position", value_name = "SPEC")]
        positions: Vec<String>,
        /// Seconds between Redeem and ClaimRedeem. Omit for the 1-day default,
        /// `0` for no cool-down at all, or pass the value (<= 30 days).
        #[arg(long)]
        timelock_delay: Option<u64>,
        /// Annual curator management fee on AUM, in bps.
        #[arg(long)]
        management_fee_bps: Option<u16>,
        /// Curator HWM performance fee on NAV gains, in bps.
        #[arg(long)]
        performance_fee_bps: Option<u16>,
        /// Instant-withdrawal exit fee in bps (omit to disable RedeemDirect).
        #[arg(long)]
        instant_redeem_fee_bps: Option<u16>,
    },
    /// Grant a delegate a permission bitmask over one target program. Defaults
    /// to the Jupiter-swap bit. For vault-internal (system) permissions —
    /// PRICE / FULFILL / PAUSE — pass `--protocol self` (the vault program id).
    GrantDelegate {
        #[arg(long)]
        curator: PathBuf,
        #[arg(long)]
        delegate: String,
        /// Target program the delegate may drive (default: Jupiter v6). Pass
        /// `self` for vault-internal system permissions.
        #[arg(long, default_value = JUPITER)]
        protocol: String,
        #[arg(long, default_value_t = JUPITER_SWAP_BIT)]
        permissions: u64,
    },
    /// Approve a collateral mint for Offerbook lending and borrowing. Lending
    /// collateral may be unlisted; borrowing collateral must be listed. Every
    /// create and fill needs this policy. `--max-principal-amount 0` closes it.
    SetCollateralPolicy {
        #[arg(long)]
        curator: PathBuf,
        /// Collateral mint, or `general` to approve lending against any mint.
        #[arg(long)]
        mint: String,
        /// Listed principal mint this collateral approval may be used with.
        #[arg(long)]
        principal_mint: Option<String>,
        #[arg(long)]
        max_principal_amount: u64,
        #[arg(long)]
        max_collateral_amount: Option<u64>,
    },
    /// PriceOpaque: a PRICE delegate posts a position's base-denominated value
    /// into `Vault.positions[slot]` (the net Fluid mark for a loop). Must be
    /// re-posted within the position's `max_staleness_seconds` or `validate-aum`
    /// halts. Grant the signer `self` with the PRICE bit (1) first. A
    /// never-priced slot's first mark is capped at one drift window (2%) of the
    /// last validated AUM; larger positions are walked up window by window.
    PriceOpaque {
        #[arg(long)]
        delegate: PathBuf,
        /// Index into Vault.positions.
        #[arg(long)]
        slot: u8,
        /// Base-asset-denominated position value.
        #[arg(long)]
        value: u64,
    },
    /// Create the lifecycle ATAs under the program that owns each mint: the
    /// investor's base + share ATAs and the vault authority's base + optional
    /// other-asset ATAs. `subscribe` creates the share ATA on demand, so this is
    /// mainly for asset custody.
    InitAccounts {
        /// Funds + owns the investor's base and share ATAs.
        #[arg(long)]
        user: PathBuf,
        /// Funds the vault-authority-owned base + position ATAs.
        #[arg(long)]
        curator: PathBuf,
    },
    /// SubscribeDirect: deposit the base asset and mint shares atomically.
    Subscribe {
        #[arg(long)]
        user: PathBuf,
        #[arg(long)]
        amount: u64,
        /// Least shares (base units) to accept; 0 = no bound.
        #[arg(long, default_value_t = 0)]
        min_shares_out: u64,
    },
    /// RedeemDirect: burn shares and withdraw the base asset atomically.
    RedeemDirect {
        #[arg(long)]
        user: PathBuf,
        #[arg(long)]
        shares: u64,
        /// Least base-asset payout (base units, after the fee) to accept; 0 = no bound.
        #[arg(long, default_value_t = 0)]
        min_out: u64,
    },
    /// ValidateAum: reprice the vault's base and optional other asset into AUM.
    ValidateAum {
        #[arg(long)]
        curator: PathBuf,
    },
    /// UpdateVault: curator-only knobs. Omitted flags are left untouched.
    /// NOTE: `timelock-delay` only affects redeems queued *after* this call —
    /// an escrow's `unlock_at` is frozen at Redeem time.
    UpdateVault {
        #[arg(long)]
        curator: PathBuf,
        /// Loan policy: APY:MIN_SECONDS:MAX_SECONDS:EXPIRY_SECONDS:LENDER.
        /// APY is a ceiling when borrowing and a floor when lending; LENDER is
        /// an address or `any`. Use `disabled` to stop new loans.
        #[arg(long, value_name = "SPEC")]
        offerbook_limits: Option<String>,
        /// Seconds between Redeem and ClaimRedeem (<= MAX_TIMELOCK_DELAY).
        #[arg(long)]
        timelock_delay: Option<u64>,
        /// Absolute Pause bitmask write (0 clears all pauses).
        #[arg(long)]
        paused: Option<u8>,
        /// Propose next curator (base58); pass the default pubkey to cancel.
        #[arg(long)]
        pending_curator: Option<String>,
        /// Annual curator management fee on AUM, in bps.
        /// Rejected with FeeAccrualPending unless crystallize-fees ran in the
        /// same slot (send it first in the same tx).
        #[arg(long, value_name = "BPS")]
        management_fee_bps: Option<u16>,
        /// Curator HWM performance fee on NAV gains, in bps.
        /// Rejected with FeeAccrualPending unless crystallize-fees ran in the
        /// same slot (send it first in the same tx).
        #[arg(long, value_name = "BPS")]
        performance_fee_bps: Option<u16>,
        /// Onboard a NEW asset row: the mint address. Stales the cached AUM —
        /// create the custody ATA, refresh the price, and validate-aum before
        /// the vault flows again.
        #[arg(long, value_name = "MINT")]
        add_asset_mint: Option<String>,
        /// Feed id the new row pins on; omit for a hand-priced row.
        #[arg(long, value_name = "FEED", requires = "add_asset_mint")]
        add_asset_feed_id: Option<String>,
        /// Read the new row's feed id as Chainlink Data Streams, not Pyth.
        #[arg(long, requires = "add_asset_feed_id")]
        add_asset_chainlink: bool,
        /// Decimal scale the feed quotes its raw mantissa at; omit to leave the
        /// source's own. Set it for a Data Streams stream that is not the
        /// documented 18 (check the stream's page) — a v3 report cannot state
        /// its scale, and an undeclared 8-decimal feed prices the asset 1e10 low
        /// and it redeems for dust. On a Pyth row it pins the expected exponent.
        #[arg(long, value_name = "N", requires = "add_asset_feed_id")]
        add_asset_report_decimals: Option<u8>,
        /// Accept deposits in the new asset (requires a real feed).
        #[arg(long, requires = "add_asset_feed_id")]
        add_asset_deposit_enabled: bool,
        /// Onboard a NEW position into a free slot — same SPEC as init's
        /// --position: SLOT:PROTOCOL:FLAGS:STALENESS:DRIFT.
        #[arg(long, value_name = "SPEC")]
        add_position: Option<String>,
        /// Retire an emptied asset row (deposits disabled, deployed == 0,
        /// custody empty) and free its slot.
        #[arg(long, value_name = "INDEX", requires = "remove_asset_mint")]
        remove_asset_index: Option<u8>,
        /// The retired row's mint — used to derive the custody ATA the program
        /// checks holds zero.
        #[arg(long, value_name = "MINT", requires = "remove_asset_index")]
        remove_asset_mint: Option<String>,
        /// Retire a position row with no live mark (never-priced, reset, or
        /// marked to zero) and free its slot.
        #[arg(long, value_name = "INDEX")]
        remove_position_index: Option<u8>,
        /// Attest full repayment of a DEBT position after verifying it off-chain.
        /// Records zero without changing drift limits; run validate afterwards.
        #[arg(long, value_name = "INDEX")]
        retire_debt: Option<u8>,
        /// Asset row the cap flags below apply to (index into Vault.assets;
        /// the base asset is 0).
        #[arg(long)]
        asset_index: Option<u8>,
        /// Capacity ceiling: reject subscriptions that would push this asset's
        /// idle custody plus `deployed` above CAP (raw token units, 0 = unlimited).
        #[arg(long, value_name = "CAP", requires = "asset_index")]
        max_deposit: Option<u64>,
        /// Strategy-outflow ceiling: reject routed CPIs that would push this
        /// asset's `deployed` above CAP (raw token units, 0 = unlimited).
        #[arg(long, value_name = "CAP", requires = "asset_index")]
        max_deployed: Option<u64>,
        /// Restate this asset's remaining `deployed` charge across all protocols
        /// (raw token units), after verifying a principal return. CCIP OffRamp
        /// deliveries and Offerbook escrow withdrawals do not lower it, leaving
        /// both deployment and deposit capacity charged. Seized collateral does
        /// not justify reducing an unrelated deployment's charge.
        /// For CCIP, pair with `--position-index N --position-reset` (full return)
        /// or a curator `price-opaque` (partial) to reconcile the remote value.
        /// Listed Offerbook withdrawals already fold the position mark: do not
        /// reset it; reconcile and re-attest the remaining value with `price-opaque`.
        #[arg(long, value_name = "AMOUNT", requires = "asset_index")]
        asset_deployed: Option<u64>,
        /// Valuation and settlement age (0 = 15s; positive values up to u32::MAX).
        #[arg(long, value_name = "SECS", requires = "asset_index")]
        asset_max_staleness: Option<i64>,
        /// Maximum future timestamp skew for this asset; 0 rejects all future timestamps.
        #[arg(long, requires = "asset_index")]
        asset_max_future_skew: Option<u32>,
        /// Toggle deposits for this asset (a hand-priced row may not take
        /// deposits).
        #[arg(long, value_name = "BOOL", requires = "asset_index")]
        asset_deposit_enabled: Option<bool>,
        /// Decimal scale this row's feed quotes at; `0` hands it back to the
        /// source. Retires the cached reading, so refresh + re-validate after.
        #[arg(long, value_name = "N", requires = "asset_index")]
        asset_report_decimals: Option<u8>,
        /// Compose this row's price with a second feed (base58 feed id): the
        /// price becomes the row's feed × this one, e.g. sUSDai/USDai ×
        /// USDai/USD. `11111111111111111111111111111111` makes it single-feed
        /// again. Retires the cached reading.
        #[arg(long, value_name = "FEED_ID", requires = "asset_index")]
        asset_quote_feed_id: Option<String>,
        /// The quote feed's `PriceUpdateV2` **account**, remembered in state so
        /// every bundled refresh carries both legs (a composed row only moves
        /// when both arrive in one `RefreshPrices`).
        #[arg(long, value_name = "ACCOUNT", requires = "asset_quote_feed_id")]
        asset_quote_oracle: Option<String>,
        /// Decimal scale the quote feed quotes at (see --asset-report-decimals).
        #[arg(long, value_name = "N", requires = "asset_index")]
        asset_quote_report_decimals: Option<u8>,
        /// Position row the risk flags below apply to (index into
        /// Vault.positions, 0..7).
        #[arg(long)]
        position_index: Option<u8>,
        /// Unbrick a slot frozen at a zero/dust mark: back to never-priced, so
        /// the next price_opaque bootstraps. Clears PRICED | INITIALIZED and
        /// zeroes last_value. validate_aum skips it until it is re-marked.
        #[arg(long, requires = "position_index")]
        position_reset: bool,
        /// Ceiling on the transient flash liability a FLASH delegate may open,
        /// in numéraire micro-units. 0 disables the path.
        #[arg(long, value_name = "MICRO_NUMERAIRE")]
        max_flash_liability: Option<u64>,
        /// Discount validate_aum applies to every sink mark, in bps.
        #[arg(long, value_name = "BPS")]
        sink_haircut_bps: Option<u16>,
        /// Sink-mark freshness window in seconds (0 = 60s, max 7 days).
        #[arg(long, value_name = "SECS")]
        sink_max_staleness: Option<u64>,
        /// Validated AUM age limit in seconds (0 = 60s). Oracle limits stay separate.
        #[arg(long, value_name = "SECS")]
        max_aum_staleness: Option<u32>,
        /// Vault-wide ceiling on aggregate sink FIRST marks, in bps of validated
        /// AUM (0 = the 2% default). Size it to the total seized-collateral book.
        /// Later marks use the global 2%-per-window walk rate.
        #[arg(long, value_name = "BPS")]
        sink_bootstrap_cap_bps: Option<u16>,
        /// How many sink slots may be claimed (0..=4). 0 keeps seized-collateral
        /// custody off: an unlisted in-flow reverts instead of provisioning a
        /// delegate-priced AUM row. Required before any liquidation flow.
        /// Gates CLAIMS ONLY — not a kill switch. Lowering it under an occupied
        /// slot leaves that slot marked and counted in AUM; use --sink-release
        /// to take a live sink out of AUM, or set-paused Strategy to stop marks.
        #[arg(long, value_name = "N")]
        sink_capacity: Option<u8>,
        /// Write off Vault.sinks[N]'s mark. The mint stays bound so the balance
        /// is still sellable; the slot frees itself once it is sold.
        #[arg(long, value_name = "N")]
        sink_release: Option<u8>,
        /// CCIP lane a SEND delegate may bridge to: CHAIN_SELECTOR:RECEIVER,
        /// RECEIVER a 0x-hex EVM address (left-padded to 32 bytes) or a base58
        /// 32-byte key. `disabled` turns bridging off. One lane per vault.
        #[arg(long, value_name = "SPEC")]
        ccip_lane: Option<String>,
        /// Nest route: COMPOSER:RELAYER:OFT_STORE:SHARE_MINT. The first two are
        /// 0x-hex EVM addresses; the latter two are Solana addresses.
        /// `disabled` turns Nest off. One route per vault.
        #[arg(long, value_name = "SPEC")]
        nest_route: Option<String>,
    },
    /// UpdateShareMeta: curator rewrites the share mint's Token-2022 metadata.
    /// Omitted fields are left untouched.
    UpdateShareMeta {
        #[arg(long)]
        curator: PathBuf,
        /// Token name (max 25 bytes).
        #[arg(long)]
        name: Option<String>,
        /// Token symbol (max 5 bytes).
        #[arg(long)]
        symbol: Option<String>,
        /// Metadata URI (max 200 bytes).
        #[arg(long)]
        uri: Option<String>,
    },
    /// CrystallizeFees: permissionless — mints the accrued fee liabilities
    /// (management + performance, split into curator and protocol components)
    /// as shares.
    /// NAV-neutral and timing-irrelevant (fees accrue as integrals folded at
    /// every flow), so run it whenever. Bundles `RefreshPrices` + `ValidateAum`:
    /// fees mint at NAV, so AUM must be validated in the same slot.
    /// The program lazily creates both fee PDAs.
    Crystallize {
        /// Any keypair — pays the tx fee and rent for new fee PDAs.
        #[arg(long)]
        signer: PathBuf,
    },
    /// ClaimFees: drain and close the signer's per-vault fee PDA. The signer
    /// must be the vault curator or protocol treasury; the destination may be
    /// any existing share token account owned by that signer.
    ClaimFees {
        /// Curator or protocol-treasury keypair.
        #[arg(long)]
        signer: PathBuf,
        /// Existing Token-2022 account for this vault's share mint.
        #[arg(long)]
        destination: String,
    },
    /// ProtocolCpi: forward a swap (or any CPI) built from a route file.
    /// The route file is the JSON below — get accounts+data from Jupiter's
    /// /swap-instructions API:
    ///   {"target_program":"JUP...","data_base64":"...",
    ///    "accounts":[{"pubkey":"..","is_signer":false,"is_writable":true}, ...]}
    /// An Offerbook create/fill, a CCIP send or a Nest call must end its
    /// `accounts` with the vault's protocol policy account
    /// `["protocol_policy", vault, protocol, zero key]` (read-only).
    ProtocolCpi {
        #[arg(long)]
        delegate: PathBuf,
        #[arg(long)]
        route_file: PathBuf,
        /// Pre-create the vault authority's ATA of MINT in the same tx
        /// (CreateIdempotent, delegate pays rent). Repeatable. Needed for a
        /// receipt-token account the routed CPI would otherwise create with the
        /// authority PDA as payer — it holds no lamports, e.g. the first
        /// deposit into a Jupiter Earn receipt-token ATA.
        #[arg(long, value_name = "MINT")]
        create_ata: Vec<String>,
        /// Address lookup table to compress account keys against, repeatable.
        /// Jupiter's /swap-instructions returns these as
        /// `addressLookupTableAddresses` — pass every one, or a real route's key
        /// list overruns the 1232-byte transaction limit.
        #[arg(long = "lut", value_name = "ADDRESS")]
        luts: Vec<String>,
    },
    /// Redeem: burn shares, open a redemption.
    Redeem {
        #[arg(long)]
        user: PathBuf,
        #[arg(long)]
        shares: u64,
        /// Escrow nonce (distinct values allow concurrent requests).
        #[arg(long, default_value_t = 0)]
        nonce: u64,
        /// Minimum raw base tokens for the full request (enforced pro rata per fill).
        #[arg(long, default_value_t = 0)]
        min_out: u64,
    },
    /// FulfillRedeem: curator prices the redemption against the base asset.
    FulfillRedeem {
        #[arg(long)]
        curator: PathBuf,
        /// Redeemer pubkey (defaults to `user` from the state file).
        #[arg(long)]
        user: Option<String>,
        /// Escrow nonce used at `redeem`.
        #[arg(long, default_value_t = 0)]
        nonce: u64,
    },
    /// ClaimRedeem: investor pulls the base asset out and closes escrow.
    ClaimRedeem {
        #[arg(long)]
        user: PathBuf,
        /// Escrow nonce used at `redeem`.
        #[arg(long, default_value_t = 0)]
        nonce: u64,
    },
    /// CancelRedeem: the investor walks away from a queued redemption. Without
    /// `--partial` the redeem must be untouched — the burned shares are re-minted
    /// and the escrow closed. Pass `--partial` when the curator already filled
    /// part of it: the escrowed base proceeds go to the investor and only the
    /// unfilled remainder comes back as shares, re-minted at current NAV.
    CancelRedeem {
        #[arg(long)]
        user: PathBuf,
        /// Escrow nonce used at `redeem`.
        #[arg(long, default_value_t = 0)]
        nonce: u64,
        /// Settle a partially filled redeem (adds the escrow/base-asset legs).
        #[arg(long)]
        partial: bool,
    },
    /// AcceptCurator: the pubkey sitting in `Vault.pending_curator` signs to
    /// complete the rotation `update-vault --pending-curator` proposed. Until
    /// this runs the old curator still holds the vault.
    AcceptCurator {
        /// Keypair of the proposed curator (must equal `Vault.pending_curator`).
        #[arg(long)]
        new_curator: PathBuf,
    },
    /// Build a Fluid (Jupiter Lend) route file for `protocol-cpi`. Encodes the
    /// ix data (borsh) and stamps signer/writable flags onto the ordered pubkey
    /// list you supply. See `earn-vaults fluid --help` for the sub-instructions.
    Fluid {
        #[command(subcommand)]
        ix: FluidIx,
    },
}

/// Fluid sub-instructions. Each takes `--accounts` (a JSON array of base58
/// pubkeys in IDL order — get the tick/branch accounts from Fluid's SDK or a
/// simulated tx; set unused optional slots to the target program id) and writes
/// the route JSON to `--out` (or stdout). Feed the result to
/// `protocol-cpi --route-file` signed by the delegate holding the right bit.
#[derive(Subcommand)]
enum FluidIx {
    /// `operate`: move collateral (`new_col`) and/or debt (`new_debt`), i128
    /// base units (negative = withdraw/repay). 35 accounts, plus any
    /// `--remaining-accounts`.
    Operate {
        #[arg(long)]
        accounts: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, allow_hyphen_values = true)]
        new_col: i128,
        #[arg(long, allow_hyphen_values = true)]
        new_debt: i128,
        /// skip | direct | claim (omit for Option::None).
        #[arg(long)]
        transfer_type: Option<String>,
        /// `remaining_accounts_indices` as `oracle,branches,tick_has_debt`
        /// account counts. Fluid requires exactly 3.
        #[arg(long, default_value = "0,0,0")]
        remaining_indices: String,
        /// JSON array of the trailing remaining accounts (oracle sources, then
        /// branches, then tick-has-debt arrays), appended after the 35. Count
        /// must equal the sum of `--remaining-indices`.
        #[arg(long)]
        remaining_accounts: Option<PathBuf>,
    },
    /// `init_position`: open the position NFT. 13 accounts.
    InitPosition {
        #[arg(long)]
        accounts: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        vault_id: u16,
        #[arg(long)]
        next_position_id: u32,
    },
    /// `flashloan_borrow`: 14 accounts.
    Borrow {
        #[arg(long)]
        accounts: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        amount: u64,
    },
    /// `flashloan_payback`: 14 accounts.
    Payback {
        #[arg(long)]
        accounts: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        amount: u64,
    },
}

/// Persisted between subcommands. Addresses stored base58 for readability.
#[derive(Serialize, Deserialize, Default)]
struct State {
    curator: String,
    vault: String,
    share_mint: String,
    authority: String,
    base_mint: String,
    other_mint: String,
    base_oracle: String,
    other_oracle: String,
    user: String,
    vault_other: String,
    /// Quote-leg `PriceUpdateV2` accounts of composed rows, by asset index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    quote_oracles: BTreeMap<u8, String>,
}

impl State {
    /// Every `PriceUpdateV2` a refresh must carry: base, other, then the quote
    /// legs of composed rows (deduped — a quote may share another row's feed).
    fn oracles(&self) -> Result<Vec<Address>> {
        let mut out = vec![addr(&self.base_oracle)?];
        if !self.vault_other.is_empty() {
            out.push(addr(&self.other_oracle)?);
        }
        for o in self.quote_oracles.values() {
            let o = addr(o)?;
            if !out.contains(&o) {
                out.push(o);
            }
        }
        Ok(out)
    }

    /// Derive the canonical base custody ATA so commands also work before
    /// `init-accounts` has populated the persisted custody field.
    fn vault_base_ata(&self, url: &str) -> Result<Address> {
        let base_mint = addr(&self.base_mint)?;
        let token_program = mint_token_program(url, &base_mint)?;
        Ok(derive_associated_token_account(
            &addr(&self.authority)?,
            &base_mint,
            &token_program,
        ))
    }

    /// Derive a user's canonical base ATA independently of persisted ATA fields.
    fn user_base_ata(&self, url: &str, user: &Address) -> Result<Address> {
        let base_mint = addr(&self.base_mint)?;
        let token_program = mint_token_program(url, &base_mint)?;
        Ok(derive_associated_token_account(
            user,
            &base_mint,
            &token_program,
        ))
    }

    /// A user's share ATA. The share mint is always Token-2022 (the program
    /// creates it), so no lookup is needed.
    fn user_share_ata(&self, user: &Address) -> Result<Address> {
        Ok(derive_associated_token_account(
            user,
            &addr(&self.share_mint)?,
            &addr(TOKEN_2022)?,
        ))
    }
}

/// Emit a route JSON to `out` (or stdout) + a hint to forward it.
fn write_route(out: &Option<PathBuf>, route: Value) -> Result<()> {
    let text = serde_json::to_string_pretty(&route)?;
    match out {
        Some(p) => {
            std::fs::write(p, &text)?;
            println!(
                "route → {} — forward with: earn-vaults protocol-cpi --delegate <kp> --route-file {}",
                p.display(),
                p.display()
            );
        }
        None => println!("{text}"),
    }
    Ok(())
}

/// Read a JSON array of base58 pubkeys (a Fluid ix's accounts, in IDL order).
fn read_pubkeys(path: &PathBuf) -> Result<Vec<String>> {
    let v: Vec<String> = serde_json::from_slice(&std::fs::read(path)?)
        .with_context(|| format!("{}: want a JSON array of base58 pubkeys", path.display()))?;
    Ok(v)
}

/// Parse `SLOT:PROTOCOL:FLAGS:STALENESS:DRIFT`. SLOT is the free index
/// in `Vault.positions`; PROTOCOL names the integration.
fn parse_position(spec: &str) -> Result<(u8, PositionInit)> {
    let f: Vec<&str> = spec.split(':').collect();
    if f.len() != 5 {
        bail!("bad position spec {spec:?}: want SLOT:PROTOCOL:FLAGS:STALENESS:DRIFT");
    }
    // Parse each field straight into its on-wire width: a `u64` parse plus an
    // `as` narrowing would wrap an out-of-range SLOT/PROTOCOL/DRIFT into a
    // valid-looking one (256 -> 0) and send that instead of failing.
    fn n<T: std::str::FromStr>(f: &[&str], i: usize, what: &str) -> Result<T>
    where
        T::Err: std::fmt::Display,
    {
        f[i].parse().map_err(|e| anyhow!("{what}: {e}"))
    }
    // STALENESS and DRIFT are three-way: empty leaves the program default, `0`
    // is the loosest setting, anything else is the value.
    fn opt<T: std::str::FromStr, P: Copy + From<T>>(
        f: &[&str],
        i: usize,
        what: &str,
    ) -> Result<PodOption<P>>
    where
        T::Err: std::fmt::Display,
    {
        if f[i].is_empty() {
            return Ok(PodOption::none());
        }
        Ok(PodOption::some(P::from(n::<T>(f, i, what)?)))
    }
    let tag: u8 = n(&f, 1, "protocol")?;
    if Protocol::from_u8(tag).is_none() {
        bail!("unknown protocol discriminant {tag}");
    }
    Ok((
        n(&f, 0, "slot")?,
        PositionInit {
            protocol: tag,
            flags: n(&f, 2, "flags")?,
            max_staleness_seconds: opt::<u32, PodU32>(&f, 3, "staleness")?,
            max_drift_bps: opt::<u16, PodU16>(&f, 4, "drift")?,
        },
    ))
}

fn parse_offerbook_limits(spec: &str) -> Result<LoanLimits> {
    if spec == "disabled" {
        return Ok(LoanLimits::zero());
    }
    let fields: Vec<_> = spec.split(':').collect();
    let [apy, min_duration, max_duration, expiry, lender] = fields.as_slice() else {
        bail!("want APY:MIN_SECONDS:MAX_SECONDS:EXPIRY_SECONDS:LENDER or disabled");
    };
    let limits = LoanLimits {
        max_apy_bps: apy.parse::<u32>().context("max APY")?.into(),
        min_duration_seconds: min_duration.parse::<u32>().context("min duration")?.into(),
        max_duration_seconds: max_duration.parse::<u32>().context("max duration")?.into(),
        max_offer_expiry_seconds: expiry.parse::<u32>().context("offer expiry")?.into(),
        lender: if *lender == "any" {
            Address::default()
        } else {
            addr(lender)?
        },
    };
    // A zero APY reads as `disabled`; say so explicitly instead.
    if !limits.enabled() || limits.validate().is_err() {
        bail!(
            "invalid Offerbook limits: require positive APY line/duration/expiry and max duration >= min duration"
        );
    }
    Ok(limits)
}

fn parse_ccip_lane(spec: &str) -> Result<CcipLane> {
    if spec == "disabled" {
        return Ok(CcipLane::zero());
    }
    let (selector, receiver) = spec
        .split_once(':')
        .ok_or_else(|| anyhow!("want CHAIN_SELECTOR:RECEIVER or disabled"))?;
    let receiver = match receiver.strip_prefix("0x") {
        Some(hex) => {
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("!"), 16))
                .collect::<Result<Vec<u8>, _>>()
                .context("receiver hex")?;
            if bytes.is_empty() || bytes.len() > 32 {
                bail!("receiver must be 1..=32 bytes of hex");
            }
            let mut padded = [0u8; 32];
            padded[32 - bytes.len()..].copy_from_slice(&bytes);
            Address::new_from_array(padded)
        }
        None => addr(receiver)?,
    };
    let lane = CcipLane {
        dest_chain_selector: selector.parse::<u64>().context("chain selector")?.into(),
        receiver,
    };
    lane.validate()
        .map_err(|_| anyhow!("invalid CCIP lane: a nonzero selector needs a nonzero receiver"))?;
    Ok(lane)
}

fn parse_nest_route(spec: &str) -> Result<NestRoute> {
    if spec == "disabled" {
        return Ok(NestRoute::zero());
    }
    let mut fields = spec.split(':');
    let (composer, relayer, oft_store, share_mint) = match (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) {
        (Some(a), Some(b), Some(c), Some(d), None) => (a, b, c, d),
        _ => bail!("want COMPOSER:RELAYER:OFT_STORE:SHARE_MINT or disabled"),
    };
    let evm = |s: &str| -> Result<[u8; 20]> {
        let hex = s.strip_prefix("0x").unwrap_or(s);
        if hex.len() != 40 {
            bail!("{s}: want a 20-byte 0x-hex EVM address");
        }
        let mut out = [0u8; 20];
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).context("EVM address hex")?;
        }
        Ok(out)
    };
    let route = NestRoute {
        composer: evm(composer)?,
        relayer: evm(relayer)?,
        oft_commitment: NestRoute::commitment(&addr(oft_store)?, &addr(share_mint)?),
    };
    route
        .validate()
        .map_err(|_| anyhow!("invalid Nest route"))?;
    Ok(route)
}

fn load_state(path: &PathBuf) -> Result<State> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("no state at {} — run init-vault first", path.display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn save_state(path: &PathBuf, s: &State) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(s)?)?;
    Ok(())
}

// --- account reads (freshness-sensitive; `rpc`/`send` live in the lib) -------

/// Observation time from a Pyth update or legacy OCR2 store account.
fn oracle_publish_time(url: &str, oracle: &Address) -> Result<i64> {
    let v = rpc(
        url,
        "getAccountInfo",
        json!([oracle.to_string(), {"encoding": "base64", "commitment": "processed"}]),
    )?;
    let account = &v["value"];
    let owner = addr(
        account["owner"]
            .as_str()
            .ok_or_else(|| anyhow!("{oracle} not found"))?,
    )?;
    let data = B64.decode(
        account["data"][0]
            .as_str()
            .ok_or_else(|| anyhow!("{oracle} has no data"))?,
    )?;
    oracle_timestamp(&owner, &data)
}

fn oracle_timestamp(owner: &Address, data: &[u8]) -> Result<i64> {
    if owner == &earn_vaults_sdk::constants::CHAINLINK_OCR2_STORE_ID {
        return earn_vaults_sdk::ocr2::read(owner, data)
            .map(|reading| reading.timestamp)
            .map_err(|e| anyhow!("invalid OCR2 account: {e}"));
    }
    if ![
        "rec2HHDDnjLfj4kE7VyEtFA1HPGQLK33259532cRyHp",
        "rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ",
    ]
    .iter()
    .any(|id| owner == &Address::from_str_const(id))
        || data.get(..8) != Some(&[34, 241, 35, 99, 157, 126, 244, 205])
        || data.get(40) != Some(&1)
    {
        bail!("not an authenticated Pyth or OCR2 account");
    }
    data.get(93..101)
        .and_then(|s| s.try_into().ok())
        .map(i64::from_le_bytes)
        .ok_or_else(|| anyhow!("truncated PriceUpdateV2"))
}

/// Block until every oracle's `publish_time` ADVANCES past its current value,
/// then return immediately — the freshest data this account will ever hold.
/// An absolute age check is hopeless here: updates are already seconds old
/// when they land, so instead we catch the keeper's next post the moment it
/// arrives (the curator daemon re-posts every ~30s tick; canonical pushers
/// ~1/min). All initial values are snapshotted up front so multi-leg vaults
/// don't miss a same-tick post on the later legs.
fn wait_fresh_oracles(url: &str, oracles: &[Address]) -> Result<()> {
    let initial: Vec<i64> = oracles
        .iter()
        .map(|o| oracle_publish_time(url, o))
        .collect::<Result<_>>()?;
    eprintln!(
        "waiting for the next oracle update ({} leg(s))…",
        oracles.len()
    );
    // Allow one OCR2 heartbeat plus reporting/landing delay (900s), with a
    // minute of polling headroom. On-chain per-asset age gates remain final.
    for _ in 0..1_920 {
        thread::sleep(Duration::from_millis(500));
        let advanced = oracles
            .iter()
            .zip(&initial)
            .map(|(o, before)| Ok(oracle_publish_time(url, o)? > *before))
            .collect::<Result<Vec<bool>>>()?;
        if advanced.iter().all(|&a| a) {
            return Ok(());
        }
    }
    bail!("oracles did not advance within 960s — check the feed publisher")
}

/// `RefreshPrices` + `ValidateAum` for the state file's rows — `validate_aum`
/// takes no price accounts of its own, so a caller carrying its own freshness
/// bundles the refresh ahead of it in the same transaction. Pyth and OCR2
/// accounts are supported; Data Streams needs a DON report this CLI cannot fetch.
fn validate_ixs(st: &State, url: &str, user: Address) -> Result<Vec<Instruction>> {
    let vault = addr(&st.vault)?;
    let mut custody_token_accounts = vec![st.vault_base_ata(url)?];
    if !st.vault_other.is_empty() {
        custody_token_accounts.push(addr(&st.vault_other)?);
    }
    let price_updates = st.oracles()?;
    Ok(vec![
        RefreshPrices {
            vault,
            user,
            price_updates,
            chainlink: None,
        }
        .into_instruction()
        .map_err(|e| anyhow!("{e}"))?,
        ValidateAum {
            vault,
            share_mint: addr(&st.share_mint)?,
            custody_token_accounts,
        }
        .into_instruction(),
    ])
}

#[derive(Clone, Copy)]
struct CommandContext<'a> {
    url: &'a str,
    state_path: &'a PathBuf,
}

fn main() -> Result<()> {
    let Cli {
        rpc,
        state,
        priority_fee,
        compute_limit,
        cmd,
    } = Cli::parse();
    set_fees(priority_fee, compute_limit);
    dispatch(
        CommandContext {
            url: &rpc,
            state_path: &state,
        },
        cmd,
    )
}

fn dispatch(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    match command {
        Cmd::Show { vault, user } => {
            println!("{:#?}", fetch_snapshot(ctx.url, vault, user)?);
            Ok(())
        }
        command @ Cmd::InitVault { .. } => run_init_vault(ctx, command),
        command @ Cmd::GrantDelegate { .. } => run_grant_delegate(ctx, command),
        command @ Cmd::SetCollateralPolicy { .. } => run_set_collateral_policy(ctx, command),
        command @ Cmd::PriceOpaque { .. } => run_price_opaque(ctx, command),
        command @ Cmd::InitAccounts { .. } => run_init_accounts(ctx, command),
        command @ Cmd::Subscribe { .. } => run_subscribe(ctx, command),
        command @ Cmd::RedeemDirect { .. } => run_redeem_direct(ctx, command),
        command @ Cmd::ValidateAum { .. } => run_validate_aum(ctx, command),
        command @ Cmd::UpdateVault { .. } => run_update_vault(ctx, command),
        command @ Cmd::UpdateShareMeta { .. } => run_update_share_meta(ctx, command),
        command @ Cmd::Crystallize { .. } => run_crystallize(ctx, command),
        command @ Cmd::ClaimFees { .. } => run_claim_fees(ctx, command),
        command @ Cmd::ProtocolCpi { .. } => run_protocol_cpi(ctx, command),
        command @ Cmd::Redeem { .. } => run_redeem(ctx, command),
        command @ Cmd::FulfillRedeem { .. } => run_fulfill_redeem(ctx, command),
        command @ Cmd::ClaimRedeem { .. } => run_claim_redeem(ctx, command),
        command @ Cmd::CancelRedeem { .. } => run_cancel_redeem(ctx, command),
        command @ Cmd::AcceptCurator { .. } => run_accept_curator(ctx, command),
        command @ Cmd::Fluid { .. } => run_fluid(command),
    }
}

fn run_init_vault(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::InitVault {
        curator,
        base_mint,
        base_oracle,
        base_feed_id,
        other_mint,
        other_oracle,
        other_feed_id,
        positions,
        timelock_delay,
        management_fee_bps,
        performance_fee_bps,
        instant_redeem_fee_bps,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_init_vault")
    };
    let url = ctx.url;

    let curator_kp = load_kp(&curator)?;
    let curator_pk = curator_kp.pubkey();
    let created_at = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)?
        .as_secs() as i64;
    let vault = derive_vault(&curator_pk, created_at);
    // The SDK places `positions[]` by array order (index = slot), so lay
    // each parsed row at its SLOT, padding gaps with disabled
    // (flags == 0) entries the program skips.
    let parsed = positions
        .iter()
        .map(|s| parse_position(s))
        .collect::<Result<Vec<(u8, PositionInit)>>>()?;
    let positions = match parsed.iter().map(|(s, _)| *s as usize).max() {
        None => vec![],
        Some(max_slot) => {
            let zero = PositionInit {
                protocol: 0,
                flags: 0,
                max_staleness_seconds: PodOption::none(),
                max_drift_bps: PodOption::none(),
            };
            let mut arr = vec![zero; max_slot + 1];
            for (slot, p) in parsed {
                arr[slot as usize] = p;
            }
            arr
        }
    };

    let (other_assets, other_oracles) = match (&other_mint, &other_oracle, &other_feed_id) {
        (Some(mint), Some(_oracle), Some(feed)) => (vec![addr(mint)?], vec![addr(feed)?]),
        (None, None, None) => (vec![], vec![]),
        _ => unreachable!("clap requires the other mint, oracle, and feed id together"),
    };
    let asset_mints: Vec<_> = [addr(&base_mint)?]
        .into_iter()
        .chain(other_assets.iter().copied())
        .collect();
    let ix = InitializeVault {
        // The sink and flash knobs all default off here; `update-vault`
        // sets them together once the vault exists.
        max_flash_liability: 0,
        sink_haircut_bps: 0,
        sink_max_staleness_seconds: None,
        sink_bootstrap_cap_bps: None,
        sink_capacity: 0,
        // Data Streams reports are not fetched by this CLI.
        chainlink_assets: vec![],
        report_decimals: vec![],
        curator: curator_pk,
        created_at,
        base_assets: vec![addr(&base_mint)?],
        base_oracles: vec![addr(&base_feed_id)?],
        max_deployed: vec![],
        max_deposit: vec![],
        other_assets,
        other_oracles,
        positions,
        timelock_delay,
        management_fee_bps: management_fee_bps.unwrap_or(0),
        performance_fee_bps: performance_fee_bps.unwrap_or(0),
        instant_redeem_fee_bps,
        metadata_name: "Share Token",
        metadata_symbol: "JUPS",
        metadata_uri: "https://jup.ag/",
    }
    .into_instruction()
    .map_err(|e| anyhow!("{e}"))?;
    let authority = derive_vault_authority(&vault);

    let sig = send_opts(url, &[ix], &[&curator_kp], &[], false)?;

    let st = State {
        curator: curator_pk.to_string(),
        vault: vault.to_string(),
        share_mint: derive_share_mint(&vault).to_string(),
        authority: derive_vault_authority(&vault).to_string(),
        base_mint,
        other_mint: other_mint.unwrap_or_default(),
        base_oracle,
        other_oracle: other_oracle.unwrap_or_default(),
        ..Default::default()
    };
    save_state(ctx.state_path, &st)?;
    println!("vault {vault} created at {created_at} ({sig})");

    let mut ixs = vec![
        InitializeExtraAccountMetas {
            payer: curator_pk,
            vault,
        }
        .into_instruction(),
    ];
    for mint in asset_mints {
        let tp = mint_token_program(url, &mint)?;
        ixs.push(create_ata_ix(curator_pk, authority, mint, tp));
    }
    let sig = send_opts(url, &ixs, &[&curator_kp], &[], false)?;
    println!("meta list + custody ATAs ({sig})");
    Ok(())
}

fn run_grant_delegate(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::GrantDelegate {
        curator,
        delegate,
        protocol,
        permissions,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_grant_delegate")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let curator_kp = load_kp(&curator)?;
    // `self` → the vault program id, which is how system-scoped perms
    // (PRICE/FULFILL/PAUSE) key their delegate PDA.
    let protocol_id = match protocol.as_str() {
        "self" => PROGRAM_ID,
        "ccip" => CCIP_ROUTER_ID,
        "nest-cctp" => CCTP_TOKEN_MESSENGER_MINTER_V2_ID,
        "nest-oft" => NEST_OFT_ID,
        other => addr(other)?,
    };
    let ix = GrantRevokeDelegate {
        curator: curator_kp.pubkey(),
        vault: addr(&st.vault)?,
        delegate: addr(&delegate)?,
        protocol_id,
        permissions,
    }
    .into_instruction();
    println!(
        "granted ({})",
        send_opts(url, &[ix], &[&curator_kp], &[], false)?
    );
    Ok(())
}

fn run_set_collateral_policy(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::SetCollateralPolicy {
        curator,
        mint,
        principal_mint,
        max_principal_amount,
        max_collateral_amount,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_set_collateral_policy")
    };
    let st = load_state(ctx.state_path)?;
    let curator_kp = load_kp(&curator)?;
    let terms =
        (max_principal_amount != 0)
            .then(|| -> Result<CollateralTerms> {
                Ok(CollateralTerms {
                    principal_mint: addr(principal_mint.as_deref().context(
                        "--principal-mint is required when enabling a collateral policy",
                    )?)?,
                    max_principal_amount: max_principal_amount.into(),
                    max_collateral_amount: max_collateral_amount
                        .context(
                            "--max-collateral-amount is required when enabling a collateral policy",
                        )?
                        .into(),
                })
            })
            .transpose()?;
    let ix = SetProtocolPolicy::offerbook_collateral(
        curator_kp.pubkey(),
        addr(&st.vault)?,
        if mint == "general" {
            GENERAL_COLLATERAL
        } else {
            addr(&mint)?
        },
        terms,
    )
    .into_instruction();
    println!(
        "collateral policy set ({})",
        send_opts(ctx.url, &[ix], &[&curator_kp], &[], false)?
    );
    Ok(())
}

fn run_price_opaque(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::PriceOpaque {
        delegate,
        slot,
        value,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_price_opaque")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let delegate_kp = load_kp(&delegate)?;
    let ix = PriceOpaque {
        signer: delegate_kp.pubkey(),
        vault: addr(&st.vault)?,
        slot,
        value,
    }
    .into_instruction();
    println!(
        "priced slot {slot} = {value} ({})",
        send_opts(url, &[ix], &[&delegate_kp], &[], false)?
    );
    Ok(())
}

fn run_init_accounts(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::InitAccounts { user, curator } = command else {
        unreachable!("dispatch passed the wrong command to run_init_accounts")
    };
    let url = ctx.url;

    let mut st = load_state(ctx.state_path)?;
    let user_kp = load_kp(&user)?;
    let curator_kp = load_kp(&curator)?;
    let user_pk = user_kp.pubkey();
    let authority = addr(&st.authority)?;
    let base_mint = addr(&st.base_mint)?;
    let share = addr(&st.share_mint)?;
    // Asset mints may be classic SPL or Token-2022; the share mint is
    // always Token-2022 (the program creates it).
    let base_token_program = mint_token_program(url, &base_mint)?;
    let token_2022 = addr(TOKEN_2022)?;

    st.user = user_pk.to_string();
    let user_base = create_ata(url, &user_kp, user_pk, base_mint, base_token_program)?;
    create_ata(url, &user_kp, user_pk, share, token_2022)?;
    create_ata(url, &curator_kp, authority, base_mint, base_token_program)?;
    // Base-only vaults (empty other asset) have no second custody ATA.
    if !st.other_mint.is_empty() {
        let other_mint = addr(&st.other_mint)?;
        let other_token_program = mint_token_program(url, &other_mint)?;
        st.vault_other =
            create_ata(url, &curator_kp, authority, other_mint, other_token_program)?.to_string();
    }
    save_state(ctx.state_path, &st)?;
    println!("accounts created (fund {user_base} with the base asset before subscribe)");
    Ok(())
}

fn run_subscribe(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::Subscribe {
        user,
        amount,
        min_shares_out,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_subscribe")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let user_kp = load_kp(&user)?;
    let user_pk = user_kp.pubkey();
    let base_mint = addr(&st.base_mint)?;
    let share = addr(&st.share_mint)?;
    let authority = addr(&st.authority)?;
    let base_token_program = mint_token_program(url, &base_mint)?;
    let token_2022 = addr(TOKEN_2022)?;
    // Deposit share-math needs AUM fresh within the vault's configured window
    // (60s). `validate_aum` is permissionless, so carry our own
    // freshness in the same tx instead of racing the keeper's cycle.
    let validate = validate_ixs(&st, url, user_pk)?;
    let ix = SubscribeDirect {
        user: user_pk,
        user_token_account: derive_associated_token_account(
            &user_pk,
            &base_mint,
            &base_token_program,
        ),
        vault: addr(&st.vault)?,
        base_mint,
        vault_token_account: derive_associated_token_account(
            &authority,
            &base_mint,
            &base_token_program,
        ),
        user_share_account: derive_associated_token_account(&user_pk, &share, &token_2022),
        base_token_program,
        amount,
        min_shares_out,
    }
    .into_instruction();
    // The program creates the vault base TA + user share TA if missing.
    // subscribe's own deposit pricing re-gates the cached reading
    // against the asset row's `max_staleness_seconds` (60s unless the
    // curator widened it). The canonical pushers' updates are already
    // 20-30s old when they land, so the state files point at the
    // curator bot's self-posted accounts and we fire the moment its
    // next tick posts (the bot must be live).
    wait_fresh_oracles(url, &st.oracles()?)?;
    let ixs: Vec<Instruction> = [validate, vec![ix]].concat();
    println!(
        "subscribed {amount} ({})",
        send_opts(url, &ixs, &[&user_kp], &[], true)?
    );
    Ok(())
}

fn run_redeem_direct(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::RedeemDirect {
        user,
        shares,
        min_out,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_redeem_direct")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let user_kp = load_kp(&user)?;
    let user_pk = user_kp.pubkey();
    let base_mint = addr(&st.base_mint)?;
    let share = addr(&st.share_mint)?;
    let curator = addr(&st.curator)?;
    let authority = addr(&st.authority)?;
    let base_token_program = mint_token_program(url, &base_mint)?;
    let token_2022 = addr(TOKEN_2022)?;
    let ix = RedeemDirect {
        user: user_pk,
        user_token_account: derive_associated_token_account(
            &user_pk,
            &base_mint,
            &base_token_program,
        ),
        user_share_account: derive_associated_token_account(&user_pk, &share, &token_2022),
        vault: addr(&st.vault)?,
        base_mint,
        vault_token_account: derive_associated_token_account(
            &authority,
            &base_mint,
            &base_token_program,
        ),
        curator_share_account: derive_associated_token_account(&curator, &share, &token_2022),
        base_token_program,
        shares,
        min_out,
    }
    .into_instruction();
    wait_fresh_oracles(url, &st.oracles()?)?;
    let ixs = [
        vec![create_ata_ix(user_pk, curator, share, token_2022)],
        validate_ixs(&st, url, user_pk)?,
        vec![ix],
    ]
    .concat();
    println!(
        "redeemed {shares} shares instantly ({})",
        send_opts(url, &ixs, &[&user_kp], &[], true)?
    );
    Ok(())
}

fn run_validate_aum(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::ValidateAum { curator } = command else {
        unreachable!("dispatch passed the wrong command to run_validate_aum")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let curator_kp = load_kp(&curator)?;
    // One custody account per PRICE_IN_AUM asset in slot order, and the
    // paired refresh carries the prices they are valued at.
    let ixs = validate_ixs(&st, url, curator_kp.pubkey())?;
    println!(
        "aum validated ({})",
        send_opts(url, &ixs, &[&curator_kp], &[], false)?
    );
    Ok(())
}

fn run_update_vault(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::UpdateVault {
        curator,
        offerbook_limits,
        timelock_delay,
        paused,
        pending_curator,
        management_fee_bps,
        performance_fee_bps,
        add_asset_mint,
        add_asset_feed_id,
        add_asset_chainlink,
        add_asset_report_decimals,
        add_asset_deposit_enabled,
        add_position,
        remove_asset_index,
        remove_asset_mint,
        remove_position_index,
        retire_debt,
        asset_index,
        max_deposit,
        max_deployed,
        asset_deployed,
        asset_max_staleness,
        asset_max_future_skew,
        asset_deposit_enabled,
        asset_report_decimals,
        asset_quote_feed_id,
        asset_quote_oracle: asset_quote_account,
        asset_quote_report_decimals,
        position_index,
        position_reset,
        max_flash_liability,
        sink_haircut_bps,
        sink_max_staleness,
        max_aum_staleness,
        sink_bootstrap_cap_bps,
        sink_capacity,
        sink_release,
        ccip_lane,
        nest_route,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_update_vault")
    };
    let url = ctx.url;

    let mut st = load_state(ctx.state_path)?;
    let curator_kp = load_kp(&curator)?;
    let pending_curator = pending_curator.map(|s| addr(&s)).transpose()?;
    let add_asset = add_asset_mint
        .map(|m| -> Result<AssetAddFields> {
            Ok(AssetAddFields {
                mint: addr(&m)?,
                // No feed = hand-priced (the program derives the flag).
                oracle: match &add_asset_feed_id {
                    Some(f) => addr(f)?,
                    None => Address::default(),
                },
                chainlink: add_asset_chainlink,
                report_decimals: add_asset_report_decimals.unwrap_or(0),
                deposit_enabled: add_asset_deposit_enabled,
                // 0 = unlimited; narrow later via --asset-index caps.
                max_deployed: 0,
                max_deposit: 0,
            })
        })
        .transpose()?;
    let add_position = add_position
        .as_deref()
        .map(parse_position)
        .transpose()?
        .map(|(slot, p)| PositionAdd {
            index: slot,
            protocol: p.protocol,
            flags: p.flags,
            max_staleness_seconds: p.max_staleness_seconds,
            max_drift_bps: p.max_drift_bps,
        });
    let remove_asset = match (remove_asset_index, &remove_asset_mint) {
        (Some(index), Some(mint)) => {
            let mint = addr(mint)?;
            let authority = derive_vault_authority(&addr(&st.vault)?);
            let tp = mint_token_program(url, &mint)?;
            Some(earn_vaults_sdk::instructions::RemoveAssetFields {
                index,
                custody: derive_associated_token_account(&authority, &mint, &tp),
            })
        }
        _ => None,
    };
    // Protocol policies are their own accounts now: each flag becomes a
    // `SetProtocolPolicy` in the same transaction; `disabled` closes it.
    let (curator_key, vault_key) = (curator_kp.pubkey(), addr(&st.vault)?);
    let mut ixs = Vec::new();
    if let Some(spec) = offerbook_limits.as_deref() {
        let limits = parse_offerbook_limits(spec)?;
        let limits = limits.enabled().then_some(limits);
        ixs.push(
            SetProtocolPolicy::protocol_wide(curator_key, vault_key, Protocol::Offerbook, limits)
                .into_instruction(),
        );
    }
    if let Some(spec) = ccip_lane.as_deref() {
        let lane = parse_ccip_lane(spec)?;
        let lane = lane.enabled().then_some(lane);
        ixs.push(
            SetProtocolPolicy::protocol_wide(curator_key, vault_key, Protocol::Ccip, lane)
                .into_instruction(),
        );
    }
    if let Some(spec) = nest_route.as_deref() {
        let route = parse_nest_route(spec)?;
        let route = route.enabled().then_some(route);
        ixs.push(
            SetProtocolPolicy::protocol_wide(curator_key, vault_key, Protocol::Nest, route)
                .into_instruction(),
        );
    }
    let asset_quote_oracle = asset_quote_feed_id.as_deref().map(addr).transpose()?;
    // A composed row's quote account must land in state, or every later
    // refresh ships one leg and the row goes stale.
    let quote_update = match (asset_index, asset_quote_oracle) {
        (Some(index), Some(feed)) if feed == Address::default() => Some((index, None)),
        (Some(index), Some(_)) => {
            let account = asset_quote_account
                .ok_or_else(|| anyhow!("--asset-quote-feed-id needs --asset-quote-oracle"))?;
            addr(&account)?;
            Some((index, Some(account)))
        }
        _ => None,
    };
    let ix = UpdateVault {
        add_asset,
        add_position,
        remove_asset,
        remove_position: remove_position_index,
        retire_debt,
        max_flash_liability,
        sink_haircut_bps,
        sink_max_staleness_seconds: sink_max_staleness,
        max_aum_staleness_seconds: max_aum_staleness,
        sink_bootstrap_cap_bps,
        sink_capacity,
        sink_release,
        curator: curator_kp.pubkey(),
        vault: addr(&st.vault)?,
        paused,
        timelock_delay,
        pending_curator,
        management_fee_bps,
        performance_fee_bps,
        asset_update: asset_index.map(|index| AssetUpdateFields {
            report_decimals: asset_report_decimals,
            quote_oracle: asset_quote_oracle,
            quote_report_decimals: asset_quote_report_decimals,
            index,
            max_staleness_seconds: asset_max_staleness,
            max_future_skew_seconds: asset_max_future_skew,
            deposit_enabled: asset_deposit_enabled,
            max_deployed,
            deployed: asset_deployed,
            max_deposit,
            ..Default::default()
        }),
        position_update: position_index.map(|index| PositionUpdateFields {
            index,
            reset: position_reset.then_some(true),
            ..Default::default()
        }),
        ..Default::default()
    }
    .into_instruction();
    ixs.push(ix);
    println!(
        "vault updated ({})",
        send_opts(url, &ixs, &[&curator_kp], &[], false)?
    );
    if let Some((index, account)) = quote_update {
        match account {
            Some(a) => st.quote_oracles.insert(index, a),
            None => st.quote_oracles.remove(&index),
        };
        save_state(ctx.state_path, &st)?;
    }
    Ok(())
}

fn run_update_share_meta(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::UpdateShareMeta {
        curator,
        name,
        symbol,
        uri,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_update_share_meta")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let curator_kp = load_kp(&curator)?;
    let ix = UpdateShareMeta {
        curator: curator_kp.pubkey(),
        vault: addr(&st.vault)?,
        name: name.as_deref(),
        symbol: symbol.as_deref(),
        uri: uri.as_deref(),
    }
    .into_instruction()
    .map_err(|e| anyhow!("{e}"))?;
    println!(
        "share metadata updated ({})",
        send_opts(url, &[ix], &[&curator_kp], &[], false)?
    );
    Ok(())
}

fn run_crystallize(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::Crystallize { signer } = command else {
        unreachable!("dispatch passed the wrong command to run_crystallize")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let signer_kp = load_kp(&signer)?;
    let signer_pk = signer_kp.pubkey();
    let ix = CrystallizeFees {
        signer: signer_pk,
        vault: addr(&st.vault)?,
    }
    .into_instruction();
    // The fee shares price off `Vault.aum`, gated at 60s — carry our own
    // refresh + validate rather than racing the keeper's cycle, same as
    // `subscribe`. The program creates missing fee PDAs with this signer as
    // payer, so off-chain callers do not have to provision destinations.
    wait_fresh_oracles(url, &st.oracles()?)?;
    let ixs = [validate_ixs(&st, url, signer_pk)?, vec![ix]].concat();
    println!(
        "fees crystallized ({})",
        send_opts(url, &ixs, &[&signer_kp], &[], true)?
    );
    Ok(())
}

fn run_claim_fees(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::ClaimFees {
        signer,
        destination,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_claim_fees")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let signer_kp = load_kp(&signer)?;
    let ix = ClaimFees {
        signer: signer_kp.pubkey(),
        vault: addr(&st.vault)?,
        destination: addr(&destination)?,
    }
    .into_instruction();
    println!(
        "fees claimed ({})",
        send_opts(url, &[ix], &[&signer_kp], &[], false)?
    );
    Ok(())
}

fn run_protocol_cpi(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::ProtocolCpi {
        delegate,
        route_file,
        create_ata,
        luts,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_protocol_cpi")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let delegate_kp = load_kp(&delegate)?;
    let route: Route = serde_json::from_slice(&std::fs::read(&route_file)?)?;
    let forwarded = route
        .accounts
        .iter()
        .map(|a| {
            let pk = addr(&a.pubkey)?;
            Ok(if a.is_writable {
                AccountMeta::new(pk, a.is_signer)
            } else {
                AccountMeta::new_readonly(pk, a.is_signer)
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let vault = addr(&st.vault)?;
    let authority = derive_vault_authority(&vault);
    let mut ixs = Vec::new();
    for mint in &create_ata {
        let mint = addr(mint)?;
        let tp = mint_token_program(url, &mint)?;
        println!(
            "  ata {} (create-idempotent)",
            derive_associated_token_account(&authority, &mint, &tp)
        );
        ixs.push(create_ata_ix(delegate_kp.pubkey(), authority, mint, tp));
    }
    ixs.push(
        ProtocolCpi {
            signer: delegate_kp.pubkey(),
            vault,
            target_program: addr(&route.target_program)?,
            forwarded,
            cpi_data: B64.decode(route.data_base64.as_bytes())?,
        }
        .into_instruction(),
    );
    let luts = luts.iter().map(|l| addr(l)).collect::<Result<Vec<_>>>()?;
    println!(
        "protocol cpi ({})",
        send_opts(url, &ixs, &[&delegate_kp], &luts, false)?
    );
    Ok(())
}

fn run_redeem(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::Redeem {
        user,
        shares,
        nonce,
        min_out,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_redeem")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let user_kp = load_kp(&user)?;
    let ix = Redeem {
        user: user_kp.pubkey(),
        user_share_account: st.user_share_ata(&user_kp.pubkey())?,
        vault: addr(&st.vault)?,
        shares,
        nonce,
        min_out,
        payout_mint: addr(&st.base_mint)?,
    }
    .into_instruction();
    println!(
        "redeemed {shares} ({})",
        send_opts(url, &[ix], &[&user_kp], &[], false)?
    );
    Ok(())
}

fn run_fulfill_redeem(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::FulfillRedeem {
        curator,
        user,
        nonce,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_fulfill_redeem")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let curator_kp = load_kp(&curator)?;
    // First fill requires now == vault.last_priced_at, so reprice the AUM
    // in the same tx (same Clock) before fulfilling.
    let validate = validate_ixs(&st, url, curator_kp.pubkey())?;
    // Which redeemer to settle is a real input, not derivable — but an
    // empty `st.user` (no `init-accounts` on this state file) would
    // otherwise surface as `bad address :`.
    let redeemer = user.as_deref().unwrap_or(&st.user);
    if redeemer.is_empty() {
        bail!("no redeemer — pass --user <pubkey> (state file records none)");
    }
    let ix = FulfillRedeem {
        authority: curator_kp.pubkey(),
        vault: addr(&st.vault)?,
        user: addr(redeemer)?,
        base_mint: addr(&st.base_mint)?,
        vault_token_account: st.vault_base_ata(url)?,
        token_program: mint_token_program(url, &addr(&st.base_mint)?)?,
        nonce,
    }
    .into_instruction();
    println!(
        "fulfilled ({})",
        send_opts(
            url,
            &[validate.as_slice(), &[ix]].concat(),
            &[&curator_kp],
            &[],
            false,
        )?
    );
    Ok(())
}

fn run_claim_redeem(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::ClaimRedeem { user, nonce } = command else {
        unreachable!("dispatch passed the wrong command to run_claim_redeem")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let user_kp = load_kp(&user)?;
    let ix = ClaimRedeem {
        user: user_kp.pubkey(),
        vault: addr(&st.vault)?,
        user_token_account: st.user_base_ata(url, &user_kp.pubkey())?,
        base_mint: addr(&st.base_mint)?,
        token_program: mint_token_program(url, &addr(&st.base_mint)?)?,
        nonce,
    }
    .into_instruction();
    println!(
        "claimed ({})",
        send_opts(url, &[ix], &[&user_kp], &[], false)?
    );
    Ok(())
}

fn run_cancel_redeem(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::CancelRedeem {
        user,
        nonce,
        partial,
    } = command
    else {
        unreachable!("dispatch passed the wrong command to run_cancel_redeem")
    };
    let url = ctx.url;

    let st = load_state(ctx.state_path)?;
    let user_kp = load_kp(&user)?;
    let user_pk = user_kp.pubkey();
    // The partial legs settle the base proceeds already escrowed; a
    // never-filled redeem has no escrow token account to sweep.
    let partial = if partial {
        let base_mint = addr(&st.base_mint)?;
        Some(CancelPartialRedeem {
            user_token_account: st.user_base_ata(url, &user_pk)?,
            base_mint,
            token_program: mint_token_program(url, &base_mint)?,
        })
    } else {
        None
    };
    let ix = CancelRedeem {
        user: user_pk,
        vault: addr(&st.vault)?,
        user_share_account: st.user_share_ata(&user_pk)?,
        nonce,
        partial,
    }
    .into_instruction();
    println!(
        "redeem cancelled ({})",
        send_opts(url, &[ix], &[&user_kp], &[], false)?
    );
    Ok(())
}

fn run_accept_curator(ctx: CommandContext<'_>, command: Cmd) -> Result<()> {
    let Cmd::AcceptCurator { new_curator } = command else {
        unreachable!("dispatch passed the wrong command to run_accept_curator")
    };
    let url = ctx.url;

    let mut st = load_state(ctx.state_path)?;
    let new_curator_kp = load_kp(&new_curator)?;
    let ix = AcceptCurator {
        new_curator: new_curator_kp.pubkey(),
        vault: addr(&st.vault)?,
    }
    .into_instruction();
    let sig = send_opts(url, &[ix], &[&new_curator_kp], &[], false)?;
    // The state file's `curator` feeds subsequent curator operations and the
    // instant-redeem fee destination, so it must follow the on-chain rotation.
    st.curator = new_curator_kp.pubkey().to_string();
    save_state(ctx.state_path, &st)?;
    println!("curator is now {} ({sig})", st.curator);
    Ok(())
}

fn run_fluid(command: Cmd) -> Result<()> {
    let Cmd::Fluid { ix } = command else {
        unreachable!("dispatch passed the wrong command to run_fluid")
    };

    let route = match ix {
        FluidIx::Operate {
            accounts,
            out,
            new_col,
            new_debt,
            transfer_type,
            remaining_indices,
            remaining_accounts,
        } => {
            let transfer = transfer_type
                .as_deref()
                .map(fluid::TransferType::parse)
                .transpose()?;
            let remaining = remaining_indices
                .split(',')
                .map(|b| b.trim().parse::<u8>().map_err(|e| anyhow!("{e}")))
                .collect::<Result<Vec<u8>>>()?;
            let extra = remaining_accounts.as_ref().map(read_pubkeys).transpose()?;
            let data = fluid::operate_data(new_col, new_debt, transfer, &remaining);
            let mut route = fluid::build_route(&fluid::OPERATE, data, &read_pubkeys(&accounts)?)?;
            fluid::append_operate_remaining(&mut route, &remaining, &extra.unwrap_or_default())?;
            (route, out)
        }
        FluidIx::InitPosition {
            accounts,
            out,
            vault_id,
            next_position_id,
        } => {
            let data = fluid::init_position_data(vault_id, next_position_id);
            fluid::build_route(&fluid::INIT_POSITION, data, &read_pubkeys(&accounts)?)
                .map(|r| (r, out))?
        }
        FluidIx::Borrow {
            accounts,
            out,
            amount,
        } => {
            let data = fluid::flashloan_data(&fluid::FLASHLOAN_BORROW, amount);
            fluid::build_route(&fluid::FLASHLOAN_BORROW, data, &read_pubkeys(&accounts)?)
                .map(|r| (r, out))?
        }
        FluidIx::Payback {
            accounts,
            out,
            amount,
        } => {
            let data = fluid::flashloan_data(&fluid::FLASHLOAN_PAYBACK, amount);
            fluid::build_route(&fluid::FLASHLOAN_PAYBACK, data, &read_pubkeys(&accounts)?)
                .map(|r| (r, out))?
        }
    };
    write_route(&route.1, route.0)?;
    Ok(())
}

#[derive(Deserialize)]
struct Route {
    target_program: String,
    data_base64: String,
    accounts: Vec<RouteAcct>,
}

#[derive(Deserialize)]
struct RouteAcct {
    pubkey: String,
    is_signer: bool,
    is_writable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_cli_accepts_addresses_and_rpc_without_state_or_keypair() {
        let vault = Address::new_from_array([1; 32]);
        let user = Address::new_from_array([2; 32]);
        for investor in [None, Some(user)] {
            let mut args = vec!["earn-vaults".to_string(), "show".into(), vault.to_string()];
            if let Some(user) = investor {
                args.extend(["--user".into(), user.to_string()]);
            }
            args.extend(["--rpc".into(), "http://localhost:8899".into()]);
            let cli = Cli::try_parse_from(args).unwrap();
            assert_eq!(cli.rpc, "http://localhost:8899");
            assert!(
                matches!(cli.cmd, Cmd::Show { vault: v, user: u } if v == vault && u == investor)
            );
        }
        assert!(Cli::try_parse_from(["earn-vaults", "show"]).is_err());
        assert!(Cli::try_parse_from(["earn-vaults", "show", "invalid"]).is_err());
        assert!(
            Cli::try_parse_from([
                "earn-vaults",
                "show",
                &vault.to_string(),
                "--user",
                "invalid",
            ])
            .is_err()
        );
    }

    #[test]
    fn offerbook_spec_validates_bounds_for_update() {
        let spec = "900:86400:604800:300:any";
        let limits = parse_offerbook_limits(spec).unwrap();
        assert_eq!(limits.max_apy_bps.get(), 900);
        assert!(!parse_offerbook_limits("disabled").unwrap().enabled());
        for invalid in [
            "",
            "900:86400",
            "0:86400:604800:300:any",
            "900:0:604800:300:any",
        ] {
            assert!(parse_offerbook_limits(invalid).is_err());
        }
    }

    /// Every narrowed field must reject an out-of-range value instead of
    /// wrapping it into a different, valid-looking one.
    #[test]
    fn position_spec_rejects_out_of_range_fields() {
        let ok = "0:0:9:86400:200";
        let (slot, p) = parse_position(ok).unwrap();
        assert_eq!((slot, p.protocol, p.flags), (0, 0, 9));
        assert_eq!(p.max_drift_bps.get().map(u16::from), Some(200));

        // Empty window fields ask for the program's default; `0` for the
        // loosest setting. The two must not collapse into the same wire value.
        let (_, d) = parse_position("0:0:9::").unwrap();
        assert_eq!(d.max_staleness_seconds.get(), None);
        assert_eq!(d.max_drift_bps.get(), None);
        let (_, w) = parse_position("0:0:9:0:0").unwrap();
        assert_eq!(w.max_staleness_seconds.get().map(u32::from), Some(0));
        assert_eq!(w.max_drift_bps.get().map(u16::from), Some(0));

        for (spec, what) in [
            ("256:0:9:86400:200", "slot"),
            ("0:256:9:86400:200", "protocol"),
            ("0:0:256:86400:200", "flags"),
            ("0:0:9:86400:65536", "drift"),
        ] {
            let err = match parse_position(spec) {
                Err(e) => e.to_string(),
                Ok((slot, p)) => panic!("{spec} wrapped to slot {slot} protocol {}", p.protocol),
            };
            assert!(
                err.contains(what),
                "{spec} failed on the wrong field: {err}"
            );
        }
    }

    #[test]
    fn state_json_schema_uses_current_asset_names() {
        let legacy = json!({
            "created_at": 123,
            "curator": "curator",
            "vault": "vault",
            "share_mint": "share",
            "authority": "authority",
            "base_mint": "base-mint",
            "other_mint": "other-mint",
            "base_oracle": "base-oracle",
            "other_oracle": "other-oracle",
            "user": "user",
            "user_base": "user-base",
            "user_share": "user-share",
            "vault_base": "vault-base",
            "vault_other": "vault-other",
        });

        let state: State = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(state.base_mint, "base-mint");
        assert_eq!(state.other_mint, "other-mint");
        assert_eq!(state.base_oracle, "base-oracle");
        assert_eq!(state.other_oracle, "other-oracle");
        assert_eq!(state.vault_other, "vault-other");

        let encoded = serde_json::to_value(state).unwrap();
        let mut current = legacy;
        for key in ["created_at", "user_base", "user_share", "vault_base"] {
            current.as_object_mut().unwrap().remove(key);
        }
        assert_eq!(encoded, current);
        assert!(encoded.get("usdc_mint").is_none());
        assert!(encoded.get("cbbtc_mint").is_none());
    }

    #[test]
    fn oracle_timestamp_decodes_ocr2_and_rejects_substituted_owners() {
        let data = include_bytes!("../../interface/tests/fixtures/ocr2_susdai_usdai.bin");
        assert_eq!(
            oracle_timestamp(&earn_vaults_sdk::constants::CHAINLINK_OCR2_STORE_ID, data).unwrap(),
            1_791_372_630
        );
        assert!(oracle_timestamp(&Address::default(), data).is_err());
        assert!(
            oracle_timestamp(
                &earn_vaults_sdk::constants::CHAINLINK_OCR2_STORE_ID,
                &data[..247]
            )
            .is_err()
        );
    }

    #[test]
    fn state_oracles_carry_quote_legs() {
        let (base, other, quote) = (
            Address::new_from_array([1; 32]).to_string(),
            Address::new_from_array([2; 32]).to_string(),
            Address::new_from_array([3; 32]).to_string(),
        );
        let mut state = State {
            base_oracle: base.clone(),
            other_oracle: other.clone(),
            vault_other: "vault-other".into(),
            ..Default::default()
        };
        // A shared feed is sent once; a distinct quote account is appended.
        state.quote_oracles.insert(0, other.clone());
        state.quote_oracles.insert(1, quote.clone());
        let oracles: Vec<String> = state
            .oracles()
            .unwrap()
            .iter()
            .map(|a| a.to_string())
            .collect();
        assert_eq!(oracles, [base, other, quote]);
    }

    #[test]
    fn init_vault_cli_defaults_are_stable() {
        let cli =
            Cli::try_parse_from(["vault", "init-vault", "--curator", "curator.json"]).unwrap();

        assert_eq!(cli.rpc, "https://api.mainnet-beta.solana.com");
        assert_eq!(cli.state, PathBuf::from("vault-state.json"));
        assert_eq!(cli.priority_fee, 0);
        assert_eq!(cli.compute_limit, 0);
        let Cmd::InitVault {
            curator,
            base_mint,
            base_oracle,
            base_feed_id,
            other_mint,
            other_oracle,
            positions,
            timelock_delay,
            management_fee_bps,
            performance_fee_bps,
            instant_redeem_fee_bps,
            ..
        } = cli.cmd
        else {
            panic!("parsed the wrong command")
        };
        assert_eq!(curator, PathBuf::from("curator.json"));
        assert_eq!(base_mint, DEFAULT_BASE_MINT);
        assert_eq!(base_oracle, DEFAULT_BASE_ORACLE);
        assert_eq!(base_feed_id, DEFAULT_BASE_FEED_ID);
        assert!(other_mint.is_none());
        assert!(other_oracle.is_none());
        assert!(positions.is_empty());
        assert!(timelock_delay.is_none());
        assert!(management_fee_bps.is_none());
        assert!(performance_fee_bps.is_none());
        assert!(instant_redeem_fee_bps.is_none());
    }

    #[test]
    fn init_vault_cli_requires_other_feed_id() {
        assert!(
            Cli::try_parse_from([
                "vault",
                "init-vault",
                "--curator",
                "curator.json",
                "--other-mint",
                "mint",
                "--other-oracle",
                "oracle",
            ])
            .is_err()
        );

        assert!(
            Cli::try_parse_from([
                "vault",
                "init-vault",
                "--curator",
                "curator.json",
                "--other-mint",
                "mint",
                "--other-oracle",
                "oracle",
                "--other-feed-id",
                "feed",
            ])
            .is_ok()
        );
    }

    #[test]
    fn claim_fees_cli_accepts_signer_and_destination() {
        let cli = Cli::try_parse_from([
            "vault",
            "claim-fees",
            "--signer",
            "claimant.json",
            "--destination",
            "destination-token-account",
        ])
        .unwrap();

        let Cmd::ClaimFees {
            signer,
            destination,
        } = cli.cmd
        else {
            panic!("parsed the wrong command")
        };
        assert_eq!(signer, PathBuf::from("claimant.json"));
        assert_eq!(destination, "destination-token-account");
    }

    #[test]
    fn protocol_cpi_cli_keeps_repeatable_and_global_arguments() {
        let cli = Cli::try_parse_from([
            "vault",
            "--state",
            "custom-state.json",
            "--priority-fee",
            "9",
            "protocol-cpi",
            "--delegate",
            "delegate.json",
            "--route-file",
            "route.json",
            "--create-ata",
            "mint-a",
            "--create-ata",
            "mint-b",
            "--lut",
            "lut-a",
            "--lut",
            "lut-b",
        ])
        .unwrap();

        assert_eq!(cli.state, PathBuf::from("custom-state.json"));
        assert_eq!(cli.priority_fee, 9);
        let Cmd::ProtocolCpi {
            delegate,
            route_file,
            create_ata,
            luts,
        } = cli.cmd
        else {
            panic!("parsed the wrong command")
        };
        assert_eq!(delegate, PathBuf::from("delegate.json"));
        assert_eq!(route_file, PathBuf::from("route.json"));
        assert_eq!(create_ata, ["mint-a", "mint-b"]);
        assert_eq!(luts, ["lut-a", "lut-b"]);
    }

    #[test]
    fn update_vault_cli_enforces_field_dependencies() {
        let incomplete: &[&[&str]] = &[
            &[
                "vault",
                "update-vault",
                "--curator",
                "curator.json",
                "--max-deposit",
                "1",
            ],
            &[
                "vault",
                "update-vault",
                "--curator",
                "curator.json",
                "--asset-deployed",
                "0",
            ],
            &[
                "vault",
                "update-vault",
                "--curator",
                "curator.json",
                "--position-reset",
            ],
            &[
                "vault",
                "update-vault",
                "--curator",
                "curator.json",
                "--add-asset-chainlink",
            ],
            &[
                "vault",
                "update-vault",
                "--curator",
                "curator.json",
                "--remove-asset-index",
                "1",
            ],
            &[
                "vault",
                "update-vault",
                "--curator",
                "curator.json",
                "--remove-asset-mint",
                "mint",
            ],
        ];
        for args in incomplete {
            let Err(error) = Cli::try_parse_from(*args) else {
                panic!("dependency-free arguments unexpectedly parsed: {args:?}")
            };
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::MissingRequiredArgument,
                "unexpected parse failure for {args:?}: {error}"
            );
        }

        let cli = Cli::try_parse_from([
            "vault",
            "update-vault",
            "--curator",
            "curator.json",
            "--asset-index",
            "2",
            "--max-deposit",
            "100",
            "--asset-deployed",
            "0",
            "--position-index",
            "3",
            "--position-reset",
            "--add-asset-mint",
            "new-mint",
            "--add-asset-feed-id",
            "new-feed",
            "--add-asset-chainlink",
            "--remove-asset-index",
            "4",
            "--remove-asset-mint",
            "old-mint",
            "--management-fee-bps",
            "125",
            "--performance-fee-bps",
            "750",
            "--retire-debt",
            "0",
        ])
        .unwrap();
        let Cmd::UpdateVault {
            asset_index,
            max_deposit,
            asset_deployed,
            position_index,
            position_reset,
            add_asset_mint,
            add_asset_feed_id,
            add_asset_chainlink,
            remove_asset_index,
            remove_asset_mint,
            management_fee_bps,
            performance_fee_bps,
            retire_debt,
            ..
        } = cli.cmd
        else {
            panic!("parsed the wrong command")
        };
        assert_eq!(asset_index, Some(2));
        assert_eq!(max_deposit, Some(100));
        assert_eq!(asset_deployed, Some(0));
        assert_eq!(position_index, Some(3));
        assert!(position_reset);
        assert_eq!(add_asset_mint.as_deref(), Some("new-mint"));
        assert_eq!(add_asset_feed_id.as_deref(), Some("new-feed"));
        assert!(add_asset_chainlink);
        assert_eq!(remove_asset_index, Some(4));
        assert_eq!(remove_asset_mint.as_deref(), Some("old-mint"));
        assert_eq!(management_fee_bps, Some(125));
        assert_eq!(performance_fee_bps, Some(750));
        assert_eq!(retire_debt, Some(0));
    }
}
