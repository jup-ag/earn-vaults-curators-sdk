# earn-vaults-curators-sdk

Synced with earn vault program commit `d9efdcbfdc674f34d4680afe7554d7348a550278`.

`max_deployed = 0` disables new deployment; `u64::MAX`
(`18446744073709551615`) means unlimited. Empty SDK initialization cap lists
and assets added through the CLI default to unlimited. Explicit zero caps are
preserved. The separate `max_deposit = 0` convention still means unlimited.

Account and instruction layouts are unchanged, and existing stored caps are
not rewritten. Before upgrading the program, set existing zero deployment caps
to a finite cap or `u64::MAX` where deployment should remain enabled. Recalls
and repayments retain their existing checks.
