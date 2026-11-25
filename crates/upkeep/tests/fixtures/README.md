# Mainnet fixtures

Vault states recorded from mainnet on 2026-11-25, one `getMultipleAccounts`
(commitment `confirmed`) per vault at the slot in each file. The keys are what
`EarnPlan::keys` and `OfferbookPlan::keys` name, plus the share mint and the
custody ATAs. Loans were found with the `offerbook::loan_filters` scans just
before the read. Reads only; nothing was sent.

The vaults run program 0.1.0. Vault accounts have the same 2,334-byte layout on
0.2.0, where 0.2.0's new fields read as zero.

| File | Vault | Positions valued |
|---|---|---|
| `bitwise-juplend-optimizer.json` | `9Cw71wvdfjS5DSLPGVfFrPNMusGh32Xhm2B7R1s6uzNw` | Earn, five markets |
| `bitwise-susdai-yield.json` | `Hj9eXER2esW7FMSX1UjgNk29ToX2rpa4QF6ZCKVvXgdH` | Earn, Offerbook (68 active lent loans, escrow yield) |
| `re7-nopal-lending.json` | `6QDoDjzvgUKaaYBptR9CGebT4WUTKd7tTDRwvgD1jmvD` | Earn (empty), Offerbook (3 lent loans) |
| `re7-nopal-looping.json` | `9EdtkcPbuM8zsyhknzo2CRLyPVXNJXCt6zgSjviMYCNc` | Offerbook (3 borrowed loans); its Nest slot is out of scope |

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
