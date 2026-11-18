package io.github.litheidea.plugin.sdk;

import static io.github.litheidea.plugin.sdk.Fixtures.map;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.util.Arrays;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

/** View functions; mirrors {@code sdk/rust/plugin-sdk/src/view.rs} and {@code tests/view.rs}. */
@Timeout(value = 30, unit = TimeUnit.SECONDS)
class ViewTest {
    private static Map<String, Object> event(Object... extra) {
        Map<String, Object> params = map(
                "event", "change",
                "instance", "i1",
                "surface", map("settingRegionID", "otp"),
                "language", "zh-Hans",
                "settings", map("effective", map("otpRelease", "26"), "draft", map("otpRelease", "27")));
        params.putAll(map(extra));
        return params;
    }

    @Test
    void readsEveryFieldTheHostSends() {
        ViewEvent parsed = ViewEvent.parse(event(
                "node", "otp", "setting", "otpRelease", "value", "27",
                "viewport", map("x", 0L, "y", 10.5, "width", 1200L, "height", 640L)));
        assertEquals(ViewEvent.Kind.CHANGE, parsed.kind());
        assertEquals("otp", parsed.settingRegionId());
        assertNull(parsed.toolWindowId());
        assertEquals("zh-Hans", parsed.language());
        assertEquals("26", parsed.effective().get("otpRelease"));
        assertEquals("27", parsed.draft().get("otpRelease"));
        assertEquals("otpRelease", parsed.setting());
        assertEquals("27", parsed.value());
        assertEquals(10.5, parsed.viewport().y());
        assertEquals(1200.0, parsed.viewport().width());
        assertNull(parsed.item());
        assertThrows(UnsupportedOperationException.class, () -> parsed.draft().put("otpRelease", "28"));
        // An optional field sent as null counts as absent.
        ViewEvent absent = ViewEvent.parse(event("node", null, "value", null, "viewport", null));
        assertNull(absent.node());
        assertNull(absent.value());
        assertNull(absent.viewport());
    }

    @Test
    void refusesEventsWithoutTheirRequiredShape() {
        Map<String, Object> noDraft = event();
        noDraft.put("settings", map("effective", map()));
        List<Map<String, Object>> cases = Arrays.asList(
                null,
                event("event", "hover"),
                event("surface", map("toolWindowID", "a", "settingRegionID", "b")),
                event("surface", map("toolWindowID", "a", "settingRegionID", 3L)),
                event("surface", map()),
                noDraft,
                event("node", 3L),
                event("value", Arrays.asList("a")),
                event("viewport", map("x", 0L)));
        for (Map<String, Object> params : cases) {
            PluginSdkError error = assertThrows(PluginSdkError.class, () -> ViewEvent.parse(params), String.valueOf(params));
            assertEquals("sdk.view.invalidEvent", error.code());
        }
    }

    @Test
    void resultsAndPatchesSerializeAsTheSchemaNamesThem() {
        assertEquals(map(), ViewResult.unchanged().toJson());
        assertEquals(map("view", Arrays.asList(map("type", "label", "text", "a"))),
                ViewResult.view(Arrays.asList(map("type", "label", "text", "a"))).toJson());
        ViewPatch insert = ViewPatch.insert(map("id", "web", "label", "web")).inTree("modules");
        ViewResult patches = ViewResult.patches(Arrays.asList(
                ViewPatch.set("filter", map("enabled", false)),
                ViewPatch.remove("guava").inCanvas("graph"),
                insert.under("app").at(0),
                ViewPatch.replace("refresh", map("type", "button", "id", "refresh", "text", "Stop"))));
        assertEquals(map("patches", Arrays.asList(
                map("op", "set", "id", "filter", "fields", map("enabled", false)),
                map("op", "remove", "id", "guava", "canvas", "graph"),
                map("op", "insert", "node", map("id", "web", "label", "web"), "tree", "modules", "parent", "app",
                        "index", 0L),
                map("op", "replace", "id", "refresh", "node", map("type", "button", "id", "refresh", "text", "Stop")))),
                patches.toJson());
        // Modifiers leave the patch they started from unchanged.
        assertEquals(map("op", "insert", "node", map("id", "web", "label", "web"), "tree", "modules"), insert.toJson());
    }

    @Test
    void aViewFunctionAnswersViewsPatchesAndNothing() {
        Backend backend = Backend.create("test-backend", "1.0.0").view("explorer.draw", (event, context) -> {
            assertEquals("dev.example.explorer", event.toolWindowId());
            switch (event.kind()) {
                case OPEN:
                    return ViewResult.view(Collections.singletonList(map("type", "label", "text", event.language())));
                case CLICK:
                    return ViewResult.patches(Collections.singletonList(
                            ViewPatch.set(event.node(), map("enabled", false))));
                default:
                    return null;
            }
        });
        TestHost host = new TestHost(backend);
        Map<String, Object> answer = Json.object(host.initialize().get("result"));
        assertEquals(Arrays.asList(map("id", "explorer.draw")),
                Json.object(Json.object(Json.object(answer.get("capabilities")).get("experimental"))
                        .get("litheFunctions")).get("functions"));

        assertEquals(map("view", Arrays.asList(map("type", "label", "text", "zh-Hans"))), invoke(host, "open").get("result"));
        assertEquals(map("patches", Arrays.asList(map("op", "set", "id", "refresh", "fields", map("enabled", false)))),
                invoke(host, "click").get("result"));
        assertEquals(map(), invoke(host, "close").get("result"));
        // Params that are not a view event never reach the handler.
        Map<String, Object> refused = host.request("lithe/function/invoke",
                map("functionID", "explorer.draw", "operationID", "bad", "params", map("event", "open")));
        assertEquals(-32603L, Json.object(refused.get("error")).get("code"));
        assertEquals(1, host.close());
    }

    @Test
    void aViewFunctionIsCheckedLikeAnyFunction() {
        PluginSdkError invalid = assertThrows(PluginSdkError.class,
                () -> Backend.create("test-backend", "1.0.0").view("Bad-ID", (event, context) -> null));
        assertEquals("sdk.function.invalidID", invalid.code());
        assertThrows(IllegalArgumentException.class, () -> Backend.create("test-backend", "1.0.0").view("explorer.draw", null));
    }

    private static Map<String, Object> invoke(TestHost host, String kind) {
        Map<String, Object> params = new LinkedHashMap<>(map(
                "event", kind,
                "instance", "i1",
                "surface", map("toolWindowID", "dev.example.explorer"),
                "language", "zh-Hans",
                "settings", map("effective", map(), "draft", map()),
                "node", "refresh"));
        return host.request("lithe/function/invoke", map("functionID", "explorer.draw", "operationID", kind, "params", params));
    }
}
