//! Minecraft: Java Edition 1.21 / 1.21.1 protocol layer (protocol 767).
//!
//! Zasady:
//! * Packet IDs come from Mojang's official report (`ids.rs`, generated).
//! * The field layout of every packet was reconstructed from the decompiled vanilla server.

pub mod buf;
pub mod error;
pub mod frame;
pub mod ids;
pub mod nbt;
pub mod packets;
pub mod text;

pub use buf::{Put, Reader};
pub use error::{Error, Result};
