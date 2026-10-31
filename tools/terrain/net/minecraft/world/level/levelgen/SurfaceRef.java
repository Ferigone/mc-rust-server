package net.minecraft.world.level.levelgen;

import java.io.DataOutputStream;
import java.io.FileOutputStream;
import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.QuartPos;
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
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

/**
 * Runs the REAL surface stage: biomes of the chunk and its neighbours (BiomeSource), the NoiseChunk fill loop (doFill without structures)
 * and SurfaceSystem.buildSurface with the BiomeManager, for the first chunk found for every biome in a spiral around the origin.
 *   SurfaceRef <out file> <noise settings> <seed> <overworld|nether|end> <minY> <height> <maxRingChunks> <stepChunks>
 * File (big endian ints, little endian shorts / ints in the arrays): int count, then per chunk
 *   int cx, int cz, byte nameLength, name (the biome at the chunk centre), height*256 shorts (block state ids, (y - minY) << 8 | z << 4 | x),
 *   256 ints (the WORLD_SURFACE_WG heightmap after the surface stage, x + z * 16, absolute y).
 */
public class SurfaceRef {
    static void fillChunk(ProtoChunk chunk, NoiseChunk nc, NoiseSettings ns, BlockState defaultBlock) {
        int cellW = nc.cellWidth();
        int cellH = nc.cellHeight();
        int minCellY = Math.floorDiv(ns.minY(), cellH);
        int cellCountY = Math.floorDiv(ns.height(), cellH);
        int minX = chunk.getPos().getMinBlockX();
        int minZ = chunk.getPos().getMinBlockZ();
        BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();
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
                            nc.updateForX(x, (double) ix / (double) cellW);
                            for (int iz = 0; iz < cellW; iz++) {
                                int z = minZ + cellZ * cellW + iz;
                                nc.updateForZ(z, (double) iz / (double) cellW);
                                BlockState st = nc.getInterpolatedState();
                                if (st == null) {
                                    st = defaultBlock;
                                }
                                if (!st.isAir()) {
                                    chunk.setBlockState(pos.set(x, y, z), st, false);
                                }
                            }
                        }
                    }
                }
            }
            nc.swapSlices();
        }
        nc.stopInterpolation();
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
        String kind = args[3];
        int minY = Integer.parseInt(args[4]);
        int height = Integer.parseInt(args[5]);
        int maxRing = Integer.parseInt(args[6]);
        int step = Integer.parseInt(args[7]);
        Registry<NoiseGeneratorSettings> settingsReg = wg.registryOrThrow(Registries.NOISE_SETTINGS);
        NoiseGeneratorSettings settings = settingsReg.getOrThrow(ResourceKey.create(Registries.NOISE_SETTINGS, settingsName));
        HolderGetter<NormalNoise.NoiseParameters> noises = wg.lookupOrThrow(Registries.NOISE);
        Registry<Biome> biomeRegistry = wg.registryOrThrow(Registries.BIOME);
        RandomState random = RandomState.create(settings, noises, seed);
        Climate.Sampler sampler = random.sampler();
        BiomeSource source = kind.equals("end")
            ? TheEndBiomeSource.create(wg.lookupOrThrow(Registries.BIOME))
            : MultiNoiseBiomeSource.createFromPreset(wg.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                .getOrThrow(kind.equals("nether") ? MultiNoiseBiomeSourceParameterLists.NETHER : MultiNoiseBiomeSourceParameterLists.OVERWORLD));
        NoiseBasedChunkGenerator generator = new NoiseBasedChunkGenerator(source, Holder.direct(settings));
        LevelHeightAccessor accessor = new LevelHeightAccessor() {
            public int getHeight() { return height; }
            public int getMinBuildHeight() { return minY; }
        };
        Aquifer.FluidPicker picker = TerrainRef.createFluidPicker(settings);

        // the first chunk for every biome (found at the chunk centre, y = 64), in rings around the origin
        List<int[]> chosen = new ArrayList<>();
        List<String> names = new ArrayList<>();
        Set<String> seen = new HashSet<>();
        for (int ring = 0; ring <= maxRing; ring += step) {
            for (int dx = -ring; dx <= ring; dx += step) {
                for (int dz = -ring; dz <= ring; dz += step) {
                    if (Math.max(Math.abs(dx), Math.abs(dz)) != ring) continue;
                    String name = source.getNoiseBiome(dx * 4 + 2, 16, dz * 4 + 2, sampler).unwrapKey().get().location().toString();
                    if (seen.add(name)) {
                        chosen.add(new int[] {dx, dz});
                        names.add(name);
                    }
                }
            }
        }

        ByteArrayOutputStream body = new ByteArrayOutputStream();
        DataOutputStream out = new DataOutputStream(body);
        out.writeInt(chosen.size());
        for (int c = 0; c < chosen.size(); c++) {
            int cx = chosen.get(c)[0];
            int cz = chosen.get(c)[1];
            // biomes of the 3x3 neighbourhood, like the BIOMES status of the neighbours
            Map<Long, ProtoChunk> chunks = new HashMap<>();
            for (int dx = -1; dx <= 1; dx++) {
                for (int dz = -1; dz <= 1; dz++) {
                    ProtoChunk pc = new ProtoChunk(new ChunkPos(cx + dx, cz + dz), UpgradeData.EMPTY, accessor, biomeRegistry, null);
                    pc.fillBiomesFromNoise(source, sampler);
                    pc.setPersistedStatus(ChunkStatus.BIOMES);
                    chunks.put(ChunkPos.asLong(cx + dx, cz + dz), pc);
                }
            }
            ProtoChunk chunk = chunks.get(ChunkPos.asLong(cx, cz));
            NoiseChunk nc = NoiseChunk.forChunk(chunk, random, DensityFunctions.BeardifierMarker.INSTANCE, settings, picker, Blender.empty());
            NoiseSettings ns = settings.noiseSettings().clampToHeightAccessor(accessor);
            fillChunk(chunk, nc, ns, settings.defaultBlock());
            chunk.setPersistedStatus(ChunkStatus.NOISE);
            BiomeManager manager = new BiomeManager(
                (qx, qy, qz) -> chunks.get(ChunkPos.asLong(QuartPos.toSection(qx), QuartPos.toSection(qz))).getNoiseBiome(qx, qy, qz),
                BiomeManager.obfuscateSeed(seed));
            random.surfaceSystem().buildSurface(random, manager, biomeRegistry, settings.useLegacyRandomSource(), new WorldGenerationContext(generator, accessor), chunk, nc, settings.surfaceRule());

            out.writeInt(cx);
            out.writeInt(cz);
            byte[] name = names.get(c).getBytes(StandardCharsets.UTF_8);
            out.writeByte(name.length);
            out.write(name);
            ByteBuffer blocks = ByteBuffer.allocate(16 * 16 * height * 2).order(ByteOrder.LITTLE_ENDIAN);
            BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();
            for (int y = minY; y < minY + height; y++) {
                for (int z = 0; z < 16; z++) {
                    for (int x = 0; x < 16; x++) {
                        blocks.putShort((short) Block.getId(chunk.getBlockState(pos.set(cx * 16 + x, y, cz * 16 + z))));
                    }
                }
            }
            out.write(blocks.array());
            ByteBuffer hm = ByteBuffer.allocate(256 * 4).order(ByteOrder.LITTLE_ENDIAN);
            for (int z = 0; z < 16; z++) {
                for (int x = 0; x < 16; x++) {
                    hm.putInt(chunk.getHeight(Heightmap.Types.WORLD_SURFACE_WG, x, z));
                }
            }
            out.write(hm.array());
        }
        try (FileOutputStream f = new FileOutputStream(outFile)) {
            f.write(body.toByteArray());
        }
        System.out.println("RESULT: wrote " + chosen.size() + " chunks of " + settingsName + " seed " + seed + " to " + outFile);
    }
}
