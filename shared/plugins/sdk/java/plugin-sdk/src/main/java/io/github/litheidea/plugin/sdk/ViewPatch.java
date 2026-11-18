package io.github.litheidea.plugin.sdk;

import java.util.LinkedHashMap;
import java.util.Map;

/**
 * One edit to a drawn view, addressed by node ID within the host controls, or
 * within one canvas scene ({@link #inCanvas}) or one tree's items
 * ({@link #inTree}). Patches are immutable; each modifier returns a new one.
 */
public final class ViewPatch {
    private final Map<String, Object> fields;

    private ViewPatch(Map<String, Object> fields) {
        this.fields = fields;
    }

    private static ViewPatch of(String op, String id) {
        Map<String, Object> patch = new LinkedHashMap<>();
        patch.put("op", op);
        if (id != null) {
            patch.put("id", id);
        }
        return new ViewPatch(patch);
    }

    private ViewPatch with(String key, Object value) {
        Map<String, Object> patch = new LinkedHashMap<>(fields);
        patch.put(key, value);
        return new ViewPatch(patch);
    }

    /** Merges {@code fields} into the node; a null field removes it. */
    public static ViewPatch set(String id, Map<String, ?> fields) {
        return of("set", id).with("fields", fields);
    }

    /** Swaps the node, with its subtree, for {@code node}. */
    public static ViewPatch replace(String id, Map<String, ?> node) {
        return of("replace", id).with("node", node);
    }

    /** Deletes the node and its subtree. */
    public static ViewPatch remove(String id) {
        return of("remove", id);
    }

    /** Adds {@code node} at the end of the top level; see {@link #under} and {@link #at}. */
    public static ViewPatch insert(Map<String, ?> node) {
        return of("insert", null).with("node", node);
    }

    /** An insert into the children of {@code parent}. */
    public ViewPatch under(String parent) {
        return with("parent", parent);
    }

    /** An insert at {@code index} instead of the end. */
    public ViewPatch at(int index) {
        return with("index", (long) index);
    }

    /** Addresses the nodes of canvas {@code canvas}. */
    public ViewPatch inCanvas(String canvas) {
        return with("canvas", canvas);
    }

    /** Addresses the items of tree {@code tree}. */
    public ViewPatch inTree(String tree) {
        return with("tree", tree);
    }

    /** The patch as a JSON map. */
    public Map<String, Object> toJson() {
        return new LinkedHashMap<>(fields);
    }
}
