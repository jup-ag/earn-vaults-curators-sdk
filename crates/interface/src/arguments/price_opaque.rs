use {
    codama_macros::CodamaType,
    wincode::{SchemaRead, SchemaWrite},
    zeropod::U64,
};

/// `slot_idx` bit that re-targets the write from a position slot to a hand-priced
/// asset row (`ASSET_FLAG_MANUAL_PRICE`): pass `ASSET_TARGET | asset_idx`, and
/// `value` is then a **price** in numéraire micro-units per whole token
/// ([`MANUAL_PRICE_EXPO`](crate::constants::MANUAL_PRICE_EXPO)) rather than the
/// slot's whole worth.
pub const ASSET_TARGET: u8 = 1 << 7;

/// Retarget `slot_idx` to a [`SinkSlot`](crate::state::SinkSlot).
pub const SINK_TARGET: u8 = 1 << 6;

/// Mask for the index after removing target bits.
pub const TARGET_INDEX_MASK: u8 = !(ASSET_TARGET | SINK_TARGET);

#[derive(SchemaRead, SchemaWrite, CodamaType)]
#[wincode(assert_zero_copy)]
#[repr(C)]
pub struct PriceOpaqueArgs {
    pub slot_idx: u8,
    #[codama(type = number(u64))]
    pub value: U64,
}
