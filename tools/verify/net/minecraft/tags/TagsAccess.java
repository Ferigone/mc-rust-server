package net.minecraft.tags;

import it.unimi.dsi.fastutil.ints.IntList;
import java.util.Map;
import net.minecraft.resources.ResourceLocation;

/** Helper in the net.minecraft.tags package: exposes the package-private `tags` field of the network payload. */
public final class TagsAccess {
    private TagsAccess() {}

    public static Map<ResourceLocation, IntList> tags(TagNetworkSerialization.NetworkPayload p) {
        return p.tags;
    }
}
