use {
    crate::constants::HOOK_SEED_EXTRA_ACCOUNT_METAS, solana_address::Address,
    solana_instruction_view::cpi::Seed,
};

/// Anything that can be spliced into a PDA seed list as raw bytes.
pub trait AsSeed {
    fn as_seed(&self) -> &[u8];
}

impl AsSeed for Address {
    fn as_seed(&self) -> &[u8] {
        self.as_array()
    }
}

impl AsSeed for [u8] {
    fn as_seed(&self) -> &[u8] {
        self
    }
}

impl<const N: usize> AsSeed for [u8; N] {
    fn as_seed(&self) -> &[u8] {
        self
    }
}

/// Declares a PDA seed struct plus its three accessors from a single seed list:
/// `Name = [prefix, field: Ty, ... => optional_suffix]`.
macro_rules! seeds {
    ($(
        $(#[$doc:meta])*
        $name:ident = [$prefix:expr $(, $field:ident: $ty:ty)* $(=> $suffix:expr)?];
    )*) => {$(
        $(#[$doc])*
        pub struct $name<'a> {
            $(pub $field: &'a $ty,)*
        }

        impl<'a> $name<'a> {
            pub fn as_seeds(&'a self) -> [&'a [u8]; seeds!(@len $($field)* $(| $suffix)?)] {
                [$prefix, $(self.$field.as_seed(),)* $($suffix,)?]
            }

            pub fn as_seeds_with_bump(
                &'a self,
                bump: &'a [u8],
            ) -> [&'a [u8]; seeds!(@len $($field)* $(| $suffix)?) + 1] {
                [$prefix, $(self.$field.as_seed(),)* $($suffix,)? bump]
            }

            pub fn signer_seeds_with_bump(
                &'a self,
                bump: &'a [u8],
            ) -> [Seed<'a>; seeds!(@len $($field)* $(| $suffix)?) + 1] {
                [
                    Seed::from($prefix),
                    $(Seed::from(self.$field.as_seed()),)*
                    $(Seed::from($suffix),)?
                    Seed::from(bump),
                ]
            }
        }
    )*};
    // prefix + one per field + optional suffix
    (@len $($field:ident)* $(| $suffix:expr)?) => {
        1usize $(+ seeds!(@one $field))* $(+ seeds!(@one_expr $suffix))?
    };
    (@one $_field:ident) => { 1usize };
    (@one_expr $_e:expr) => { 1usize };
}

seeds! {
    /// `["vault", curator, created_at]`
    VaultSeeds = [b"vault", curator: Address, created_at: [u8; 8]];

    /// `["authority", vault]` — the vault's signing authority.
    AuthoritySeeds = [b"authority", vault: Address];

    /// `["share", vault]` — the vault's share mint.
    ShareMintSeeds = [b"share", vault: Address];

    /// `["policy", vault]`
    PolicySeeds = [b"policy", vault: Address];

    /// `["extra-account-metas", mint]` on the **hook** program
    /// ([`HOOK_ID`](crate::constants::HOOK_ID)) — the SPL transfer-hook
    /// `ExtraAccountMetaList` PDA. Token-2022 reads it at transfer time to learn
    /// which extra accounts (here: the vault's `Policy`) to forward to the hook.
    /// One per share mint.
    ExtraAccountMetasSeeds = [HOOK_SEED_EXTRA_ACCOUNT_METAS, mint: Address];

    /// `["delegate", vault, delegate, protocol_id]`
    DelegateSeeds = [b"delegate", vault: Address, delegate: Address, protocol_id: Address];

    /// `["escrow", vault, owner, nonce]`
    EscrowSeeds = [b"escrow", vault: Address, owner: Address, nonce: [u8]];

    /// `["escrow", vault, owner, nonce, "ta"]` — the escrow's token account.
    EscrowTokenAccountSeeds =
        [b"escrow", vault: Address, owner: Address, nonce: [u8] => b"ta"];

    /// `["escrow", vault, owner, nonce, "shares"]` — fulfilled-but-not-yet-claimed
    /// share token account.
    EscrowShareAccountSeeds =
        [b"escrow", vault: Address, owner: Address, nonce: [u8] => b"shares"];

    /// `["fee", vault, "curator"]` — accrued management + performance shares.
    CuratorFeeAccountSeeds = [b"fee", vault: Address => b"curator"];

    /// `["fee", vault, "protocol"]` — accrued protocol management +
    /// performance fee shares.
    ProtocolFeeAccountSeeds = [b"fee", vault: Address => b"protocol"];
}
