use {
    crate::SdkErrors,
    earn_vaults_interface::{
        VaultInstruction,
        constants::{CHAINLINK_VERIFIER_ID, MAX_SIGNED_REPORT_LEN},
    },
    solana_address::Address,
    solana_instruction::{AccountMeta, Instruction},
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
    /// Pyth `PriceUpdateV2` or legacy Chainlink OCR2 `Transmissions` accounts,
    /// in any order. For OCR2 the row's oracle is the account address itself;
    /// leave the Data Streams (`chainlink`) flag unset.
    pub price_updates: Vec<Address>,
    pub chainlink: Option<ChainlinkReports>,
}

impl RefreshPrices {
    /// Returns `[native_ed25519, refresh_prices]`. `first_instruction_index` is the
    /// final transaction index of native_ed25519, including any compute-budget
    /// or other prefix instructions. Do not reorder this pair after building.
    ///
    /// `message` is the complete signed Solana-format Pro message, not JSON or
    /// the unsigned payload. Fetch `treasury` from Pyth's canonical storage;
    /// the verifier authenticates it. Core/OCR2 accounts and Chainlink reports
    /// on `self` can participate in the same composed refresh.
    pub fn into_instructions_with_pyth_pro(
        self,
        message: &[u8],
        treasury: Address,
        first_instruction_index: u16,
    ) -> Result<[Instruction; 2], SdkErrors> {
        use earn_vaults_interface::pyth_pro::*;
        let payload = message_payload(message)
            .ok_or(SdkErrors::InvalidArgs("invalid Pro message framing"))?;
        let refresh_index = first_instruction_index
            .checked_add(1)
            .filter(|index| *index != u16::MAX)
            .ok_or(SdkErrors::InvalidArgs("instruction index out of range"))?;
        let reports = match &self.chainlink {
            Some(c) if c.reports.is_empty() => {
                return Err(SdkErrors::InvalidArgs("no Chainlink reports"));
            }
            Some(c) => c.encode()?,
            None => Vec::new(),
        };
        let mut envelope =
            Vec::with_capacity(REFRESH_PRO_HEADER_LEN + message.len() + reports.len());
        envelope.extend_from_slice(&REFRESH_PRO_PREFIX);
        envelope.extend_from_slice(&first_instruction_index.to_le_bytes());
        envelope.extend_from_slice(&(message.len() as u16).to_le_bytes());
        envelope.extend_from_slice(message);
        envelope.extend_from_slice(&reports);
        let data = super::encode(&VaultInstruction::RefreshPrices { reports: &envelope });
        let mut accounts = vec![
            AccountMeta::new(self.vault, false),
            AccountMeta::new(self.user, true),
            AccountMeta::new_readonly(PROGRAM_ID, false),
            AccountMeta::new_readonly(STORAGE_ID, false),
            AccountMeta::new(treasury, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(INSTRUCTIONS_SYSVAR_ID, false),
        ];
        if let Some(c) = self.chainlink {
            accounts.extend([
                AccountMeta::new_readonly(CHAINLINK_VERIFIER_ID, false),
                AccountMeta::new_readonly(c.verifier_account, false),
                AccountMeta::new_readonly(c.access_controller, false),
                AccountMeta::new_readonly(c.report_config_account, false),
            ]);
        }
        accounts.extend(
            self.price_updates
                .into_iter()
                .map(|a| AccountMeta::new_readonly(a, false)),
        );
        // pyth-lazer-solana-contract::Ed25519SignatureOffsets wire ABI.
        // Native verification references the message inside RefreshPrices.
        let mut offsets = vec![1, 0];
        for value in [
            MESSAGE_OFFSET + 4,
            refresh_index,
            MESSAGE_OFFSET + 68,
            refresh_index,
            MESSAGE_OFFSET + 102,
            payload.len() as u16,
            refresh_index,
        ] {
            offsets.extend_from_slice(&value.to_le_bytes());
        }
        Ok([
            Instruction {
                program_id: ED25519_PROGRAM_ID,
                accounts: vec![],
                data: offsets,
            },
            Instruction {
                program_id: crate::ID,
                accounts,
                data,
            },
        ])
    }

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

#[cfg(test)]
mod tests {
    use {
        super::*,
        earn_vaults_interface::{VaultInstruction, pyth_pro::*},
        wincode::Deserialize,
    };

    fn refresh() -> RefreshPrices {
        RefreshPrices {
            vault: Address::new_from_array([42; 32]),
            user: Address::new_from_array([42; 32]),
            price_updates: vec![Address::new_from_array([42; 32])],
            chainlink: None,
        }
    }

    #[test]
    fn legacy_encoding_and_accounts_are_unchanged() {
        let ix = refresh().into_instruction().unwrap();
        assert_eq!(ix.data, [23, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(ix.accounts.len(), 3);
        assert!(ix.accounts[0].is_writable);
        assert!(ix.accounts[1].is_signer);
        assert!(!ix.accounts[1].is_writable);
        let mut r = refresh();
        r.chainlink = Some(ChainlinkReports {
            verifier_account: Address::new_from_array([42; 32]),
            access_controller: Address::new_from_array([42; 32]),
            report_config_account: Address::new_from_array([42; 32]),
            reports: vec![vec![1, 2, 3]],
        });
        let ix = r.into_instruction().unwrap();
        assert_eq!(ix.data, [23, 5, 0, 0, 0, 0, 0, 0, 0, 3, 0, 1, 2, 3]);
        assert_eq!(ix.accounts.len(), 7);
        assert_eq!(ix.accounts[2].pubkey, CHAINLINK_VERIFIER_ID);
    }

    #[test]
    fn pro_offsets_point_to_exact_refresh_message_after_transaction_prefix() {
        let mut message = vec![0u8; 106];
        message[..4].copy_from_slice(&SOLANA_MAGIC.to_le_bytes());
        message[100..102].copy_from_slice(&4u16.to_le_bytes());
        message[102..].copy_from_slice(&PAYLOAD_MAGIC.to_le_bytes());
        let treasury = Address::new_from_array([42; 32]);
        for prefix in [0, 1, 4, 100] {
            let [ed, ix] = refresh()
                .into_instructions_with_pyth_pro(&message, treasury, prefix)
                .unwrap();
            assert_eq!(ed.program_id, ED25519_PROGRAM_ID);
            assert_eq!(ed.data.len(), 16);
            let offset = |n: usize| u16::from_le_bytes(ed.data[n..n + 2].try_into().unwrap());
            assert_eq!(
                &ix.data[offset(2) as usize..offset(2) as usize + 64],
                &message[4..68]
            );
            assert_eq!(
                &ix.data[offset(6) as usize..offset(6) as usize + 32],
                &message[68..100]
            );
            assert_eq!(
                &ix.data[offset(10) as usize..offset(10) as usize + offset(12) as usize],
                &message[102..]
            );
            for index in [4, 8, 14] {
                assert_eq!(offset(index), prefix + 1);
            }
            assert_eq!(ix.data[0], 23);
            let VaultInstruction::RefreshPrices { reports } =
                VaultInstruction::deserialize(&ix.data).unwrap()
            else {
                panic!()
            };
            let pro = ProRefreshPayload::decode(reports).unwrap().unwrap();
            assert_eq!(pro.ed25519_instruction_index, prefix);
            assert_eq!(pro.message, message);
            assert!(pro.chainlink_reports.is_empty());
            assert!(ix.accounts[1].is_signer && ix.accounts[1].is_writable);
            assert_eq!(ix.accounts[2].pubkey, PROGRAM_ID);
            assert_eq!(ix.accounts[3].pubkey, STORAGE_ID);
            assert_eq!(ix.accounts[4].pubkey, treasury);
            assert!(ix.accounts[4].is_writable);
            assert_eq!(ix.accounts[5].pubkey, SYSTEM_PROGRAM_ID);
            assert_eq!(ix.accounts[6].pubkey, INSTRUCTIONS_SYSVAR_ID);
        }
        assert!(
            refresh()
                .into_instructions_with_pyth_pro(&message, treasury, u16::MAX - 1)
                .is_err()
        );
        for len in 0..message.len() {
            assert!(
                refresh()
                    .into_instructions_with_pyth_pro(&message[..len], treasury, 0)
                    .is_err()
            );
        }
        message.push(0);
        assert!(
            refresh()
                .into_instructions_with_pyth_pro(&message, treasury, 0)
                .is_err()
        );
    }
}
