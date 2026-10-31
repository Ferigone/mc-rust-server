//! `playerdata/<uuid>.dat`: per-player saved state (gzip NBT). The stored compound is kept as the base when
//! saving, so fields this server does not manage yet (attributes, brain, recipe book, ender chest, ...)
//! round-trip unchanged.

use anyhow::Result;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use mc_protocol::nbt::{Compound, Tag};
use mc_protocol::Reader;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const DATA_VERSION: i32 = 3955;

pub fn path(dir: &Path, uuid: Uuid) -> PathBuf {
    dir.join(format!("{uuid}.dat"))
}

pub fn load(dir: &Path, uuid: Uuid) -> Option<Compound> {
    let p = path(dir, uuid);
    let mut raw = Vec::new();
    GzDecoder::new(std::fs::File::open(p).ok()?).read_to_end(&mut raw).ok()?;
    match Tag::read_named(&mut Reader::new(&raw)).ok()? {
        Some((_, Tag::Compound(c))) => Some(c),
        _ => None,
    }
}

pub fn save(dir: &Path, uuid: Uuid, nbt: &Compound) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut raw = Vec::new();
    Tag::Compound(nbt.clone()).write_named("", &mut raw);
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&raw)?;
    let bytes = enc.finish()?;
    // like vanilla: write a temporary file, keep the previous one as `.dat_old`, then rename
    let p = path(dir, uuid);
    let tmp = dir.join(format!("{uuid}.dat.tmp"));
    std::fs::write(&tmp, bytes)?;
    if p.exists() {
        let old = dir.join(format!("{uuid}.dat_old"));
        let _ = std::fs::remove_file(&old);
        std::fs::rename(&p, &old)?;
    }
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// A fresh player's saved state with the fields vanilla writes for a new player.
pub fn new_player(uuid: Uuid, pos: (f64, f64, f64), rotation: (f32, f32), game_type: i32) -> Compound {
    let u = uuid.as_u128();
    let ints = vec![(u >> 96) as i32, (u >> 64) as i32, (u >> 32) as i32, u as i32];
    let creative = game_type == 1;
    let spectator = game_type == 3;
    Compound::new()
        .with("DataVersion", Tag::Int(DATA_VERSION))
        .with("UUID", Tag::IntArray(ints))
        .with("Pos", Tag::List(vec![Tag::Double(pos.0), Tag::Double(pos.1), Tag::Double(pos.2)]))
        .with("Motion", Tag::List(vec![Tag::Double(0.0), Tag::Double(0.0), Tag::Double(0.0)]))
        .with("Rotation", Tag::List(vec![Tag::Float(rotation.0), Tag::Float(rotation.1)]))
        .with("Dimension", Tag::string("minecraft:overworld"))
        .with("playerGameType", Tag::Int(game_type))
        .with("Health", Tag::Float(20.0))
        .with("foodLevel", Tag::Int(20))
        .with("foodSaturationLevel", Tag::Float(5.0))
        .with("foodExhaustionLevel", Tag::Float(0.0))
        .with("foodTickTimer", Tag::Int(0))
        .with("XpLevel", Tag::Int(0))
        .with("XpP", Tag::Float(0.0))
        .with("XpTotal", Tag::Int(0))
        .with("XpSeed", Tag::Int(0))
        .with("Score", Tag::Int(0))
        .with("Air", Tag::Short(300))
        .with("Fire", Tag::Short(-20))
        .with("FallDistance", Tag::Float(0.0))
        .with("OnGround", Tag::Byte(0))
        .with("Invulnerable", Tag::Byte(0))
        .with("SelectedItemSlot", Tag::Int(0))
        .with("Inventory", Tag::List(Vec::new()))
        .with("EnderItems", Tag::List(Vec::new()))
        .with(
            "abilities",
            Tag::Compound(
                Compound::new()
                    .with("invulnerable", Tag::Byte((creative || spectator) as i8))
                    .with("mayfly", Tag::Byte((creative || spectator) as i8))
                    .with("instabuild", Tag::Byte(creative as i8))
                    .with("flying", Tag::Byte(spectator as i8))
                    .with("mayBuild", Tag::Byte(!spectator as i8))
                    .with("walkSpeed", Tag::Float(0.1))
                    .with("flySpeed", Tag::Float(0.05)),
            ),
        )
}

/// Menu slot (`InventoryMenu`) of a stored inventory `Slot` byte; `None` for unknown slots.
pub fn menu_slot(stored: i32) -> Option<usize> {
    match stored {
        0..=8 => Some(36 + stored as usize),
        9..=35 => Some(stored as usize),
        100 => Some(8), // feet
        101 => Some(7), // legs
        102 => Some(6), // chest
        103 => Some(5), // head
        -106 => Some(45), // offhand
        _ => None,
    }
}

/// Inverse of [`menu_slot`].
pub fn stored_slot(menu: usize) -> Option<i8> {
    match menu {
        36..=44 => Some((menu - 36) as i8),
        9..=35 => Some(menu as i8),
        8 => Some(100),
        7 => Some(101),
        6 => Some(102),
        5 => Some(103),
        45 => Some(-106),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_mapping_roundtrip() {
        for menu in [5usize, 6, 7, 8, 9, 20, 35, 36, 40, 44, 45] {
            let s = stored_slot(menu).unwrap();
            assert_eq!(menu_slot(s as i32), Some(menu));
        }
        assert_eq!(menu_slot(2), Some(38));
        assert_eq!(menu_slot(77), None);
    }

    #[test]
    fn save_load_roundtrip() {
        let d = std::env::temp_dir().join(format!("mc-pd-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let u = Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788);
        let c = new_player(u, (1.5, 64.0, -2.5), (90.0, 10.0), 1);
        save(&d, u, &c).unwrap();
        let back = load(&d, u).unwrap();
        assert_eq!(back.get_list("Pos").unwrap().len(), 3);
        assert_eq!(back.get_i32("playerGameType"), Some(1));
        assert!(load(&d, Uuid::nil()).is_none());
    }
}
