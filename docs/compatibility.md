# Compatibility notes

Synced with earn vault program commit `4b20d514b2f40ca871903f441c6c93403944ccd8`.

## Share metadata, CLI transactions and Offerbook (`4b20d51`)

`InitializeVaultArgs.metadata_name` and `UpdateShareMetaArgs.metadata_name` now
hold 50 UTF-8 bytes instead of 25; symbols remain 5 bytes and URIs 200 bytes.
Both instruction payloads grow by 25 bytes. Upgrade the program and rebuild SDK,
CLI and IDL clients together: old payloads are rejected. Existing vault accounts
and share mints need no data migration from `3ea4f47`. Rebuild pending metadata
and initialization transactions; rollback requires matching program and clients.

CLI transactions without lookup tables now use v1 (up to 4096 bytes); supplying
`--lut` retains v0 and its 1232-byte limit. `--priority-fee` remains in
micro-lamports per CU, converted to a total lamport fee for v1. The default v1
budget is 200k CU per instruction, capped at 1.4M, with 64 MiB of loaded account
data. The RPC and cluster must support v1 transactions.

The new `offerbook` SDK module and CLI routes target
[Offerbook `736d809`](https://github.com/jup-ag/offerbook-program/tree/736d809).
`offerbook::SetYieldOperator` names an operator with `YIELD` (64), or clears it
with `ESCROW` (1). It forwards the vault authority, its writable Offerbook User,
event authority and program, without loan policies. The operator can top up and
compound that User's yield directly through Offerbook; revoking a vault delegate
does not revoke the operator. Clear it explicitly with `operator: None`.

`offerbook::ExtendLoan` derives the new 18-account token/token renewal layout,
including collateral accounts and an absent Core asset, then appends the two
vault loan policies. The previous generated 13-account builder must no longer be
used after the Offerbook upgrade. The vault adapter accepts both layouts, but
each Offerbook version rejects the other's: upgrade the vault first, hold
renewals across the Offerbook upgrade, and then switch clients. Extension needs
`BORROW` (8); re-price the position and validate AUM after renewal. No vault
account migration or hook change is needed for these Offerbook routes.

The regenerated `offerbook-client` includes the five new optional fields on
`ExtendLoanAccounts`, operator and fee-profile instructions, and `LoanExtendedV3`.
Its `User` and `Config` types consume reserved bytes without growing; update
Rust struct literals for the new fields and reduced reserves. Event consumers
must recognize `LoanExtendedV3` and its interest/fee amounts.

The audited Offerbook recall also turns yield OFF; re-enable explicitly when
needed. Yield supply requires at least 10 raw atoms, and top-up may deliver up
to 9 extra atoms. Renewals recheck Offerbook's live rate and duration limits,
charge interest and new-period fees, and release surplus collateral after a
paydown. Per-user fee profiles affect valuation; do not assume global fees.
See the [upstream yield guide](https://github.com/jup-ag/earn-vaults-programs/blob/4b20d514b2f40ca871903f441c6c93403944ccd8/docs/OFFERBOOK_ESCROW_YIELD.md)
and [renewal reference](https://github.com/jup-ag/earn-vaults-programs/blob/4b20d514b2f40ca871903f441c6c93403944ccd8/docs/technical-reference.md).
Rollback after the Offerbook upgrade must preserve a vault adapter and renewal
clients compatible with its 18-account layout.

## Upgrade-authority pause and treasury (`3ea4f47`)

The current program upgrade authority can add per-vault pause flags without a
delegate grant. Use `SetPaused::into_upgrade_authority_instruction()` or the
offline CLI `export-pause --authority SQUADS_VAULT_PDA --vault VAULT_ADDRESS`.
The builder replaces the delegate meta with the readonly canonical
`PROGRAM_DATA_PDA`; the signer must be the upgrade authority at execution time.
The interface exports the loader ID, ProgramData PDA and `program_data::upgrade_authority`
header decoder. Callers of the decoder must separately bind the account address
to the expected program.

The existing `SetPaused::into_instruction()` delegate path is unchanged.
Instruction tags, payloads and vault layouts are unchanged from `7c50646`;
existing vaults on that version need no account migration for this update.
Only the curator can clear pause flags through `UpdateVault`. Claims and
cancellations remain available; share-transfer pauses remain curator-only.
See the [emergency pause runbook](emergency-pause.md).

`PROTOCOL_TREASURY` changes from the development wallet to the Squads vault
`7vYZz8z6VAMyVb8SoSBBmyd5MivbgbxJnM6nih8Ms4ix`. `ClaimFees` now selects the
protocol fee PDA for that signer; its destination share account must be owned by
that PDA. Rebuild and deploy the vault program and transfer hook together and
update clients. The constant does not transfer program upgrade authority.
Upstream records bootstrap 1-of-1 treasury governance at this commit; its stated
production target is 3-of-5. Verify live governance before production use.

The new emergency path requires the matching program upgrade. Before rollback,
stop submissions and restore matching program, hook and clients together;
rebuild fee-claim transactions and revalidate pending multisig proposals.

## Previous layout upgrade (`7c50646`)

The `7c50646` update breaks the vault account layout, initialization/update arguments,
event layouts and `PriceOpaque` instruction ABI.
Deploy matching program, SDK and regenerated IDL clients together. Instruction
tags remain unchanged from `179a757`.

- `Vault::LEN` grows from 2,184 to 2,318 bytes, including its discriminator.
  `SinkSlot` grows from 49 to 57 bytes with `balance` after `value`, shifting
  fields after the sink array by 32 bytes. `Vault.nest_receipt_mint` follows
  `max_aum_staleness_seconds`, followed by `queued_redeem_fee_bps: u16`; the
  final reserve is 128 bytes. Old accounts
  are rejected; simply appending zero bytes does not migrate the moved fields.
  The target commit does not provide an account migration instruction. Existing
  vaults need a separate migration before this program/SDK can operate on them.
- Every `PriceOpaque` payload is now 18 bytes, formerly 10: tag, slot, value,
  then `expected_balance: u64` (little-endian). Position and asset builders
  supply zero. For sinks use `into_sink_instruction(custody, expected_balance)`;
  it appends the canonical custody ATA as a readonly account. Sign the balance
  used to calculate the value; a balance change rejects the mark. Re-attest
  sinks after custody changes before AUM validation or liquidation.
- `UpdateVault` appends the readonly share mint for management or performance
  fee changes, after added-asset mint and removed-asset custody accounts.
  The builder derives it automatically. Pending performance must be crystallized
  before changing fee rates.
- Use `SetProtocolPolicy::nest_route` for Nest. Enabled routes append the OFT
  store and receipt mint as readonly proof accounts. The receipt binding survives
  policy closure and excludes that mint from assets and sinks. Existing Nest
  routes must be re-submitted with their proof accounts after migration, before
  admitting/valuing sinks or closing an unbound route. Changing the receipt
  requires exiting and removing the old Nest position first.

Queued redemptions now support `queued_redeem_fee_bps` (0..=1,000, default zero).
Set it on `InitializeVault` or through `UpdateVault`; `Some(0)` disables it and
`None` preserves it on update. Each fill uses the current rate, including for
already queued requests: `floor(settled_shares * bps / 10_000)` shares go to the
curator fee account, collectible with `ClaimFees`. Remaining shares price the
payout, and `min_out` checks net proceeds. Rounding is per fill; cancelled,
unsettled shares pay no fee.

`FulfillRedeem` now makes the share mint writable and appends the writable
curator fee account and readonly Token-2022 program after the vault program,
before asset transfer-hook accounts. The builder supplies these automatically;
the delegate funds fee-account rent if creation is needed. The program still
accepts old fee-free callers, but charging a fee requires the new accounts.
Initialization appends a `u16` fee, updates append a fixed `Option<u16>`,
`InitializeVaultEvent` appends the fee rate, and `RedeemFulfilled` appends
`fee_shares: u64`. Regenerate clients and event decoders. Relative to `a217fc6`,
the account grows from 2,316 to 2,318 bytes; migration must initialize the new
fee to zero and retain the 128-byte reserve. No migration instruction is supplied.

Stop submissions during rollout and rebuild pending transactions with the new
encoding and accounts. Coordinate any account migration separately; rollback
requires compatible account state as well as the previous program and clients.
Do not deploy this SDK against an older program or unmigrated vault accounts.

Manual deposit assets are now supported. Use a zero oracle, clear Chainlink and
quote-feed configuration, and keep instant redemption disabled (`u16::MAX`).
The curator seeds the first positive price with `PriceOpaque` using
`ASSET_TARGET | asset_index`, in numeraire micro-units per whole token. Delegates
can subsequently price within configured limits. Validate AUM before settlement.
No-op oracle updates preserve cached prices and replay watermarks; changing a
feed resets its scale unless that scale is supplied explicitly.

`SubscribeDirect` requires a user-owned immutable Token-2022 share account;
creating it requires the user's canonical ATA. `RedeemDirect` rejects positive
exit fees that round to zero curator shares with `FeeAmountTooSmall`. Redemption
payouts invalidate AUM snapshots. Shared fee/HWM and slippage calculations now
match the program, including preserved slippage debt after AUM shrinks.
`Vault::ratchet_hwm_after_fee_mint` is renamed to `ratchet_hwm_after_mint`.
Clean bootstrap also requires no residual deployed balances, initialized
positions or occupied sinks; a zero mark alone does not clear that exposure.

Offerbook borrower renewals can now route the existing `ExtendLoan` builder via
`ProtocolCpi` with `BORROW` (`1 << 3`). Use the vault authority as borrower and
clear its outer signer flag. Append the loan-limits policy followed by the
collateral-mint policy after the Offerbook accounts (18 at `4b20d51`, previously 13). Current limits, lender
consent and minimum renewal spacing apply; renewal fees require idle principal
liquidity. Re-price the position and validate AUM after renewal. Lender-only
vaults cannot renew as borrower.

CCIP SPL fees use an operator-funded, untracked canonical fee ATA, separate from
the bridged asset. SEND delegates can spend that budget without a per-send fee
cap. Jupiter Lend's unsupported `close_position` route is no longer accepted.

The earlier `179a757` upgrade changed the instruction mapping:

`SetProtocolPolicy` uses instruction tag `27` (previously `28`), and
`PrepareNestRedeem` uses `28` (previously `27`). This is a breaking instruction
ABI change at `179a757`. SDK builders, CLI commands, CPI callers and IDL-based
clients must use the mapping matching the deployed vault program. That tag swap
alone did not change account layouts, PDA seeds or stored policies; the current
update does require account migration.

Stop submissions of these two instructions before upgrading. Rebuild clients
and regenerate IDL-based clients and decoders. Cancel or rebuild pending multisig
proposals and stored transactions containing the old bytes, including
durable-nonce transactions; let ordinary in-flight transactions confirm or expire.
Upgrade the vault program, switch clients and published IDLs together, verify
both operations, then resume submissions. The same tag now selects the other
handler, so do not mix versions. Historical decoders must retain the old mapping
for transactions before the upgrade slot. Rollback requires stopping submissions
again and restoring the previous program, clients and IDL together.

`state::permissions::jupiter_lend::PRICE` (`1 << 4`) permits pricing an enabled,
non-debt, lender-only `Protocol::JupiterLend` position when granted on the
Jupiter Earn program (`jup3YeL8QhtSx1e253b2FDvsMNC87fDrgQZivbrndc9`). Use
`PriceOpaque::into_earn_instruction()` or CLI `price-opaque --earn` to select
that grant. It cannot price other positions, asset rows, or sinks; granting the
bit on Fluid Vaults gives no pricing authority. Drift/bootstrap, Strategy pause,
and flash-liability guards still apply. Existing `into_instruction()` calls
continue to use the vault-internal PRICE grant. Scoped pricing requires the
program upgrade; older programs reject the Earn grant. Review wildcard Earn
permission masks, which now include PRICE.

`max_deployed = 0` blocks new deployment. `max_deposit = 0` blocks deposits,
including queued fulfillment. `u64::MAX` means unlimited and is the default for
omitted SDK initialization caps and CLI asset additions.

Drift and sink-bootstrap budgets use `u16::MAX` to waive the limit. At
initialization or position addition, `None` or `0` uses the global default.
When updating an existing field, `None` leaves it unchanged.

Older programs interpret zero caps and budgets differently. Before upgrading,
replace stored zero caps where deployment or deposits should stay enabled.
Before rolling back, disable the deposit flag where a zero cap must keep
blocking deposits. Older initialization and position-add instructions reject
`u16::MAX` budgets and treat zero as a waiver.

Offerbook yield uses `ProtocolCpi` with the generated `offerbook-client`
instructions. `state::permissions::offerbook::YIELD` (`1 << 6`) permits enable
and supply; `ESCROW` permits disable and recall. These routes require the
program upgrade. Disable yield and recall all shares before rolling back.
See the [escrow-yield guide](https://github.com/jup-ag/earn-vaults-programs/blob/7c5064685d42b9f7c178bb74ba6f7b6f0dddf300/docs/OFFERBOOK_ESCROW_YIELD.md)
for accounts, pricing and operator requirements.

Offerbook principal offers support partial fills through the existing generated
client. The whole offer remains subject to curator loan limits; Offerbook
validates fill sizes. Collateral and interest round down per loan, so tiny fills
can earn zero interest despite a nonzero nominal APY.

`state::permissions::offerbook::RENEW` (`1 << 7`) permits enabling or disabling
borrower renewals through the generated `SetLoanExtendable` builder routed via
`ProtocolCpi`. Use the vault authority as its signer, clear that account meta's
`is_signer` flag for the outer instruction, and forward the four accounts without
policy accounts. `OFFER` alone also permits disabling renewals.
Creating principal offers with `allow_extend = true` requires both `OFFER` and
`RENEW`. Revoking the grant does not clear existing loan consent: disable it on
each loan. Duration limits apply per period, not to the total loan lifetime.
Partial fills and renewal controls require the program upgrade; older adapters
reject these routes. Review wildcard permission masks, which include `RENEW`.
