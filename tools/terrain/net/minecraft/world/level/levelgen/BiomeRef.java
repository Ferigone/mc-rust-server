package net.minecraft.world.level.levelgen;

import com.mojang.datafixers.util.Pair;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;
import java.util.TreeSet;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterList;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

/**
 * Biome reference data from the REAL Mojang classes: "<key>\t<value>" lines.
 *   list.<preset>.<i>     the parameter list of a preset (all seven parameters + the biome)
 *   tree.<preset>.<i>     R-tree result and brute-force result for synthetic target points (many exact ties)
 *   src.<cfg>.<i>         climate target + biome of the real biome source at quart positions
 *   spawn.<cfg>           Climate.findSpawnPosition
 *   BiomeRef <out file> <settings>@<seed>@<overworld|nether|end>...
 */
public class BiomeRef {
    static final StringBuilder OUT = new StringBuilder();

    static String name(ResourceKey<Biome> k) { return k.location().toString(); }

    static String param(Climate.Parameter p) { return p.min() + "," + p.max(); }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        MultiPackResourceManager rm = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        RegistryAccess staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        RegistryAccess.Frozen wg = RegistryDataLoader.load(rm, staticAccess, RegistryDataLoader.WORLDGEN_REGISTRIES);
        Registry<NoiseGeneratorSettings> settingsReg = wg.registryOrThrow(Registries.NOISE_SETTINGS);
        HolderGetter<NormalNoise.NoiseParameters> noises = wg.lookupOrThrow(Registries.NOISE);

        // phase A: the parameter lists of the presets
        Map<MultiNoiseBiomeSourceParameterList.Preset, Climate.ParameterList<ResourceKey<Biome>>> presets = MultiNoiseBiomeSourceParameterList.knownPresets();
        for (var e : presets.entrySet()) {
            String id = e.getKey().id().getPath();
            List<Pair<Climate.ParameterPoint, ResourceKey<Biome>>> values = e.getValue().values();
            for (int i = 0; i < values.size(); i++) {
                Climate.ParameterPoint p = values.get(i).getFirst();
                OUT.append("list.").append(id).append('.').append(i).append('\t')
                    .append(param(p.temperature())).append(',').append(param(p.humidity())).append(',').append(param(p.continentalness())).append(',')
                    .append(param(p.erosion())).append(',').append(param(p.depth())).append(',').append(param(p.weirdness())).append(',')
                    .append(p.offset()).append(',').append(name(values.get(i).getSecond())).append('\n');
            }
        }

        // phase B: the R-tree against brute force on synthetic targets, with many coordinates exactly on the parameter boundaries
        for (String id : new String[] {"overworld", "nether"}) {
            Climate.ParameterList<ResourceKey<Biome>> list = null;
            for (var e : MultiNoiseBiomeSourceParameterList.knownPresets().entrySet()) {
                if (e.getKey().id().getPath().equals(id)) list = e.getValue();
            }
            List<TreeSet<Long>> bounds = new ArrayList<>();
            for (int d = 0; d < 6; d++) bounds.add(new TreeSet<>());
            for (var v : list.values()) {
                Climate.ParameterPoint p = v.getFirst();
                Climate.Parameter[] ps = {p.temperature(), p.humidity(), p.continentalness(), p.erosion(), p.depth(), p.weirdness()};
                for (int d = 0; d < 6; d++) { bounds.get(d).add(ps[d].min()); bounds.get(d).add(ps[d].max()); }
            }
            List<List<Long>> pool = new ArrayList<>();
            for (int d = 0; d < 6; d++) pool.add(new ArrayList<>(bounds.get(d)));
            long state = 0x9E3779B97F4A7C15L;
            for (int i = 0; i < 30000; i++) {
                long[] t = new long[6];
                for (int d = 0; d < 6; d++) {
                    state = state * 6364136223846793005L + 1442695040888963407L;
                    long r = state >>> 33;
                    if ((r & 3) == 0) {
                        t[d] = (r >> 2) % 24001 - 12000;
                    } else {
                        List<Long> b = pool.get(d);
                        t[d] = b.get((int) ((r >> 2) % b.size()));
                    }
                }
                Climate.TargetPoint tp = new Climate.TargetPoint(t[0], t[1], t[2], t[3], t[4], t[5]);
                ResourceKey<Biome> tree = list.findValue(tp);
                ResourceKey<Biome> brute = list.findValueBruteForce(tp);
                OUT.append("tree.").append(id).append('.').append(i).append('\t').append(name(tree)).append(',').append(name(brute)).append('\n');
            }
        }

        // phase B2: random lists on a coarse grid (distinct values, so exact ties between different entries are common) against brute force
        long[] grid = {-10000, -5000, -2500, 0, 2500, 5000, 10000};
        int[] sizes = {5, 40, 700, 300, 1500};
        for (int li = 0; li < sizes.length; li++) {
            long state = 0x1234567L + li;
            List<Pair<Climate.ParameterPoint, Integer>> entries = new ArrayList<>();
            Climate.Parameter[] prev = null;
            long prevOffset = 0;
            for (int i = 0; i < sizes[li]; i++) {
                Climate.Parameter[] ps = new Climate.Parameter[6];
                long offset;
                state = state * 6364136223846793005L + 1442695040888963407L;
                // lists 3 and 4: clusters of entries that differ in a single parameter (like the surface / depth pairs of the real table)
                boolean clustered = li >= 3 && prev != null && (state >>> 33) % 12 != 0;
                int changed = -1;
                if (clustered) {
                    state = state * 6364136223846793005L + 1442695040888963407L;
                    changed = (int) ((state >>> 33) % 7);
                }
                for (int d = 0; d < 6; d++) {
                    if (clustered && changed != d) { ps[d] = prev[d]; continue; }
                    state = state * 6364136223846793005L + 1442695040888963407L;
                    long a = grid[(int) ((state >>> 33) % 7)];
                    state = state * 6364136223846793005L + 1442695040888963407L;
                    long b = grid[(int) ((state >>> 33) % 7)];
                    ps[d] = new Climate.Parameter(Math.min(a, b), Math.max(a, b));
                }
                if (clustered && changed != 6) {
                    offset = prevOffset;
                } else {
                    state = state * 6364136223846793005L + 1442695040888963407L;
                    offset = new long[] {0, 0, 0, 1000, 2500}[(int) ((state >>> 33) % 5)];
                }
                prev = ps;
                prevOffset = offset;
                entries.add(Pair.of(new Climate.ParameterPoint(ps[0], ps[1], ps[2], ps[3], ps[4], ps[5], offset), i));
            }
            Climate.ParameterList<Integer> list = new Climate.ParameterList<>(entries);
            for (int i = 0; i < 20000; i++) {
                long[] t = new long[6];
                for (int d = 0; d < 6; d++) {
                    state = state * 6364136223846793005L + 1442695040888963407L;
                    t[d] = ((state >>> 33) % 17) * 1250 - 10000;
                }
                Climate.TargetPoint tp = new Climate.TargetPoint(t[0], t[1], t[2], t[3], t[4], t[5]);
                OUT.append("rand.").append(li).append('.').append(i).append('\t').append(list.findValue(tp)).append(',').append(list.findValueBruteForce(tp)).append('\n');
            }
        }

        // phase C: the real biome sources at quart positions
        BiomeSource overworld = MultiNoiseBiomeSource.createFromPreset(
            wg.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST).getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
        BiomeSource nether = MultiNoiseBiomeSource.createFromPreset(
            wg.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST).getOrThrow(MultiNoiseBiomeSourceParameterLists.NETHER));
        BiomeSource end = TheEndBiomeSource.create(wg.lookupOrThrow(Registries.BIOME));
        for (int a = 1; a < args.length; a++) {
            String[] cfg = args[a].split("@");
            long seed = Long.parseLong(cfg[1]);
            NoiseGeneratorSettings s = settingsReg.getOrThrow(ResourceKey.create(Registries.NOISE_SETTINGS, ResourceLocation.parse(cfg[0])));
            RandomState rs = RandomState.create(s, noises, seed);
            Climate.Sampler sampler = rs.sampler();
            BiomeSource src = cfg[2].equals("nether") ? nether : cfg[2].equals("end") ? end : overworld;
            String key = "src." + (a - 1) + ".";
            int n = 0;
            for (int i = 0; i < 2500; i++) {
                int qx = (i * 7919 + i * i * 13) % 3001 - 1500;
                int qz = (i * 104729 + i * i * 7) % 3001 - 1500;
                int qy = (i * 31) % 112 - 32;
                n = record(key, n, src, sampler, qx, qy, qz);
            }
            for (int gx = 0; gx < 60; gx++) {
                for (int gz = 0; gz < 60; gz++) {
                    n = record(key, n, src, sampler, (gx - 30) * 24, 16, (gz - 30) * 24);
                }
            }
            if (cfg[2].equals("overworld")) {
                BlockPos spawn = sampler.findSpawnPosition();
                OUT.append("spawn.").append(a - 1).append('\t').append(spawn.getX()).append(',').append(spawn.getY()).append(',').append(spawn.getZ()).append('\n');
            }
        }
        // phase E: the temperature of every biome (the value behind coldEnoughToSnow / shouldMeltFrozenOceanIcebergSlightly) at many positions
        Registry<Biome> biomeRegistry = wg.registryOrThrow(Registries.BIOME);
        java.lang.reflect.Method getTemperature = Biome.class.getDeclaredMethod("getTemperature", BlockPos.class);
        getTemperature.setAccessible(true);
        List<ResourceLocation> biomeNames = new ArrayList<>();
        for (ResourceLocation id : biomeRegistry.keySet()) biomeNames.add(id);
        java.util.Collections.sort(biomeNames);
        int[] ys = {-60, 40, 63, 80, 81, 90, 120, 200, 319};
        for (ResourceLocation id : biomeNames) {
            Biome biome = biomeRegistry.get(id);
            for (int i = 0; i < 400; i++) {
                int x = (i * 7919 + i * i * 31) % 20001 - 10000;
                int z = (i * 104729 + i * i * 17) % 20001 - 10000;
                int y = ys[i % ys.length];
                BlockPos pos = new BlockPos(x, y, z);
                float t = (Float) getTemperature.invoke(biome, pos);
                OUT.append("clim.").append(id).append('.').append(i).append('\t').append(Float.floatToRawIntBits(t)).append(',')
                    .append(biome.coldEnoughToSnow(pos)).append(',').append(biome.shouldMeltFrozenOceanIcebergSlightly(pos)).append('\n');
            }
        }
        Files.writeString(Path.of(args[0]), OUT.toString());
        System.out.println("RESULT: wrote " + OUT.chars().filter(c -> c == '\n').count() + " biome reference lines");
    }

    static int record(String key, int n, BiomeSource src, Climate.Sampler sampler, int qx, int qy, int qz) {
        Climate.TargetPoint t = sampler.sample(qx, qy, qz);
        Holder<Biome> b = src.getNoiseBiome(qx, qy, qz, sampler);
        OUT.append(key).append(n).append('\t').append(qx).append(',').append(qy).append(',').append(qz).append(',')
            .append(t.temperature()).append(',').append(t.humidity()).append(',').append(t.continentalness()).append(',')
            .append(t.erosion()).append(',').append(t.depth()).append(',').append(t.weirdness()).append(',')
            .append(name(b.unwrapKey().get())).append('\n');
        return n + 1;
    }
}
