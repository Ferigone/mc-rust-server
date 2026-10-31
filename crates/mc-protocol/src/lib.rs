//! Warstwa protokołu Minecraft: Java Edition 1.21 / 1.21.1 (protokół 767).
//!
//! Zasady:
//! * ID pakietów pochodzą z oficjalnego raportu Mojang (`ids.rs`, generowane).
//! * Układ pól każdego pakietu jest odtworzony z zdekompilowanego kodu serwera vanilla.

pub mod buf;
pub mod error;
pub mod frame;
pub mod ids;
pub mod nbt;
pub mod packets;
pub mod text;

pub use buf::{Put, Reader};
pub use error::{Error, Result};
