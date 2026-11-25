# earn-vaults-upkeep

Pure position valuations and upkeep instruction lists for earn vault keepers.
No RPC, no async: inputs are account bytes read at one slot, outputs are values
in numéraire micro-units and `Vec<Instruction>`.

```rust
use earn_vaults_upkeep::{Snapshot, earn::EarnPlan, jobs, listing, mark::{Mark, MarkDecision}};

// Once per vault configuration: the vault, its row mints, and the authority's
// token listing. Scan each candidate with fluid::position_filters; those with
// no Position are `unrelated`.
let candidates = listing::candidate_mints(state, &listings)?;
let plan = EarnPlan::new(vault, &discovery_snapshot, unrelated)?;
// Every cycle, at one slot: getMultipleAccounts over plan.keys() and the
// authority's getTokenAccountsByOwner under both token programs.
let earn = plan.value(&Snapshot::new(slot, fetched)?, &listings)?;
if let MarkDecision::Post { value, bootstrap } = earn.slot.decide(earn.value) {
    let keys = jobs::VaultKeys::new(vault, state, &plan.rows)?;
    let mark = Mark { signer, slot: earn.slot.index, value, bootstrap, earn_scoped: true };
    let ixs = jobs::attest(&keys, refresh_ixs, &[mark])?;
}
```

- `earn`: Jupiter Earn receipts in every market the vault holds, at the stored
  exchange price and the row's floor price (a lender-only JupiterLend slot).
- `fluid`: a JupiterLend slot that is not lender-only: Earn receipts plus every
  Fluid T1 position, collateral less debt. Indices come from simulating the
  Vaults program's `get_exchange_prices` at the same slot. Supply-only and
  live-tick positions only; liquidation history and smart vaults fail closed.
- `listing`: the authority's token accounts at the snapshot's slot, the proof
  that nothing else sits in the slot. Receipts with no row, an unplanned Fluid
  NFT, or a mint discovery did not classify fail closed; a stray airdrop is
  reported and ignored.
- `offerbook`: escrow, escrow yield, lent loans (net of Offerbook's repay fee), and borrowed loans (collateral
  less debt). Loans come from `getProgramAccounts` with `loan_filters`; the
  `User` loan counters prove none appeared since.
- `mark`: which slots are due, and `decide`, which never posts a zero first mark.
- `jobs`: `attest` (refresh → marks → `ValidateAum`), `fill_redeem` (fresh
  `ValidateAum`, `CrystallizeFees` when needed, `FulfillRedeem`) and
  `crystallize`, in program 0.2.0's order.

Every failure is an `Error`, never a zero. See the crate docs for the rules.

Not covered: Fluid positions on a liquidated tick or in a smart vault, Nest
and CCIP marks,
transfer-hook accounts on a payout mint, and the price-source abstraction the
keeper adds on top.
