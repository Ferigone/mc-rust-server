package net.minecraft.world.level.levelgen;

import it.unimi.dsi.fastutil.ints.IntArraySet;
import it.unimi.dsi.fastutil.objects.ObjectArraySet;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.FileOutputStream;
import java.lang.reflect.InvocationHandler;
import java.lang.reflect.Method;
import java.lang.reflect.Proxy;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.EnumSet;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.function.Supplier;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Holder;
import net.minecraft.core.HolderGetter;
import net.minecraft.core.HolderSet;
import net.minecraft.core.QuartPos;
import net.minecraft.core.Registry;
import net.minecraft.core.RegistryAccess;
import net.minecraft.core.SectionPos;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.ChunkPos;
import net.minecraft.world.level.LevelHeightAccessor;
import net.minecraft.world.level.WorldGenLevel;
import net.minecraft.world.level.biome.Biome;
import net.minecraft.world.level.biome.BiomeGenerationSettings;
import net.minecraft.world.level.biome.BiomeManager;
import net.minecraft.world.level.biome.BiomeSource;
import net.minecraft.world.level.biome.Climate;
import net.minecraft.world.level.biome.FeatureSorter;
import net.minecraft.world.level.biome.MultiNoiseBiomeSource;
import net.minecraft.world.level.biome.MultiNoiseBiomeSourceParameterLists;
import net.minecraft.world.level.biome.TheEndBiomeSource;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.CarvingMask;
import net.minecraft.world.level.chunk.ChunkAccess;
import net.minecraft.world.level.chunk.ProtoChunk;
import net.minecraft.world.level.chunk.UpgradeData;
import net.minecraft.world.level.chunk.status.ChunkStatus;
import net.minecraft.world.level.levelgen.blending.Blender;
import net.minecraft.world.level.levelgen.carver.CarvingContext;
import net.minecraft.world.level.levelgen.carver.ConfiguredWorldCarver;
import net.minecraft.world.level.levelgen.placement.PlacedFeature;
import net.minecraft.world.level.levelgen.synth.NormalNoise;

/**
 * Runs the REAL feature decoration (ChunkGenerator.applyBiomeDecoration, without structures) on chunks that went through the real noise, surface and carver
 * stages, for the first chunk found for every biome in a spiral around the origin.
 *   DecorateRef <out file> <noise settings> <seed> <overworld|nether|end> <minY> <height> <maxRingChunks> <stepChunks> <supported features file>
 * Only the placed features listed in the "supported features" file (one registry key per line) are placed: the others need parts of the level
 * the reference does not provide yet. Everything else is the real code: the feature order of FeatureSorter, the decoration seeds, the placement
 * modifiers, PlacedFeature.placeWithBiomeCheck and the features themselves. The WorldGenLevel is a proxy that behaves like the WorldGenRegion of
 * the FEATURES step: the 3x3 chunks around the centre are carved ProtoChunks and writable, the chunks around them are empty (they are at the
 * STRUCTURE_STARTS status in the real game).
 * File (big endian ints, little endian shorts in the arrays): int count, then per centre
 *   int cx, int cz, byte nameLength, name (the biome at the chunk centre), 9 chunks (dx = -1..1 outer, dz = -1..1 inner) of height*256 shorts
 *   (block state ids, (y - minY) << 8 | z << 4 | x) after the decoration.
 */
public class DecorateRef {
    /** The WorldGenRegion of the FEATURES step. */
    static final class Region implements InvocationHandler {
        final Map<Long, ProtoChunk> chunks;
        final int centerX, centerZ, minY, height;
        final long seed;
        final BiomeSource source;
        final Climate.Sampler sampler;
        final Registry<Biome> biomeRegistry;
        final LevelHeightAccessor accessor;
        final RegistryAccess registries;
        final BiomeManager biomeManager;
        WorldGenLevel proxy;

        Region(Map<Long, ProtoChunk> chunks, int centerX, int centerZ, int minY, int height, long seed, BiomeSource source, Climate.Sampler sampler,
               Registry<Biome> biomeRegistry, LevelHeightAccessor accessor, RegistryAccess registries) {
            this.chunks = chunks;
            this.centerX = centerX;
            this.centerZ = centerZ;
            this.minY = minY;
            this.height = height;
            this.seed = seed;
            this.source = source;
            this.sampler = sampler;
            this.biomeRegistry = biomeRegistry;
            this.accessor = accessor;
            this.registries = registries;
            this.biomeManager = new BiomeManager(
                (qx, qy, qz) -> chunk(QuartPos.toSection(qx), QuartPos.toSection(qz)).getNoiseBiome(qx, qy, qz), BiomeManager.obfuscateSeed(seed));
            this.proxy = (WorldGenLevel) Proxy.newProxyInstance(DecorateRef.class.getClassLoader(), new Class<?>[] {WorldGenLevel.class}, this);
        }

        ProtoChunk chunk(int x, int z) {
            long key = ChunkPos.asLong(x, z);
            ProtoChunk c = chunks.get(key);
            if (c == null) {
                // a chunk the real game holds at STRUCTURE_STARTS: biomes, no blocks
                c = new ProtoChunk(new ChunkPos(x, z), UpgradeData.EMPTY, accessor, biomeRegistry, null);
                c.fillBiomesFromNoise(source, sampler);
                chunks.put(key, c);
            }
            return c;
        }

        ProtoChunk chunkAt(BlockPos pos) {
            return chunk(SectionPos.blockToSectionCoord(pos.getX()), SectionPos.blockToSectionCoord(pos.getZ()));
        }

        boolean ensureCanWrite(BlockPos pos) {
            int dx = Math.abs(centerX - SectionPos.blockToSectionCoord(pos.getX()));
            int dz = Math.abs(centerZ - SectionPos.blockToSectionCoord(pos.getZ()));
            // ChunkStep.blockStateWriteRadius() of the FEATURES step
            return dx <= 1 && dz <= 1;
        }

        @Override
        public Object invoke(Object proxy, Method m, Object[] a) throws Throwable {
            switch (m.getName()) {
                case "getBlockState":
                    return chunkAt((BlockPos) a[0]).getBlockState((BlockPos) a[0]);
                case "getFluidState":
                    return chunkAt((BlockPos) a[0]).getFluidState((BlockPos) a[0]);
                case "setBlock":
                    if (m.getParameterCount() == 4) {
                        BlockPos pos = (BlockPos) a[0];
                        if (!ensureCanWrite(pos)) {
                            return false;
                        }
                        chunkAt(pos).setBlockState(pos, (BlockState) a[1], false);
                        return true;
                    }
                    break;
                case "getChunk":
                    if (m.getParameterCount() == 4) {
                        return chunk((Integer) a[0], (Integer) a[1]);
                    }
                    break;
                case "getHeight":
                    if (m.getParameterCount() == 3) {
                        int x = (Integer) a[1];
                        int z = (Integer) a[2];
                        return chunk(SectionPos.blockToSectionCoord(x), SectionPos.blockToSectionCoord(z)).getHeight((Heightmap.Types) a[0], x & 15, z & 15) + 1;
                    }
                    if (m.getParameterCount() == 0) {
                        return height;
                    }
                    break;
                case "getMinBuildHeight":
                    return minY;
                case "ensureCanWrite":
                    return ensureCanWrite((BlockPos) a[0]);
                case "getSeed":
                    return seed;
                case "getBiomeManager":
                    return biomeManager;
                case "registryAccess":
                    return registries;
                case "setCurrentlyGenerating":
                    return null;
                case "hashCode":
                    return System.identityHashCode(proxy);
                case "equals":
                    return proxy == a[0];
                case "toString":
                    return "DecorateRef.Region";
                default:
                    break;
            }
            if (m.isDefault()) {
                return InvocationHandler.invokeDefault(proxy, m, a);
            }
            throw new UnsupportedOperationException("WorldGenLevel." + m);
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        MultiPackResourceManager rm = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        RegistryAccess staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        RegistryAccess.Frozen wg = RegistryDataLoader.load(rm, staticAccess, RegistryDataLoader.WORLDGEN_REGISTRIES);
        staticAccess.registries().filter(e -> e.key().equals(Registries.BLOCK)).forEach(e -> SurfaceRef.bindTags(rm, e));

        String outFile = args[0];
        ResourceLocation settingsName = ResourceLocation.parse(args[1]);
        long seed = Long.parseLong(args[2]);
        String kind = args[3];
        int minY = Integer.parseInt(args[4]);
        int height = Integer.parseInt(args[5]);
        int maxRing = Integer.parseInt(args[6]);
        int step = Integer.parseInt(args[7]);
        Set<String> supported = new HashSet<>(Files.readAllLines(Path.of(args[8])));
        supported.remove("");
        Registry<NoiseGeneratorSettings> settingsReg = wg.registryOrThrow(Registries.NOISE_SETTINGS);
        NoiseGeneratorSettings settings = settingsReg.getOrThrow(ResourceKey.create(Registries.NOISE_SETTINGS, settingsName));
        HolderGetter<NormalNoise.NoiseParameters> noises = wg.lookupOrThrow(Registries.NOISE);
        Registry<Biome> biomeRegistry = wg.registryOrThrow(Registries.BIOME);
        Registry<PlacedFeature> placedRegistry = wg.registryOrThrow(Registries.PLACED_FEATURE);
        RandomState random = RandomState.create(settings, noises, seed);
        Climate.Sampler sampler = random.sampler();
        BiomeSource source = kind.equals("end")
            ? TheEndBiomeSource.create(wg.lookupOrThrow(Registries.BIOME))
            : MultiNoiseBiomeSource.createFromPreset(wg.lookupOrThrow(Registries.MULTI_NOISE_BIOME_SOURCE_PARAMETER_LIST)
                .getOrThrow(kind.equals("nether") ? MultiNoiseBiomeSourceParameterLists.NETHER : MultiNoiseBiomeSourceParameterLists.OVERWORLD));
        net.minecraft.world.level.chunk.ChunkGenerator generator = new NoiseBasedChunkGenerator(source, Holder.direct(settings));
        LevelHeightAccessor accessor = new LevelHeightAccessor() {
            public int getHeight() { return height; }
            public int getMinBuildHeight() { return minY; }
        };
        Aquifer.FluidPicker picker = TerrainRef.createFluidPicker(settings);
        List<FeatureSorter.StepFeatureData> featuresPerStep =
            FeatureSorter.buildFeaturesPerStep(List.copyOf(source.possibleBiomes()), (Holder<Biome> b) -> b.value().getGenerationSettings().features(), true);

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
        long placedCount = 0;
        for (int c = 0; c < chosen.size(); c++) {
            int cx = chosen.get(c)[0];
            int cz = chosen.get(c)[1];
            // biomes of the 5x5 chunks around the centre (the surface of the outer carved chunks reads one chunk further)
            Map<Long, ProtoChunk> chunks = new HashMap<>();
            for (int dx = -2; dx <= 2; dx++) {
                for (int dz = -2; dz <= 2; dz++) {
                    ProtoChunk pc = new ProtoChunk(new ChunkPos(cx + dx, cz + dz), UpgradeData.EMPTY, accessor, biomeRegistry, null);
                    pc.fillBiomesFromNoise(source, sampler);
                    pc.setPersistedStatus(ChunkStatus.BIOMES);
                    chunks.put(ChunkPos.asLong(cx + dx, cz + dz), pc);
                }
            }
            BiomeManager manager = new BiomeManager(
                (qx, qy, qz) -> chunks.get(ChunkPos.asLong(QuartPos.toSection(qx), QuartPos.toSection(qz))).getNoiseBiome(qx, qy, qz),
                BiomeManager.obfuscateSeed(seed));
            BiomeManager carveManager = new BiomeManager((qx, qy, qz) -> source.getNoiseBiome(qx, qy, qz, sampler), BiomeManager.obfuscateSeed(seed));
            // noise, surface and carvers of the 3x3 chunks
            for (int ox = -1; ox <= 1; ox++) {
                for (int oz = -1; oz <= 1; oz++) {
                    int px = cx + ox;
                    int pz = cz + oz;
                    ProtoChunk chunk = chunks.get(ChunkPos.asLong(px, pz));
                    NoiseChunk nc = NoiseChunk.forChunk(chunk, random, DensityFunctions.BeardifierMarker.INSTANCE, settings, picker, Blender.empty());
                    NoiseSettings ns = settings.noiseSettings().clampToHeightAccessor(accessor);
                    SurfaceRef.fillChunk(chunk, nc, ns, settings.defaultBlock());
                    chunk.setPersistedStatus(ChunkStatus.NOISE);
                    random.surfaceSystem().buildSurface(random, manager, biomeRegistry, settings.useLegacyRandomSource(), new WorldGenerationContext(generator, accessor), chunk, nc, settings.surfaceRule());
                    CarvingContext carving = new CarvingContext((NoiseBasedChunkGenerator) generator, wg, accessor, nc, random, settings.surfaceRule());
                    CarvingMask mask = chunk.getOrCreateCarvingMask(GenerationStep.Carving.AIR);
                    Aquifer aquifer = nc.aquifer();
                    WorldgenRandom carveRandom = new WorldgenRandom(new LegacyRandomSource(0L));
                    for (int dx = -8; dx <= 8; dx++) {
                        for (int dz = -8; dz <= 8; dz++) {
                            ChunkPos cp = new ChunkPos(px + dx, pz + dz);
                            BiomeGenerationSettings gs = source.getNoiseBiome(QuartPos.fromBlock(cp.getMinBlockX()), 0, QuartPos.fromBlock(cp.getMinBlockZ()), sampler).value().getGenerationSettings();
                            int index = 0;
                            for (Holder<ConfiguredWorldCarver<?>> holder : gs.getCarvers(GenerationStep.Carving.AIR)) {
                                ConfiguredWorldCarver<?> carver = holder.value();
                                carveRandom.setLargeFeatureSeed(seed + (long) index, cp.x, cp.z);
                                if (carver.isStartChunk(carveRandom)) {
                                    carver.carve(carving, chunk, carveManager::getBiome, carveRandom, aquifer, cp, mask);
                                }
                                index++;
                            }
                        }
                    }
                    chunk.setPersistedStatus(ChunkStatus.CARVERS);
                }
            }

            // ChunkStatusTasks.generateFeatures
            ProtoChunk center = chunks.get(ChunkPos.asLong(cx, cz));
            Heightmap.primeHeightmaps(center, EnumSet.of(Heightmap.Types.MOTION_BLOCKING, Heightmap.Types.MOTION_BLOCKING_NO_LEAVES, Heightmap.Types.OCEAN_FLOOR, Heightmap.Types.WORLD_SURFACE));
            Region region = new Region(chunks, cx, cz, minY, height, seed, source, sampler, biomeRegistry, accessor, wg);
            WorldGenLevel level = region.proxy;

            // ChunkGenerator.applyBiomeDecoration (no structures); only the supported placed features are placed
            BlockPos origin = SectionPos.of(new ChunkPos(cx, cz), accessor.getMinSection()).origin();
            WorldgenRandom worldgenRandom = new WorldgenRandom(new XoroshiroRandomSource(1L));
            long decorationSeed = worldgenRandom.setDecorationSeed(seed, origin.getX(), origin.getZ());
            Set<Holder<Biome>> biomes = new ObjectArraySet<>();
            for (int dx = -1; dx <= 1; dx++) {
                for (int dz = -1; dz <= 1; dz++) {
                    for (var section : chunks.get(ChunkPos.asLong(cx + dx, cz + dz)).getSections()) {
                        section.getBiomes().getAll(biomes::add);
                    }
                }
            }
            biomes.retainAll(source.possibleBiomes());
            for (int stepIndex = 0; stepIndex < featuresPerStep.size(); stepIndex++) {
                IntArraySet indices = new IntArraySet();
                for (Holder<Biome> holder : biomes) {
                    List<HolderSet<PlacedFeature>> lists = generator.getBiomeGenerationSettings(holder).features();
                    if (stepIndex >= lists.size()) continue;
                    List<PlacedFeature> stepFeatures = featuresPerStep.get(stepIndex).features();
                    lists.get(stepIndex).stream().map(Holder::value).forEach(pf -> indices.add(stepFeatures.indexOf(pf)));
                }
                int[] sorted = indices.toIntArray();
                Arrays.sort(sorted);
                for (int index : sorted) {
                    PlacedFeature pf = featuresPerStep.get(stepIndex).features().get(index);
                    String key = placedRegistry.getKey(pf).toString();
                    if (!supported.contains(key)) continue;
                    worldgenRandom.setFeatureSeed(decorationSeed, index, stepIndex);
                    if (pf.placeWithBiomeCheck(level, generator, worldgenRandom, origin)) {
                        placedCount++;
                    }
                }
            }

            out.writeInt(cx);
            out.writeInt(cz);
            byte[] name = names.get(c).getBytes(StandardCharsets.UTF_8);
            out.writeByte(name.length);
            out.write(name);
            BlockPos.MutableBlockPos pos = new BlockPos.MutableBlockPos();
            for (int ox = -1; ox <= 1; ox++) {
                for (int oz = -1; oz <= 1; oz++) {
                    ChunkAccess chunk = chunks.get(ChunkPos.asLong(cx + ox, cz + oz));
                    ByteBuffer blocks = ByteBuffer.allocate(16 * 16 * height * 2).order(ByteOrder.LITTLE_ENDIAN);
                    for (int y = minY; y < minY + height; y++) {
                        for (int z = 0; z < 16; z++) {
                            for (int x = 0; x < 16; x++) {
                                blocks.putShort((short) Block.getId(chunk.getBlockState(pos.set((cx + ox) * 16 + x, y, (cz + oz) * 16 + z))));
                            }
                        }
                    }
                    out.write(blocks.array());
                }
            }
        }
        try (FileOutputStream f = new FileOutputStream(outFile)) {
            f.write(body.toByteArray());
        }
        System.out.println("RESULT: wrote " + chosen.size() + " centres of " + settingsName + " seed " + seed + " to " + outFile + " (" + placedCount + " placed features reported success)");
    }
}
