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
 * Weryfikacja zrzutu pakietów serwera Rust PRAWDZIWYMI dekoderami i kodekami Mojang (z jara serwera 1.21.1):
 *  - każdy pakiet login/configuration/play jest dekodowany oryginalnym kodekiem i musi zużyć cały bufor,
 *  - rejestry są ładowane tak jak robi to klient (RegistryDataLoader.load z sieci),
 *  - kolejność wpisów rejestrów i zawartość tagów porównywane z tym, co policzy prawdziwy serwer vanilla.
 */
public class Verify {
    static int ok = 0, fail = 0;

    static void check(String name, boolean cond, String detail) {
        if (cond) ok++; else fail++;
        System.out.println("  [" + (cond ? "OK " : "BŁĄD") + "] " + name + (detail.isEmpty() ? "" : "  " + detail));
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

        // --- prawdziwe dane vanilla (jak na serwerze)
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
        System.out.println("== " + file + ": " + recs.size() + " pakietów");

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
            if (buf.isReadable()) check("decode " + p.getClass().getSimpleName() + " zużył cały bufor", false, buf.readableBytes() + " B zostało");
            if (r.state == 0) loginN++; else configN++;
            if (p instanceof ClientboundRegistryDataPacket rd) regData.put(rd.registry(), rd.entries());
            if (p instanceof ClientboundUpdateTagsPacket t) tagsPacket = t;
        }
        check("login+config: wszystkie pakiety zdekodowane prawdziwymi kodekami i w pełni zużyte", fail == 0, loginN + " login, " + configN + " config");

        // --- rejestry jak u klienta
        RegistryAccess.Frozen netAccess = RegistryDataLoader.load(regData, rm, staticAccess, RegistryDataLoader.SYNCHRONIZED_REGISTRIES);
        check("rejestry: RegistryDataLoader.load (ścieżka klienta) przyjął wszystkie wpisy z serwera Rust", true, regData.size() + " rejestrów");
        for (var rd : RegistryDataLoader.SYNCHRONIZED_REGISTRIES) {
            ResourceKey key = rd.key();
            Registry serverReg = serverWorldgen.registryOrThrow(key);
            Registry clientReg = netAccess.registryOrThrow(key);
            List<ResourceLocation> a = new ArrayList<>(), b = new ArrayList<>();
            for (Object k : serverReg.keySet()) a.add((ResourceLocation) k);
            for (Object k : clientReg.keySet()) b.add((ResourceLocation) k);
            // kolejność ID: porównujemy getId po kluczu
            boolean same = a.size() == b.size();
            if (same) for (ResourceLocation rl : a) {
                Object sv = serverReg.get(rl), cv = clientReg.get(rl);
                if (sv == null || cv == null || serverReg.getId(sv) != clientReg.getId(cv)) { same = false; break; }
            }
            check("rejestr " + key.location() + ": liczba wpisów i ID zgodne z serwerem vanilla", same, a.size() + " wpisów");
            // głębokie porównanie: oba obiekty kodowane z powrotem PRAWDZIWYM kodekiem do JSON muszą być równe
            int diffs = 0; String firstDiff = "";
            // każdy obiekt kodujemy w kontekście SWOJEGO zestawu rejestrów (holdery wskazują na własne rejestry)
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
            check("rejestr " + key.location() + ": obiekty z pakietów Rust == obiekty serwera vanilla (kodek→JSON)", diffs == 0, diffs == 0 ? "" : diffs + " różnic, np. " + firstDiff);
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
        check("tagi: te same rejestry co w vanilla", exp.keySet().equals(got.keySet()), "vanilla=" + exp.keySet().size() + " rust=" + got.keySet().size() + (exp.keySet().equals(got.keySet()) ? "" : " różnica: " + diffKeys(exp.keySet(), got.keySet())));
        int tagsTotal = 0, tagsBad = 0;
        for (var e : exp.entrySet()) {
            Map<String, Set<Integer>> g = got.getOrDefault(e.getKey(), Map.of());
            boolean regOk = e.getValue().keySet().equals(g.keySet());
            for (var t : e.getValue().entrySet()) {
                tagsTotal++;
                if (!t.getValue().equals(g.get(t.getKey()))) {
                    tagsBad++;
                    if (tagsBad <= 5) System.out.println("      różnica tagu " + e.getKey() + " " + t.getKey() + ": vanilla=" + t.getValue().size() + " rust=" + (g.get(t.getKey()) == null ? "brak" : g.get(t.getKey()).size()));
                }
            }
            if (!regOk) check("tagi " + e.getKey() + ": zestaw nazw tagów", false, "vanilla=" + e.getValue().size() + " rust=" + g.size());
        }
        check("tagi: zawartość (ID) identyczna z prawdziwym serwerem dla każdego tagu", tagsBad == 0, tagsTotal + " tagów, " + tagsBad + " różnic");

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
                System.out.println("      BŁĄD dekodowania play id=" + (r.payload[0] & 0xff) + ": " + t);
                continue;
            }
            counts.merge(p.getClass().getSimpleName(), 1, Integer::sum);
            if (buf.isReadable()) { playBad++; System.out.println("      " + p.getClass().getSimpleName() + ": " + buf.readableBytes() + " B niezużytych"); }
            if (p instanceof ClientboundLoginPacket l) {
                check("play/login: wymiar = overworld, typ wymiaru rozwiązany przez rejestr klienta", l.commonPlayerSpawnInfo().dimension().location().toString().equals("minecraft:overworld") && l.commonPlayerSpawnInfo().dimensionType().is(net.minecraft.world.level.dimension.BuiltinDimensionTypes.OVERWORLD), "eid=" + l.playerId() + " view=" + l.chunkRadius());
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
                            check("chunk: biom = plains (po ID z rejestru klienta)", sec.getNoiseBiome(1, 1, 1).is(net.minecraft.world.level.biome.Biomes.PLAINS), "");
                        }
                    }
                    if (data.isReadable()) { good = false; }
                    if (!good) { chunkBad++; }
                    var ld = c.getLightData();
                    if (chunksChecked == 1) {
                        check("chunk: światło — maski i tablice spójne", ld.getSkyYMask().cardinality() == ld.getSkyUpdates().size() && ld.getBlockYMask().cardinality() == ld.getBlockUpdates().size() && ld.getSkyUpdates().stream().allMatch(a -> a.length == 2048), "sky=" + ld.getSkyYMask().cardinality() + " empty=" + ld.getEmptySkyYMask().cardinality());
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
        check("play: wszystkie pakiety zdekodowane prawdziwymi kodekami i w pełni zużyte", playBad == 0, play.size() + " pakietów");
        check("play: chunki w całości odczytane przez LevelChunkSection.read", chunkBad == 0 && chunksChecked > 0, chunksChecked + " chunków");
        System.out.println("  typy play: " + counts);
        System.out.println("== WYNIK: " + ok + " OK, " + fail + " BŁĘDÓW");
        System.exit(fail == 0 ? 0 : 1);
    }

    static RegistryAccess.Frozen composite0(LayeredRegistryAccess<RegistryLayer> layered, RegistryAccess.Frozen netAccess) {
        return layered.replaceFrom(RegistryLayer.WORLDGEN, netAccess).compositeAccess();
    }

    static String diffKeys(Set<String> a, Set<String> b) {
        Set<String> onlyA = new TreeSet<>(a); onlyA.removeAll(b);
        Set<String> onlyB = new TreeSet<>(b); onlyB.removeAll(a);
        return "tylko vanilla=" + onlyA + " tylko rust=" + onlyB;
    }
}
