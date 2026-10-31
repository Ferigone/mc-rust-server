//! Per-state block facts and per-item facts dumped from the REAL Mojang classes (`tools/dump_props.sh`):
//! hardness, light emission / opacity, collision / outline / occlusion shapes, flags such as `isAir` or "holds a fluid".
//! Nothing in here is typed by hand: the tables are read from `block_props.tsv` and `item_props.tsv`.

use anyhow::{bail, Context, Result};
use std::path::Path;

/// Bit positions of [`StateProps::flags`] (the order of the columns written by `PropsDump.java`).
pub mod flag {
    pub const AIR: u32 = 1 << 0;
    /// `!getFluidState().isEmpty()`
    pub const FLUID: u32 = 1 << 1;
    pub const CAN_OCCLUDE: u32 = 1 << 2;
    pub const USE_SHAPE_FOR_LIGHT_OCCLUSION: u32 = 1 << 3;
    pub const RANDOMLY_TICKING: u32 = 1 << 4;
    pub const REQUIRES_CORRECT_TOOL: u32 = 1 << 5;
    pub const REPLACEABLE: u32 = 1 << 6;
    pub const SOLID: u32 = 1 << 7;
    pub const BLOCKS_MOTION: u32 = 1 << 8;
    pub const PROPAGATES_SKYLIGHT_DOWN: u32 = 1 << 9;
    pub const SOLID_RENDER: u32 = 1 << 10;
    pub const LARGE_COLLISION_SHAPE: u32 = 1 << 11;
    pub const COLLISION_FULL_BLOCK: u32 = 1 << 12;
    pub const IGNITED_BY_LAVA: u32 = 1 << 13;
    pub const SUFFOCATING: u32 = 1 << 14;
    pub const VIEW_BLOCKING: u32 = 1 << 15;
    pub const BLOCK_ENTITY: u32 = 1 << 16;
    pub const OFFSET: u32 = 1 << 17;
}

/// An axis aligned box in block coordinates: `[min_x, min_y, min_z, max_x, max_y, max_z]`.
pub type Aabb = [f64; 6];

/// Index of the six faces in [`StateProps::face_occlusion`]: `Direction.values()` order.
pub const DOWN: usize = 0;
pub const UP: usize = 1;
pub const NORTH: usize = 2;
pub const SOUTH: usize = 3;
pub const WEST: usize = 4;
pub const EAST: usize = 5;

#[derive(Debug, Clone, Default)]
pub struct StateProps {
    pub flags: u32,
    pub light_emission: u8,
    /// `BlockState.getLightBlock`: how much light the state absorbs (0..=15).
    pub light_block: u8,
    pub push_reaction: u8,
    pub map_color: u8,
    /// `getDestroySpeed`; negative = unbreakable.
    pub destroy_speed: f32,
    pub explosion_resistance: f32,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    collision: u16,
    outline: u16,
    occlusion: u16,
    support: u16,
    faces: [u16; 6],
}

impl StateProps {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Default)]
pub struct BlockProps {
    states: Vec<StateProps>,
    shapes: Vec<Vec<Aabb>>,
}

impl BlockProps {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("{} (run tools/dump_props.sh)", path.display()))?;
        let mut shapes: Vec<Vec<Aabb>> = Vec::new();
        let mut states: Vec<StateProps> = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            let ctx = || format!("{}:{}", path.display(), n + 1);
            match f[0] {
                "S" => {
                    let idx: usize = f[1].parse().with_context(ctx)?;
                    let count: usize = f[2].parse().with_context(ctx)?;
                    if f.len() != 3 + 6 * count || idx != shapes.len() {
                        bail!("bad shape line at {}", ctx());
                    }
                    let mut boxes = Vec::with_capacity(count);
                    for b in 0..count {
                        let mut a = [0f64; 6];
                        for (k, v) in a.iter_mut().enumerate() {
                            *v = f[3 + b * 6 + k].parse().with_context(ctx)?;
                        }
                        boxes.push(a);
                    }
                    shapes.push(boxes);
                }
                "B" => {
                    if f.len() != 22 {
                        bail!("bad state line at {}", ctx());
                    }
                    let id: usize = f[1].parse().with_context(ctx)?;
                    if id != states.len() {
                        bail!("state ids must be contiguous at {}", ctx());
                    }
                    let u = |i: usize| -> Result<u16> { f[i].parse().with_context(ctx) };
                    let fl = |i: usize| -> Result<f32> { f[i].parse().with_context(ctx) };
                    states.push(StateProps {
                        flags: f[2].parse().with_context(ctx)?,
                        light_emission: u(3)? as u8,
                        light_block: u(4)? as u8,
                        destroy_speed: fl(5)?,
                        explosion_resistance: fl(6)?,
                        friction: fl(7)?,
                        speed_factor: fl(8)?,
                        jump_factor: fl(9)?,
                        push_reaction: u(10)? as u8,
                        map_color: u(11)? as u8,
                        collision: u(12)?,
                        outline: u(13)?,
                        occlusion: u(14)?,
                        support: u(15)?,
                        faces: [u(16)?, u(17)?, u(18)?, u(19)?, u(20)?, u(21)?],
                    });
                }
                _ => bail!("unknown line kind at {}", ctx()),
            }
        }
        Ok(Self { states, shapes })
    }

    pub fn len(&self) -> usize {
        self.states.len()
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Properties of a block state (`None` for an unknown ID).
    pub fn state(&self, id: u32) -> Option<&StateProps> {
        self.states.get(id as usize)
    }

    pub fn shape(&self, index: u16) -> &[Aabb] {
        &self.shapes[index as usize]
    }

    /// `BlockState.getCollisionShape` (with an empty collision context).
    pub fn collision_shape(&self, id: u32) -> &[Aabb] {
        self.state(id).map_or(&[], |s| self.shape(s.collision))
    }

    /// `BlockState.getShape`: the outline the client draws and ray casts against.
    pub fn outline_shape(&self, id: u32) -> &[Aabb] {
        self.state(id).map_or(&[], |s| self.shape(s.outline))
    }

    /// `BlockState.getOcclusionShape`.
    pub fn occlusion_shape(&self, id: u32) -> &[Aabb] {
        self.state(id).map_or(&[], |s| self.shape(s.occlusion))
    }

    /// `BlockState.getBlockSupportShape`.
    pub fn support_shape(&self, id: u32) -> &[Aabb] {
        self.state(id).map_or(&[], |s| self.shape(s.support))
    }

    /// `BlockState.getFaceOcclusionShape` for a face (`DOWN`, `UP`, `NORTH`, ...).
    pub fn face_occlusion_shape(&self, id: u32, face: usize) -> &[Aabb] {
        self.state(id).map_or(&[], |s| self.shape(s.faces[face]))
    }
}

#[derive(Debug, Clone)]
pub struct ItemInfo {
    pub name: String,
    pub max_stack_size: i32,
    pub max_damage: i32,
    /// The block a `BlockItem` places.
    pub block: Option<String>,
}

#[derive(Default)]
pub struct ItemProps {
    pub items: Vec<ItemInfo>,
}

impl ItemProps {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("{} (run tools/dump_props.sh)", path.display()))?;
        let mut items = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() != 5 || f[0].parse::<usize>().ok() != Some(items.len()) {
                bail!("bad item line {} in {}", n + 1, path.display());
            }
            items.push(ItemInfo {
                name: f[1].to_owned(),
                max_stack_size: f[2].parse()?,
                max_damage: f[3].parse()?,
                block: (f[4] != "-").then(|| f[4].to_owned()),
            });
        }
        Ok(Self { items })
    }

    pub fn get(&self, id: i32) -> Option<&ItemInfo> {
        usize::try_from(id).ok().and_then(|i| self.items.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Blocks;

    /// Runs only when the reference data has been generated (tools/fetch_reference.sh + tools/dump_props.sh).
    fn load() -> Option<(Blocks, BlockProps, ItemProps)> {
        let reports = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../reference/gen/generated/reports");
        if !reports.join("block_props.tsv").exists() {
            return None;
        }
        Some((Blocks::load(&reports.join("blocks.json")).unwrap(), BlockProps::load(&reports.join("block_props.tsv")).unwrap(), ItemProps::load(&reports.join("item_props.tsv")).unwrap()))
    }

    const FULL_CUBE: [f64; 6] = [0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

    #[test]
    fn tables_cover_every_state_and_known_facts_hold() {
        let Some((blocks, props, items)) = load() else { return };
        assert_eq!(props.len(), blocks.state_count as usize);
        let state = |name: &str, p: &[(&str, &str)]| blocks.state_id(name, p.iter().copied()).unwrap();
        // values known from the game: hardness / blast resistance / light
        let stone = props.state(state("minecraft:stone", &[])).unwrap();
        assert_eq!((stone.destroy_speed, stone.explosion_resistance, stone.light_block, stone.light_emission), (1.5, 6.0, 15, 0));
        assert!(stone.has(flag::SOLID_RENDER) && stone.has(flag::CAN_OCCLUDE) && stone.has(flag::COLLISION_FULL_BLOCK));
        assert_eq!(props.collision_shape(state("minecraft:stone", &[])), &[FULL_CUBE]);
        assert!(props.state(state("minecraft:bedrock", &[])).unwrap().destroy_speed < 0.0);
        assert_eq!(props.state(state("minecraft:glowstone", &[])).unwrap().light_emission, 15);
        assert_eq!(props.state(state("minecraft:torch", &[])).unwrap().light_emission, 14);
        assert_eq!(props.state(state("minecraft:glass", &[])).unwrap().light_block, 0);
        // shapes: slabs, torches (no collision), fences are taller than a block
        assert_eq!(props.collision_shape(state("minecraft:oak_slab", &[("type", "bottom")])), &[[0.0, 0.0, 0.0, 1.0, 0.5, 1.0]]);
        assert_eq!(props.collision_shape(state("minecraft:oak_slab", &[("type", "top")])), &[[0.0, 0.5, 0.0, 1.0, 1.0, 1.0]]);
        assert!(props.collision_shape(state("minecraft:torch", &[])).is_empty());
        assert_eq!(props.collision_shape(state("minecraft:oak_fence", &[])).iter().map(|b| b[4]).fold(0.0, f64::max), 1.5);
        // air states and fluid states (the counts were also cross-checked against BlockState.isAir / getFluidState)
        let air: Vec<usize> = (0..props.len()).filter(|&i| props.state(i as u32).unwrap().has(flag::AIR)).collect();
        assert_eq!(air, vec![0, 12958, 12959]);
        assert_eq!((0..props.len()).filter(|&i| props.state(i as u32).unwrap().has(flag::FLUID)).count(), 9246);
        // items
        let stone_item = items.get(1).unwrap();
        assert_eq!((stone_item.name.as_str(), stone_item.max_stack_size, stone_item.block.as_deref()), ("minecraft:stone", 64, Some("minecraft:stone")));
        let sword = items.items.iter().find(|i| i.name == "minecraft:diamond_sword").unwrap();
        assert_eq!((sword.max_stack_size, sword.max_damage, sword.block.clone()), (1, 1561, None));
        assert_eq!(items.items.iter().find(|i| i.name == "minecraft:ender_pearl").unwrap().max_stack_size, 16);
    }
}
