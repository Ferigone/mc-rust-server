//! Writes the names of the placed features the port can place (one per line, sorted) and, with `--unsupported`, why the others cannot be.
//! `tools/verify_features.sh` hands the list to the Java reference (`DecorateRef`), which places exactly these features, so that the
//! comparison covers the whole decoration pipeline of the features the port implements.
//!   cargo run --release -p mc-worldgen --example supported_features -- <generated dir> <out file> [--unsupported]

use mc_data::GameData;
use mc_worldgen::generator::{BiomeSourceKind, NoiseGenerator};
use std::sync::Arc;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let game = Arc::new(GameData::load(std::path::Path::new(&args[1]))?);
    let generator = NoiseGenerator::new(game, -64, 384, 0, "minecraft:overworld", BiomeSourceKind::Overworld)?;
    let names = generator.decorator.supported_names(&generator.features);
    std::fs::write(&args[2], names.join("\n") + "\n")?;
    println!("{} placed features supported, {} not yet", names.len(), generator.decorator.unsupported.len());
    if args.iter().any(|a| a == "--unsupported") {
        let mut reasons: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for (_, why) in &generator.decorator.unsupported {
            *reasons.entry(why.as_str()).or_default() += 1;
        }
        for (why, n) in reasons {
            println!("{n:5}  {why}");
        }
    }
    Ok(())
}
