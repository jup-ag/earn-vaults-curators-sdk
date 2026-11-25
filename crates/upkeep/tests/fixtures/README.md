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
- `*_if_extended_allowed`: the same, with each loan's `extension_count` cleared.
  This crate and the API refuse extended loans; these numbers show what the
  netting gives on real loans.
- `onchain_mark`: the slot's `last_value` at the recorded slot, posted
  `onchain_mark_age_seconds` before it.

Where the numbers differ:

- sUSDai Yield Offerbook: the API and the market maker's borrowing path leave
  out the jlUSDC escrow yield the Offerbook `User` holds (22,499 USDC). The
  managed path counts it, and equals this crate.
- RE7 Lending Offerbook: the other copies accrue lent interest from
  `created_at`. After an extension that books the whole period's interest again
  (123.06 USDC here). This crate accrues from the start of the current period,
  as Offerbook does (`expired_at − duration`).
- RE7 Looping Offerbook: within 2 micro-units of the other copies, which round
  each loan separately. This crate sums per mint, then rounds once.
