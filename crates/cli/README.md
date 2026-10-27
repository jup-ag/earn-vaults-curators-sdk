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
