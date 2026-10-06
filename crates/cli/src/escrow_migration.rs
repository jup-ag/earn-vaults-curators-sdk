use {
    anyhow::{Context, Result, ensure},
    base64::{Engine, engine::general_purpose::STANDARD as B64},
    earn_vaults_cli::rpc,
    earn_vaults_sdk::{
        ID,
        pda::derive_escrow,
        state::{Account, Escrow, LegacyEscrow},
    },
    serde_json::{Value, json},
    solana_address::Address,
};

/// Read-only finalized inventory. Fail on unknown layouts instead of undercounting.
pub fn inventory(url: &str) -> Result<Value> {
    let genesis = rpc(url, "getGenesisHash", json!([]))?;
    // Base58 "4" is the single-byte Escrow discriminator (3).
    let result = rpc(
        url,
        "getProgramAccounts",
        json!([ID.to_string(), {
            "encoding": "base64", "commitment": "finalized", "withContext": true,
            "filters": [{"memcmp": {"offset": 0, "bytes": "4"}}]
        }]),
    )?;
    let slot = result["context"]["slot"]
        .as_u64()
        .context("missing finalized slot")?;
    let accounts = result["value"].as_array().context("missing account list")?;
    let rent = |size| -> Result<u64> {
        rpc(
            url,
            "getMinimumBalanceForRentExemption",
            json!([size, {"commitment": "finalized"}]),
        )?
        .as_u64()
        .context("invalid rent response")
    };
    let old_rent = rent(LegacyEscrow::LEN)?;
    let new_rent = rent(Escrow::LEN)?;
    let mut entries = Vec::new();
    let mut count = 0;
    let mut total_top_up = 0u64;
    for item in accounts {
        let address: Address = item["pubkey"].as_str().context("missing pubkey")?.parse()?;
        let account = &item["account"];
        ensure!(
            account["owner"].as_str() == Some(&ID.to_string()),
            "wrong owner: {address}"
        );
        ensure!(
            account["executable"] == false,
            "executable escrow: {address}"
        );
        ensure!(
            account["data"][1] == "base64",
            "unexpected encoding: {address}"
        );
        let data = B64.decode(account["data"][0].as_str().context("missing data")?)?;
        let e = Escrow::read_header(&data)
            .with_context(|| format!("unknown escrow layout: {address}"))?;
        ensure!(
            derive_escrow(&e.vault, &e.owner, e.nonce.get()) == address,
            "noncanonical escrow: {address}"
        );
        let request = match e.request_type {
            0 => "deposit",
            1 => "redeem",
            _ => anyhow::bail!("unknown request type: {address}"),
        };
        let status = match e.fulfilled {
            0 => "pending",
            1 => "fulfilled",
            2 if e.request_type == 1 => "partial",
            _ => anyhow::bail!("unknown request status: {address}"),
        };
        let needs_migration = data.len() == LegacyEscrow::LEN;
        let lamports = account["lamports"].as_u64().context("missing lamports")?;
        let top_up = if needs_migration {
            count += 1;
            new_rent
                .saturating_sub(old_rent)
                .max(new_rent.saturating_sub(lamports))
        } else {
            0
        };
        total_top_up = total_top_up
            .checked_add(top_up)
            .context("rent total overflow")?;
        entries.push(json!({
            "escrow": address.to_string(), "vault": e.vault.to_string(),
            "owner": e.owner.to_string(), "nonce": e.nonce.get(),
            "request": request, "status": status, "data_len": data.len(),
            "needs_migration": needs_migration, "rent_top_up_lamports": top_up,
        }));
    }
    entries.sort_by_key(|v| v["escrow"].as_str().unwrap_or_default().to_owned());
    Ok(json!({
        "program": ID.to_string(), "genesis_hash": genesis, "finalized_slot": slot,
        "escrows_total": entries.len(), "accounts_to_migrate": count,
        "rent_top_up_lamports": total_top_up, "accounts": entries,
    }))
}
