# Compatibility notes

Synced with earn vault program commit `4157cb50e6ce7de661991e2fcb314123747cb855`.

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
See the [escrow-yield guide](https://github.com/jup-ag/earn-vaults-programs/blob/4157cb50e6ce7de661991e2fcb314123747cb855/docs/OFFERBOOK_ESCROW_YIELD.md)
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
