# Emergency pause through the upgrade multisig

The program's current upgrade authority can add pause flags to any vault with
`SetPaused`. For Squads, the signer is the Squads vault PDA holding the upgrade
authority, not the multisig account or a member wallet. No delegate grant or
global pause account is needed.

Export an unsigned instruction offline:

```sh
cargo run -q -p earn-vaults-cli --bin earn-vaults -- \
  export-pause --authority SQUADS_VAULT_PDA --vault VAULT_ADDRESS > pause.json
```

The command reads no wallet or state file, makes no RPC calls, and submits
nothing. The JSON contains `programId`, ordered `keys` with signer/writable
flags, and base64 `data`. Import these into a custom Rust-program instruction
in Squads TX Builder. The second account is the canonical ProgramData account.
The default payload is raw bytes `19, 7` (base64 `Ewc=`).

Flags are ORed into the existing pause mask: `1` stops deposits, `2` stops
redemption requests and fulfillment, and `4` stops strategy operations. `7`
sets all three. `0` leaves the mask unchanged, but still accrues capital time
and emits an event. Claims and cancellations remain available. Only the vault
curator can resume through `UpdateVault.paused`; share-transfer pauses remain
curator-only.

Add an instruction for each vault to protect, simulate and review the proposal,
then collect approvals. For emergency readiness, approve in advance and let
the timelock elapse. Any member with Execute permission can execute a ready
proposal; review those permissions. A proposal is usable once and covers only
its fixed vault list. Prepare a replacement after execution and new proposals
for newly added vaults. Verify each vault's pause mask afterward.

Revalidate pending proposals after program upgrades, authority rotations or
Squads configuration changes. Authority is checked at execution time. Making
the program immutable removes this emergency path; delegates and curator
controls remain available. Recovery follows the curator's normal signing or
multisig approval process.

SDK: `SetPaused { signer, vault, flags }.into_upgrade_authority_instruction()`.
The existing `.into_instruction()` builds the PAUSE-delegate path.
