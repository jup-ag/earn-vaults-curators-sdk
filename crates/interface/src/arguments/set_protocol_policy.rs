use {
    codama_macros::CodamaType,
    solana_address::Address,
    wincode::{SchemaRead, SchemaWrite},
};

/// Which [`ProtocolPolicy`](crate::state::ProtocolPolicy) `SetProtocolPolicy` writes: `protocol` is a
/// [`Protocol`](crate::state::Protocol) discriminant, `key` the adapter's key.
#[derive(SchemaRead, SchemaWrite, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct SetProtocolPolicyArgs {
    pub protocol: u8,
    pub key: Address,
}
