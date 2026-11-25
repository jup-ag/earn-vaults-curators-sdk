# Mainnet fixtures

Vault states recorded from mainnet on 2026-11-25, one `getMultipleAccounts`
(commitment `confirmed`) per vault at the slot in each file. The keys are what
`EarnPlan::keys` and `OfferbookPlan::keys` name, plus the share mint and the
custody ATAs. Loans were found with the `offerbook::loan_filters` scans just
before the read. Reads only; nothing was sent.

`bitwise-susdai-multiply.json` was recorded with one JSON-RPC batch of
`getMultipleAccounts` over `FluidPlan::keys` (plus the custody ATAs), the vault
authority's `getTokenAccountsByOwner` under both token programs, and a
`simulateTransaction` of `get_exchange_prices` for Fluid vault 101. All four
answered at slot 473,664,805, two seconds after the USDC row's refresh
(60 s window). `positions` and `unrelated` are the discovery scans by
`fluid::position_filters` taken just before. The earlier fixtures predate
listings; their tests rebuild one from the recorded token accounts the
authority owns. That shows the fold, not that the authority held nothing else.

The vaults run program 0.1.0. Vault accounts have the same 2,334-byte layout on
0.2.0, where 0.2.0's new fields read as zero.

| File | Vault | Positions valued |
|---|---|---|
| `bitwise-juplend-optimizer.json` | `9Cw71wvdfjS5DSLPGVfFrPNMusGh32Xhm2B7R1s6uzNw` | Earn, five markets |
| `bitwise-susdai-yield.json` | `Hj9eXER2esW7FMSX1UjgNk29ToX2rpa4QF6ZCKVvXgdH` | Earn, Offerbook (68 active lent loans, escrow yield) |
| `re7-nopal-lending.json` | `6QDoDjzvgUKaaYBptR9CGebT4WUTKd7tTDRwvgD1jmvD` | Earn (empty), Offerbook (3 lent loans) |
| `re7-nopal-looping.json` | `9EdtkcPbuM8zsyhknzo2CRLyPVXNJXCt6zgSjviMYCNc` | Offerbook (3 borrowed loans); its Nest slot is out of scope |
| `bitwise-susdai-multiply.json` | `bcYAZdziuuaYHAkrT3WP3dLzDUQg7XJVPxYwXUaah5w` | Jupiter Lend: one Fluid T1 position (vault 101, NFT 189), sUSDai collateral, USDC debt |

## `expected`

All values are numéraire micro-units.

- `sdk`: this crate.
- `optimizer`, `curator_api`, `mm_borrowing`, `mm_managed`: the existing Earn
  and Offerbook valuations of the stablecoin optimizer, the curator API and the
  Offerbook market maker, run over the same bytes in a separate harness. The API
  prices Offerbook holdings from live Pyth Pro; the harness gave it the vault's
  cached row prices so the inputs match.
- `*_if_extended_allowed`: the curator API refuses extended loans; this is its
  value with each loan's `extension_count` cleared, to compare the netting.
- `repay_fee_bps`: the fee Offerbook takes from the vault's lent interest, from
  the vault's own Offerbook fee profile or else `Config.repay_fee_bps`.
- `onchain_mark`: the slot's `last_value` at the recorded slot, posted
  `onchain_mark_age_seconds` before it.

Where the numbers differ:

- Lent interest: this crate counts interest from the start of the loan's
  current period (`expired_at − duration`), as Offerbook does, less Offerbook's
  repay fee rounded up. The other copies count from `created_at`, which after an
  extension books the whole period again, and deduct no fee. Both read high.
- sUSDai Yield Offerbook: the API and the market maker's borrowing path also
  leave out the jlUSDC escrow yield the Offerbook `User` holds (about 22,500
  USDC). The managed path counts it.
- RE7 Looping Offerbook: within 2 micro-units of the other copies, which round
  each loan separately. This crate sums per mint, then rounds once.
- Bitwise sUSDai Multiply (`jupiter_lend`): `report_formula` is the research
  report's Liquidity and vault index projection and conversion, run over the
  same bytes in a separate script. It reproduces the simulated indices exactly
  (Liquidity borrow 1,067,581,275,843, vault borrow 1,010,455,367,801), so it
  equals `sdk`. `midpoint` prices both legs at the cached price with no
  confidence band; `stored_indices` uses the stored vault borrow index without
  projection, which understates the debt. `onchain_mark` was posted one slot
  before the snapshot, in the same second, after the same USDC refresh and
  manual sUSDai price. It is $6.290516 above `sdk`. Striking the USDC debt at
  price − confidence instead of price + confidence accounts for $6.290278. A
  borrow index about four seconds older, 245 micro-USDC less debt, accounts for
  the rest to within rounding. That attribution is inferred from the numbers;
  the posting keeper's inputs are not on chain.
