use {
    crate::arguments::{
        InitializeVaultArgs, PriceOpaqueArgs, SetProtocolPolicyArgs, UpdateShareMetaArgs,
        UpdateVaultArgs,
    },
    codama_macros::CodamaInstructions,
    solana_address::Address,
    wincode::{
        SchemaRead, SchemaWrite, WriteResult,
        config::Config,
        error::ReadError,
        io::{Reader, Writer},
    },
    zeropod::U64,
};

#[derive(CodamaInstructions)]
#[repr(u8)]
pub enum VaultInstruction<'de> {
    #[codama(account(name = "curator", signer, writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    #[codama(account(name = "policy", writable))]
    InitializeVault { args: &'de InitializeVaultArgs },
    #[codama(account(name = "curator", signer))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    UpdateVault { args: &'de UpdateVaultArgs },
    #[codama(account(name = "curator", signer))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "recipient", writable))]
    #[codama(account(name = "vaultAuthority", writable))]
    #[codama(account(name = "policy", writable))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "systemProgram"))]
    CloseVault,
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "userTokenAccount", writable))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "escrowTokenAccount", writable))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    Subscribe {
        #[codama(type = number(u64))]
        amount: &'de U64,
        #[codama(type = number(u64))]
        nonce: &'de U64,
        #[codama(type = number(u64))]
        min_shares_out: &'de U64,
    },
    #[codama(account(name = "authority", signer, writable))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "escrowTokenAccount", writable))]
    #[codama(account(name = "vaultTokenAccount", writable))]
    #[codama(account(name = "escrowShareAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "baseTokenProgram"))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    Fulfill,
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "escrowShareAccount", writable))]
    #[codama(account(name = "userShareAccount", writable))]
    #[codama(account(name = "shareMint"))]
    #[codama(account(name = "metaList"))]
    #[codama(account(name = "policy"))]
    #[codama(account(name = "hookProgram"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    Claim,
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "escrowTokenAccount", writable))]
    #[codama(account(name = "userTokenAccount", writable))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    Cancel,
    #[codama(account(name = "signer", signer))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "targetProgram"))]
    ProtocolCPI {
        // wincode length-prefixes slices with a u64; without the explicit
        // size_prefix the IDL claims bare bytes and IDL-driven decoders read
        // the payload 4 bytes early (janus did).
        #[codama(type = bytes)]
        #[codama(size_prefix = number(u64))]
        data: &'de [u8],
    },
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    ValidateAum,
    #[codama(account(name = "curator", signer))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "policy", writable))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    SetPausedTransfers { paused: &'de u8 },
    #[codama(account(name = "curator", signer, writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "delegate", writable))]
    #[codama(account(name = "delegateAuthority"))]
    #[codama(account(name = "protocolId"))]
    #[codama(account(name = "systemProgram"))]
    GrantRevokeDelegate {
        #[codama(type = number(u64))]
        permissions: &'de U64,
    },
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "userShareAccount", writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    #[codama(account(name = "vaultAuthority"))]
    Redeem {
        #[codama(type = number(u64))]
        shares: &'de U64,
        #[codama(type = number(u64))]
        nonce: &'de U64,
        #[codama(type = number(u64))]
        min_out: &'de U64,
        payout_mint: &'de Address,
    },
    #[codama(account(name = "authority", signer, writable))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint"))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "escrowTokenAccount", writable))]
    #[codama(account(name = "vaultTokenAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    FulfillRedeem,
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "escrowTokenAccount", writable))]
    #[codama(account(name = "userTokenAccount", writable))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    ClaimRedeem,
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "escrow", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "userShareAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    CancelRedeem,
    #[codama(account(name = "signer", signer))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    PriceOpaque { args: &'de PriceOpaqueArgs },
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "userTokenAccount", writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "vaultTokenAccount", writable))]
    #[codama(account(name = "userShareAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "baseTokenProgram"))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "associatedTokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    SubscribeDirect {
        #[codama(type = number(u64))]
        amount: &'de U64,
        #[codama(type = number(u64))]
        min_shares_out: &'de U64,
    },
    #[codama(account(name = "signer", signer, writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "curatorFeeAccount", writable))]
    #[codama(account(name = "protocolFeeAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "systemProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    CrystallizeFees,
    #[codama(account(name = "user", signer, writable))]
    #[codama(account(name = "userTokenAccount", writable))]
    #[codama(account(name = "userShareAccount", writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "baseMint"))]
    #[codama(account(name = "vaultTokenAccount", writable))]
    #[codama(account(name = "curatorShareAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "baseTokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    RedeemDirect {
        #[codama(type = number(u64))]
        shares: &'de U64,
        #[codama(type = number(u64))]
        min_out: &'de U64,
    },
    #[codama(account(name = "signer", signer))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    SetPaused { flags: &'de u8 },
    #[codama(account(name = "newCurator", signer))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    AcceptCurator,
    #[codama(account(name = "curator", signer, writable))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "shareMint", writable))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "systemProgram"))]
    UpdateShareMeta { args: &'de UpdateShareMetaArgs },
    #[codama(account(name = "curator", signer))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "shareMint"))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "custodyTokenAccount", writable))]
    #[codama(account(name = "destinationTokenAccount", writable))]
    #[codama(account(name = "mint"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    SweepIdle,
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "user", signer))]
    /// Empty reports read Core/OCR2 accounts; legacy nonempty reports are
    /// Chainlink frames. A [0, 0, 1] prefix selects the signed Pro envelope in
    /// `pyth_pro::ProRefreshPayload`. Pro requires a writable user and five
    /// remaining accounts before the legacy accounts: verifier, storage,
    /// writable treasury, system program, instructions sysvar.
    RefreshPrices {
        #[codama(type = bytes)]
        #[codama(size_prefix = number(u64))]
        reports: &'de [u8],
    },
    #[codama(account(name = "signer", signer))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "mint"))]
    #[codama(account(name = "signerTokenAccount", writable))]
    #[codama(account(name = "custodyTokenAccount", writable))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "instructionsSysvar"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    FlashDeposit {
        #[codama(type = number(u64))]
        amount: &'de U64,
    },
    #[codama(account(name = "signer", signer))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "mint"))]
    #[codama(account(name = "signerTokenAccount", writable))]
    #[codama(account(name = "custodyTokenAccount", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "instructionsSysvar"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    FlashWithdraw {
        #[codama(type = number(u64))]
        amount: &'de U64,
    },
    #[codama(account(name = "signer", signer, writable))]
    #[codama(account(name = "vault"))]
    #[codama(account(name = "shareMint"))]
    #[codama(account(name = "feeAccount", writable))]
    #[codama(account(name = "destination", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "metaList"))]
    #[codama(account(name = "policy"))]
    #[codama(account(name = "hookProgram"))]
    #[codama(account(name = "token2022"))]
    #[codama(account(name = "eventAuthority"))]
    #[codama(account(name = "program"))]
    ClaimFees,

    /// Remaining accounts are whatever the protocol's policy validation reads
    /// (Offerbook: the collateral mint). An empty payload closes the policy.
    #[codama(account(name = "curator", signer, writable))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "protocolPolicy", writable))]
    #[codama(account(name = "systemProgram"))]
    SetProtocolPolicy {
        args: &'de SetProtocolPolicyArgs,
        #[codama(type = bytes)]
        #[codama(size_prefix = number(u64))]
        payload: &'de [u8],
    },
    #[codama(account(name = "signer", signer))]
    #[codama(account(name = "delegate"))]
    #[codama(account(name = "vault", writable))]
    #[codama(account(name = "vaultAuthority"))]
    #[codama(account(name = "shareMint"))]
    #[codama(account(name = "vaultShareAccount", writable))]
    #[codama(account(name = "signerShareAccount", writable))]
    #[codama(account(name = "tokenProgram"))]
    #[codama(account(name = "instructionsSysvar"))]
    #[codama(account(name = "routePolicy"))]
    PrepareNestRedeem,
    #[codama(skip)]
    Event = 254,
}

impl VaultInstruction<'_> {
    pub fn discriminant(&self) -> u8 {
        // SAFETY: Because `Self` is marked `repr(u8)`, its layout is a `repr(C)` `union`
        // between `repr(C)` structs, each of which has the `u8` discriminant as its first
        // field, so we can read the discriminant without offsetting the pointer.
        unsafe { *<*const _>::from(self).cast::<u8>() }
    }
}

unsafe impl<'de, C: Config> SchemaRead<'de, C> for VaultInstruction<'de> {
    type Dst = Self;

    fn read(
        mut reader: impl Reader<'de>,
        dst: &mut core::mem::MaybeUninit<Self::Dst>,
    ) -> wincode::ReadResult<()> {
        let value = match reader.take_byte()? {
            0 => Self::InitializeVault {
                args: <&InitializeVaultArgs as SchemaRead<'de, C>>::get(reader)?,
            },
            1 => Self::UpdateVault {
                args: <&UpdateVaultArgs as SchemaRead<'de, C>>::get(reader)?,
            },
            2 => Self::CloseVault,
            3 => Self::Subscribe {
                amount: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                nonce: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                min_shares_out: <&U64 as SchemaRead<'de, C>>::get(reader)?,
            },
            4 => Self::Fulfill,
            5 => Self::Claim,
            6 => Self::Cancel,
            7 => Self::ProtocolCPI {
                data: <&[u8] as SchemaRead<'de, C>>::get(reader)?,
            },
            8 => Self::ValidateAum,
            9 => Self::SetPausedTransfers {
                paused: <&u8 as SchemaRead<'de, C>>::get(reader)?,
            },
            10 => Self::GrantRevokeDelegate {
                permissions: <&U64 as SchemaRead<'de, C>>::get(reader)?,
            },
            11 => Self::Redeem {
                shares: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                nonce: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                min_out: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                payout_mint: <&Address as SchemaRead<'de, C>>::get(reader)?,
            },
            12 => Self::FulfillRedeem,
            13 => Self::ClaimRedeem,
            14 => Self::CancelRedeem,
            15 => Self::PriceOpaque {
                args: <&PriceOpaqueArgs as SchemaRead<'de, C>>::get(reader)?,
            },
            16 => Self::SubscribeDirect {
                amount: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                min_shares_out: <&U64 as SchemaRead<'de, C>>::get(reader)?,
            },
            17 => Self::CrystallizeFees,
            18 => Self::RedeemDirect {
                shares: <&U64 as SchemaRead<'de, C>>::get(reader.by_ref())?,
                min_out: <&U64 as SchemaRead<'de, C>>::get(reader)?,
            },
            19 => Self::SetPaused {
                flags: <&u8 as SchemaRead<'de, C>>::get(reader)?,
            },
            20 => Self::AcceptCurator,
            21 => Self::UpdateShareMeta {
                args: <&UpdateShareMetaArgs as SchemaRead<'de, C>>::get(reader)?,
            },
            22 => Self::SweepIdle,
            23 => Self::RefreshPrices {
                reports: <&[u8] as SchemaRead<'de, C>>::get(reader)?,
            },
            24 => Self::FlashDeposit {
                amount: <&U64 as SchemaRead<'de, C>>::get(reader)?,
            },
            25 => Self::FlashWithdraw {
                amount: <&U64 as SchemaRead<'de, C>>::get(reader)?,
            },
            26 => Self::ClaimFees,
            27 => Self::SetProtocolPolicy {
                args: <&SetProtocolPolicyArgs as SchemaRead<'de, C>>::get(reader.by_ref())?,
                payload: <&[u8] as SchemaRead<'de, C>>::get(reader)?,
            },
            28 => Self::PrepareNestRedeem,
            254 => Self::Event,
            tag => return Err(ReadError::InvalidTagEncoding(tag as usize)),
        };
        // Borrowed wincode reads do not run the fixed fields' semantic checks.
        use crate::encoding::ZcValidate;
        match &value {
            Self::InitializeVault { args } => InitializeVaultArgs::validate_ref(args),
            Self::UpdateVault { args } => UpdateVaultArgs::validate_ref(args),
            Self::UpdateShareMeta { args } => UpdateShareMetaArgs::validate_ref(args),
            _ => Ok(()),
        }
        .map_err(|_| ReadError::InvalidValue("invalid fixed instruction field"))?;
        dst.write(value);
        Ok(())
    }
}

unsafe impl<C: Config> SchemaWrite<C> for VaultInstruction<'_> {
    type Src = Self;

    fn size_of(src: &Self::Src) -> WriteResult<usize> {
        let payload = match src {
            Self::InitializeVault { args } => {
                <&InitializeVaultArgs as SchemaWrite<C>>::size_of(args)?
            }
            Self::UpdateVault { args } => <&UpdateVaultArgs as SchemaWrite<C>>::size_of(args)?,
            Self::Subscribe {
                amount,
                nonce,
                min_shares_out,
            } => {
                <&U64 as SchemaWrite<C>>::size_of(amount)?
                    + <&U64 as SchemaWrite<C>>::size_of(nonce)?
                    + <&U64 as SchemaWrite<C>>::size_of(min_shares_out)?
            }
            Self::ProtocolCPI { data } => <&[u8] as SchemaWrite<C>>::size_of(data)?,
            Self::RefreshPrices { reports } => <&[u8] as SchemaWrite<C>>::size_of(reports)?,
            Self::SetPausedTransfers { paused } => <&u8 as SchemaWrite<C>>::size_of(paused)?,
            Self::GrantRevokeDelegate { permissions } => {
                <&U64 as SchemaWrite<C>>::size_of(permissions)?
            }
            Self::Redeem {
                shares,
                nonce,
                min_out,
                payout_mint,
            } => {
                <&U64 as SchemaWrite<C>>::size_of(shares)?
                    + <&U64 as SchemaWrite<C>>::size_of(nonce)?
                    + <&U64 as SchemaWrite<C>>::size_of(min_out)?
                    + <&Address as SchemaWrite<C>>::size_of(payout_mint)?
            }
            Self::PriceOpaque { args } => <&PriceOpaqueArgs as SchemaWrite<C>>::size_of(args)?,
            Self::SubscribeDirect {
                amount,
                min_shares_out,
            } => {
                <&U64 as SchemaWrite<C>>::size_of(amount)?
                    + <&U64 as SchemaWrite<C>>::size_of(min_shares_out)?
            }
            Self::RedeemDirect { shares, min_out } => {
                <&U64 as SchemaWrite<C>>::size_of(shares)?
                    + <&U64 as SchemaWrite<C>>::size_of(min_out)?
            }
            Self::SetPaused { flags } => <&u8 as SchemaWrite<C>>::size_of(flags)?,
            Self::FlashDeposit { amount } | Self::FlashWithdraw { amount } => {
                <&U64 as SchemaWrite<C>>::size_of(amount)?
            }
            Self::UpdateShareMeta { args } => {
                <&UpdateShareMetaArgs as SchemaWrite<C>>::size_of(args)?
            }
            Self::SetProtocolPolicy { args, payload } => {
                <&SetProtocolPolicyArgs as SchemaWrite<C>>::size_of(args)?
                    + <&[u8] as SchemaWrite<C>>::size_of(payload)?
            }
            _ => 0,
        };
        // 1 tag byte + payload.
        Ok(1 + payload)
    }

    fn write(mut writer: impl Writer, src: &Self::Src) -> WriteResult<()> {
        writer.write(&[src.discriminant()])?;
        match src {
            Self::InitializeVault { args } => {
                <&InitializeVaultArgs as SchemaWrite<C>>::write(writer.by_ref(), args)?;
            }
            Self::UpdateVault { args } => {
                <&UpdateVaultArgs as SchemaWrite<C>>::write(writer.by_ref(), args)?;
            }
            Self::Subscribe {
                amount,
                nonce,
                min_shares_out,
            } => {
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), amount)?;
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), nonce)?;
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), min_shares_out)?;
            }
            Self::ProtocolCPI { data } => {
                <&[u8] as SchemaWrite<C>>::write(writer.by_ref(), data)?;
            }
            Self::RefreshPrices { reports } => {
                <&[u8] as SchemaWrite<C>>::write(writer.by_ref(), reports)?;
            }
            Self::SetPausedTransfers { paused } => {
                <&u8 as SchemaWrite<C>>::write(writer.by_ref(), paused)?;
            }
            Self::GrantRevokeDelegate { permissions } => {
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), permissions)?;
            }
            Self::Redeem {
                shares,
                nonce,
                min_out,
                payout_mint,
            } => {
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), shares)?;
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), nonce)?;
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), min_out)?;
                <&Address as SchemaWrite<C>>::write(writer.by_ref(), payout_mint)?;
            }
            Self::PriceOpaque { args } => {
                <&PriceOpaqueArgs as SchemaWrite<C>>::write(writer.by_ref(), args)?;
            }
            Self::SubscribeDirect {
                amount,
                min_shares_out,
            } => {
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), amount)?;
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), min_shares_out)?;
            }
            Self::RedeemDirect { shares, min_out } => {
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), shares)?;
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), min_out)?;
            }
            Self::SetPaused { flags } => {
                <&u8 as SchemaWrite<C>>::write(writer.by_ref(), flags)?;
            }
            Self::FlashDeposit { amount } | Self::FlashWithdraw { amount } => {
                <&U64 as SchemaWrite<C>>::write(writer.by_ref(), amount)?;
            }
            Self::UpdateShareMeta { args } => {
                <&UpdateShareMetaArgs as SchemaWrite<C>>::write(writer.by_ref(), args)?;
            }
            Self::SetProtocolPolicy { args, payload } => {
                <&SetProtocolPolicyArgs as SchemaWrite<C>>::write(writer.by_ref(), args)?;
                <&[u8] as SchemaWrite<C>>::write(writer.by_ref(), payload)?;
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use {super::*, wincode::Deserialize};

    #[test]
    fn protocol_policy_and_nest_instruction_tags() {
        let args = SetProtocolPolicyArgs {
            protocol: 2,
            key: Address::new_from_array([7; 32]),
        };
        for payload in [&[][..], &[1, 2, 3][..]] {
            let data = wincode::serialize(&VaultInstruction::SetProtocolPolicy {
                args: &args,
                payload,
            })
            .unwrap();
            assert_eq!(data[0], 27);
            let VaultInstruction::SetProtocolPolicy {
                args: decoded,
                payload: decoded_payload,
            } = VaultInstruction::deserialize(&data).unwrap()
            else {
                panic!("tag 27 must decode as SetProtocolPolicy");
            };
            assert_eq!(decoded.protocol, args.protocol);
            assert_eq!(decoded.key, args.key);
            assert_eq!(decoded_payload, payload);
        }

        let data = wincode::serialize(&VaultInstruction::PrepareNestRedeem).unwrap();
        assert_eq!(data, [28]);
        assert!(matches!(
            VaultInstruction::deserialize(&data).unwrap(),
            VaultInstruction::PrepareNestRedeem
        ));
        assert!(VaultInstruction::deserialize(&[27]).is_err());
    }

    #[test]
    fn removed_migration_tag_is_rejected() {
        let args = SetProtocolPolicyArgs {
            protocol: 0,
            key: Address::default(),
        };
        let ix = VaultInstruction::SetProtocolPolicy {
            args: &args,
            payload: &[],
        };
        let mut data = [0u8; 42];
        wincode::serialize_into(&mut data[..], &ix).unwrap();
        assert_eq!(data[0], 27);
        assert!(VaultInstruction::deserialize(&data).is_ok());
        data[0] = 29;
        assert!(VaultInstruction::deserialize(&data).is_err());
        data[0] = 30;
        assert!(VaultInstruction::deserialize(&data).is_err());
    }
}
