# offerbook-client

`idl.json` is the source for this generated client. From the repository root, regenerate it with:

```sh
cargo run -p anchor-gen -- crates/offerbook-client/idl.json -o crates/offerbook-client --name offerbook-client
cargo +nightly fmt --all
```

The generator preserves this crate's existing `Cargo.toml`.
