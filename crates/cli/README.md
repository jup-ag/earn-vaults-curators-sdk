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

## Escrow migration

Deploy the updated transfer hook first (it accepts both complete layouts),
then the vault program. Run the read-only inventory against your deployment's
RPC; it verifies escrow ownership, layout, and canonical PDA at a finalized
slot and reports every account and the required rent top-up:

```sh
cargo run -p earn-vaults-cli --bin earn-vaults -- escrow-migration-status
```

For each `needs_migration: true` entry, run after the program upgrade:

```sh
cargo run -p earn-vaults-cli --bin earn-vaults -- migrate-escrow <ESCROW> --payer <KEYPAIR.json>
```

Both commands use `SOLANA_RPC_URL` or the global `--rpc` option (mainnet by
default). No vault state file is required. Any payer may migrate an escrow;
only the payer signs. Migration preserves the header byte-for-byte, adds
48 zero bytes, and funds the extra rent without consuming prepaid keeper
rent. Repeating migration on a 150-byte escrow changes neither its limits nor
its balance. The top-up is refundable to the escrow owner when it closes.

Existing requests remain unbounded because no minimum was authorized when
they were submitted. New redeem requests always store their original share
count and selected payout mint, even when `min_out` is zero.

After upgrading, migrate every outstanding 102-byte escrow before fulfill,
claim, or cancel; the SDK's `MigrateEscrow` can be prepended to the same
transaction. This includes fully fulfilled requests awaiting claim and
partially filled redemptions. Owners can instead claim or cancel eligible
requests before upgrading, reducing the migration count. New requests are
created at 150 bytes and need no migration. Re-run the inventory after
upgrading, since requests may be created or closed between scans.

Rollback requires draining/closing **all** 150-byte escrows with the upgraded
program first; do not shrink accounts or reset their limits. Pause `Subscribe`
and `Redeem` on every vault of the program before the final inventory so no
new escrows appear. The pauses also block `Fulfill` and `FulfillRedeem`, so
owners drain the rest by claiming or by cancelling after the timelock. Keep the
compatible hook until those escrows are closed. Older programs reject the
extended layout. Claim and cancellation instruction formats are unchanged.
