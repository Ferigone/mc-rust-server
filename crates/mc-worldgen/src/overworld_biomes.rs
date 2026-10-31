//! `OverworldBiomeBuilder` and the presets of `MultiNoiseBiomeSourceParameterList`: the climate-space table that maps the six noise
//! values to a biome. Ported rule by rule (the vanilla server builds the very same list at start-up).

use crate::biomes as b;
use crate::climate::{Parameter, ParameterList, ParameterPoint};

type P = Parameter;
type Biome = &'static str;
type Out = Vec<(ParameterPoint, Biome)>;

fn span(a: f32, z: f32) -> P {
    Parameter::span(a, z)
}

fn join(a: P, z: P) -> P {
    Parameter::span_of(a, z)
}

/// `OverworldBiomeBuilder`
pub struct OverworldBiomeBuilder {
    full: P,
    temperatures: [P; 5],
    humidities: [P; 5],
    erosions: [P; 7],
    frozen: P,
    unfrozen: P,
    mushroom_fields: P,
    deep_ocean: P,
    ocean: P,
    coast: P,
    inland: P,
    near_inland: P,
    mid_inland: P,
    far_inland: P,
    oceans: [[Biome; 5]; 2],
    middle: [[Biome; 5]; 5],
    middle_variant: [[Option<Biome>; 5]; 5],
    plateau: [[Biome; 5]; 5],
    plateau_variant: [[Option<Biome>; 5]; 5],
    shattered: [[Option<Biome>; 5]; 5],
}

impl Default for OverworldBiomeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl OverworldBiomeBuilder {
    pub fn new() -> Self {
        let temperatures = [span(-1.0, -0.45), span(-0.45, -0.15), span(-0.15, 0.2), span(0.2, 0.55), span(0.55, 1.0)];
        let n = None;
        Self {
            full: span(-1.0, 1.0),
            temperatures,
            humidities: [span(-1.0, -0.35), span(-0.35, -0.1), span(-0.1, 0.1), span(0.1, 0.3), span(0.3, 1.0)],
            erosions: [
                span(-1.0, -0.78),
                span(-0.78, -0.375),
                span(-0.375, -0.2225),
                span(-0.2225, 0.05),
                span(0.05, 0.45),
                span(0.45, 0.55),
                span(0.55, 1.0),
            ],
            frozen: temperatures[0],
            unfrozen: join(temperatures[1], temperatures[4]),
            mushroom_fields: span(-1.2, -1.05),
            deep_ocean: span(-1.05, -0.455),
            ocean: span(-0.455, -0.19),
            coast: span(-0.19, -0.11),
            inland: span(-0.11, 0.55),
            near_inland: span(-0.11, 0.03),
            mid_inland: span(0.03, 0.3),
            far_inland: span(0.3, 1.0),
            oceans: [
                [b::DEEP_FROZEN_OCEAN, b::DEEP_COLD_OCEAN, b::DEEP_OCEAN, b::DEEP_LUKEWARM_OCEAN, b::WARM_OCEAN],
                [b::FROZEN_OCEAN, b::COLD_OCEAN, b::OCEAN, b::LUKEWARM_OCEAN, b::WARM_OCEAN],
            ],
            middle: [
                [b::SNOWY_PLAINS, b::SNOWY_PLAINS, b::SNOWY_PLAINS, b::SNOWY_TAIGA, b::TAIGA],
                [b::PLAINS, b::PLAINS, b::FOREST, b::TAIGA, b::OLD_GROWTH_SPRUCE_TAIGA],
                [b::FLOWER_FOREST, b::PLAINS, b::FOREST, b::BIRCH_FOREST, b::DARK_FOREST],
                [b::SAVANNA, b::SAVANNA, b::FOREST, b::JUNGLE, b::JUNGLE],
                [b::DESERT, b::DESERT, b::DESERT, b::DESERT, b::DESERT],
            ],
            middle_variant: [
                [Some(b::ICE_SPIKES), n, Some(b::SNOWY_TAIGA), n, n],
                [n, n, n, n, Some(b::OLD_GROWTH_PINE_TAIGA)],
                [Some(b::SUNFLOWER_PLAINS), n, n, Some(b::OLD_GROWTH_BIRCH_FOREST), n],
                [n, n, Some(b::PLAINS), Some(b::SPARSE_JUNGLE), Some(b::BAMBOO_JUNGLE)],
                [n, n, n, n, n],
            ],
            plateau: [
                [b::SNOWY_PLAINS, b::SNOWY_PLAINS, b::SNOWY_PLAINS, b::SNOWY_TAIGA, b::SNOWY_TAIGA],
                [b::MEADOW, b::MEADOW, b::FOREST, b::TAIGA, b::OLD_GROWTH_SPRUCE_TAIGA],
                [b::MEADOW, b::MEADOW, b::MEADOW, b::MEADOW, b::DARK_FOREST],
                [b::SAVANNA_PLATEAU, b::SAVANNA_PLATEAU, b::FOREST, b::FOREST, b::JUNGLE],
                [b::BADLANDS, b::BADLANDS, b::BADLANDS, b::WOODED_BADLANDS, b::WOODED_BADLANDS],
            ],
            plateau_variant: [
                [Some(b::ICE_SPIKES), n, n, n, n],
                [Some(b::CHERRY_GROVE), n, Some(b::MEADOW), Some(b::MEADOW), Some(b::OLD_GROWTH_PINE_TAIGA)],
                [Some(b::CHERRY_GROVE), Some(b::CHERRY_GROVE), Some(b::FOREST), Some(b::BIRCH_FOREST), n],
                [n, n, n, n, n],
                [Some(b::ERODED_BADLANDS), Some(b::ERODED_BADLANDS), n, n, n],
            ],
            shattered: [
                [Some(b::WINDSWEPT_GRAVELLY_HILLS), Some(b::WINDSWEPT_GRAVELLY_HILLS), Some(b::WINDSWEPT_HILLS), Some(b::WINDSWEPT_FOREST), Some(b::WINDSWEPT_FOREST)],
                [Some(b::WINDSWEPT_GRAVELLY_HILLS), Some(b::WINDSWEPT_GRAVELLY_HILLS), Some(b::WINDSWEPT_HILLS), Some(b::WINDSWEPT_FOREST), Some(b::WINDSWEPT_FOREST)],
                [Some(b::WINDSWEPT_HILLS), Some(b::WINDSWEPT_HILLS), Some(b::WINDSWEPT_HILLS), Some(b::WINDSWEPT_FOREST), Some(b::WINDSWEPT_FOREST)],
                [n, n, n, n, n],
                [n, n, n, n, n],
            ],
        }
    }

    /// `spawnTarget`: where the world spawn is searched (inland, near the surface).
    pub fn spawn_target(&self) -> Vec<ParameterPoint> {
        let zero = Parameter::point(0.0);
        vec![
            ParameterPoint { temperature: self.full, humidity: self.full, continentalness: join(self.inland, self.full), erosion: self.full, depth: zero, weirdness: span(-1.0, -0.16), offset: 0 },
            ParameterPoint { temperature: self.full, humidity: self.full, continentalness: join(self.inland, self.full), erosion: self.full, depth: zero, weirdness: span(0.16, 1.0), offset: 0 },
        ]
    }

    /// `addBiomes`
    pub fn add_biomes(&self, out: &mut Out) {
        self.add_off_coast_biomes(out);
        self.add_inland_biomes(out);
        self.add_underground_biomes(out);
    }

    /// The overworld preset (`MultiNoiseBiomeSourceParameterList.Preset.OVERWORLD`)
    pub fn overworld_list() -> ParameterList<Biome> {
        let mut out = Vec::new();
        Self::new().add_biomes(&mut out);
        ParameterList::new(out)
    }

    fn add_off_coast_biomes(&self, out: &mut Out) {
        let f = self.full;
        self.add_surface(out, f, f, self.mushroom_fields, f, f, 0.0, b::MUSHROOM_FIELDS);
        for i in 0..self.temperatures.len() {
            let t = self.temperatures[i];
            self.add_surface(out, t, f, self.deep_ocean, f, f, 0.0, self.oceans[0][i]);
            self.add_surface(out, t, f, self.ocean, f, f, 0.0, self.oceans[1][i]);
        }
    }

    fn add_inland_biomes(&self, out: &mut Out) {
        self.add_mid_slice(out, span(-1.0, -0.93333334));
        self.add_high_slice(out, span(-0.93333334, -0.7666667));
        self.add_peaks(out, span(-0.7666667, -0.56666666));
        self.add_high_slice(out, span(-0.56666666, -0.4));
        self.add_mid_slice(out, span(-0.4, -0.26666668));
        self.add_low_slice(out, span(-0.26666668, -0.05));
        self.add_valleys(out, span(-0.05, 0.05));
        self.add_low_slice(out, span(0.05, 0.26666668));
        self.add_mid_slice(out, span(0.26666668, 0.4));
        self.add_high_slice(out, span(0.4, 0.56666666));
        self.add_peaks(out, span(0.56666666, 0.7666667));
        self.add_high_slice(out, span(0.7666667, 0.93333334));
        self.add_mid_slice(out, span(0.93333334, 1.0));
    }

    fn add_peaks(&self, out: &mut Out, w: P) {
        let (e, coast, near, mid, far) = (&self.erosions, self.coast, self.near_inland, self.mid_inland, self.far_inland);
        for ti in 0..5 {
            let t = self.temperatures[ti];
            for hi in 0..5 {
                let h = self.humidities[hi];
                let middle = self.pick_middle(ti, hi, w);
                let middle_or_badlands = self.pick_middle_or_badlands_if_hot(ti, hi, w);
                let middle_badlands_slope = self.pick_middle_or_badlands_if_hot_or_slope_if_cold(ti, hi, w);
                let plateau = self.pick_plateau(ti, hi, w);
                let shattered = self.pick_shattered(ti, hi, w);
                let windswept = self.maybe_windswept_savanna(ti, hi, w, shattered);
                let peak = self.pick_peak(ti, hi, w);
                self.add_surface(out, t, h, join(coast, far), e[0], w, 0.0, peak);
                self.add_surface(out, t, h, join(coast, near), e[1], w, 0.0, middle_badlands_slope);
                self.add_surface(out, t, h, join(mid, far), e[1], w, 0.0, peak);
                self.add_surface(out, t, h, join(coast, near), join(e[2], e[3]), w, 0.0, middle);
                self.add_surface(out, t, h, join(mid, far), e[2], w, 0.0, plateau);
                self.add_surface(out, t, h, mid, e[3], w, 0.0, middle_or_badlands);
                self.add_surface(out, t, h, far, e[3], w, 0.0, plateau);
                self.add_surface(out, t, h, join(coast, far), e[4], w, 0.0, middle);
                self.add_surface(out, t, h, join(coast, near), e[5], w, 0.0, windswept);
                self.add_surface(out, t, h, join(mid, far), e[5], w, 0.0, shattered);
                self.add_surface(out, t, h, join(coast, far), e[6], w, 0.0, middle);
            }
        }
    }

    fn add_high_slice(&self, out: &mut Out, w: P) {
        let (e, coast, near, mid, far) = (&self.erosions, self.coast, self.near_inland, self.mid_inland, self.far_inland);
        for ti in 0..5 {
            let t = self.temperatures[ti];
            for hi in 0..5 {
                let h = self.humidities[hi];
                let middle = self.pick_middle(ti, hi, w);
                let middle_or_badlands = self.pick_middle_or_badlands_if_hot(ti, hi, w);
                let middle_badlands_slope = self.pick_middle_or_badlands_if_hot_or_slope_if_cold(ti, hi, w);
                let plateau = self.pick_plateau(ti, hi, w);
                let shattered = self.pick_shattered(ti, hi, w);
                let windswept = self.maybe_windswept_savanna(ti, hi, w, middle);
                let slope = self.pick_slope(ti, hi, w);
                let peak = self.pick_peak(ti, hi, w);
                self.add_surface(out, t, h, coast, join(e[0], e[1]), w, 0.0, middle);
                self.add_surface(out, t, h, near, e[0], w, 0.0, slope);
                self.add_surface(out, t, h, join(mid, far), e[0], w, 0.0, peak);
                self.add_surface(out, t, h, near, e[1], w, 0.0, middle_badlands_slope);
                self.add_surface(out, t, h, join(mid, far), e[1], w, 0.0, slope);
                self.add_surface(out, t, h, join(coast, near), join(e[2], e[3]), w, 0.0, middle);
                self.add_surface(out, t, h, join(mid, far), e[2], w, 0.0, plateau);
                self.add_surface(out, t, h, mid, e[3], w, 0.0, middle_or_badlands);
                self.add_surface(out, t, h, far, e[3], w, 0.0, plateau);
                self.add_surface(out, t, h, join(coast, far), e[4], w, 0.0, middle);
                self.add_surface(out, t, h, join(coast, near), e[5], w, 0.0, windswept);
                self.add_surface(out, t, h, join(mid, far), e[5], w, 0.0, shattered);
                self.add_surface(out, t, h, join(coast, far), e[6], w, 0.0, middle);
            }
        }
    }

    fn add_mid_slice(&self, out: &mut Out, w: P) {
        let (e, coast, near, mid, far, f) = (&self.erosions, self.coast, self.near_inland, self.mid_inland, self.far_inland, self.full);
        let tm = &self.temperatures;
        self.add_surface(out, f, f, coast, join(e[0], e[2]), w, 0.0, b::STONY_SHORE);
        self.add_surface(out, join(tm[1], tm[2]), f, join(near, far), e[6], w, 0.0, b::SWAMP);
        self.add_surface(out, join(tm[3], tm[4]), f, join(near, far), e[6], w, 0.0, b::MANGROVE_SWAMP);
        for ti in 0..5 {
            let t = tm[ti];
            for hi in 0..5 {
                let h = self.humidities[hi];
                let middle = self.pick_middle(ti, hi, w);
                let middle_or_badlands = self.pick_middle_or_badlands_if_hot(ti, hi, w);
                let middle_badlands_slope = self.pick_middle_or_badlands_if_hot_or_slope_if_cold(ti, hi, w);
                let shattered = self.pick_shattered(ti, hi, w);
                let plateau = self.pick_plateau(ti, hi, w);
                let beach = self.pick_beach(ti, hi);
                let windswept = self.maybe_windswept_savanna(ti, hi, w, middle);
                let shattered_coast = self.pick_shattered_coast(ti, hi, w);
                let slope = self.pick_slope(ti, hi, w);
                self.add_surface(out, t, h, join(near, far), e[0], w, 0.0, slope);
                self.add_surface(out, t, h, join(near, mid), e[1], w, 0.0, middle_badlands_slope);
                self.add_surface(out, t, h, far, e[1], w, 0.0, if ti == 0 { slope } else { plateau });
                self.add_surface(out, t, h, near, e[2], w, 0.0, middle);
                self.add_surface(out, t, h, mid, e[2], w, 0.0, middle_or_badlands);
                self.add_surface(out, t, h, far, e[2], w, 0.0, plateau);
                self.add_surface(out, t, h, join(coast, near), e[3], w, 0.0, middle);
                self.add_surface(out, t, h, join(mid, far), e[3], w, 0.0, middle_or_badlands);
                if w.max < 0 {
                    self.add_surface(out, t, h, coast, e[4], w, 0.0, beach);
                    self.add_surface(out, t, h, join(near, far), e[4], w, 0.0, middle);
                } else {
                    self.add_surface(out, t, h, join(coast, far), e[4], w, 0.0, middle);
                }
                self.add_surface(out, t, h, coast, e[5], w, 0.0, shattered_coast);
                self.add_surface(out, t, h, near, e[5], w, 0.0, windswept);
                self.add_surface(out, t, h, join(mid, far), e[5], w, 0.0, shattered);
                if w.max < 0 {
                    self.add_surface(out, t, h, coast, e[6], w, 0.0, beach);
                } else {
                    self.add_surface(out, t, h, coast, e[6], w, 0.0, middle);
                }
                if ti == 0 {
                    self.add_surface(out, t, h, join(near, far), e[6], w, 0.0, middle);
                }
            }
        }
    }

    fn add_low_slice(&self, out: &mut Out, w: P) {
        let (e, coast, near, mid, far, f) = (&self.erosions, self.coast, self.near_inland, self.mid_inland, self.far_inland, self.full);
        let tm = &self.temperatures;
        self.add_surface(out, f, f, coast, join(e[0], e[2]), w, 0.0, b::STONY_SHORE);
        self.add_surface(out, join(tm[1], tm[2]), f, join(near, far), e[6], w, 0.0, b::SWAMP);
        self.add_surface(out, join(tm[3], tm[4]), f, join(near, far), e[6], w, 0.0, b::MANGROVE_SWAMP);
        for ti in 0..5 {
            let t = tm[ti];
            for hi in 0..5 {
                let h = self.humidities[hi];
                let middle = self.pick_middle(ti, hi, w);
                let middle_or_badlands = self.pick_middle_or_badlands_if_hot(ti, hi, w);
                let middle_badlands_slope = self.pick_middle_or_badlands_if_hot_or_slope_if_cold(ti, hi, w);
                let beach = self.pick_beach(ti, hi);
                let windswept = self.maybe_windswept_savanna(ti, hi, w, middle);
                let shattered_coast = self.pick_shattered_coast(ti, hi, w);
                self.add_surface(out, t, h, near, join(e[0], e[1]), w, 0.0, middle_or_badlands);
                self.add_surface(out, t, h, join(mid, far), join(e[0], e[1]), w, 0.0, middle_badlands_slope);
                self.add_surface(out, t, h, near, join(e[2], e[3]), w, 0.0, middle);
                self.add_surface(out, t, h, join(mid, far), join(e[2], e[3]), w, 0.0, middle_or_badlands);
                self.add_surface(out, t, h, coast, join(e[3], e[4]), w, 0.0, beach);
                self.add_surface(out, t, h, join(near, far), e[4], w, 0.0, middle);
                self.add_surface(out, t, h, coast, e[5], w, 0.0, shattered_coast);
                self.add_surface(out, t, h, near, e[5], w, 0.0, windswept);
                self.add_surface(out, t, h, join(mid, far), e[5], w, 0.0, middle);
                self.add_surface(out, t, h, coast, e[6], w, 0.0, beach);
                if ti == 0 {
                    self.add_surface(out, t, h, join(near, far), e[6], w, 0.0, middle);
                }
            }
        }
    }

    fn add_valleys(&self, out: &mut Out, w: P) {
        let (e, coast, near, mid, far, f) = (&self.erosions, self.coast, self.near_inland, self.mid_inland, self.far_inland, self.full);
        let (tm, frozen, unfrozen) = (&self.temperatures, self.frozen, self.unfrozen);
        let negative = w.max < 0;
        self.add_surface(out, frozen, f, coast, join(e[0], e[1]), w, 0.0, if negative { b::STONY_SHORE } else { b::FROZEN_RIVER });
        self.add_surface(out, unfrozen, f, coast, join(e[0], e[1]), w, 0.0, if negative { b::STONY_SHORE } else { b::RIVER });
        self.add_surface(out, frozen, f, near, join(e[0], e[1]), w, 0.0, b::FROZEN_RIVER);
        self.add_surface(out, unfrozen, f, near, join(e[0], e[1]), w, 0.0, b::RIVER);
        self.add_surface(out, frozen, f, join(coast, far), join(e[2], e[5]), w, 0.0, b::FROZEN_RIVER);
        self.add_surface(out, unfrozen, f, join(coast, far), join(e[2], e[5]), w, 0.0, b::RIVER);
        self.add_surface(out, frozen, f, coast, e[6], w, 0.0, b::FROZEN_RIVER);
        self.add_surface(out, unfrozen, f, coast, e[6], w, 0.0, b::RIVER);
        self.add_surface(out, join(tm[1], tm[2]), f, join(self.inland, far), e[6], w, 0.0, b::SWAMP);
        self.add_surface(out, join(tm[3], tm[4]), f, join(self.inland, far), e[6], w, 0.0, b::MANGROVE_SWAMP);
        self.add_surface(out, frozen, f, join(self.inland, far), e[6], w, 0.0, b::FROZEN_RIVER);
        for ti in 0..5 {
            let t = tm[ti];
            for hi in 0..5 {
                let h = self.humidities[hi];
                let middle_or_badlands = self.pick_middle_or_badlands_if_hot(ti, hi, w);
                self.add_surface(out, t, h, join(mid, far), join(e[0], e[1]), w, 0.0, middle_or_badlands);
            }
        }
    }

    fn add_underground_biomes(&self, out: &mut Out) {
        let f = self.full;
        self.add_underground(out, f, f, span(0.8, 1.0), f, f, 0.0, b::DRIPSTONE_CAVES);
        self.add_underground(out, f, span(0.7, 1.0), f, f, f, 0.0, b::LUSH_CAVES);
        self.add_bottom(out, f, f, f, join(self.erosions[0], self.erosions[1]), f, 0.0, b::DEEP_DARK);
    }

    fn pick_middle(&self, t: usize, h: usize, w: P) -> Biome {
        if w.max < 0 {
            self.middle[t][h]
        } else {
            self.middle_variant[t][h].unwrap_or(self.middle[t][h])
        }
    }

    fn pick_middle_or_badlands_if_hot(&self, t: usize, h: usize, w: P) -> Biome {
        if t == 4 { self.pick_badlands(h, w) } else { self.pick_middle(t, h, w) }
    }

    fn pick_middle_or_badlands_if_hot_or_slope_if_cold(&self, t: usize, h: usize, w: P) -> Biome {
        if t == 0 { self.pick_slope(t, h, w) } else { self.pick_middle_or_badlands_if_hot(t, h, w) }
    }

    fn maybe_windswept_savanna(&self, t: usize, h: usize, w: P, fallback: Biome) -> Biome {
        if t > 1 && h < 4 && w.max >= 0 { b::WINDSWEPT_SAVANNA } else { fallback }
    }

    fn pick_shattered_coast(&self, t: usize, h: usize, w: P) -> Biome {
        let base = if w.max >= 0 { self.pick_middle(t, h, w) } else { self.pick_beach(t, h) };
        self.maybe_windswept_savanna(t, h, w, base)
    }

    fn pick_beach(&self, t: usize, _h: usize) -> Biome {
        if t == 0 {
            b::SNOWY_BEACH
        } else if t == 4 {
            b::DESERT
        } else {
            b::BEACH
        }
    }

    fn pick_badlands(&self, h: usize, w: P) -> Biome {
        if h < 2 {
            if w.max < 0 { b::BADLANDS } else { b::ERODED_BADLANDS }
        } else if h < 3 {
            b::BADLANDS
        } else {
            b::WOODED_BADLANDS
        }
    }

    fn pick_plateau(&self, t: usize, h: usize, w: P) -> Biome {
        if w.max >= 0 {
            if let Some(v) = self.plateau_variant[t][h] {
                return v;
            }
        }
        self.plateau[t][h]
    }

    fn pick_peak(&self, t: usize, h: usize, w: P) -> Biome {
        if t <= 2 {
            if w.max < 0 { b::JAGGED_PEAKS } else { b::FROZEN_PEAKS }
        } else if t == 3 {
            b::STONY_PEAKS
        } else {
            self.pick_badlands(h, w)
        }
    }

    fn pick_slope(&self, t: usize, h: usize, w: P) -> Biome {
        if t >= 3 {
            self.pick_plateau(t, h, w)
        } else if h <= 1 {
            b::SNOWY_SLOPES
        } else {
            b::GROVE
        }
    }

    fn pick_shattered(&self, t: usize, h: usize, w: P) -> Biome {
        self.shattered[t][h].unwrap_or_else(|| self.pick_middle(t, h, w))
    }

    /// `addSurfaceBiome`: a surface entry exists at depth 0 (the surface) and at depth 1 (one octave below it).
    #[allow(clippy::too_many_arguments)]
    fn add_surface(&self, out: &mut Out, t: P, h: P, c: P, e: P, w: P, offset: f32, biome: Biome) {
        out.push((ParameterPoint::new(t, h, c, e, Parameter::point(0.0), w, offset), biome));
        out.push((ParameterPoint::new(t, h, c, e, Parameter::point(1.0), w, offset), biome));
    }

    #[allow(clippy::too_many_arguments)]
    fn add_underground(&self, out: &mut Out, t: P, h: P, c: P, e: P, w: P, offset: f32, biome: Biome) {
        out.push((ParameterPoint::new(t, h, c, e, span(0.2, 0.9), w, offset), biome));
    }

    #[allow(clippy::too_many_arguments)]
    fn add_bottom(&self, out: &mut Out, t: P, h: P, c: P, e: P, w: P, offset: f32, biome: Biome) {
        out.push((ParameterPoint::new(t, h, c, e, Parameter::point(1.1), w, offset), biome));
    }
}

/// `MultiNoiseBiomeSourceParameterList.Preset.NETHER`
pub fn nether_list() -> ParameterList<Biome> {
    ParameterList::new(vec![
        (ParameterPoint::points(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0), b::NETHER_WASTES),
        (ParameterPoint::points(0.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0), b::SOUL_SAND_VALLEY),
        (ParameterPoint::points(0.4, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0), b::CRIMSON_FOREST),
        (ParameterPoint::points(0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.375), b::WARPED_FOREST),
        (ParameterPoint::points(-0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.175), b::BASALT_DELTAS),
    ])
}
