package net.minecraft.tags;

import it.unimi.dsi.fastutil.ints.IntList;
import java.util.Map;
import net.minecraft.resources.ResourceLocation;

/** Pomocnik w pakiecie net.minecraft.tags: odsłania prywatne pole `tags` ładunku sieciowego. */
public final class TagsAccess {
    private TagsAccess() {}

    public static Map<ResourceLocation, IntList> tags(TagNetworkSerialization.NetworkPayload p) {
        return p.tags;
    }
}
