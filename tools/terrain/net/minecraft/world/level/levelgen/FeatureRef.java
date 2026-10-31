package net.minecraft.world.level.levelgen;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.FeatureSorter;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;

/**
 * Reference data for the feature decoration, from the REAL Mojang classes: "<key>\t<value>" lines.
 *   order.<overworld|nether|end>.<step>.<i>   FeatureSorter.buildFeaturesPerStep: the registry key of the i-th feature of a step
 *   wgr.<n>                                    WorldgenRandom over a Xoroshiro source: seeds and draws
 *   FeatureRef <out file>
 */
public class FeatureRef {
    static final StringBuilder OUT = new StringBuilder();

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        MultiPackResourceManager rm = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        RegistryAccess staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        RegistryAccess.Frozen wg = RegistryDataLoader.load(rm, staticAccess, RegistryDataLoader.WORLDGEN_REGISTRIES);
        Registry<PlacedFeature> placed = wg.registryOrThrow(Registries.PLACED_FEATURE);

        BiomeSource overworld = MultiNoiseBiomeSource.createFromPreset(wg.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST).getOrThrow(MultiNoiseBiomeSourceParameterLists.OVERWORLD));
        BiomeSource nether = MultiNoiseBiomeSource.createFromPreset(wg.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST).getOrThrow(MultiNoiseBiomeSourceParameterLists.NETHER));
        BiomeSource end = TheEndBiomeSource.create(wg.lookupOrThrow(Registries.BIOME));
        String[] names = {"overworld", "nether", "end"};
        BiomeSource[] sources = {overworld, nether, end};
        for (int s = 0; s < 3; s++) {
            List<FeatureSorter.StepFeatureData> steps = FeatureSorter.buildFeaturesPerStep(List.copyOf(sources[s].possibleBiomes()), (Holder<Biome> b) -> b.value().getGenerationSettings().features(), true);
            for (int step = 0; step < steps.size(); step++) {
                List<PlacedFeature> features = steps.get(step).features();
                for (int i = 0; i < features.size(); i++) {
                    OUT.append("order.").append(names[s]).append('.').append(step).append('.').append(i).append('\t').append(placed.getKey(features.get(i))).append('\n');
                }
            }
        }

        // WorldgenRandom over Xoroshiro
        int n = 0;
        long[][] cases = {{12345L, 0, 0}, {-4172144997902289642L, 16, -32}, {0L, -48, 80}, {987654321L, 160, 160}};
        for (long[] c : cases) {
            WorldgenRandom r = new WorldgenRandom(new XoroshiroRandomSource(1L));
            long dec = r.setDecorationSeed(c[0], (int) c[1], (int) c[2]);
            OUT.append("wgr.").append(n++).append('\t').append(dec).append('\n');
            for (int[] is : new int[][] {{0, 0}, {5, 3}, {17, 9}}) {
                r.setFeatureSeed(dec, is[0], is[1]);
                StringBuilder sb = new StringBuilder();
                sb.append(r.nextInt(10)).append(',').append(r.nextInt(16)).append(',').append(r.nextInt(3)).append(',').append(r.nextInt(1000003)).append(',');
                sb.append(Float.floatToRawIntBits(r.nextFloat())).append(',').append(Double.doubleToRawLongBits(r.nextDouble())).append(',');
                sb.append(r.nextLong()).append(',').append(r.nextBoolean()).append(',').append(r.nextInt());
                sb.append(',').append(Double.doubleToRawLongBits(r.nextGaussian()));
                // the cached Gaussian survives setSeed
                r.setFeatureSeed(dec, is[0] + 1, is[1]);
                sb.append(',').append(Double.doubleToRawLongBits(r.nextGaussian())).append(',').append(Double.doubleToRawLongBits(r.nextGaussian()));
                OUT.append("wgr.").append(n++).append('\t').append(sb).append('\n');
            }
        }
        Files.writeString(Path.of(args[0]), OUT.toString());
        System.out.println("RESULT: wrote " + OUT.chars().filter(ch -> ch == '\n').count() + " feature reference lines");
    }
}
