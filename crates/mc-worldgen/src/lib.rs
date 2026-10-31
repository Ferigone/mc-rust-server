//! World generation primitives ported from the vanilla 1.21.1 server (`net.minecraft.world.level.levelgen`).
//!
//! Everything here is deterministic and is checked bit for bit against the real Mojang classes by `tools/verify_noise.sh`.

pub mod mth;
pub mod noise;
pub mod random;
