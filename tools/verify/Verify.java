import io.netty.buffer.ByteBuf;
import io.netty.buffer.Unpooled;
import it.unimi.dsi.fastutil.ints.IntList;
import java.io.DataInputStream;
import java.io.EOFException;
import java.io.FileInputStream;
import java.util.*;
import java.util.concurrent.Executor;
import net.minecraft.SharedConstants;
import net.minecraft.commands.Commands;
import net.minecraft.core.*;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.core.registries.Registries;
import net.minecraft.network.FriendlyByteBuf;
import net.minecraft.network.ProtocolInfo;
import net.minecraft.network.RegistryFriendlyByteBuf;
import net.minecraft.network.protocol.Packet;
import net.minecraft.network.protocol.common.ClientboundUpdateTagsPacket;
import net.minecraft.network.protocol.configuration.ClientboundRegistryDataPacket;
import net.minecraft.network.protocol.configuration.ConfigurationProtocols;
import net.minecraft.network.protocol.game.*;
import net.minecraft.network.protocol.login.LoginProtocols;
import net.minecraft.resources.RegistryDataLoader;
import net.minecraft.resources.ResourceKey;
import net.minecraft.resources.ResourceLocation;
import net.minecraft.server.Bootstrap;
import net.minecraft.server.RegistryLayer;
import net.minecraft.server.ReloadableServerResources;
import net.minecraft.server.packs.PackType;
import net.minecraft.server.packs.repository.ServerPacksSource;
import net.minecraft.server.packs.resources.MultiPackResourceManager;
import net.minecraft.tags.TagNetworkSerialization;
import net.minecraft.tags.TagsAccess;
import net.minecraft.world.entity.EntityType;
import net.minecraft.world.flag.FeatureFlags;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.chunk.LevelChunkSection;

/**
 * Verification of a Rust server packet dump with the REAL Mojang decoders and codecs (from the 1.21.1 server jar):
 *  - every login/configuration/play packet is decoded with the original codec and must consume the whole buffer,
 *  - registries are loaded the way the client does it (RegistryDataLoader.load from the network),
 *  - registry entry order and tag contents are compared with what the real vanilla server computes.
 */
public class Verify {
    static int ok = 0, fail = 0;

    static void check(String name, boolean cond, String detail) {
        if (cond) ok++; else fail++;
        System.out.println("  [" + (cond ? "OK " : "FAIL") + "] " + name + (detail.isEmpty() ? "" : "  " + detail));
    }

    record Rec(int state, byte[] payload) {}

    static List<Rec> read(String path) throws Exception {
        List<Rec> out = new ArrayList<>();
        try (DataInputStream in = new DataInputStream(new java.io.BufferedInputStream(new FileInputStream(path)))) {
            while (true) {
                int st;
                try { st = in.readUnsignedByte(); } catch (EOFException e) { break; }
                int len = in.readInt();
                byte[] b = new byte[len];
                in.readFully(b);
                out.add(new Rec(st, b));
            }
        }
        return out;
    }

    @SuppressWarnings({"rawtypes", "unchecked"})
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        Executor direct = Runnable::run;

        // --- real vanilla data (like on the server)
        MultiPackResourceManager rm = new MultiPackResourceManager(PackType.SERVER_DATA, List.of(ServerPacksSource.createVanillaPackSource()));
        RegistryAccess staticAccess = RegistryAccess.fromRegistryOfRegistries(BuiltInRegistries.REGISTRY);
        LayeredRegistryAccess<RegistryLayer> layered = RegistryLayer.createRegistryAccess();
        RegistryAccess.Frozen serverWorldgen = RegistryDataLoader.load(rm, staticAccess, RegistryDataLoader.WORLDGEN_REGISTRIES);
        layered = layered.replaceFrom(RegistryLayer.WORLDGEN, serverWorldgen);
        ReloadableServerResources res = ReloadableServerResources.loadResources(rm, layered, FeatureFlags.VANILLA_SET, Commands.CommandSelection.DEDICATED, 2, direct, direct).get();
        res.updateRegistryTags();
        Map<ResourceKey<? extends Registry<?>>, TagNetworkSerialization.NetworkPayload> expectedTags = TagNetworkSerialization.serializeTagsToNetwork(layered);

        String file = args[0];
        boolean expectFullData = args.length > 1 && args[1].equals("full");
        List<Rec> recs = read(file);
        System.out.println("== " + file + ": " + recs.size() + " packets");

        // --- login
        ProtocolInfo<?> login = LoginProtocols.CLIENTBOUND;
        ProtocolInfo<?> config = ConfigurationProtocols.CLIENTBOUND;

        Map<ResourceKey<? extends Registry<?>>, List<net.minecraft.core.RegistrySynchronization.PackedRegistryEntry>> regData = new LinkedHashMap<>();
        ClientboundUpdateTagsPacket tagsPacket = null;
        List<Rec> play = new ArrayList<>();
        int loginN = 0, configN = 0;
        for (Rec r : recs) {
            if (r.state == 2) { play.add(r); continue; }
            ByteBuf buf = Unpooled.wrappedBuffer(r.payload);
            ProtocolInfo<?> info = r.state == 0 ? login : config;
            Packet<?> p;
            try {
                p = (Packet<?>) info.codec().decode(buf);
            } catch (Throwable t) {
                check("decode " + (r.state == 0 ? "login" : "config") + " id=" + r.payload[0], false, t.toString());
                continue;
            }
            if (buf.isReadable()) check("decode " + p.getClass().getSimpleName() + " consumed the whole buffer", false, buf.readableBytes() + " B left");
            if (r.state == 0) loginN++; else configN++;
            if (p instanceof ClientboundRegistryDataPacket rd) regData.put(rd.registry(), rd.entries());
            if (p instanceof ClientboundUpdateTagsPacket t) tagsPacket = t;
        }
        check("login+config: all packets decoded with the real codecs and fully consumed", fail == 0, loginN + " login, " + configN + " config");

        // --- registries like on the client
        RegistryAccess.Frozen netAccess = RegistryDataLoader.load(regData, rm, staticAccess, RegistryDataLoader.SYNCHRONIZED_REGISTRIES);
        check("registries: RegistryDataLoader.load (the client path) accepted all entries from the Rust server", true, regData.size() + " registries");
        for (var rd : RegistryDataLoader.SYNCHRONIZED_REGISTRIES) {
            ResourceKey key = rd.key();
            Registry serverReg = serverWorldgen.registryOrThrow(key);
            Registry clientReg = netAccess.registryOrThrow(key);
            List<ResourceLocation> a = new ArrayList<>(), b = new ArrayList<>();
            for (Object k : serverReg.keySet()) a.add((ResourceLocation) k);
            for (Object k : clientReg.keySet()) b.add((ResourceLocation) k);
            // ID order: compare getId by key
            boolean same = a.size() == b.size();
            if (same) for (ResourceLocation rl : a) {
                Object sv = serverReg.get(rl), cv = clientReg.get(rl);
                if (sv == null || cv == null || serverReg.getId(sv) != clientReg.getId(cv)) { same = false; break; }
            }
            check("registry " + key.location() + ": entry count and IDs match the vanilla server", same, a.size() + " entries");
            // deep comparison: both objects encoded back to JSON with the REAL codec must be equal
            int diffs = 0; String firstDiff = "";
            // every object is encoded in the context of ITS OWN registry set (holders point to their own registries)
            var opsClient = net.minecraft.resources.RegistryOps.create(com.mojang.serialization.JsonOps.INSTANCE, (HolderLookup.Provider) composite0(layered, netAccess));
            var opsServer = net.minecraft.resources.RegistryOps.create(com.mojang.serialization.JsonOps.INSTANCE, (HolderLookup.Provider) layered.compositeAccess());
            com.mojang.serialization.Codec codec = rd.elementCodec();
            for (ResourceLocation rl : a) {
                Object sv = serverReg.get(rl), cv = clientReg.get(rl);
                if (sv == null || cv == null) continue;
                com.google.gson.JsonElement sj = (com.google.gson.JsonElement) ((com.mojang.serialization.DataResult) codec.encodeStart(opsServer, sv)).getOrThrow();
                com.google.gson.JsonElement cj = (com.google.gson.JsonElement) ((com.mojang.serialization.DataResult) codec.encodeStart(opsClient, cv)).getOrThrow();
                if (!sj.equals(cj)) { diffs++; if (firstDiff.isEmpty()) firstDiff = rl.toString(); }
            }
            check("registry " + key.location() + ": objects from the Rust packets == objects of the vanilla server (codec->JSON)", diffs == 0, diffs == 0 ? "" : diffs + " differences, e.g. " + firstDiff);
        }

        // --- tagi
        Map<String, Map<String, Set<Integer>>> exp = new TreeMap<>(), got = new TreeMap<>();
        for (var e : expectedTags.entrySet()) {
            Map<String, Set<Integer>> m = new TreeMap<>();
            TagsAccess.tags(e.getValue()).forEach((k, v) -> m.put(k.toString(), new TreeSet<>(v)));
            exp.put(e.getKey().location().toString(), m);
        }
        if (tagsPacket != null) {
            for (var e : tagsPacket.getTags().entrySet()) {
                Map<String, Set<Integer>> m = new TreeMap<>();
                TagsAccess.tags(e.getValue()).forEach((k, v) -> m.put(k.toString(), new TreeSet<>(v)));
                got.put(e.getKey().location().toString(), m);
            }
        }
        check("tags: the same registries as in vanilla", exp.keySet().equals(got.keySet()), "vanilla=" + exp.keySet().size() + " rust=" + got.keySet().size() + (exp.keySet().equals(got.keySet()) ? "" : " difference: " + diffKeys(exp.keySet(), got.keySet())));
        int tagsTotal = 0, tagsBad = 0;
        for (var e : exp.entrySet()) {
            Map<String, Set<Integer>> g = got.getOrDefault(e.getKey(), Map.of());
            boolean regOk = e.getValue().keySet().equals(g.keySet());
            for (var t : e.getValue().entrySet()) {
                tagsTotal++;
                if (!t.getValue().equals(g.get(t.getKey()))) {
                    tagsBad++;
                    if (tagsBad <= 5) System.out.println("      tag difference " + e.getKey() + " " + t.getKey() + ": vanilla=" + t.getValue().size() + " rust=" + (g.get(t.getKey()) == null ? "missing" : g.get(t.getKey()).size()));
                }
            }
            if (!regOk) check("tagi " + e.getKey() + ": set of tag names", false, "vanilla=" + e.getValue().size() + " rust=" + g.size());
        }
        check("tags: contents (IDs) identical to the real server for every tag", tagsBad == 0, tagsTotal + " tags, " + tagsBad + " differences");

        // --- play
        RegistryAccess.Frozen composite = layered.replaceFrom(RegistryLayer.WORLDGEN, netAccess).compositeAccess();
        ProtocolInfo<?> game = GameProtocols.CLIENTBOUND_TEMPLATE.bind(RegistryFriendlyByteBuf.decorator(composite));
        Map<String, Integer> counts = new TreeMap<>();
        int playBad = 0, chunksChecked = 0, chunkBad = 0;
        for (Rec r : play) {
            ByteBuf buf = Unpooled.wrappedBuffer(r.payload);
            Packet<?> p;
            try {
                p = (Packet<?>) game.codec().decode(buf);
            } catch (Throwable t) {
                playBad++;
                System.out.println("      play decode FAIL id=" + (r.payload[0] & 0xff) + ": " + t);
                continue;
            }
            counts.merge(p.getClass().getSimpleName(), 1, Integer::sum);
            if (buf.isReadable()) { playBad++; System.out.println("      " + p.getClass().getSimpleName() + ": " + buf.readableBytes() + " B unconsumed"); }
            if (p instanceof ClientboundLoginPacket l) {
                check("play/login: dimension = overworld, dimension type resolved through the client registry", l.commonPlayerSpawnInfo().dimension().location().toString().equals("minecraft:overworld") && l.commonPlayerSpawnInfo().dimensionType().is(net.minecraft.world.level.dimension.BuiltinDimensionTypes.OVERWORLD), "eid=" + l.playerId() + " view=" + l.chunkRadius());
            }
            if (p instanceof ClientboundLevelChunkWithLightPacket c) {
                chunksChecked++;
                try {
                    FriendlyByteBuf data = c.getChunkData().getReadBuffer();
                    Registry<net.minecraft.world.level.biome.Biome> biomes = composite.registryOrThrow(Registries.BIOME);
                    boolean good = true;
                    for (int s = 0; s < 24; s++) {
                        LevelChunkSection sec = new LevelChunkSection(biomes);
                        sec.read(data);
                        if (s == 0 && chunksChecked == 1) {
                            check("chunk: blok y=-64 = bedrock (rzeczywisty BlockState)", sec.getBlockState(3, 0, 5).is(Blocks.BEDROCK), "");
                            check("chunk: blok y=-63/-62 = dirt", sec.getBlockState(3, 1, 5).is(Blocks.DIRT) && sec.getBlockState(3, 2, 5).is(Blocks.DIRT), "");
                            check("chunk: blok y=-61 = grass_block", sec.getBlockState(3, 3, 5).is(Blocks.GRASS_BLOCK), "");
                            check("chunk: blok y=-60 = powietrze", sec.getBlockState(3, 4, 5).isAir(), "");
                            check("chunk: biome = plains (by ID from the client registry)", sec.getNoiseBiome(1, 1, 1).is(net.minecraft.world.level.biome.Biomes.PLAINS), "");
                        }
                    }
                    if (data.isReadable()) { good = false; }
                    if (!good) { chunkBad++; }
                    var ld = c.getLightData();
                    if (chunksChecked == 1) {
                        check("chunk: light: masks and arrays consistent", ld.getSkyYMask().cardinality() == ld.getSkyUpdates().size() && ld.getBlockYMask().cardinality() == ld.getBlockUpdates().size() && ld.getSkyUpdates().stream().allMatch(a -> a.length == 2048), "sky=" + ld.getSkyYMask().cardinality() + " empty=" + ld.getEmptySkyYMask().cardinality());
                    }
                } catch (Throwable t) {
                    chunkBad++;
                    System.out.println("      chunk: " + t);
                }
            }
            if (p instanceof ClientboundAddEntityPacket ae) {
                check("add_entity: typ = player", ae.getType() == EntityType.PLAYER, "id=" + ae.getId());
            }
        }
        check("play: all packets decoded with the real codecs and fully consumed", playBad == 0, play.size() + " packets");
        check("play: chunks fully read by LevelChunkSection.read", chunkBad == 0 && chunksChecked > 0, chunksChecked + " chunks");
        System.out.println("  play types: " + counts);
        System.out.println("== RESULT: " + ok + " OK, " + fail + " FAILURES");
        System.exit(fail == 0 ? 0 : 1);
    }

    static RegistryAccess.Frozen composite0(LayeredRegistryAccess<RegistryLayer> layered, RegistryAccess.Frozen netAccess) {
        return layered.replaceFrom(RegistryLayer.WORLDGEN, netAccess).compositeAccess();
    }

    static String diffKeys(Set<String> a, Set<String> b) {
        Set<String> onlyA = new TreeSet<>(a); onlyA.removeAll(b);
        Set<String> onlyB = new TreeSet<>(b); onlyB.removeAll(a);
        return "only vanilla=" + onlyA + " only rust=" + onlyB;
    }
}
