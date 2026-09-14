use {
    crate::SdkErrors,
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
    earn_vaults_interface::{
        VaultInstruction,
        constants::{CHAINLINK_VERIFIER_ID, MAX_SIGNED_REPORT_LEN},
    },
};

/// A batch of Chainlink Data Streams reports plus the verifier plumbing they
/// share. The verifier checks these accounts itself — the vault only pins the
/// program.
pub struct ChainlinkReports {
    /// Verifier config PDA: `["verifier"]` under the verifier program.
    pub verifier_account: Address,
    /// Access controller named by the verifier's config.
    pub access_controller: Address,
    /// Report config PDA: the first 32 bytes of the *uncompressed* report as the
    /// seed, under the verifier program.
    pub report_config_account: Address,
    /// Snappy-compressed signed reports, straight from the Data Streams API.
    /// How many fit is a transaction-budget question: ~600–800 bytes and
    /// 25 000 CU per DON signature each.
    pub reports: Vec<Vec<u8>>,
}

impl ChainlinkReports {
    /// Frame the batch as the instruction carries it: `[len: u16 LE][bytes]`, repeated.
    fn encode(&self) -> Result<Vec<u8>, SdkErrors> {
        let mut out = Vec::with_capacity(self.reports.iter().map(|r| r.len() + 2).sum());
        for report in &self.reports {
            if report.is_empty() || report.len() > MAX_SIGNED_REPORT_LEN {
                return Err(SdkErrors::InvalidArgs("report length out of range"));
            }
            out.extend_from_slice(&(report.len() as u16).to_le_bytes());
            out.extend_from_slice(report);
        }
        Ok(out)
    }
}

/// Cache fresh readings on asset rows — the only instruction that touches a price
/// source, and the one every value site depends on having run.
pub struct RefreshPrices {
    pub vault: Address,
    /// Signs and pays; the Chainlink verifier requires a signer of its own.
    pub user: Address,
    /// Pyth `PriceUpdateV2` accounts, in any order.
    pub price_updates: Vec<Address>,
    pub chainlink: Option<ChainlinkReports>,
}

impl RefreshPrices {
    pub fn into_instruction(self) -> Result<Instruction, SdkErrors> {
        if self.price_updates.is_empty() && self.chainlink.is_none() {
            return Err(SdkErrors::InvalidArgs("nothing to refresh"));
        }
        let reports = match &self.chainlink {
            Some(c) if c.reports.is_empty() => {
                return Err(SdkErrors::InvalidArgs("no Chainlink reports"));
            }
            Some(c) => c.encode()?,
            None => Vec::new(),
        };
        let data = super::encode(&VaultInstruction::RefreshPrices { reports: &reports });

        let mut accounts = vec![
            AccountMeta::new(self.vault, false),
            AccountMeta::new_readonly(self.user, true),
        ];
        // The verifier accounts sit in front of the Pyth ones, and only when a
        // report is present — a Pyth-only refresh carries no Chainlink plumbing.
        if let Some(c) = &self.chainlink {
            accounts.extend([
                AccountMeta::new_readonly(CHAINLINK_VERIFIER_ID, false),
                AccountMeta::new_readonly(c.verifier_account, false),
                AccountMeta::new_readonly(c.access_controller, false),
                AccountMeta::new_readonly(c.report_config_account, false),
            ]);
        }
        for price_update in &self.price_updates {
            accounts.push(AccountMeta::new_readonly(*price_update, false));
        }
        Ok(Instruction {
            program_id: crate::ID,
            accounts,
            data,
        })
    }
}
