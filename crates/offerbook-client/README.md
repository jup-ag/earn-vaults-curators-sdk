# offerbook-client

Synced to [Offerbook `736d809`](https://github.com/jup-ag/offerbook-program/tree/736d809)
(audit-2026-11), as used by earn-vaults-programs `4b20d51`.

`idl.json` is the source for this generated client. From the repository root, regenerate it with:

```sh
cargo run -p anchor-gen -- crates/offerbook-client/idl.json -o crates/offerbook-client --name offerbook-client
cargo +nightly fmt --all
```

The generator preserves this crate's existing `Cargo.toml`.

`ExtendLoanAccounts` now requires five optional collateral fields before its
event accounts, producing 18 accounts even when all five are `None`. Use the
matching upgraded Offerbook program; the previous 13-account layout is rejected.
The client includes `SetYieldOperator`, admin-only `SetUserFeeProfile`,
`LoanExtendedV3` and the User operator/fee-profile fields. `User` and `Config`
retain their account sizes by consuming reserved bytes; update struct literals
for the new fields and smaller reserves. See the
[SDK compatibility notes](../../docs/compatibility.md).
