# Compatibility notes

Synced with earn vault program commit `4e77519fb667f1b4921d9d5f621ef24f3eb5fff4`.

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
See the [escrow-yield guide](https://github.com/jup-ag/earn-vaults-programs/blob/4e77519fb667f1b4921d9d5f621ef24f3eb5fff4/docs/OFFERBOOK_ESCROW_YIELD.md)
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
