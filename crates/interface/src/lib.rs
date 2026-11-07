#![no_std]

use solana_address::Address;

pub mod arguments;
pub mod constants;
pub mod encoding;
pub mod errors;
pub mod events;
pub mod nav;
pub mod ocr2;
pub mod program_data;
pub mod pyth_pro;
pub mod seeds;
pub mod state;

mod instructions;
pub mod utils;

pub use {instructions::VaultInstruction, zeropod};

pub const ID: Address = Address::from_str_const("JUPVau1tSdENvv7KCwFY6Geer1a442q9mAfmaRXm83g");
