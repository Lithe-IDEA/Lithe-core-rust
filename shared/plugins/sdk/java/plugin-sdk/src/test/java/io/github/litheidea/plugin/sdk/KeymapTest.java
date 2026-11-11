package io.github.litheidea.plugin.sdk;

import static io.github.litheidea.plugin.sdk.Fixtures.map;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNotNull;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

@Timeout(value = 30, unit = TimeUnit.SECONDS)
class KeymapTest {
    @SuppressWarnings("unchecked")
    private static Map<String, Object> object(Object value) { return (Map<String, Object>) value; }

    @Test
    void negotiatesOnlyOfferedProvidersAndRejectsUnscopedRequests() {
        Backend backend = Backend.create("test-backend", null)
                .keymapImportProvider("first", (snapshot, context) -> {
                    assertEquals("first", snapshot.providerId());
                    assertEquals("snapshot-1", snapshot.snapshotId());
                    return map("bindings", Collections.emptyList());
                })
                .keymapImportProvider("not-offered", (snapshot, context) -> map("bindings", Collections.emptyList()));
        TestHost host = new TestHost(backend);
        try {
            Map<String, Object> initialized = host.initialize(map("litheKeymapImport",
                    map("versions", Arrays.asList(0), "providers", Arrays.asList("first", "unknown"))));
            Map<String, Object> capabilities = object(object(initialized.get("result")).get("capabilities"));
            assertEquals(map("version", 0L, "providers", Arrays.asList("first")),
                    object(capabilities.get("experimental")).get("litheKeymapImport"));
            assertEquals(map("bindings", Collections.emptyList()), host.request("lithe/keymap/import",
                    map("providerID", "first", "snapshotID", "snapshot-1")).get("result"));
            // The terminal fact precedes the response and names the wire method Core accepts.
            List<List<Object>> facts = new ArrayList<>();
            for (Map<String, Object> message : host.messages()) {
                if ("lithe/log".equals(message.get("method"))) {
                    Map<String, Object> fact = object(message.get("params"));
                    facts.add(Arrays.asList(fact.get("kind"), fact.get("operation")));
                }
            }
            assertEquals(Collections.singletonList(Arrays.asList("operationFinished", "lithe/keymap/import")), facts);
            assertNotNull(host.request("lithe/keymap/import", map("providerID", "not-offered", "snapshotID", "snapshot-1")).get("error"));
            assertNotNull(host.request("lithe/keymap/import", map("providerID", "first", "snapshotID", "../private")).get("error"));
        } finally { host.close(); }
    }

    @Test
    void readsTheSelectedSnapshotThroughTheScopedHost() {
        Backend backend = Backend.create("test-backend", null).keymapImportProvider("first", (snapshot, context) -> {
            Backend.KeymapSourceFile file = snapshot.listInputs().get(3, TimeUnit.SECONDS).get(0);
            byte[] bytes = snapshot.readChunk(file.id(), 0, 4).get(3, TimeUnit.SECONDS);
            assertEquals("data", new String(bytes, StandardCharsets.UTF_8));
            return map("bindings", Arrays.asList(map("commandID", "find-next", "keys",
                    Arrays.asList(map("key", "f3", "modifiers", Collections.emptyList())))));
        });
        TestHost host = new TestHost(backend);
        try {
            host.initialize(map("litheFunctions", map("versions", Arrays.asList(0)), "litheKeymapImport",
                    map("versions", Arrays.asList(0), "providers", Arrays.asList("first"))));
            CompletableFuture<Map<String, Object>> pending = host.requestAsync("lithe/keymap/import",
                    map("providerID", "first", "snapshotID", "snapshot-1"), 101L);
            Map<String, Object> list = host.waitFor(message -> "lithe/host/keymapImport/listInputs".equals(message.get("method")), "list inputs");
            host.send(map("id", list.get("id"), "result", map("files", Arrays.asList(map("id", "file-0", "path", "keys.json", "byteCount", 4)))));
            Map<String, Object> read = host.waitFor(message -> "lithe/host/keymapImport/readChunk".equals(message.get("method")), "read chunk");
            assertEquals("file-0", object(read.get("params")).get("fileID"));
            host.send(map("id", read.get("id"), "result", map("data", "ZGF0YQ==")));
            assertEquals(map("bindings", Arrays.asList(map("commandID", "find-next", "keys",
                    Arrays.asList(map("key", "f3", "modifiers", Collections.emptyList()))))),
                    Fixtures.await(pending, "import result").get("result"));
        } finally { host.close(); }
    }

    @Test
    void cancellationReachesTheRunningProvider() {
        CountDownLatch entered = new CountDownLatch(1), cancelled = new CountDownLatch(1);
        Backend backend = Backend.create("test-backend", null).keymapImportProvider("first", (snapshot, context) -> {
            context.cancellation().onCancel(cancelled::countDown);
            entered.countDown();
            if (!cancelled.await(3, TimeUnit.SECONDS)) throw new AssertionError("provider was not cancelled");
            context.throwIfCancelled();
            return map("bindings", Collections.emptyList());
        });
        TestHost host = new TestHost(backend);
        try {
            host.initialize(map("litheKeymapImport", map("versions", Arrays.asList(0), "providers", Arrays.asList("first"))));
            CompletableFuture<Map<String, Object>> pending = host.requestAsync("lithe/keymap/import",
                    map("providerID", "first", "snapshotID", "snapshot-1"), 102L);
            Fixtures.await(entered, "provider started");
            host.notify("$/cancelRequest", map("id", 102L));
            Fixtures.await(cancelled, "provider cancelled");
            assertEquals(-32800L, object(Fixtures.await(pending, "cancelled import").get("error")).get("code"));
        } finally { host.close(); }
    }
}
