import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.state.BlockState;

/**
 * Dumps per-state facts of the REAL Mojang block classes: "<state id> <isAir 0/1> <fluid non-empty 0/1>".
 * The Rust tables (mc-data `Blocks::has_fluid`, the air IDs of `Palette`) are tested against this dump.
 */
public class BlockFlags {
    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        StringBuilder sb = new StringBuilder();
        for (BlockState s : Block.BLOCK_STATE_REGISTRY) {
            sb.append(Block.BLOCK_STATE_REGISTRY.getId(s)).append(' ')
              .append(s.isAir() ? 1 : 0).append(' ')
              .append(s.getFluidState().isEmpty() ? 0 : 1).append('\n');
        }
        java.nio.file.Files.writeString(java.nio.file.Path.of(args[0]), sb.toString());
        System.out.println("RESULT: wrote " + Block.BLOCK_STATE_REGISTRY.size() + " states to " + args[0]);
    }
}
