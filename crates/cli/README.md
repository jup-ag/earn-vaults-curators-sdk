# earn-vaults CLI

Operate vaults through the Rust SDK and JSON-RPC. Run `earn-vaults --help` for
subcommands, including `subscribe`, `validate-aum`, `redeem`, `fulfill-redeem`,
and `claim-redeem`. Lifecycle commands persist derived addresses in
`vault-state.json` (override with `--state`).

```sh
cargo build --bin earn-vaults
./target/debug/earn-vaults show <vault>
watch -n5 ./target/debug/earn-vaults show <vault>
```

`show` prints the existing `VaultSnapshot` with named fields, assets, positions,
and fees. It reads directly from the vault address, without a state file or
keypair. Add `--user <pubkey>` to include that investor's shares and redeem value.
Use `--rpc <url>` or `SOLANA_RPC_URL` to select the cluster (default: mainnet).

To export an unsigned emergency pause instruction for the upgrade multisig:

```sh
earn-vaults export-pause --authority SQUADS_VAULT_PDA --vault VAULT_ADDRESS
```

This prints JSON without RPC, state-file or keypair access. `--flags` defaults
to `7` (all three pause bits) and accepts `0..=7`. See the
[emergency pause runbook](../../docs/emergency-pause.md) for authorization,
proposal execution and recovery.

To sweep residual custody during teardown, once share supply and pending
redeems are both zero, run the curator-signed command for each listed asset or
written-off sink. The destination must be an existing token account for the mint:

```sh
earn-vaults --rpc RPC_URL sweep-idle \
  --curator CURATOR_KEYPAIR --vault VAULT_PUBKEY --mint ASSET_MINT \
  --destination-token-account DESTINATION_TOKEN_ACCOUNT
```

This command uses the supplied vault address without reading a state file.

To mark an Earn-only position using a delegate's Jupiter Earn PRICE grant:

```sh
earn-vaults --rpc RPC_URL --state vault-state.json price-opaque \
  --delegate DELEGATE_KEYPAIR --earn --slot POSITION_INDEX --value RAW_BASE_VALUE
```

Grant PRICE (`16`) on the Jupiter Earn program
(`jup3YeL8QhtSx1e253b2FDvsMNC87fDrgQZivbrndc9`); add EARN (`8`) if the same
delegate also deposits or withdraws. The slot must be enabled, non-debt, and
lender-only. Omitting `--earn` selects the vault-internal PRICE grant (`1`).
Scoped pricing requires the upgraded program; see the
[compatibility notes](../../docs/compatibility.md).

To mark seized collateral in sink slot 0 (`64 = SINK_TARGET | 0`):

```sh
earn-vaults --rpc RPC_URL --state vault-state.json price-opaque \
  --delegate DELEGATE_KEYPAIR --slot 64 --value RAW_BASE_VALUE \
  --custody CANONICAL_CUSTODY_ATA --expected-balance RAW_TOKEN_BALANCE
```

Use the custody balance that produced the valuation. Both flags are required
for sink pricing and cannot be combined with `--earn`. All pricing payloads now
include the balance field; upgrade clients with the program. Existing vaults
also require a separate account migration; see the compatibility notes above.

`update-vault --nest-route COMPOSER:RELAYER:OFT_STORE:SHARE_MINT` now forwards
the OFT store and receipt mint to bind the route. `--nest-route disabled` closes
a bound route. Management/performance fee updates automatically include the
share mint required by the upgraded program.

Amounts are raw integers: USD values use 6 decimals, token balances use the
asset's `decimals`, and NAV/HWM use the interface's `NAV_SCALE` fixed point.
Timestamps are Unix seconds; `last_priced_at` records the last on-chain pricing,
which can be older than the latest fetch. Fee estimates use the local clock.

For direct settlement, pass `subscribe --min-shares-out <raw-shares>` or
`redeem-direct --min-out <raw-base-tokens>` to bound the output. Both default
to zero (unbounded); use a minimum derived from your quote and slippage
tolerance for protection against execution-time price or fee changes.

Both direct instructions now encode 17 bytes (tag, input, minimum), replacing
the previous 9-byte encoding. Coordinate the program upgrade with every
client: old payloads are rejected by the updated program.

Queued `redeem --min-out <raw-base-tokens>` pins the state file's base mint and
requires the same minimum tokens-per-share rate on every partial fill. The
Rust SDK's queued `Subscribe` accepts `min_shares_out`, enforced at fulfillment.
Zero opts out of the output bound; redemption still pins the payout mint.

Queued request formats also change: `Subscribe` is 25 bytes (tag, amount,
nonce, minimum shares); `Redeem` is 57 bytes (tag, shares, nonce, minimum tokens,
payout mint). `Escrow` retains discriminator 3 and the same PDA, with its
102-byte header followed by `min_out`, `original_shares`, and `payout_mint`
(150 bytes total). Update account consumers for the extended layout.

Set `--queued-redeem-fee-bps 75` on `init-vault` or `update-vault` to charge
0.75% of settled queued shares for the curator. Initialization defaults to zero;
omitting the update flag leaves the rate unchanged, and passing zero disables
it. The allowed range is 0..=1,000 bps. `show` includes the configured rate.
Each fill uses the current rate, including requests queued before a rate change;
`min_out` applies to net proceeds. Fulfillment supplies the curator fee account
automatically, with the delegate paying rent if it needs creation.
