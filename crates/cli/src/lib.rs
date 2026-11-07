//! Plumbing for the `earn-vaults` CLI: raw
//! JSON-RPC, transaction send, ATA creation, and a live `VaultSnapshot` fetched
//! straight from mainnet (no state file — everything derives from the vault
//! address).

use {
    anyhow::{Result, anyhow, bail},
    base64::{Engine, engine::general_purpose::STANDARD as B64},
    earn_vaults_interface::{
        nav::{assets_for_redeem, nav_per_share},
        state::{Account as StateAccount, Vault},
    },
    earn_vaults_sdk::pda::{
        ASSOCIATED_TOKEN_PROGRAM_ID, derive_associated_token_account, derive_share_mint,
        derive_vault_authority,
    },
    serde_json::{Value, json},
    solana_address::Address,
    solana_hash::Hash,
    solana_instruction::{AccountMeta, Instruction},
    solana_keypair::{Keypair, read_keypair_file},
    solana_message::{AddressLookupTableAccount, VersionedMessage, v0},
    solana_signer::Signer,
    solana_transaction::versioned::VersionedTransaction,
    std::{
        path::Path,
        str::FromStr,
        sync::atomic::{AtomicU32, AtomicU64, Ordering},
        thread,
        time::Duration,
    },
};

pub const TOKEN_2022: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";
pub const SYSTEM: &str = "11111111111111111111111111111111";
pub const COMPUTE_BUDGET: &str = "ComputeBudget111111111111111111111111111111";
pub const ADDRESS_LOOKUP_TABLE: &str = "AddressLookupTab1e1111111111111111111111111111";

pub fn addr(s: &str) -> Result<Address> {
    Address::from_str(s).map_err(|e| anyhow!("bad address {s}: {e}"))
}

pub fn load_kp(path: &Path) -> Result<Keypair> {
    read_keypair_file(path).map_err(|e| anyhow!("read keypair {}: {e}", path.display()))
}

// --- raw JSON-RPC over ureq --------------------------------------------------

pub fn rpc(url: &str, method: &str, params: Value) -> Result<Value> {
    let mut resp: Value = ureq::post(url)
        .send_json(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}))?
        .body_mut()
        .read_json()?;
    if let Some(e) = resp.get("error") {
        bail!("rpc {method} error: {e}");
    }
    Ok(resp.get_mut("result").map_or(Value::Null, Value::take))
}

pub fn latest_blockhash(url: &str) -> Result<Hash> {
    let v = rpc(
        url,
        "getLatestBlockhash",
        json!([{"commitment": "confirmed"}]),
    )?;
    let bh = v["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| anyhow!("no blockhash in response"))?;
    Hash::from_str(bh).map_err(|e| anyhow!("bad blockhash: {e}"))
}

/// Compute-budget knobs, applied to every transaction this process sends.
// Process globals avoid threading a `SendOpts` value through every
// call site — a CLI resolves these once from its flags at startup. Make them a
// parameter if a caller ever needs per-transaction values.
static PRIORITY_FEE: AtomicU64 = AtomicU64::new(0);
static COMPUTE_LIMIT: AtomicU32 = AtomicU32::new(0);

/// `priority_fee`: micro-lamports per compute unit. `compute_limit`: explicit
/// CU ceiling. Either at 0 omits its instruction, leaving the runtime defaults
/// (no priority fee, 200k CU per instruction).
pub fn set_fees(priority_fee: u64, compute_limit: u32) {
    PRIORITY_FEE.store(priority_fee, Ordering::Relaxed);
    COMPUTE_LIMIT.store(compute_limit, Ordering::Relaxed);
}

/// `SetComputeUnitLimit` (tag 2, u32) / `SetComputeUnitPrice` (tag 3, u64).
/// Hand-encoded — two byte strings aren't worth a dependency.
fn compute_budget_ixs(priority_fee: u64, compute_limit: u32) -> Vec<Instruction> {
    let program_id = Address::from_str(COMPUTE_BUDGET).unwrap();
    let mut ixs = Vec::new();
    if compute_limit > 0 {
        let mut data = vec![2u8];
        data.extend_from_slice(&compute_limit.to_le_bytes());
        ixs.push(Instruction {
            program_id,
            accounts: vec![],
            data,
        });
    }
    if priority_fee > 0 {
        let mut data = vec![3u8];
        data.extend_from_slice(&priority_fee.to_le_bytes());
        ixs.push(Instruction {
            program_id,
            accounts: vec![],
            data,
        });
    }
    ixs
}

/// A lookup table's metadata is a fixed-size header; the addresses follow it.
const LUT_META_SIZE: usize = 56;

/// The addresses a lookup-table account holds.
///
/// A trailing partial chunk is an error, not something to drop: the lookup-table
/// program only ever appends whole 32-byte keys, so a remainder means these
/// bytes are not a table's address region — wrong account, wrong header offset,
/// or a truncated read. The addresses that *did* decode are then reads at the
/// wrong offset, and every one of them is silently wrong rather than missing.
///
/// That matters because only the **index** travels on the wire: the runtime
/// resolves it against the real on-chain table, so a locally miscounted address
/// list builds a transaction naming accounts the caller never chose. Failing
/// here turns that into an error at build time.
fn lut_addresses(key: &Address, data: &[u8]) -> Result<Vec<Address>> {
    let (addresses, rest) = data
        .get(LUT_META_SIZE..)
        .ok_or_else(|| anyhow!("{key} is too small to be an address lookup table"))?
        .as_chunks::<32>();
    if !rest.is_empty() {
        bail!(
            "{key} is not a well-formed address lookup table: {} trailing byte(s) \
             past the last whole address",
            rest.len()
        );
    }
    Ok(addresses.iter().copied().map(Address::from).collect())
}

/// Fetch and decode address lookup tables so `v0::Message` can compress the
/// account keys they cover. Jupiter routes ship with these — without them a
/// real route's key list overruns the 1232-byte transaction limit.
pub fn fetch_luts(url: &str, keys: &[Address]) -> Result<Vec<AddressLookupTableAccount>> {
    keys.iter()
        .map(|key| {
            let v = rpc(
                url,
                "getAccountInfo",
                json!([key.to_string(), {"encoding": "base64", "commitment": "confirmed"}]),
            )?;
            let data = v["value"]["data"][0]
                .as_str()
                .ok_or_else(|| anyhow!("lookup table {key} not found on this cluster"))?;
            // Owner first: `lut_addresses` reads a fixed header offset, which
            // only means anything on an account the lookup-table program wrote.
            // Any other account of the right length would decode into
            // plausible-looking garbage, and the indices compiled against it
            // resolve on-chain to whatever the real table holds.
            let owner = v["value"]["owner"]
                .as_str()
                .ok_or_else(|| anyhow!("lookup table {key} has no owner"))?;
            if owner != ADDRESS_LOOKUP_TABLE {
                bail!("{key} is owned by {owner}, not the address-lookup-table program");
            }
            Ok(AddressLookupTableAccount {
                key: *key,
                addresses: lut_addresses(key, &B64.decode(data)?)?,
            })
        })
        .collect()
}

/// `skip_preflight`: for oracle-freshness-critical txs the preflight is a
/// liability — it may simulate on a pool node whose confirmed bank hasn't seen
/// the keeper's latest post, rejecting a tx the leader would accept.
/// Sign and send a **v0** transaction, waiting for confirmation. Any configured
/// compute-budget instructions are prepended; `luts` are the lookup tables to
/// compress account keys against (empty is a normal v0 tx with no lookups —
/// there is no legacy path, v0 is universally accepted).
pub fn send_opts(
    url: &str,
    ixs: &[Instruction],
    signers: &[&Keypair],
    luts: &[Address],
    skip_preflight: bool,
) -> Result<String> {
    let luts = fetch_luts(url, luts)?;
    let hash = latest_blockhash(url)?;
    let mut all = compute_budget_ixs(
        PRIORITY_FEE.load(Ordering::Relaxed),
        COMPUTE_LIMIT.load(Ordering::Relaxed),
    );
    all.extend_from_slice(ixs);
    let msg = v0::Message::try_compile(&signers[0].pubkey(), &all, &luts, hash)
        .map_err(|e| anyhow!("compile v0 message: {e}"))?;
    let tx = VersionedTransaction::try_new(VersionedMessage::V0(msg), signers)
        .map_err(|e| anyhow!("sign transaction: {e}"))?;
    let bytes = wincode::serialize(&tx)?;
    // preflightCommitment must match the commitment the blockhash was fetched
    // at ("confirmed") — otherwise preflight simulates against the finalized
    // bank, which may not know the just-confirmed blockhash → BlockhashNotFound.
    let sig = rpc(
        url,
        "sendTransaction",
        json!([B64.encode(bytes), {"encoding": "base64", "preflightCommitment": "confirmed", "skipPreflight": skip_preflight}]),
    )?;
    let sig = sig.as_str().unwrap_or_default().to_string();
    // Wait for confirmation before returning: a sent-but-failed tx must surface
    // as an error, not a signature, and a following command's tx would
    // otherwise simulate against a bank that doesn't yet see this one's writes
    // (e.g. grant-delegate reading a vault init-vault just created →
    // InvalidAccountOwner).
    confirm(url, &sig)?;
    Ok(sig)
}

/// Poll `getSignatureStatuses` until the tx is confirmed (or errored/timed out).
pub fn confirm(url: &str, sig: &str) -> Result<()> {
    for _ in 0..60 {
        let v = rpc(
            url,
            "getSignatureStatuses",
            json!([[sig], {"searchTransactionHistory": false}]),
        )?;
        if let Some(status) = v["value"].get(0).filter(|s| !s.is_null()) {
            if let Some(err) = status.get("err").filter(|e| !e.is_null()) {
                bail!("tx {sig} failed on-chain: {err}");
            }
            if matches!(
                status["confirmationStatus"].as_str(),
                Some("confirmed" | "finalized")
            ) {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(500));
    }
    bail!("tx {sig} not confirmed after 30s")
}

/// On-chain owner (token program) of a mint — the vault is token-program-agnostic.
pub fn mint_token_program(url: &str, mint: &Address) -> Result<Address> {
    let v = rpc(
        url,
        "getAccountInfo",
        json!([mint.to_string(), {"encoding": "base64", "commitment": "confirmed"}]),
    )?;
    let owner = v["value"]["owner"]
        .as_str()
        .ok_or_else(|| anyhow!("mint {mint} not found on this cluster"))?;
    addr(owner)
}

/// Idempotent ATA-create instruction (owner's ATA of `mint` under `token_program`).
pub fn create_ata_ix(
    payer: Address,
    owner: Address,
    mint: Address,
    token_program: Address,
) -> Instruction {
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(
                derive_associated_token_account(&owner, &mint, &token_program),
                false,
            ),
            AccountMeta::new_readonly(owner, false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new_readonly(Address::from_str(SYSTEM).unwrap(), false),
            AccountMeta::new_readonly(token_program, false),
        ],
        data: vec![1u8], // CreateIdempotent
    }
}

pub fn create_ata(
    url: &str,
    payer: &Keypair,
    owner: Address,
    mint: Address,
    token_program: Address,
) -> Result<Address> {
    let ata = derive_associated_token_account(&owner, &mint, &token_program);
    let sig = send_opts(
        url,
        &[create_ata_ix(payer.pubkey(), owner, mint, token_program)],
        &[payer],
        &[],
        false,
    )?;
    println!("  ata {ata} ({sig})");
    Ok(ata)
}

// --- live snapshot -----------------------------------------------------------

/// One asset row, flattened from `Vault.assets` + its live vault-ATA balance.
#[derive(Debug)]
pub struct AssetRow {
    pub mint: Address,
    pub oracle: Address,
    pub deposit_enabled: bool,
    pub price_in_aum: bool,
    /// `oracle` is a Chainlink Data Streams feed id, refreshed with a DON report
    /// rather than a Pyth account — the CLI cannot fetch reports, so these rows
    /// are left to whatever keeper does.
    pub chainlink: bool,
    /// Declared scale of the feed's raw mantissa (`AssetConfig::report_decimals`);
    /// `0` = the source's own. A wrong value here mis-prices the row by a power
    /// of ten, so it is worth eyeballing.
    pub report_decimals: u8,
    pub decimals: u8,
    pub deployed: u64,
    /// Live balance of the vault-authority's ATA for this mint (raw units).
    pub vault_balance: u64,
}

/// One enabled position row from `Vault.positions`.
#[derive(Debug)]
pub struct PositionRow {
    pub protocol: u8,
    pub debt: bool,
    pub required_in_aum: bool,
    /// `POSITION_FLAG_LENDER_ONLY`: Offerbook lender-only, or JupiterLend
    /// Earn-only (Fluid Vaults refused; instant redemption admissible).
    pub lender_only: bool,
    /// Stored mark windows: `0` = the program default; drift `65535` = waived.
    pub max_staleness_seconds: u64,
    pub max_drift_bps: u16,
    pub last_value: u64,
    pub last_priced_at: i64,
}

/// Live vault state, owned (no borrows from RPC buffers).
#[derive(Debug)]
pub struct VaultSnapshot {
    pub address: Address,
    pub curator: Address,
    /// Two-step curator rotation in flight (zero address = none → `None`).
    pub pending_curator: Option<Address>,
    pub created_at: i64,
    /// `Pause` bitmask: 1 subscribe, 2 redeem, 4 strategy.
    pub paused: u8,
    pub aum: u64,
    pub last_priced_at: i64,
    pub timelock_delay: u64,
    /// Per-vault annual management fee minted to the curator.
    pub management_fee_bps: u16,
    /// HWM performance fee on NAV gains, minted to the curator.
    pub performance_fee_bps: u16,
    /// `u16::MAX` = instant redeem disabled.
    pub instant_redeem_fee_bps: u16,
    pub queued_redeem_fee_bps: u16,
    pub share_mint: Address,
    pub authority: Address,
    pub share_supply: u64,
    pub pending_redeem_shares: u64,
    /// Streaming (curator + protocol management) fee liability accrued but not yet
    /// crystallized — the program's own `accrued_fee_assets` at fetch time.
    pub streaming_fee_accrued: u64,
    /// Performance fee liability (folded accumulator + pending NAV-rise
    /// segment) — `fee_liability − accrued_fee_assets`.
    pub perf_fee_accrued: u64,
    /// Fee-net per-share NAV (`NAV_SCALE` fixed point) on the exact basis the
    /// program prices subscribes/redeems (`Vault::pricing_basis`).
    pub nav_net: u128,
    /// Performance high-water mark (`NAV_SCALE` fixed point).
    pub hwm_nav: u128,
    pub assets: Vec<AssetRow>,
    pub positions: Vec<PositionRow>,
    /// Present only when a `--user` was given: their share balance, and their
    /// redeem-value in the USD numéraire's raw units (`NUMERAIRE_DECIMALS`).
    pub user_shares: Option<u64>,
    pub user_value: Option<u128>,
}

fn token_amount(url: &str, method: &str, target: &Address) -> u64 {
    // Returns 0 if the account/mint doesn't exist yet (not an error for display).
    // Read at "confirmed" so it matches the vault account's commitment.
    rpc(
        url,
        method,
        json!([target.to_string(), {"commitment": "confirmed"}]),
    )
    .ok()
    .and_then(|v| {
        v["value"]["amount"]
            .as_str()
            .map(|s| s.parse().unwrap_or(0))
    })
    .unwrap_or(0)
}

/// Fetch and decode a vault straight from the cluster. `user`, if given, adds
/// their share balance and pro-rata value.
pub fn fetch_snapshot(url: &str, vault: Address, user: Option<Address>) -> Result<VaultSnapshot> {
    let v = rpc(
        url,
        "getAccountInfo",
        json!([vault.to_string(), {"encoding": "base64", "commitment": "confirmed"}]),
    )?;
    let data_b64 = v["value"]["data"][0]
        .as_str()
        .ok_or_else(|| anyhow!("vault {vault} not found"))?;
    let data = B64.decode(data_b64)?;
    let vs = <Vault as StateAccount>::read(&data).map_err(|e| anyhow!("decode vault: {e:?}"))?;

    let share_mint = derive_share_mint(&vault);
    let authority = derive_vault_authority(&vault);
    let token_2022 = addr(TOKEN_2022)?;

    let mut assets = Vec::new();
    for a in vs.assets.iter().filter(|a| a.mint != Address::default()) {
        let tp = mint_token_program(url, &a.mint).unwrap_or(token_2022);
        let vault_ata = derive_associated_token_account(&authority, &a.mint, &tp);
        assets.push(AssetRow {
            mint: a.mint,
            oracle: a.oracle,
            deposit_enabled: a.deposit_enabled(),
            price_in_aum: a.price_in_aum(),
            chainlink: a.chainlink(),
            report_decimals: a.report_decimals,
            decimals: a.decimals,
            deployed: a.deployed.get(),
            vault_balance: token_amount(url, "getTokenAccountBalance", &vault_ata),
        });
    }

    let positions = vs
        .positions
        .iter()
        .filter(|p| p.enabled())
        .map(|p| PositionRow {
            protocol: p.protocol,
            debt: p.debt(),
            required_in_aum: p.required_in_aum(),
            lender_only: p.lender_only(),
            max_staleness_seconds: p.max_staleness_seconds.get(),
            max_drift_bps: p.max_drift_bps.get(),
            last_value: p.last_value.get(),
            last_priced_at: p.last_priced_at.into(),
        })
        .collect();

    let share_supply = token_amount(url, "getTokenSupply", &share_mint);

    // Fee liabilities + fee-net pricing basis, computed by the SAME interface
    // methods the program uses (local clock stands in for the cluster clock —
    // display-grade accuracy).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let streaming_fee_accrued = vs.accrued_fee_assets(now);
    let perf_fee_accrued = vs
        .raw_fee_liability(share_supply, now)
        .saturating_sub(streaming_fee_accrued);
    let (eff_supply, eff_aum) = (vs.eff_supply(share_supply), vs.eff_aum(share_supply, now));
    let nav_net = nav_per_share(eff_supply, eff_aum).unwrap_or(0);

    let (user_shares, user_value) = match user {
        Some(u) => {
            let ata = derive_associated_token_account(&u, &share_mint, &token_2022);
            let shares = token_amount(url, "getTokenAccountBalance", &ata);
            // Value on the fee-net basis — what a redeem would actually price at.
            let value = assets_for_redeem(shares, eff_supply, eff_aum)
                .map(u128::from)
                .map_err(|e| anyhow!("snapshot redeem value: {e:?}"))?;
            (Some(shares), Some(value))
        }
        None => (None, None),
    };

    Ok(VaultSnapshot {
        address: vault,
        curator: vs.curator,
        pending_curator: (vs.pending_curator != Address::default()).then_some(vs.pending_curator),
        created_at: vs.created_at.into(),
        paused: vs.paused,
        aum: vs.aum.get(),
        last_priced_at: vs.last_priced_at.into(),
        timelock_delay: vs.timelock_delay.get(),
        management_fee_bps: vs.management_fee_bps.get(),
        performance_fee_bps: vs.performance_fee_bps.get(),
        instant_redeem_fee_bps: vs.instant_redeem_fee_bps.get(),
        queued_redeem_fee_bps: vs.queued_redeem_fee_bps.get(),
        share_mint,
        authority,
        share_supply,
        pending_redeem_shares: vs.pending_redeem_shares.get(),
        streaming_fee_accrued,
        perf_fee_accrued,
        nav_net,
        hwm_nav: vs.hwm_nav.get(),
        assets,
        positions,
        user_shares,
        user_value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_preserves_json_request_and_errors() {
        use std::{
            io::{BufRead, BufReader, Read, Write},
            net::TcpListener,
        };

        for (status, body, succeeds) in [
            (
                "200 OK",
                r#"{"jsonrpc":"2.0","id":1,"result":{"value":42}}"#,
                true,
            ),
            (
                "200 OK",
                r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"bad params"}}"#,
                false,
            ),
            (
                "500 Internal Server Error",
                r#"{"result":{"value":42}}"#,
                false,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(&mut stream);
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert_eq!(line, "POST / HTTP/1.1\r\n");
                let mut length = None;
                loop {
                    line.clear();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = Some(value.trim().parse::<usize>().unwrap());
                    }
                }
                let mut request = vec![0; length.unwrap()];
                reader.read_exact(&mut request).unwrap();
                assert_eq!(
                    serde_json::from_slice::<Value>(&request).unwrap(),
                    json!({"jsonrpc":"2.0","id":1,"method":"getBalance","params":["vault"]})
                );
                write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let response = rpc(&url, "getBalance", json!(["vault"]));
            server.join().unwrap();
            if succeeds {
                assert_eq!(response.unwrap(), json!({"value":42}));
            } else {
                let error = response.unwrap_err().to_string();
                assert!(
                    error.contains(if status == "200 OK" {
                        "bad params"
                    } else {
                        "500"
                    }),
                    "{error}"
                );
            }
        }
    }

    /// Compute-budget encoding: tags and little-endian widths are what the
    /// runtime parses, and a 0 means "omit", not "bid nothing".
    #[test]
    fn compute_budget_encoding() {
        assert!(compute_budget_ixs(0, 0).is_empty());

        let ixs = compute_budget_ixs(5_000, 400_000);
        assert_eq!(ixs.len(), 2);
        // Limit first — the price ix is metered against it.
        assert_eq!(
            ixs[0].data,
            [vec![2u8], 400_000u32.to_le_bytes().to_vec()].concat()
        );
        assert_eq!(
            ixs[1].data,
            [vec![3u8], 5_000u64.to_le_bytes().to_vec()].concat()
        );
        assert!(ixs.iter().all(|i| i.accounts.is_empty()));

        assert_eq!(
            compute_budget_ixs(5_000, 0).len(),
            1,
            "limit 0 omits its ix"
        );
        assert_eq!(
            compute_budget_ixs(0, 400_000).len(),
            1,
            "fee 0 omits its ix"
        );
    }

    /// Lookup-table decode: addresses start after a fixed 56-byte header, so an
    /// off-by-one here silently yields garbage keys that `try_compile` would
    /// then fail to match — leaving a route uncompressed and over the size cap.
    #[test]
    fn lut_decode() {
        let key = Address::new_from_array([9u8; 32]);
        let mut data = vec![0u8; LUT_META_SIZE];
        data.extend_from_slice(&[1u8; 32]);
        data.extend_from_slice(&[2u8; 32]);

        let got = lut_addresses(&key, &data).unwrap();
        assert_eq!(
            got,
            vec![
                Address::new_from_array([1u8; 32]),
                Address::new_from_array([2u8; 32])
            ]
        );

        // An empty (just-created) table decodes to no addresses, not an error.
        assert!(
            lut_addresses(&key, &[0u8; LUT_META_SIZE])
                .unwrap()
                .is_empty()
        );
        // Anything shorter than the header is not a lookup table.
        assert!(lut_addresses(&key, &[0u8; 8]).is_err());

        // A trailing partial address means the offset is wrong, so the whole
        // decode is wrong — every index compiled against it would name an
        // account the caller never picked. Reject, don't truncate.
        for tail in [1usize, 31, 33] {
            let mut data = vec![0u8; LUT_META_SIZE];
            data.extend_from_slice(&[1u8; 32]);
            data.extend_from_slice(&vec![7u8; tail]);
            assert!(
                lut_addresses(&key, &data).is_err(),
                "{tail} trailing byte(s) must not decode"
            );
        }
    }

    /// A v0 message with a lookup table must actually compress: the same
    /// instruction compiled against a table covering its keys serializes
    /// smaller than one without, and stays inside the packet limit.
    #[test]
    fn v0_compresses_against_lut() {
        let payer = Address::new_from_array([1u8; 32]);
        let program = Address::new_from_array([2u8; 32]);
        // 20 read-only keys is the shape of a real multi-hop route.
        let keys: Vec<Address> = (0..20)
            .map(|i| Address::new_from_array([i + 10; 32]))
            .collect();
        let ix = Instruction {
            program_id: program,
            accounts: keys
                .iter()
                .map(|k| AccountMeta::new_readonly(*k, false))
                .collect(),
            data: vec![7u8; 32],
        };
        let lut = AddressLookupTableAccount {
            key: Address::new_from_array([3u8; 32]),
            addresses: keys,
        };
        let hash = [4u8; 32];

        let bare = v0::Message::try_compile(
            &payer,
            std::slice::from_ref(&ix),
            &[],
            Hash::new_from_array(hash),
        )
        .unwrap();
        let with_lut =
            v0::Message::try_compile(&payer, &[ix], &[lut], Hash::new_from_array(hash)).unwrap();

        assert!(
            with_lut.serialize().len() < bare.serialize().len(),
            "lookup table did not shrink the message — keys were not extracted"
        );
        assert!(with_lut.serialize().len() < 1232);
    }
}
