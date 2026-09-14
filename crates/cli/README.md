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
