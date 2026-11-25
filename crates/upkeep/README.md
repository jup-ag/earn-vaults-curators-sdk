# earn-vaults-upkeep

Pure position valuations and upkeep instruction lists for earn vault keepers.
No RPC, no async: inputs are account bytes read at one slot, outputs are values
in numéraire micro-units and `Vec<Instruction>`.

```rust
use earn_vaults_upkeep::{Snapshot, earn::{EarnPlan, EarnScope}, jobs, mark::{Mark, MarkDecision}};

// Once per vault configuration: the vault and its row mints.
let plan = EarnPlan::new(vault, &discovery_snapshot, EarnScope::LenderOnly)?;
// Every cycle: one getMultipleAccounts over plan.keys(), at one slot.
let earn = plan.value(&Snapshot::new(slot, fetched)?)?;
if let MarkDecision::Post { value, bootstrap } = earn.slot.decide(earn.value) {
    let keys = jobs::VaultKeys::new(vault, state, &plan.rows)?;
    let mark = Mark { signer, slot: earn.slot.index, value, bootstrap, earn_scoped: true };
    let ixs = jobs::attest(&keys, refresh_ixs, &[mark])?;
}
```

- `earn`: Jupiter Earn receipts in every market the vault holds, at the stored
  exchange price and the row's floor price.
- `offerbook`: escrow, escrow yield, lent loans (net of Offerbook's repay fee), and borrowed loans (collateral
  less debt). Loans come from `getProgramAccounts` with `loan_filters`; the
  `User` loan counters prove none appeared since.
- `mark`: which slots are due, and `decide`, which never posts a zero first mark.
- `jobs`: `attest` (refresh → marks → `ValidateAum`), `fill_redeem` (fresh
  `ValidateAum`, `CrystallizeFees` when needed, `FulfillRedeem`) and
  `crystallize`, in program 0.2.0's order.

Every failure is an `Error`, never a zero. See the crate docs for the rules.

Not covered: Fluid (Jupiter Lend vault) positions, Nest and CCIP marks,
transfer-hook accounts on a payout mint, and the price-source abstraction the
keeper adds on top.
