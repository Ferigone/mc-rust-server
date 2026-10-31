package net.minecraft.world.level.levelgen;

import java.io.DataOutputStream;
import java.io.FileOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.util.List;
import java.util.concurrent.Executor;
import net.minecraft.SharedConstants;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.LayeredRegistryAccess;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.RegistryLayer;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.dimension.DimensionType;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

/**
 * Runs the REAL NoiseChunk fill loop (NoiseBasedChunkGenerator.doFill without structures) for some chunks and writes the block state IDs:
 *   TerrainRef <out file> <noise settings name> <seed> <minY> <height> <cx,cz> ...
 * File: for each chunk int cx, int cz (big endian), then height*256 unsigned shorts (little endian), index (y << 8 | z << 4 | x).
 * Lives in the levelgen package for access to the protected members (BeardifierMarker, NoiseChunk.getInterpolatedState).
 */
public class TerrainRef {
    static Aquifer.FluidPicker createFluidPicker(NoiseGeneratorSettings s) {
        Aquifer.FluidStatus lava = new Aquifer.FluidStatus(-54, Blocks.LAVA.defaultBlockState());
        int sea = s.seaLevel();
        Aquifer.FluidStatus fluid = new Aquifer.FluidStatus(sea, s.defaultFluid());
        return (x, y, z) -> y < Math.min(-54, sea) ? lava : fluid;
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        MultiPackResourceManager rm = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        RegistryAccess staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        RegistryAccess.Frozen wg = RegistryDataLoader.load(rm, staticAccess, RegistryDataLoader.WORLDGEN_REGISTRIES);

        String outFile = args[0];
        ResourceLocation settingsName = ResourceLocation.parse(args[1]);
        long seed = Long.parseLong(args[2]);
        int minY = Integer.parseInt(args[3]);
        int height = Integer.parseInt(args[4]);
        Registry<NoiseGeneratorSettings> settingsReg = wg.registryOrThrow(Registries.NOISE_SETTINGS);
        NoiseGeneratorSettings settings = settingsReg.getOrThrow(ResourceKey.create(Registries.NOISE_SETTINGS, settingsName));
        HolderGetter<NormalNoise.NoiseParameters> noises = wg.lookupOrThrow(Registries.NOISE);
        RandomState random = RandomState.create(settings, noises, seed);
        LevelHeightAccessor accessor = new LevelHeightAccessor() {
            public int getHeight() {
                return height;
            }

            public int getMinBuildHeight() {
                return minY;
            }
        };
        NoiseSettings ns = settings.noiseSettings().clampToHeightAccessor(accessor);
        Aquifer.FluidPicker picker = createFluidPicker(settings);
        BlockState defaultBlock = settings.defaultBlock();

        try (DataOutputStream out = new DataOutputStream(new FileOutputStream(outFile))) {
            for (int a = 5; a < args.length; a++) {
                String[] c = args[a].split(",");
                int cx = Integer.parseInt(c[0]);
                int cz = Integer.parseInt(c[1]);
                int minX = cx * 16;
                int minZ = cz * 16;
                NoiseChunk nc = new NoiseChunk(16 / ns.getCellWidth(), random, minX, minZ, ns, DensityFunctions.BeardifierMarker.INSTANCE, settings, picker, Blender.empty());
                int cellW = nc.cellWidth();
                int cellH = nc.cellHeight();
                int minCellY = Math.floorDiv(ns.minY(), cellH);
                int cellCountY = Math.floorDiv(ns.height(), cellH);
                int airId = Block.getId(Blocks.AIR.defaultBlockState());
                int[] ids = new int[16 * 16 * ns.height()];
                java.util.Arrays.fill(ids, airId);
                nc.initializeForFirstCellX();
                for (int cellX = 0; cellX < 16 / cellW; cellX++) {
                    nc.advanceCellX(cellX);
                    for (int cellZ = 0; cellZ < 16 / cellW; cellZ++) {
                        for (int cellY = cellCountY - 1; cellY >= 0; cellY--) {
                            nc.selectCellYZ(cellY, cellZ);
                            for (int iy = cellH - 1; iy >= 0; iy--) {
                                int y = (minCellY + cellY) * cellH + iy;
                                double fy = (double) iy / (double) cellH;
                                nc.updateForY(y, fy);
                                for (int ix = 0; ix < cellW; ix++) {
                                    int x = minX + cellX * cellW + ix;
                                    double fx = (double) ix / (double) cellW;
                                    nc.updateForX(x, fx);
                                    for (int iz = 0; iz < cellW; iz++) {
                                        int z = minZ + cellZ * cellW + iz;
                                        double fz = (double) iz / (double) cellW;
                                        nc.updateForZ(z, fz);
                                        BlockState st = nc.getInterpolatedState();
                                        if (st == null) {
                                            st = defaultBlock;
                                        }
                                        ids[((y - ns.minY()) * 16 + (z & 15)) * 16 + (x & 15)] = Block.getId(st);
                                    }
                                }
                            }
                        }
                    }
                    nc.swapSlices();
                }
                nc.stopInterpolation();
                out.writeInt(cx);
                out.writeInt(cz);
                ByteBuffer bb = ByteBuffer.allocate(ids.length * 2).order(ByteOrder.LITTLE_ENDIAN);
                for (int id : ids) {
                    bb.putShort((short) id);
                }
                out.write(bb.array());
            }
        }
        System.out.println("RESULT: wrote " + (args.length - 5) + " chunks of " + settingsName + " seed " + seed + " to " + outFile);
    }
}
