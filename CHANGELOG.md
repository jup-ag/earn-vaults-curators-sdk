# Changelog

## 0.2.0 — Unreleased

`InitializeVaultArgs` and `UpdateVaultArgs` gain a trailing `max_aum`, so clients of those two instructions must be rebuilt. Errors `AumCapExceeded` (249) and `FeeDebtAboveCap` (250) are appended. Vault accounts need no migration, but a curator with a CCIP lane to an SVM chain must close that 40-byte policy and configure the new SVM lane before sends to it resume (#280).

- Add a vault-level AUM cap, `max_aum`, in numeraire micro-units (0 means no cap). `SubscribeDirect` and `Fulfill` fail with `AumCapExceeded` when a deposit would take AUM past it (#273).
- Refuse `SubscribeDirect` and `RedeemDirect` while any priced, required JupiterLend mark is older than the newest cached price; keepers run refresh, `PriceOpaque`, then `ValidateAum` (#274).
- Order composed Pyth Pro observations by each leg's microsecond feed time instead of the envelope time (#277).
- Check the SVM token owner on CCIP sends. SVM lanes are configured as `CHAIN_SELECTOR:MESSAGE_RECEIVER:TOKEN_OWNER` and require `SVMExtraArgsV1`; existing 40-byte lanes keep working for EVM sends and reject `SVMExtraArgsV1`, and `SetProtocolPolicy` will not overwrite them with the 72-byte format (#280).
- Clear every position and sink mark after any Jupiter swap, not only a seized-collateral sale, so settlement waits for a fresh keeper attestation. Inside a flash window the position marks are cleared when the window closes (#288).
- Size the aggregate sink cap from validated AUM excluding existing sink marks, so repeated marking and validation cannot compound it (#292).
- Map Pyth Pro's USDGO/USD price to its Pyth Core feed (#279).
- Reserve only the part of a listed Nest receipt's oracle band that the gross Nest mark does not already cover (#287).
- Value Offerbook escrow transfers by the spendable balance change so transfer fees do not inflate the position mark (#275).
- Refuse `RedeemDirect` while haircut-adjusted sink marks exceed `instant_redeem_fee_bps` of fee-net AUM (#286).
- Require a fresh `ValidateAum` after a direct deposit or queued deposit settlement before the next settlement or fee mint (#278).
- Refuse a fee-free final `RedeemDirect` while any position or sink mark has room to move; that exit uses the queue (#285).
- Preserve the drawdown recovery gap when issuing shares for direct or queued deposits (#276).
- Refill the swap and liquidation slippage buckets at the AUM that held each interval, so a later AUM change no longer rescales refill already earned (#284).
- Refuse a `RedeemDirect` or `FulfillRedeem` that would leave uncrystallized fees above half of the remaining AUM, where the next accrual forgave them to the shares still in; run `CrystallizeFees` first. The check is waived when no fee mint is possible: an AUM confidence sentinel or effective supply above half the u64 range (#290).

## 0.1.0 — 2026-11-11

Initial mainnet-beta baseline for the vault and transfer hook. Source: [`883da7d`](https://github.com/jup-ag/yield-store/commit/883da7d59b59670e999f04f14423f49f2fd6def4).

| Program | Finalized deployment slot | Executable SHA-256 |
|---|---:|---|
| Vault | 468215066 | `e1c7bb6bc96f31d4cdf637f555a377193366079d7f91e002b8a35e16c11303cc` |
| Transfer hook | 468215137 | `fb146ecf3b96e78e7cbc7d6df73e6f1393b9e9dd6f584a0ce0907ad0a28e4b41` |

On 2026-11-20, `just build-sbf` at that commit (cargo-build-sbf 4.4.0, platform-tools v1.57) reproduced both mainnet executables. Hashes use `solana-verify get-executable-hash`, which excludes trailing zero padding.
