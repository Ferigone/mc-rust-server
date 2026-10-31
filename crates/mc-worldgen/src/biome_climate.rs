//! The climate part of `Biome` (`getTemperature`, `coldEnoughToSnow`, ...), which the surface rules and the iceberg generation depend on.

use crate::noise::PerlinSimplexNoise;
use crate::random::LegacyRandomSource;
use anyhow::{anyhow, Result};
use serde_json::Value;
use std::sync::OnceLock;

/// `Biome.TemperatureModifier`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemperatureModifier {
    None,
    Frozen,
}

/// `Biome.ClimateSettings`
#[derive(Clone, Debug)]
pub struct BiomeClimate {
    pub has_precipitation: bool,
    pub temperature: f32,
    pub modifier: TemperatureModifier,
    pub downfall: f32,
}

struct Noises {
    temperature: PerlinSimplexNoise,
    frozen_temperature: PerlinSimplexNoise,
    biome_info: PerlinSimplexNoise,
}

/// `Biome.TEMPERATURE_NOISE`, `FROZEN_TEMPERATURE_NOISE` and `BIOME_INFO_NOISE` (fixed seeds, independent of the world seed)
fn noises() -> &'static Noises {
    static NOISES: OnceLock<Noises> = OnceLock::new();
    NOISES.get_or_init(|| Noises {
        temperature: PerlinSimplexNoise::new(&mut LegacyRandomSource::new(1234), &[0]),
        frozen_temperature: PerlinSimplexNoise::new(&mut LegacyRandomSource::new(3456), &[-2, -1, 0]),
        biome_info: PerlinSimplexNoise::new(&mut LegacyRandomSource::new(2345), &[0]),
    })
}

fn float(v: &Value, key: &str) -> Result<f32> {
    match v.get(key) {
        Some(Value::Number(n)) => n.to_string().parse::<f32>().map_err(|e| anyhow!("{key}: {e}")),
        _ => Err(anyhow!("biome: missing {key}")),
    }
}

impl BiomeClimate {
    /// Reads the climate fields of a `worldgen/biome/*.json`.
    pub fn from_json(v: &Value) -> Result<Self> {
        let modifier = match v.get("temperature_modifier").and_then(Value::as_str) {
            None | Some("none") => TemperatureModifier::None,
            Some("frozen") => TemperatureModifier::Frozen,
            Some(other) => return Err(anyhow!("unknown temperature modifier {other}")),
        };
        Ok(Self {
            has_precipitation: v.get("has_precipitation").and_then(Value::as_bool).ok_or_else(|| anyhow!("biome: missing has_precipitation"))?,
            temperature: float(v, "temperature")?,
            modifier,
            downfall: float(v, "downfall")?,
        })
    }

    fn modify_temperature(&self, x: i32, z: i32, temperature: f32) -> f32 {
        match self.modifier {
            TemperatureModifier::None => temperature,
            TemperatureModifier::Frozen => {
                let n = noises();
                let frozen = n.frozen_temperature.get_value(x as f64 * 0.05, z as f64 * 0.05, false) * 7.0;
                let info = n.biome_info.get_value(x as f64 * 0.2, z as f64 * 0.2, false);
                if frozen + info < 0.3 {
                    let extra = n.biome_info.get_value(x as f64 * 0.09, z as f64 * 0.09, false);
                    if extra < 0.8 {
                        return 0.2;
                    }
                }
                temperature
            }
        }
    }

    /// `Biome.getHeightAdjustedTemperature` (= `getTemperature`; vanilla caches it per position, the value does not change)
    pub fn temperature_at(&self, x: i32, y: i32, z: i32) -> f32 {
        let t = self.modify_temperature(x, z, self.temperature);
        if y > 80 {
            let noise = (noises().temperature.get_value((x as f32 / 8.0f32) as f64, (z as f32 / 8.0f32) as f64, false) * 8.0) as f32;
            t - (noise + y as f32 - 80.0f32) * 0.05f32 / 40.0f32
        } else {
            t
        }
    }

    /// `Biome.warmEnoughToRain`
    pub fn warm_enough_to_rain(&self, x: i32, y: i32, z: i32) -> bool {
        self.temperature_at(x, y, z) >= 0.15
    }

    /// `Biome.coldEnoughToSnow`
    pub fn cold_enough_to_snow(&self, x: i32, y: i32, z: i32) -> bool {
        !self.warm_enough_to_rain(x, y, z)
    }

    /// `Biome.shouldMeltFrozenOceanIcebergSlightly`
    pub fn should_melt_frozen_ocean_iceberg_slightly(&self, x: i32, y: i32, z: i32) -> bool {
        self.temperature_at(x, y, z) > 0.1
    }
}
