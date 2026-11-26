# earn-vaults-upkeep

Pure position valuations and upkeep instruction lists for earn vault keepers.
No RPC, no async: inputs are account bytes read at one slot, outputs are values
in numéraire micro-units and `Vec<Instruction>`.

```rust
use earn_vaults_upkeep::{Snapshot, earn::{EarnPlan, EarnScope}, jobs, mark::Grant};

// Once per vault configuration: the vault and its row mints.
let plan = EarnPlan::new(vault, &discovery_snapshot, EarnScope::LenderOnly)?;
// Every cycle: one getMultipleAccounts over plan.keys(), at one slot. With a
// refresh in the same transaction, apply the simulated refresh's vault.
let snapshot = Snapshot::new(slot, fetched)?.with_refresh(&vault, &sim_vault, &sim_clock)?;
let earn = plan.value(&snapshot)?;
if let Some(mark) = earn.slot.decide(earn.value, earn.now).mark(signer, Grant::JupiterLend) {
    let keys = jobs::VaultKeys::new(vault, snapshot.vault(&vault)?, &plan.rows)?;
    let ixs = jobs::attest(&keys, refresh_ixs, &[mark])?;
}
```

- `rows`: the vault's configured asset rows and their token programs.
- `earn`: Jupiter Earn receipts in every market of a configured row, at the
  stored exchange price and the row's floor price.
- `offerbook`: escrow, escrow yield, lent loans (net of Offerbook's repay fee), and borrowed loans (collateral
  less debt). Loans come from `getProgramAccounts` with `loan_filters`; the
  `User` loan counters and the scans' slots prove none was missed.
- `mark`: which slots are due, and `decide`, which builds only marks the
  program accepts: inside the slot's drift budget, and no zero first mark
  while the vault is in bootstrap.
- `jobs`: `attest` (refresh → marks → `ValidateAum`, with a fresh `ValidateAum`
  before each first mark), `fill_redeem` (fresh `ValidateAum`, optional
  `CrystallizeFees`, `FulfillRedeem`) and `crystallize`, in program 0.2.0's order.

Every failure is an `Error`, never a zero. See the crate docs for the rules.

Not covered: Fluid (Jupiter Lend vault) positions, Nest and CCIP marks,
transfer-hook accounts on a payout mint, and the price-source abstraction the
keeper adds on top.
