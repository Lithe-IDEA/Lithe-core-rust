package io.github.litheidea.plugin.sdk;

import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.Map;

/**
 * The params of one call to a view function: the function a tool window or
 * settings region names in {@code view.function}. Declare it with
 * {@link Backend#view}; the SDK reads the params into this event and answers
 * the handler's {@link ViewResult}.
 *
 * <p>Contract: {@code contract/v1/README.md} ("Plugin views"). View and canvas
 * nodes stay plain JSON maps shaped as {@code plugin-ui.schema.json} describes;
 * Core checks every result before the host draws it.
 */
public final class ViewEvent {
    static final String INVALID_EVENT = "sdk.view.invalidEvent";

    /** Why the host called the view function. */
    public enum Kind {
        /** The view became visible; answer a whole view. */
        OPEN("open"),
        /** The view went away; the result is ignored. */
        CLOSE("close"),
        CLICK("click"),
        CHANGE("change"),
        SELECT("select"),
        ACTIVATE("activate"),
        VIEWPORT("viewport"),
        /**
         * The view's width stopped changing, for a view with a canvas whose
         * {@code on} lists {@code resize}; {@link ViewEvent#width()} is the new width.
         */
        RESIZE("resize"),
        /** The effective or draft setting values changed. */
        SETTINGS("settings");

        private final String wireName;

        Kind(String wireName) {
            this.wireName = wireName;
        }

        /** The name the contract uses, such as {@code open}. */
        public String wireName() {
            return wireName;
        }

        static Kind parse(String name) {
            for (Kind kind : values()) {
                if (kind.wireName.equals(name)) {
                    return kind;
                }
            }
            return null;
        }
    }

    /** The visible part of a canvas, in view-box coordinates. */
    public static final class Viewport {
        private final double x;
        private final double y;
        private final double width;
        private final double height;

        Viewport(double x, double y, double width, double height) {
            this.x = x;
            this.y = y;
            this.width = width;
            this.height = height;
        }

        public double x() {
            return x;
        }

        public double y() {
            return y;
        }

        public double width() {
            return width;
        }

        public double height() {
            return height;
        }
    }

    private final Kind kind;
    private final String instance;
    private final String toolWindowId;
    private final String settingRegionId;
    private final String language;
    private final Map<String, Object> effective;
    private final Map<String, Object> draft;
    private final String node;
    private final String canvas;
    private final Object value;
    private final String setting;
    private final String item;
    private final Viewport viewport;
    private final Double width;

    private ViewEvent(Map<String, Object> params) {
        kind = Kind.parse(required(params, "event"));
        if (kind == null) {
            throw invalid();
        }
        instance = required(params, "instance");
        Map<String, Object> surface = object(params.get("surface"));
        toolWindowId = optional(surface, "toolWindowID");
        settingRegionId = optional(surface, "settingRegionID");
        if ((toolWindowId == null) == (settingRegionId == null)) {
            throw invalid();
        }
        language = required(params, "language");
        Map<String, Object> settings = object(params.get("settings"));
        effective = Collections.unmodifiableMap(new LinkedHashMap<>(object(settings.get("effective"))));
        draft = Collections.unmodifiableMap(new LinkedHashMap<>(object(settings.get("draft"))));
        node = optional(params, "node");
        canvas = optional(params, "canvas");
        value = params.get("value");
        if (value != null && !(value instanceof String || value instanceof Boolean || value instanceof Number)) {
            throw invalid();
        }
        setting = optional(params, "setting");
        item = optional(params, "item");
        Object box = params.get("viewport");
        if (box == null) {
            viewport = null;
        } else {
            Map<String, Object> area = object(box);
            viewport = new Viewport(number(area, "x"), number(area, "y"), number(area, "width"), number(area, "height"));
        }
        width = params.get("width") == null ? null : number(params, "width");
    }

    /**
     * Reads the params the host sent. An optional field that is null counts
     * as absent.
     *
     * @throws PluginSdkError {@code sdk.view.invalidEvent} when a required field
     *     is missing or a field has the wrong type
     */
    public static ViewEvent parse(Map<String, ?> params) {
        if (params == null) {
            throw invalid();
        }
        @SuppressWarnings("unchecked")
        Map<String, Object> fields = (Map<String, Object>) params;
        return new ViewEvent(fields);
    }

    public Kind kind() {
        return kind;
    }

    /** Host-chosen; each {@link Kind#OPEN} starts a new instance. */
    public String instance() {
        return instance;
    }

    /** The tool window being drawn, or null for a settings region. */
    public String toolWindowId() {
        return toolWindowId;
    }

    /** The settings region being drawn, or null for a tool window. */
    public String settingRegionId() {
        return settingRegionId;
    }

    /** Interface language tag, such as {@code en} or {@code zh-Hans}; return view text in it. */
    public String language() {
        return language;
    }

    /** Saved setting values in force, by setting ID; unmodifiable. */
    public Map<String, Object> effective() {
        return effective;
    }

    /** Values changed in the open settings window but not applied; empty in a tool window. Unmodifiable. */
    public Map<String, Object> draft() {
        return draft;
    }

    /** The node acted on: the group inside a canvas, the tree for an item; or null. */
    public String node() {
        return node;
    }

    /** A click inside a canvas, and a viewport event: the canvas ID; or null. */
    public String canvas() {
        return canvas;
    }

    /**
     * A change: a {@code String}, a {@code Boolean} for a toggle, or the bound
     * setting's new draft value typed as the setting stores it; or null.
     */
    public Object value() {
        return value;
    }

    /** A change of a control bound to a setting: the setting ID; or null. */
    public String setting() {
        return setting;
    }

    /** A select or activate event: the tree item ID; or null. */
    public String item() {
        return item;
    }

    /** A viewport event: the visible part of the canvas; or null. */
    public Viewport viewport() {
        return viewport;
    }

    /**
     * The width in points the host lays the view out in, on every event from a
     * host that reports it; or null. List {@code resize} in a canvas's
     * {@code on} only when it is present: a host without it may refuse the canvas.
     */
    public Double width() {
        return width;
    }

    private static PluginSdkError invalid() {
        return new PluginSdkError(INVALID_EVENT, "Invalid view event.");
    }

    private static String required(Map<String, Object> object, String key) {
        String text = optional(object, key);
        if (text == null) {
            throw invalid();
        }
        return text;
    }

    private static String optional(Map<String, Object> object, String key) {
        Object value = object.get(key);
        if (value == null) {
            return null;
        }
        if (!(value instanceof String)) {
            throw invalid();
        }
        return (String) value;
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> object(Object value) {
        if (!(value instanceof Map)) {
            throw invalid();
        }
        return (Map<String, Object>) value;
    }

    private static double number(Map<String, Object> object, String key) {
        Object value = object.get(key);
        if (!(value instanceof Number)) {
            throw invalid();
        }
        return ((Number) value).doubleValue();
    }
}
