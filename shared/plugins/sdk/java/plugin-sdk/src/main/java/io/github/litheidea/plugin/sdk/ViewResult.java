package io.github.litheidea.plugin.sdk;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * What a view function answers. The first result after
 * {@link ViewEvent.Kind#OPEN} is a whole view.
 */
public final class ViewResult {
    private static final ViewResult UNCHANGED = new ViewResult(null, null);

    private final List<Object> view;
    private final List<Object> patches;

    private ViewResult(List<Object> view, List<Object> patches) {
        this.view = view;
        this.patches = patches;
    }

    /** The whole view: its nodes, top to bottom, as JSON maps. */
    public static ViewResult view(List<?> nodes) {
        if (nodes == null) {
            throw new IllegalArgumentException("A view must not be null.");
        }
        return new ViewResult(new ArrayList<Object>(nodes), null);
    }

    /** Edits to the view the host is drawing, applied in order. */
    public static ViewResult patches(List<ViewPatch> patches) {
        if (patches == null) {
            throw new IllegalArgumentException("Patches must not be null.");
        }
        List<Object> edits = new ArrayList<>();
        for (ViewPatch patch : patches) {
            edits.add(patch.toJson());
        }
        return new ViewResult(null, edits);
    }

    /** Nothing changed; the same answer as a handler returning null. */
    public static ViewResult unchanged() {
        return UNCHANGED;
    }

    /** The result as the contract names it: {@code {view}}, {@code {patches}}, or {@code {}}. */
    public Map<String, Object> toJson() {
        Map<String, Object> result = new LinkedHashMap<>();
        if (view != null) {
            result.put("view", new ArrayList<>(view));
        }
        if (patches != null) {
            result.put("patches", new ArrayList<>(patches));
        }
        return result;
    }
}
