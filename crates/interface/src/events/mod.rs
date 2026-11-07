mod admin;
mod aum;
mod deposit;
mod flash;
mod initialize_vault;
mod redemption;

use wincode::{SchemaRead, SchemaWrite, ZeroCopy};
pub use {admin::*, aum::*, deposit::*, flash::*, initialize_vault::*, redemption::*};

/// Discriminated union of every emittable event. Serialized as
/// `[variant tag][fields]` and wrapped in a self-CPI by the program's
/// `emit_event` helper; indexers deserialize it off the inner instruction.
#[derive(SchemaRead, SchemaWrite)]
#[wincode(tag_encoding = "u8")]
#[repr(u8)]
pub enum VaultEvent {
    InitializeVault,
    Deposited,
    SubscribeRequested,
    SubscribeFulfilled,
    SubscribeCancelled,
    RedeemRequested,
    RedeemFulfilled,
    Redeemed,
    RedeemCancelled,
    AumValidated,
    RedeemedInstant,
    Priced,
    FeeAccrued,
    Paused,
    CuratorRotated,
    Swept,
    AssetPriced,
    FlashMoved,
    FeesClaimed,
}

const _: () = assert!(VaultEvent::FeeAccrued as u8 == 12);
const _: () = assert!(VaultEvent::FeesClaimed as u8 == 18);

/// Zero-copy event wrapper: `[tag byte][payload]`.
#[derive(SchemaRead, SchemaWrite)]
#[repr(C)]
pub struct Event<T: ZeroCopy> {
    pub ty: u8,
    pub data: T,
}

// SAFETY: `Event<T>` is `#[repr(C)]` = a `u8` tag then `T`. A zero-copy `T`
// has no invalid bit patterns; when `T` is also align-1 there is no padding
// after the tag, so every byte image of `Event<T>` is a valid value. Every
// event payload is built from align-1 zeropod Pods + `[u8; N]` addresses, and
// the asserts below pin the no-padding invariant for the instantiations we
// actually emit — instantiating with an align>1 `T` would add padding and
// must not be done.
unsafe impl<T: ZeroCopy> wincode::config::ZeroCopy<wincode::config::DefaultConfig> for Event<T> {}

impl<T: ZeroCopy> Event<T> {
    pub fn new(ty: VaultEvent, data: T) -> Self {
        Self { ty: ty as u8, data }
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        zeropod::{I64, U64},
    };

    #[test]
    fn fee_event_has_expected_wire_layout() {
        assert_eq!(core::mem::size_of::<FeeAccrued>(), 88);
        assert_eq!(core::mem::size_of::<Event<FeeAccrued>>(), 89);

        let vault = solana_address::Address::new_from_array([7; 32]);
        let event = Event::new(
            VaultEvent::FeeAccrued,
            FeeAccrued {
                vault,
                curator_management: U64::from(1u64),
                protocol_management: U64::from(2u64),
                curator_performance: U64::from(3u64),
                protocol_performance: U64::from(4u64),
                curator_shares: U64::from(5u64),
                protocol_shares: U64::from(6u64),
                timestamp: I64::from(7i64),
            },
        );
        // SAFETY: Event<T> is asserted zero-copy and align-1 above; view its
        // fixed wire representation without extending the value's lifetime.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                (&raw const event).cast::<u8>(),
                core::mem::size_of_val(&event),
            )
        };
        assert_eq!(bytes[0], 12);
        assert_eq!(&bytes[1..33], vault.as_array());
        for (i, value) in (1u64..=7).enumerate() {
            let start = 33 + i * 8;
            assert_eq!(&bytes[start..start + 8], &value.to_le_bytes());
        }
    }
}
