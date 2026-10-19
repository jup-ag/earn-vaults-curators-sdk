# earn-vaults-curators-sdk

Synced with earn vault program commit `1a10a73a7a97ed461af67dd38a902869fa404026`.

`max_deployed = 0` disables new deployment; `max_deposit = 0` disables deposits,
including queued fulfillment. For both caps, `u64::MAX` (`18446744073709551615`)
means unlimited. Empty SDK initialization cap lists and assets added through
the CLI default to unlimited. Explicit zero caps are preserved.

Account and instruction layouts are unchanged, and existing stored caps are
not rewritten. Before upgrading the program, set existing zero deployment caps
to a finite cap or `u64::MAX` where deployment should remain enabled, and do the
same for zero deposit caps where deposits should remain enabled. Pending
subscriptions use the cap at fulfillment and can still be cancelled. Recalls
and repayments retain their existing checks. Before rolling back to a program
that treats zero deposit caps as unlimited, disable the deposit flag on assets
whose zero cap must continue blocking deposits.

Drift and sink-bootstrap budget inputs use `u16::MAX` (`65535`) to waive the
budget; `None` or `0` selects the global default at initialization and when
adding positions. For updates to existing fields, `None` leaves the value
unchanged. Stored budgets are unchanged. Coordinate these client inputs with
the program upgrade or rollback: older initialization and position-add
instructions reject `65535` and interpret explicit zero as a waiver.
