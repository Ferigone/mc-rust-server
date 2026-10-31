import java.nio.file.Files;
import java.nio.file.Path;
import java.util.LinkedHashMap;
import java.util.Map;
import net.minecraft.SharedConstants;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.component.DataComponents;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.item.BlockItem;
import net.minecraft.world.item.Item;
import net.minecraft.world.level.EmptyBlockGetter;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.phys.AABB;
import net.minecraft.world.phys.shapes.CollisionContext;
import net.minecraft.world.phys.shapes.VoxelShape;

/**
 * Dumps facts of the REAL Mojang block and item classes (after Bootstrap) into text tables that the Rust server loads:
 *   block_props.tsv: "S" lines = deduplicated voxel shapes, "B" lines = one block state each.
 *   item_props.tsv:  one item per line.
 * Usage: PropsDump <output dir>. Run through tools/dump_props.sh.
 */
public class PropsDump {
    static final Map<String, Integer> SHAPES = new LinkedHashMap<>();

    static int shape(VoxelShape s) {
        StringBuilder sb = new StringBuilder();
        int n = 0;
        for (AABB b : s.toAabbs()) {
            sb.append('\t').append(b.minX).append('\t').append(b.minY).append('\t').append(b.minZ)
              .append('\t').append(b.maxX).append('\t').append(b.maxY).append('\t').append(b.maxZ);
            n++;
        }
        String key = n + sb.toString();
        Integer id = SHAPES.get(key);
        if (id == null) {
            id = SHAPES.size();
            SHAPES.put(key, id);
        }
        return id;
    }

    static int bit(boolean v, int i) {
        return v ? (1 << i) : 0;
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        var level = EmptyBlockGetter.INSTANCE;
        var pos = BlockPos.ZERO;
        var ctx = CollisionContext.empty();
        StringBuilder states = new StringBuilder();
        int count = 0;
        for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
            int flags = bit(s.isAir(), 0) | bit(!s.getFluidState().isEmpty(), 1) | bit(s.canOcclude(), 2)
                | bit(s.useShapeForLightOcclusion(), 3) | bit(s.isRandomlyTicking(), 4) | bit(s.requiresCorrectToolForDrops(), 5)
                | bit(s.canBeReplaced(), 6) | bit(s.isSolid(), 7) | bit(s.blocksMotion(), 8) | bit(s.propagatesSkylightDown(level, pos), 9)
                | bit(s.isSolidRender(level, pos), 10) | bit(s.hasLargeCollisionShape(), 11) | bit(s.isCollisionShapeFullBlock(level, pos), 12)
                | bit(s.ignitedByLava(), 13) | bit(s.isSuffocating(level, pos), 14) | bit(s.isViewBlocking(level, pos), 15)
                | bit(s.hasBlockEntity(), 16) | bit(s.hasOffsetFunction(), 17);
            Block b = s.getBlock();
            states.append("B\t").append(Block.BLOCK_STATE_REGISTRY.getId(s))
                .append('\t').append(flags)
                .append('\t').append(s.getLightEmission())
                .append('\t').append(s.getLightBlock(level, pos))
                .append('\t').append(s.getDestroySpeed(level, pos))
                .append('\t').append(b.getExplosionResistance())
                .append('\t').append(b.getFriction())
                .append('\t').append(b.getSpeedFactor())
                .append('\t').append(b.getJumpFactor())
                .append('\t').append(s.getPistonPushReaction().ordinal())
                .append('\t').append(s.getMapColor(level, pos).id)
                .append('\t').append(shape(s.getCollisionShape(level, pos, ctx)))
                .append('\t').append(shape(s.getShape(level, pos, ctx)))
                .append('\t').append(shape(s.getOcclusionShape(level, pos)))
                .append('\t').append(shape(s.getBlockSupportShape(level, pos)));
            for (Direction d : Direction.values()) {
                states.append('\t').append(shape(s.getFaceOcclusionShape(level, pos, d)));
            }
            states.append('\n');
            count++;
        }
        StringBuilder out = new StringBuilder();
        out.append("# block_props v1: S <index> <boxes> (minX minY minZ maxX maxY maxZ)*; B <state> <flags> <emission> <lightBlock> "
            + "<destroySpeed> <explosionResistance> <friction> <speedFactor> <jumpFactor> <pushReaction> <mapColor> "
            + "<collision> <outline> <occlusion> <support> <face occlusion DOWN UP NORTH SOUTH WEST EAST>\n");
        for (Map.Entry<String, Integer> e : SHAPES.entrySet()) {
            out.append("S\t").append(e.getValue()).append('\t').append(e.getKey()).append('\n');
        }
        out.append(states);
        Path dir = Path.of(args[0]);
        Files.createDirectories(dir);
        Files.writeString(dir.resolve("block_props.tsv"), out.toString());

        StringBuilder items = new StringBuilder("# item_props v1: <id> <name> <maxStackSize> <maxDamage> <block name or ->\n");
        for (Item it : BuiltInRegistries.ITEM) {
            String block = it instanceof BlockItem bi ? BuiltInRegistries.BLOCK.getKey(bi.getBlock()).toString() : "-";
            items.append(BuiltInRegistries.ITEM.getId(it)).append('\t').append(BuiltInRegistries.ITEM.getKey(it))
                .append('\t').append(it.getDefaultMaxStackSize())
                .append('\t').append(it.components().getOrDefault(DataComponents.MAX_DAMAGE, 0))
                .append('\t').append(block).append('\n');
        }
        Files.writeString(dir.resolve("item_props.tsv"), items.toString());
        System.out.println("RESULT: " + count + " block states, " + SHAPES.size() + " shapes, " + BuiltInRegistries.ITEM.size() + " items -> " + dir);
    }
}
