//! Chunk NBT <-> in-memory chunk (`ChunkSerializer.read` / `write` of vanilla 1.21.1).
//!
//! Loading keeps what is needed to answer the client byte-for-byte like vanilla (palette order, stored light,
//! stored heightmaps, block entities) and keeps the rest of the compound (`raw`) so that saving a chunk does
//! not lose data this server does not understand yet (structures, ticks, ...).

use crate::world::{pack_bits, unpack_bits, ceillog2, Biomes, BlockEntity, Blocks, Chunk, Dimension, Heightmaps, Palette};
use anyhow::Result;
use mc_data::GameData;
use mc_protocol::nbt::{Compound, Tag};
use std::collections::HashMap;

const DATA_VERSION: i32 = 3955;

fn zero_or_none(arr: &[i8]) -> Option<Box<[u8; 2048]>> {
    if arr.len() != 2048 || arr.iter().all(|&b| b == 0) {
        return None;
    }
    let mut out = Box::new([0u8; 2048]);
    for (o, &b) in out.iter_mut().zip(arr) {
        *o = b as u8;
    }
    Some(out)
}

/// Parses a chunk compound. `Ok(None)` when the chunk is not fully generated (`Status != minecraft:full`).
pub fn chunk_from_nbt(nbt: &Compound, dim: &Dimension, pal: &Palette, data: &GameData, biome_ids: &HashMap<String, u16>) -> Result<Option<Chunk>> {
    if nbt.get_str("Status") != Some("minecraft:full") {
        return Ok(None);
    }
    let plains = biome_ids.get("minecraft:plains").copied().unwrap_or(0);
    let mut chunk = Chunk::empty(dim, pal, plains);
    let min_section = dim.min_section();
    for sec in nbt.get_list("sections").map(|l| l.as_slice()).unwrap_or(&[]) {
        let Some(sec) = sec.as_compound() else { continue };
        let idx = sec.get_i64("Y").unwrap_or(i64::MIN) as i32 - min_section;
        if idx < 0 || idx as usize >= chunk.sections.len() {
            // a light-only section just below / above the world
            let sky = sec.get_byte_array("SkyLight").and_then(|a| zero_or_none(a));
            let block = sec.get_byte_array("BlockLight").and_then(|a| zero_or_none(a));
            if sky.is_some() || block.is_some() {
                chunk.edge_light.push((idx + min_section, sky, block));
            }
            continue;
        }
        let s = &mut chunk.sections[idx as usize];
        if let Some(bs) = sec.get_compound("block_states") {
            let palette: Vec<u16> = bs
                .get_list("palette")
                .map(|l| l.as_slice())
                .unwrap_or(&[])
                .iter()
                .filter_map(Tag::as_compound)
                .map(|e| {
                    let name = e.get_str("Name").unwrap_or("minecraft:air");
                    let props = e.get_compound("Properties");
                    let it = props.into_iter().flat_map(|c| c.0.iter().filter_map(|(k, v)| match v {
                        Tag::String(sv) => Some((k.as_str(), sv.as_str())),
                        _ => None,
                    }));
                    data.blocks.state_id(name, it).unwrap_or(pal.air as u32) as u16
                })
                .collect();
            if palette.len() == 1 {
                s.blocks = Blocks::Uniform(palette[0]);
            } else if palette.len() > 1 {
                let bits = ceillog2(palette.len() as u32).max(4);
                let longs = bs.get_long_array("data").map(|v| v.as_slice()).unwrap_or(&[]);
                let idx = unpack_bits(longs, 4096, bits);
                let mut arr = Box::new([pal.air; 4096]);
                for (o, i) in arr.iter_mut().zip(idx) {
                    *o = palette.get(i as usize).copied().unwrap_or(pal.air);
                }
                s.blocks = Blocks::Full(arr);
                s.block_palette = Some(palette);
            }
        }
        s.recalc_fluid_extra(pal);
        if let Some(bi) = sec.get_compound("biomes") {
            let palette: Vec<u16> =
                bi.get_list("palette").map(|l| l.as_slice()).unwrap_or(&[]).iter().filter_map(|t| match t {
                    Tag::String(n) => Some(biome_ids.get(n).copied().unwrap_or(plains)),
                    _ => None,
                }).collect();
            if palette.len() == 1 {
                s.biomes = Biomes::Uniform(palette[0]);
            } else if palette.len() > 1 {
                let bits = ceillog2(palette.len() as u32).max(1);
                let longs = bi.get_long_array("data").map(|v| v.as_slice()).unwrap_or(&[]);
                let idx = unpack_bits(longs, 64, bits);
                let mut arr = Box::new([plains; 64]);
                for (o, i) in arr.iter_mut().zip(idx) {
                    *o = palette.get(i as usize).copied().unwrap_or(plains);
                }
                s.biomes = Biomes::Cells(arr);
                s.biome_palette = Some(palette);
            }
        }
        s.sky_light = sec.get_byte_array("SkyLight").and_then(|a| zero_or_none(a));
        s.block_light = sec.get_byte_array("BlockLight").and_then(|a| zero_or_none(a));
    }
    if let Some(hm) = nbt.get_compound("Heightmaps") {
        if let (Some(ws), Some(mb)) = (hm.get_long_array("WORLD_SURFACE"), hm.get_long_array("MOTION_BLOCKING")) {
            chunk.heightmaps = Some(Heightmaps { world_surface: ws.clone(), motion_blocking: mb.clone() });
        }
    }
    let be_types = data.registries.get("minecraft:block_entity_type");
    for be in nbt.get_list("block_entities").map(|l| l.as_slice()).unwrap_or(&[]) {
        let Some(c) = be.as_compound() else { continue };
        let (Some(id), Some(x), Some(y), Some(z)) = (c.get_str("id"), c.get_i32("x"), c.get_i32("y"), c.get_i32("z")) else { continue };
        if let Some(t) = be_types.and_then(|r| r.ids.get(id)) {
            chunk.block_entities.push(BlockEntity { type_id: *t, x, y, z, nbt: c.clone() });
        }
    }
    chunk.stored_light = nbt.get_bool("isLightOn").unwrap_or(false);
    chunk.relit = !chunk.stored_light;
    let mut raw = nbt.clone();
    raw.remove("sections");
    chunk.raw = Some(raw);
    Ok(Some(chunk))
}

fn state_nbt(state: u16, data: &GameData) -> Tag {
    let mut c = Compound::new();
    match data.blocks.block_of_state(state as u32) {
        Some(b) => {
            c.put("Name", Tag::string(b.name.clone()));
            let props = data.blocks.properties_of(state as u32);
            if !props.is_empty() {
                let mut pc = Compound::new();
                for (k, v) in props {
                    pc.put(k.clone(), Tag::string(v.clone()));
                }
                c.put("Properties", Tag::Compound(pc));
            }
        }
        None => {
            c.put("Name", Tag::string("minecraft:air"));
        }
    }
    Tag::Compound(c)
}

/// Palette order: the stored hint (entries still in use), then new entries in first-appearance order.
fn ordered_palette(values: &[u16], hint: Option<&[u16]>, front: Option<u16>) -> Vec<u16> {
    let mut seen: Vec<u16> = Vec::new();
    for &v in values {
        if !seen.contains(&v) {
            seen.push(v);
        }
    }
    match (hint, front) {
        (Some(h), _) => {
            let mut p: Vec<u16> = h.iter().copied().filter(|x| seen.contains(x)).collect();
            for v in seen {
                if !p.contains(&v) {
                    p.push(v);
                }
            }
            p
        }
        (None, Some(f)) => {
            if let Some(pos) = seen.iter().position(|&x| x == f) {
                let v = seen.remove(pos);
                seen.insert(0, v);
            }
            seen
        }
        _ => seen,
    }
}

/// Serializes a chunk. Modified chunks drop their light arrays and heightmaps and set `isLightOn` to false,
/// so vanilla recomputes them when it loads the chunk.
#[allow(clippy::too_many_arguments)]
pub fn chunk_to_nbt(chunk: &Chunk, cx: i32, cz: i32, dim: &Dimension, pal: &Palette, data: &GameData, biome_ids: &HashMap<String, u16>, game_time: i64) -> Compound {
    let _ = biome_ids;
    let mut c = chunk.raw.clone().unwrap_or_else(|| {
        let n = dim.sections();
        Compound::new()
            .with("Status", Tag::string("minecraft:full"))
            .with("InhabitedTime", Tag::Long(0))
            .with("structures", Tag::Compound(Compound::new().with("References", Tag::Compound(Compound::new())).with("starts", Tag::Compound(Compound::new()))))
            .with("block_entities", Tag::List(Vec::new()))
            .with("block_ticks", Tag::List(Vec::new()))
            .with("fluid_ticks", Tag::List(Vec::new()))
            .with("PostProcessing", Tag::List(vec![Tag::List(Vec::new()); n]))
    });
    c.put("DataVersion", Tag::Int(DATA_VERSION));
    c.put("xPos", Tag::Int(cx));
    c.put("zPos", Tag::Int(cz));
    c.put("yPos", Tag::Int(dim.min_section()));
    c.put("LastUpdate", Tag::Long(game_time));
    let keep_light = !chunk.modified && chunk.stored_light;
    c.put("isLightOn", Tag::Byte(keep_light as i8));
    if chunk.modified {
        c.put("Heightmaps", Tag::Compound(Compound::new()));
    }
    let biome_names: Vec<String> = data.dynamic_registry("minecraft:worldgen/biome").map(|r| r.entries.iter().map(|e| e.id.clone()).collect()).unwrap_or_default();
    let mut sections = Vec::with_capacity(chunk.sections.len());
    for (i, s) in chunk.sections.iter().enumerate() {
        let mut sc = Compound::new().with("Y", Tag::Byte((dim.min_section() + i as i32) as i8));
        // block states
        let mut bs = Compound::new();
        match &s.blocks {
            Blocks::Uniform(v) => {
                bs.put("palette", Tag::List(vec![state_nbt(*v, data)]));
            }
            Blocks::Full(arr) => {
                let palette = ordered_palette(&arr[..], s.block_palette.as_deref(), Some(pal.air));
                bs.put("palette", Tag::List(palette.iter().map(|&p| state_nbt(p, data)).collect()));
                let bits = ceillog2(palette.len() as u32).max(4);
                let idx = arr.iter().map(|v| palette.iter().position(|p| p == v).unwrap() as u32);
                bs.put("data", Tag::LongArray(pack_bits(idx, 4096, bits)));
            }
        }
        sc.put("block_states", Tag::Compound(bs));
        // biomes
        let name_of = |id: u16| Tag::string(biome_names.get(id as usize).cloned().unwrap_or_else(|| "minecraft:plains".into()));
        let mut bi = Compound::new();
        match &s.biomes {
            Biomes::Uniform(b) => {
                bi.put("palette", Tag::List(vec![name_of(*b)]));
            }
            Biomes::Cells(cells) => {
                let palette = ordered_palette(&cells[..], s.biome_palette.as_deref(), None);
                bi.put("palette", Tag::List(palette.iter().map(|&p| name_of(p)).collect()));
                let bits = ceillog2(palette.len() as u32).max(1);
                let idx = cells.iter().map(|v| palette.iter().position(|p| p == v).unwrap() as u32);
                bi.put("data", Tag::LongArray(pack_bits(idx, 64, bits)));
            }
        }
        sc.put("biomes", Tag::Compound(bi));
        if keep_light {
            if let Some(a) = &s.sky_light {
                sc.put("SkyLight", Tag::ByteArray(a.iter().map(|&b| b as i8).collect()));
            }
            if let Some(a) = &s.block_light {
                sc.put("BlockLight", Tag::ByteArray(a.iter().map(|&b| b as i8).collect()));
            }
        }
        sections.push(Tag::Compound(sc));
    }
    c.put("sections", Tag::List(sections));
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_palette_rules() {
        let vals = [5u16, 9, 5, 7, 9];
        assert_eq!(ordered_palette(&vals, None, None), vec![5, 9, 7]);
        assert_eq!(ordered_palette(&vals, None, Some(7)), vec![7, 5, 9]);
        assert_eq!(ordered_palette(&vals, Some(&[9, 1, 5]), None), vec![9, 5, 7]);
    }
}
