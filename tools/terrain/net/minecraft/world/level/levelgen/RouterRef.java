package net.minecraft.world.level.levelgen;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import net.minecraft.SharedConstants;
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
import net.minecraft.world.level.levelgen.synth.NormalNoise;

/**
 * Evaluates every function of the REAL wired NoiseRouter at fixed positions (plain contexts, no chunk): "<name>\t<raw bits>".
 *   RouterRef <out file> <settings> <seed>...   (settings/seed pairs are separate invocations of one process: "<settings>:<seed>")
 */
public class RouterRef {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        MultiPackResourceManager rm = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        RegistryAccess staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        RegistryAccess.Frozen wg = RegistryDataLoader.load(rm, staticAccess, RegistryDataLoader.WORLDGEN_REGISTRIES);
        Registry<NoiseGeneratorSettings> settingsReg = wg.registryOrThrow(Registries.NOISE_SETTINGS);
        HolderGetter<NormalNoise.NoiseParameters> noises = wg.lookupOrThrow(Registries.NOISE);
        StringBuilder out = new StringBuilder();
        for (int a = 1; a < args.length; a++) {
            String[] cfg = args[a].split("@");
            ResourceLocation name = ResourceLocation.parse(cfg[0]);
            long seed = Long.parseLong(cfg[1]);
            NoiseGeneratorSettings s = settingsReg.getOrThrow(ResourceKey.create(Registries.NOISE_SETTINGS, name));
            NoiseRouter r = RandomState.create(s, noises, seed).router();
            String[] names = {"barrier", "floodedness", "spread", "lava", "temperature", "vegetation", "continents", "erosion", "depth", "ridges", "initial", "final", "vein_toggle", "vein_ridged", "vein_gap"};
            DensityFunction[] fns = {r.barrierNoise(), r.fluidLevelFloodednessNoise(), r.fluidLevelSpreadNoise(), r.lavaNoise(), r.temperature(), r.vegetation(), r.continents(), r.erosion(), r.depth(), r.ridges(), r.initialDensityWithoutJaggedness(), r.finalDensity(), r.veinToggle(), r.veinRidged(), r.veinGap()};
            for (int i = 0; i < 48; i++) {
                int x = ((i * 7919) % 6001) - 3000;
                int z = ((i * 104729) % 6007) - 3000;
                int y = ((i * 31) % 380) - 60;
                if (i < 6) {
                    x = i * 16;
                    z = -i * 16;
                    y = i * 50 - 40;
                }
                for (int f = 0; f < fns.length; f++) {
                    double v = fns[f].compute(new DensityFunction.SinglePointContext(x, y, z));
                    out.append(cfg[0]).append('@').append(seed).append('.').append(i).append('.').append(names[f]).append('\t').append(Double.doubleToRawLongBits(v)).append('\n');
                }
            }
        }
        Files.writeString(Path.of(args[0]), out.toString());
        System.out.println("RESULT: wrote " + out.chars().filter(c -> c == '\n').count() + " router values");
    }
}
