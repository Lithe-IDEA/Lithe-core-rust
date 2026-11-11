package io.github.litheidea.plugin.sdk;

import static io.github.litheidea.plugin.sdk.Fixtures.map;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assertions.fail;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CompletionException;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutionException;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.TimeoutException;
import java.util.concurrent.atomic.AtomicReference;
import java.util.stream.Collectors;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

/** Host services; mirrors {@code sdk/node/test/host.test.mjs}. */
@Timeout(value = 30, unit = TimeUnit.SECONDS)
class HostTest {
    private static final Map<String, Object> CONTEXT = Collections.unmodifiableMap(map("contextID", "w1", "generation", 2L));
    private static final Map<String, Object> FIXTURE = loadFixture();

    private static Map<String, Object> loadFixture() {
        try {
            return object(Json.parse(new String(Files.readAllBytes(
                    Paths.get("../../../contract/v1/fixtures/backend/function-protocol.json")), StandardCharsets.UTF_8)));
        } catch (IOException error) {
            throw new UncheckedIOException(error);
        }
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> object(Object value) {
        return (Map<String, Object>) value;
    }

    private static List<Map<String, Object>> cases(String key) {
        List<Map<String, Object>> cases = new ArrayList<>();
        for (Object entry : (List<?>) FIXTURE.get(key)) {
            cases.add(object(entry));
        }
        return cases;
    }

    /** A host whose session negotiated plugin functions. */
    private static TestHost session(Backend backend) {
        TestHost host = new TestHost(backend);
        host.initialize(map("litheFunctions", map("versions", Arrays.asList(0L))));
        return host;
    }

    /** The next host request the backend wrote that the test has not taken yet. */
    private static Map<String, Object> nextRequest(TestHost host, String method, Set<Object> taken) {
        Map<String, Object> request = host.waitFor(message -> method.equals(message.get("method"))
                && message.get("id") != null && !taken.contains(message.get("id")), method);
        taken.add(request.get("id"));
        return request;
    }

    private static Map<String, Object> nextRequest(TestHost host, String method) {
        return nextRequest(host, method, new HashSet<>());
    }

    private static <T> T value(CompletableFuture<T> future) {
        return Fixtures.await(future, "the host request");
    }

    /** What {@code future} failed with. */
    private static Throwable failure(CompletableFuture<?> future) {
        try {
            future.get(TestHost.WAIT_MILLIS, TimeUnit.MILLISECONDS);
        } catch (ExecutionException error) {
            return error.getCause();
        } catch (InterruptedException | TimeoutException error) {
            throw new AssertionError("The host request did not fail in time.", error);
        }
        return fail("The host request succeeded.");
    }

    private static String code(CompletableFuture<?> future) {
        Throwable error = failure(future);
        assertTrue(error instanceof PluginSdkError, String.valueOf(error));
        return ((PluginSdkError) error).code();
    }

    private static List<Map<String, Object>> sent(TestHost host, String method) {
        return host.messages().stream().filter(message -> method.equals(message.get("method"))).collect(Collectors.toList());
    }

    @Test
    void nativeTerminalMethodsKeepContextAndLiteralArguments() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        try {
            CompletableFuture<Map<String, Object>> creating = backend.host().terminal().create(CONTEXT,
                    map("executable", "example-client", "arguments", Arrays.asList("literal;$(value)", "space value")));
            Map<String, Object> request = nextRequest(host, Protocol.HOST_TERMINAL_CREATE);
            assertEquals(CONTEXT, object(request.get("params")).get("context"));
            assertEquals(Arrays.asList("literal;$(value)", "space value"), object(request.get("params")).get("arguments"));
            host.send(map("id", request.get("id"), "result", map("terminalID", "owned")));
            assertEquals("owned", value(creating).get("terminalID"));
            CompletableFuture<Map<String, Object>> closing = backend.host().terminal().close(CONTEXT, map("terminalID", "owned"));
            Map<String, Object> close = nextRequest(host, Protocol.HOST_TERMINAL_CLOSE);
            host.send(map("id", close.get("id"), "result", map("retainedContextBytes", 4096L)));
            assertEquals(4096L, value(closing).get("retainedContextBytes"));
            CompletableFuture<Map<String, Object>> reopening = backend.host().terminal().reopen(CONTEXT, map("terminalID", "owned"));
            Map<String, Object> reopen = nextRequest(host, Protocol.HOST_TERMINAL_REOPEN);
            host.send(map("id", reopen.get("id"), "result", map("terminalID", "owned", "generation", 2L, "restoredContextBytes", 4096L)));
            assertEquals(2L, value(reopening).get("generation"));
        } finally { host.close(); }
    }

    @Test
    void theSdkAnswersPluginFunctionsOnlyWhenTheHostOffersVersionZero() {
        Object[][] cases = {{Arrays.asList(0L), true}, {Arrays.asList(2L, 99L, 0L), true}, {Arrays.asList(1L), false},
                {Arrays.asList(99L), false}, {Collections.emptyList(), false}};
        for (Object[] testCase : cases) {
            Backend backend = Backend.create(null, null).function("query.run", (params, context) -> null);
            TestHost host = new TestHost(backend);
            Map<String, Object> result = object(host.initialize(map("litheFunctions", map("versions", testCase[0]))).get("result"));
            Map<String, Object> experimental = object(object(result.get("capabilities")).get("experimental"));
            boolean expected = (Boolean) testCase[1];
            if (expected) {
                assertEquals(map("version", 0L, "functions", Arrays.asList(map("id", "query.run"))),
                        experimental.get("litheFunctions"));
            } else {
                assertFalse(experimental.containsKey("litheFunctions"));
            }
            assertEquals(expected, backend.negotiated().functions());
            assertEquals(expected, backend.host().negotiated());
            host.close();
        }
    }

    @Test
    void credentialsRoundTripThroughTheHostWithTheContractShapes() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Host.Credentials credentials = backend.host().credentials();
        Set<Object> taken = new HashSet<>();

        CompletableFuture<String> reading = credentials.get("connection.prod");
        Map<String, Object> get = nextRequest(host, Protocol.HOST_CREDENTIALS_GET, taken);
        assertEquals(map("key", "connection.prod"), get.get("params"));
        host.send(map("id", get.get("id"), "result", map("secret", "s3cret")));
        assertEquals("s3cret", value(reading));

        CompletableFuture<String> missing = credentials.get("connection.prod");
        host.send(map("id", nextRequest(host, Protocol.HOST_CREDENTIALS_GET, taken).get("id"), "result", null));
        assertNull(value(missing));

        CompletableFuture<String> malformed = credentials.get("connection.prod");
        host.send(map("id", nextRequest(host, Protocol.HOST_CREDENTIALS_GET, taken).get("id"), "result", map()));
        assertEquals("sdk.host.invalidResult", code(malformed));

        CompletableFuture<Void> writing = credentials.set("connection.prod:password", "s3cret");
        Map<String, Object> set = nextRequest(host, Protocol.HOST_CREDENTIALS_SET, taken);
        assertEquals(map("key", "connection.prod:password", "secret", "s3cret"), set.get("params"));
        host.send(map("id", set.get("id"), "result", null));
        assertNull(value(writing));

        CompletableFuture<Void> deleting = credentials.delete("connection.prod");
        Map<String, Object> remove = nextRequest(host, Protocol.HOST_CREDENTIALS_DELETE, taken);
        assertEquals(map("key", "connection.prod"), remove.get("params"));
        host.send(map("id", remove.get("id"), "result", null));
        value(deleting);

        // Malformed arguments fail locally and never reach the host.
        int before = host.messages().size();
        StringBuilder longKey = new StringBuilder();
        for (int index = 0; index < 129; index++) {
            longKey.append('k');
        }
        for (String key : Arrays.asList("", "has space", longKey.toString(), null)) {
            assertEquals("sdk.credentials.invalidKey", code(credentials.get(key)), String.valueOf(key));
        }
        StringBuilder secret = new StringBuilder();
        for (int index = 0; index <= Protocol.MAX_CREDENTIAL_SECRET_BYTES; index++) {
            secret.append('x');
        }
        assertEquals("sdk.credentials.invalidSecret", code(credentials.set("key", null)));
        assertEquals("sdk.credentials.invalidSecret", code(credentials.set("key", secret.toString())));
        assertEquals(before, host.messages().size());
        host.close();
    }

    @Test
    void localKeyAndContextChecksAgreeWithTheFixtureCasesCoreRefusesOrForwards() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        for (Map<String, Object> testCase : cases("hostRequests")) {
            String name = (String) testCase.get("name");
            String method = (String) testCase.get("method");
            Object rawParams = testCase.get("params");
            if (!(rawParams instanceof Map)) {
                // Array params and absent params carry no key or context to compare.
                continue;
            }
            Map<String, Object> params = object(rawParams);
            Map<String, Object> expected = object(testCase.get("expected"));
            if (method.startsWith("lithe/host/credentials/")) {
                String key = (String) params.get("key");
                if (expected.get("forward") != null) {
                    assertTrue(Protocol.isCredentialKey(key), name);
                } else if (name.contains("key")) {
                    assertEquals("sdk.credentials.invalidKey", code(backend.host().credentials().get(key)), name);
                }
            }
            if (name.matches(".*(context (without|ID outside)|generation is positive).*")) {
                assertEquals("sdk.host.invalidContext",
                        code(backend.host().workspace().getFolders(object(params.get("context")))), name);
            }
        }
        // A full context from listContexts is passed on as its reference.
        Map<String, Object> full = map("contextID", "w1", "generation", 2L, "name", "orders", "folders", Collections.emptyList());
        CompletableFuture<Map<String, Object>> asking = backend.host().workspace().getFolders(full);
        Map<String, Object> request = nextRequest(host, Protocol.HOST_WORKSPACE_GET_FOLDERS);
        assertEquals(map("context", CONTEXT), request.get("params"));
        host.send(map("id", request.get("id"), "result", map("folders", Collections.emptyList())));
        assertEquals(map("folders", Collections.emptyList()), value(asking));
        host.close();
    }

    @Test
    void hostRefusalsBecomeHostErrorWithTheCodeNumberAndDetailsCoreSent() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Set<Object> taken = new HashSet<>();
        java.util.function.Function<Map<String, Object>, HostError> answer = wire -> {
            CompletableFuture<Object> pending = backend.host().request(Protocol.HOST_WORKSPACE_LIST_CONTEXTS, null);
            Map<String, Object> request = nextRequest(host, Protocol.HOST_WORKSPACE_LIST_CONTEXTS, taken);
            host.send(map("id", request.get("id"), "error", wire));
            Throwable error = failure(pending);
            assertTrue(error instanceof HostError, String.valueOf(error));
            return (HostError) error;
        };
        for (Map<String, Object> testCase : cases("hostRequests")) {
            Map<String, Object> expected = object(testCase.get("expected"));
            if (expected.get("code") == null) {
                continue;
            }
            String name = (String) testCase.get("name");
            Map<String, Object> data = map("code", expected.get("code"));
            if (expected.get("details") != null) {
                data.put("details", expected.get("details"));
            }
            HostError error = answer.apply(map("code", expected.get("rpcCode"), "message", "Refused.", "data", data));
            assertEquals(expected.get("code"), error.code(), name);
            assertEquals(((Number) expected.get("rpcCode")).intValue(), error.rpcCode().intValue(), name);
            assertEquals(expected.get("details"), error.details(), name);
            assertEquals(Protocol.HOST_WORKSPACE_LIST_CONTEXTS, error.method(), name);
        }
        for (Map<String, Object> testCase : cases("hostFailures")) {
            Map<String, Object> expected = object(testCase.get("expected"));
            String name = (String) testCase.get("name");
            int rpcCode = ((Number) expected.get("rpcCode")).intValue();
            assertEquals(Integer.valueOf(rpcCode), HostError.CODES.get(expected.get("code")), name);
            // The number alone names the code.
            assertEquals(expected.get("code"), answer.apply(map("code", (long) rpcCode, "message", "Failed.")).code(), name);
        }
        HostError unknown = answer.apply(map("code", -32050L, "message", "Later code.",
                "data", map("details", Arrays.asList("not", "an", "object"))));
        assertEquals("plugin.host.failed", unknown.code());
        assertNull(unknown.details());
        host.close();
    }

    @Test
    void withoutPluginFunctionsTheHostOffersNoServiceAndHostRequestsStayWithinLitheHost() {
        Backend backend = Backend.create(null, null);
        TestHost host = new TestHost(backend);
        host.initialize(map());
        assertFalse(backend.host().negotiated());
        int before = host.messages().size();
        Throwable unavailable = failure(backend.host().credentials().get("connection.prod"));
        assertTrue(unavailable instanceof HostError);
        assertEquals("plugin.host.unavailable", ((HostError) unavailable).code());
        assertEquals(Integer.valueOf(-32601), ((HostError) unavailable).rpcCode());
        assertEquals("plugin.host.unavailable", code(backend.host().capabilities()));
        assertFalse(value(backend.host().supports(Protocol.HOST_CREDENTIALS_GET)));
        assertEquals("sdk.host.method", code(backend.host().request("lithe/function/invoke", map())));
        assertEquals(before, host.messages().size());
        host.close();

        Backend idle = Backend.create(null, null);
        assertEquals("sdk.session.notReady", code(idle.host().credentials().get("key")));
    }

    @Test
    void capabilitiesAreAskedForOnceAndSupportsReadsThem() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Object answer = null;
        for (Map<String, Object> testCase : cases("hostRequests")) {
            if (object(testCase.get("expected")).get("answer") != null) {
                answer = object(testCase.get("expected")).get("answer");
                break;
            }
        }
        CompletableFuture<Boolean> first = backend.host().supports(Protocol.HOST_WORKSPACE_GET_FOLDERS);
        CompletableFuture<Boolean> second = backend.host().supports(Protocol.HOST_WORKSPACE_READ_FILE);
        Map<String, Object> request = nextRequest(host, Protocol.HOST_GET_CAPABILITIES);
        assertFalse(request.containsKey("params"));
        host.send(map("id", request.get("id"), "result", answer));
        assertEquals(Arrays.asList(true, false), Arrays.asList(value(first), value(second)));
        Map<String, Object> limits = object(value(backend.host().capabilities()).get("limits"));
        assertEquals((long) Protocol.MAX_PENDING_HOST_REQUESTS, limits.get("pendingHostRequests"));
        assertEquals(1, sent(host, Protocol.HOST_GET_CAPABILITIES).size());
        host.close();
    }

    @Test
    void aCallCarriesItsWorkspaceContextToTheHandler() {
        for (Map<String, Object> testCase : cases("invocationContexts")) {
            Map<String, Object> expected = object(testCase.get("expected"));
            if (expected.get("params") == null) {
                continue;
            }
            String name = (String) testCase.get("name");
            Map<String, Object> params = object(expected.get("params"));
            AtomicReference<Object> seen = new AtomicReference<>("unset");
            Backend backend = Backend.create(null, null).function((String) params.get("functionID"), (arguments, context) -> {
                seen.set(context.workspaceContext());
                return null;
            });
            TestHost host = session(backend);
            Map<String, Object> response = host.request(Protocol.FUNCTION_INVOKE, params);
            assertNull(response.get("error"), name);
            assertEquals(params.get("context"), seen.get(), name);
            host.close();
        }
    }

    @Test
    void cancellingACallCancelsItsHostRequestsAtCore() {
        CountDownLatch started = new CountDownLatch(1);
        AtomicReference<Throwable> outcome = new AtomicReference<>();
        Backend backend = Backend.create(null, null).function("secret.read", (params, context) -> {
            CompletableFuture<String> reading = context.host().credentials().get("connection.prod");
            started.countDown();
            try {
                return reading.join();
            } catch (CompletionException error) {
                outcome.set(error.getCause());
                throw error;
            }
        });
        TestHost host = session(backend);
        CompletableFuture<Map<String, Object>> call = host.requestAsync(Protocol.FUNCTION_INVOKE,
                map("functionID", "secret.read", "operationID", "c"), 41L);
        Fixtures.await(started, "the host request");
        Map<String, Object> request = nextRequest(host, Protocol.HOST_CREDENTIALS_GET);
        host.notify(Protocol.CANCEL_REQUEST, map("id", 41L));
        Map<String, Object> response = Fixtures.await(call, "the cancelled call");
        assertEquals((long) Protocol.REQUEST_CANCELLED, object(response.get("error")).get("code"));
        // The host request fails with the call's cancellation.
        assertTrue(outcome.get() instanceof CancelledException, String.valueOf(outcome.get()));
        List<Object> cancels = sent(host, Protocol.CANCEL_REQUEST).stream().map(message -> message.get("params"))
                .collect(Collectors.toList());
        assertEquals(Arrays.asList(map("id", request.get("id"))), cancels);
        // Core's late answer is dropped; the session continues.
        host.send(map("id", request.get("id"), "error",
                map("code", -32800L, "message", "Cancelled.", "data", map("code", "plugin.host.cancelled"))));
        assertEquals(1, host.close());
    }

    @Test
    void aCancelledTokenRejectsWithItsReasonAndIsNeverSentAgain() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Cancellation stop = new Cancellation();
        Host cancellable = backend.host().withCancellation(stop);
        Map<String, Object> edit = map("documentID", "d1", "expectedVersion", 4L, "edits", Collections.emptyList());
        CompletableFuture<Map<String, Object>> pending = cancellable.editor().applyEdits(CONTEXT, edit);
        Map<String, Object> request = nextRequest(host, Protocol.HOST_EDITOR_APPLY_EDITS);
        Map<String, Object> withContext = map("context", CONTEXT);
        withContext.putAll(edit);
        assertEquals(withContext, request.get("params"));
        stop.cancel("user stopped");
        Throwable error = failure(pending);
        assertTrue(error instanceof CancelledException);
        assertEquals("user stopped", error.getMessage());
        assertEquals(Arrays.asList(map("id", request.get("id"))),
                sent(host, Protocol.CANCEL_REQUEST).stream().map(message -> message.get("params")).collect(Collectors.toList()));
        host.send(map("id", request.get("id"), "result", map("documentID", "d1", "version", 5L, "applied", true)));
        assertTrue(failure(cancellable.request(Protocol.HOST_EDITOR_APPLY_EDITS, map())) instanceof CancelledException);
        assertEquals(1, sent(host, Protocol.HOST_EDITOR_APPLY_EDITS).size());
        host.close();
    }

    @Test
    void atMostSixteenHostRequestsAreUnansweredLaterOnesWaitForAFreeSlot() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        int max = Protocol.MAX_PENDING_HOST_REQUESTS;
        List<CompletableFuture<Map<String, Object>>> calls = new ArrayList<>();
        for (int index = 0; index < max + 2; index++) {
            calls.add(backend.host().workspace().listContexts());
        }
        // Requests with a free slot are written before listContexts returns.
        List<Map<String, Object>> first = sent(host, Protocol.HOST_WORKSPACE_LIST_CONTEXTS);
        assertEquals(max, first.size());
        Set<Object> taken = new HashSet<>();
        for (Map<String, Object> message : first) {
            taken.add(message.get("id"));
        }
        host.send(map("id", first.get(0).get("id"), "result", map("revision", 1L, "contexts", Collections.emptyList())));
        nextRequest(host, Protocol.HOST_WORKSPACE_LIST_CONTEXTS, taken);
        assertEquals(max + 1, sent(host, Protocol.HOST_WORKSPACE_LIST_CONTEXTS).size());
        // Ending the session fails the queued request too.
        host.close();
        assertEquals(map("revision", 1L, "contexts", Collections.emptyList()), value(calls.get(0)));
        for (CompletableFuture<Map<String, Object>> call : calls.subList(1, calls.size())) {
            assertEquals("sdk.session.closed", code(call));
        }
    }

    @Test
    void subscriptionEventsReachTheirListenerInOrderAlsoWhenSentBeforeTheAnswer() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        List<Host.Event> events = Collections.synchronizedList(new ArrayList<>());
        CountDownLatch received = new CountDownLatch(3);
        Map<String, Object> watched = map("resource", map("rootID", "r1", "path", ""), "recursive", true);
        CompletableFuture<Host.Subscription> watching = backend.host().workspace().watch(CONTEXT, watched, event -> {
            events.add(event);
            received.countDown();
        });
        Map<String, Object> request = nextRequest(host, Protocol.HOST_WORKSPACE_WATCH);
        Map<String, Object> withContext = map("context", CONTEXT);
        withContext.putAll(watched);
        assertEquals(withContext, request.get("params"));
        java.util.function.LongConsumer notify = sequence -> host.notify(Protocol.HOST_SUBSCRIPTION_NOTIFY, map(
                "subscriptionID", "s1", "sequence", sequence,
                "event", map("rootID", "r1", "changedPaths", Arrays.asList("f" + sequence), "requiresRescan", false)));
        notify.accept(1);
        host.send(map("id", request.get("id"), "result", map("subscriptionID", "s1")));
        Host.Subscription subscription = value(watching);
        notify.accept(2);
        notify.accept(4);
        Fixtures.await(received, "three events");
        List<List<Object>> seen = new ArrayList<>();
        synchronized (events) {
            for (Host.Event event : events) {
                seen.add(Arrays.asList(event.subscriptionId(), event.sequence(), event.gap(),
                        object(event.event()).get("changedPaths")));
            }
        }
        assertEquals(Arrays.asList(
                Arrays.asList("s1", 1L, false, Arrays.asList("f1")),
                Arrays.asList("s1", 2L, false, Arrays.asList("f2")),
                Arrays.asList("s1", 4L, true, Arrays.asList("f4"))), seen);
        assertEquals("s1", subscription.id());
        assertFalse(subscription.isDisposed());

        CompletableFuture<Void> disposing = subscription.dispose();
        assertTrue(subscription.isDisposed());
        Map<String, Object> dispose = nextRequest(host, Protocol.HOST_SUBSCRIPTIONS_DISPOSE);
        assertEquals(map("subscriptionID", "s1"), dispose.get("params"));
        notify.accept(5);
        host.send(map("id", dispose.get("id"), "result", null));
        value(disposing);
        // The reader handled event 5 before the dispose answer; it was dropped.
        assertEquals(3, events.size());
        host.close();
    }

    @Test
    void commandsContextChangesAndLaterHostNotificationsReachTheirHandlers() {
        List<Map<String, Object>> delivered = new ArrayList<>();
        for (Map<String, Object> testCase : cases("hostNotifications")) {
            if (Boolean.TRUE.equals(object(testCase.get("expected")).get("delivered"))
                    && !Protocol.HOST_SUBSCRIPTION_NOTIFY.equals(testCase.get("method"))) {
                delivered.add(testCase);
            }
        }
        Set<List<Object>> expected = new HashSet<>();
        // Run exits and language server events have no dedicated API: onHostNotification routes them.
        Set<String> otherMethods = new HashSet<>();
        for (Map<String, Object> testCase : delivered) {
            Map<String, Object> params = object(testCase.get("params"));
            String deliveredMethod = (String) testCase.get("method");
            if (Protocol.HOST_COMMAND_DID_RUN.equals(deliveredMethod)) {
                expected.add(Arrays.asList("command", params.get("commandID"), params.get("context")));
            } else if (Protocol.HOST_DID_CHANGE_CONTEXTS.equals(deliveredMethod)) {
                expected.add(Arrays.asList("contexts", params));
            } else {
                expected.add(Arrays.asList("host", deliveredMethod, params));
                otherMethods.add(deliveredMethod);
            }
        }
        expected.add(Arrays.asList("later", map("value", 1L)));
        Set<List<Object>> seen = Collections.synchronizedSet(new HashSet<>());
        CountDownLatch all = new CountDownLatch(expected.size());
        Backend backend = Backend.create(null, null)
                .onCommand("db.refresh", (commandId, context) -> {
                    seen.add(Arrays.asList("command", commandId, context));
                    all.countDown();
                })
                .onContextsChanged(params -> {
                    seen.add(Arrays.asList("contexts", params));
                    all.countDown();
                })
                .onHostNotification("lithe/host/later/didSomething", params -> {
                    seen.add(Arrays.asList("later", params));
                    all.countDown();
                });
        for (String otherMethod : otherMethods) {
            backend.onHostNotification(otherMethod, params -> {
                seen.add(Arrays.asList("host", otherMethod, params));
                all.countDown();
            });
        }
        assertEquals("sdk.method.reserved", assertThrows(PluginSdkError.class,
                () -> backend.onHostNotification(Protocol.HOST_COMMAND_DID_RUN, params -> {})).code());
        assertEquals("sdk.method.invalid", assertThrows(PluginSdkError.class,
                () -> backend.onHostNotification("workspace/didChange", params -> {})).code());
        assertEquals("sdk.command.invalidID", assertThrows(PluginSdkError.class,
                () -> backend.onCommand("DB_Refresh", (commandId, context) -> {})).code());
        TestHost host = session(backend);
        for (Map<String, Object> testCase : delivered) {
            host.notify((String) testCase.get("method"), testCase.get("params"));
        }
        // A notification this SDK does not know is ignored, as the contract asks.
        host.notify("lithe/host/unknown/ignored", map("anything", true));
        host.notify("lithe/host/later/didSomething", map("value", 1L));
        Fixtures.await(all, "the notifications");
        // Handlers run on their own threads, so only the set is fixed.
        assertEquals(expected, new HashSet<>(seen));
        assertEquals(expected.size(), seen.size());
        host.close();
    }

    /** {@code base} as a new map, with {@code overrides} (alternating keys and values) replacing or adding entries. */
    private static Map<String, Object> with(Map<String, Object> base, Object... overrides) {
        Map<String, Object> copy = map();
        copy.putAll(base);
        copy.putAll(map(overrides));
        return copy;
    }

    private static String repeat(char character, int count) {
        StringBuilder text = new StringBuilder();
        for (int index = 0; index < count; index++) {
            text.append(character);
        }
        return text.toString();
    }

    /** Makes one relayed call, lets the host answer it, and returns the params the host received. */
    private static Map<String, Object> relayed(Backend backend, TestHost host, Set<Object> taken, Map<String, Object> call) {
        CompletableFuture<Map<String, Object>> pending = backend.host().functions().invoke(CONTEXT, call);
        Map<String, Object> request = nextRequest(host, Protocol.HOST_FUNCTIONS_INVOKE, taken);
        host.send(map("id", request.get("id"), "result", map("value", null)));
        assertEquals(map("value", null), value(pending));
        return object(request.get("params"));
    }

    @Test
    void aFunctionSeesTheCallingPluginCoreWrote() {
        for (Map<String, Object> testCase : cases("invocationCallers")) {
            String name = (String) testCase.get("name");
            // The wire message Core sends, and whether it holds a caller.
            Map<String, Object> wire = object(object(testCase.get("expected")).get("params"));
            AtomicReference<Object> seen = new AtomicReference<>("unset");
            AtomicReference<Object> seenPluginId = new AtomicReference<>("unset");
            Backend backend = Backend.create(null, null).function((String) wire.get("functionID"), (arguments, context) -> {
                seen.set(context.caller());
                seenPluginId.set(context.callerPluginId());
                return null;
            });
            TestHost host = session(backend);
            assertNull(host.request(Protocol.FUNCTION_INVOKE, wire).get("error"), name);
            Object expectedCaller = wire.get("caller");
            assertEquals(expectedCaller, seen.get(), name);
            assertEquals(expectedCaller == null ? null : object(expectedCaller).get("pluginID"), seenPluginId.get(), name);
            host.close();
        }

        // Only exactly { pluginID: non-empty string } is a caller, and it cannot be changed.
        AtomicReference<Object> seen = new AtomicReference<>("unset");
        AtomicReference<Object> seenPluginId = new AtomicReference<>("unset");
        Backend backend = Backend.create(null, null).function("project.describe", (arguments, context) -> {
            seen.set(context.caller());
            seenPluginId.set(context.callerPluginId());
            return null;
        });
        TestHost host = session(backend);
        List<Object> notCallers = Arrays.<Object>asList("dev.lithe.fixture.python", Collections.emptyList(), map(),
                map("pluginID", ""), map("pluginID", 7L), map("plugin", "dev.lithe.fixture.python"),
                map("pluginID", "dev.lithe.fixture.python", "name", "python"));
        for (Object notCaller : notCallers) {
            assertNull(host.request(Protocol.FUNCTION_INVOKE,
                    map("functionID", "project.describe", "operationID", "7", "params", map(), "caller", notCaller)).get("error"));
            assertNull(seen.get(), String.valueOf(notCaller));
            assertNull(seenPluginId.get(), String.valueOf(notCaller));
        }
        assertNull(host.request(Protocol.FUNCTION_INVOKE, map("functionID", "project.describe", "operationID", "7")).get("error"));
        assertNull(seen.get());
        assertNull(host.request(Protocol.FUNCTION_INVOKE, map("functionID", "project.describe", "operationID", "7",
                "caller", map("pluginID", "dev.lithe.fixture.python"))).get("error"));
        Map<String, Object> caller = object(seen.get());
        assertEquals(map("pluginID", "dev.lithe.fixture.python"), caller);
        assertEquals("dev.lithe.fixture.python", seenPluginId.get());
        assertThrows(UnsupportedOperationException.class, () -> caller.put("pluginID", "dev.lithe.fixture.other"));
        // A caller key inside the params is ordinary data, not the calling plugin.
        assertNull(host.request(Protocol.FUNCTION_INVOKE, map("functionID", "project.describe", "operationID", "7",
                "params", map("caller", map("pluginID", "dev.lithe.fixture.other")))).get("error"));
        assertNull(seen.get());
        host.close();
    }

    @Test
    void relayRequestsCarryTheContractParamsAndAnswerTheHostsResultUnchanged() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Set<Object> taken = new HashSet<>();
        int relayCases = 0;
        for (Map<String, Object> testCase : cases("hostRequests")) {
            String method = (String) testCase.get("method");
            if (!Boolean.TRUE.equals(object(testCase.get("expected")).get("relay"))) {
                continue;
            }
            String name = (String) testCase.get("name");
            Map<String, Object> wire = object(testCase.get("params"));
            CompletableFuture<Map<String, Object>> pending;
            Object answer;
            if (Protocol.HOST_FUNCTIONS_PROVIDERS.equals(method)) {
                pending = backend.host().functions().providers((String) wire.get("functionID"));
                answer = map("providers", Arrays.asList(map("pluginID", "dev.lithe.fixture.go", "version", 1L)));
            } else {
                assertEquals(Protocol.HOST_FUNCTIONS_INVOKE, method, name);
                Map<String, Object> call = with(wire);
                call.remove("context");
                pending = backend.host().functions().invoke(object(wire.get("context")), call);
                answer = map("value", map("settings", Arrays.asList("a")));
            }
            Map<String, Object> request = nextRequest(host, method, taken);
            assertEquals(wire, request.get("params"), name);
            host.send(map("id", request.get("id"), "result", answer));
            assertEquals(answer, value(pending), name);
            relayCases++;
        }
        assertTrue(relayCases > 0, "The fixture has no relay case.");
        // A provider's null result is a result.
        CompletableFuture<Map<String, Object>> nothing = backend.host().functions().invoke(CONTEXT,
                map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings"));
        host.send(map("id", nextRequest(host, Protocol.HOST_FUNCTIONS_INVOKE, taken).get("id"), "result", map("value", null)));
        assertEquals(map("value", null), value(nothing));
        host.close();
    }

    @Test
    void withoutPluginFunctionsTheRelayFailsLocallyWithoutSendingAnything() {
        Backend backend = Backend.create(null, null);
        TestHost host = new TestHost(backend);
        host.initialize(map());
        int before = host.messages().size();
        Map<String, Object> call = map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings");
        List<CompletableFuture<?>> attempts = new ArrayList<>();
        attempts.add(backend.host().functions().providers("languageImport.settings"));
        attempts.add(backend.host().functions().invoke(CONTEXT, call));
        attempts.add(backend.host().request(Protocol.HOST_FUNCTIONS_PROVIDERS, map("functionID", "languageImport.settings")));
        attempts.add(backend.host().request(Protocol.HOST_FUNCTIONS_INVOKE, with(call, "context", CONTEXT)));
        for (CompletableFuture<?> attempt : attempts) {
            Throwable error = failure(attempt);
            assertTrue(error instanceof HostError, String.valueOf(error));
            assertEquals("plugin.host.unavailable", ((HostError) error).code());
            assertEquals(Integer.valueOf(-32601), ((HostError) error).rpcCode());
        }
        assertEquals(before, host.messages().size());
        host.close();
    }

    @Test
    void relayFailuresKeepTheirCodeNumberAndTheProvidersOwnErrorCode() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Set<Object> taken = new HashSet<>();
        Map<String, Object> call = map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings");
        java.util.function.Function<Map<String, Object>, HostError> refused = wire -> {
            CompletableFuture<Map<String, Object>> pending = backend.host().functions().invoke(CONTEXT, call);
            Map<String, Object> request = nextRequest(host, Protocol.HOST_FUNCTIONS_INVOKE, taken);
            host.send(map("id", request.get("id"), "error", wire));
            Throwable error = failure(pending);
            assertTrue(error instanceof HostError, String.valueOf(error));
            return (HostError) error;
        };
        int failures = 0;
        for (Map<String, Object> testCase : cases("relayErrors")) {
            String name = (String) testCase.get("name");
            String code = (String) testCase.get("code");
            long rpcCode = ((Number) testCase.get("rpcCode")).longValue();
            assertEquals(Integer.valueOf((int) rpcCode), HostError.CODES.get(code), name);
            HostError named = refused.apply(map("code", rpcCode, "message", "Refused.", "data", map("code", code)));
            assertEquals(code, named.code(), name);
            assertEquals((int) rpcCode, named.rpcCode().intValue(), name);
            assertEquals(Protocol.HOST_FUNCTIONS_INVOKE, named.method(), name);
            // The number alone names the code.
            assertEquals(code, refused.apply(map("code", rpcCode, "message", "Refused.")).code(), name);
            failures++;
        }
        assertTrue(failures > 0, "The fixture has no relay error case.");

        HostError providerFailed = refused.apply(map("code", -32070L, "message", "The provider failed.",
                "data", map("code", "plugin.function.failed", "details", map("pluginErrorCode", "go.parse.failed"))));
        assertEquals("plugin.function.failed", providerFailed.code());
        assertEquals("go.parse.failed", providerFailed.pluginErrorCode());
        assertEquals(map("pluginErrorCode", "go.parse.failed"), providerFailed.details());
        // Only a provider's failure has a provider error code, and only a text one.
        assertNull(refused.apply(map("code", -32070L, "message", "The provider failed.",
                "data", map("code", "plugin.function.failed"))).pluginErrorCode());
        assertNull(refused.apply(map("code", -32070L, "message", "The provider failed.",
                "data", map("code", "plugin.function.failed", "details", map("pluginErrorCode", 7L)))).pluginErrorCode());
        assertNull(refused.apply(map("code", -32071L, "message", "No result.",
                "data", map("code", "plugin.function.invalidResult", "details", map("pluginErrorCode", "go.parse.failed")))).pluginErrorCode());
        host.close();
    }

    @Test
    void aCallToAnotherPluginNamesTheFunctionCallItIsMadeInAsItsParentUnlessTheCallerSaysOtherwise() {
        Backend backend = Backend.create(null, null).function("relay.run", (arguments, context) -> {
            Host used = Boolean.TRUE.equals(arguments.get("scoped"))
                    ? context.host().withCancellation(new Cancellation())
                    : context.host();
            return used.functions().invoke(CONTEXT, object(arguments.get("call"))).join();
        });
        TestHost host = session(backend);
        Set<Object> taken = new HashSet<>();
        Map<String, Object> call = map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings");
        String longest = repeat('p', Protocol.MAX_PARENT_OPERATION_ID_BYTES);
        String tooLong = repeat('p', Protocol.MAX_PARENT_OPERATION_ID_BYTES + 1);
        // The invoke request's operationID (null: none), the call, whether the Host is
        // narrowed with withCancellation, and the parentOperationID the host must receive (null: none).
        Object[][] cases = {
            {"7", call, false, "7"},
            {"7", with(call, "parentOperationID", "17"), false, "17"},
            {"7", with(call, "parentOperationID", null), false, "7"},
            {"7", call, true, "7"},
            {longest, call, false, longest},
            {tooLong, call, false, null},
            {"", call, false, null},
            {7L, call, false, null},
            {null, call, false, null},
        };
        long id = 60;
        for (Object[] testCase : cases) {
            String name = "operationID " + testCase[0] + ", call " + testCase[1] + ", narrowed " + testCase[2];
            Map<String, Object> invocation = map("functionID", "relay.run",
                    "params", map("call", testCase[1], "scoped", testCase[2]));
            if (testCase[0] != null) {
                invocation.put("operationID", testCase[0]);
            }
            id++;
            CompletableFuture<Map<String, Object>> serving = host.requestAsync(Protocol.FUNCTION_INVOKE, invocation, id);
            Map<String, Object> request = nextRequest(host, Protocol.HOST_FUNCTIONS_INVOKE, taken);
            Map<String, Object> expected = with(call, "context", CONTEXT);
            if (testCase[3] != null) {
                expected.put("parentOperationID", testCase[3]);
            }
            // The JSON-RPC request ID and the diagnostic operation ID are never the parent.
            assertEquals(expected, request.get("params"), name);
            host.send(map("id", request.get("id"), "result", map("value", map("ok", true))));
            Map<String, Object> response = Fixtures.await(serving, "the serving call");
            assertNull(response.get("error"), name);
            assertEquals(map("value", map("ok", true)), response.get("result"), name);
        }
        // A Host that belongs to no function call has no parent to name; an explicit one is still sent.
        assertFalse(relayed(backend, host, taken, call).containsKey("parentOperationID"));
        assertEquals("17", relayed(backend, host, taken, with(call, "parentOperationID", "17")).get("parentOperationID"));
        host.close();
    }

    @Test
    void malformedRelayArgumentsFailLocallyAndNeverReachTheHost() {
        Backend backend = Backend.create(null, null);
        TestHost host = session(backend);
        Host.Functions functions = backend.host().functions();
        int before = host.messages().size();
        Map<String, Object> call = map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings");

        List<Map<String, Object>> malformed = new ArrayList<>();
        for (Object pluginId : new Object[] {null, "", 7L}) {
            malformed.add(with(call, "pluginID", pluginId));
        }
        for (Object functionId : new Object[] {null, "", "Not-Valid", 7L}) {
            malformed.add(with(call, "functionID", functionId));
        }
        for (Object params : new Object[] {Arrays.asList(1L), "text"}) {
            malformed.add(with(call, "params", params));
        }
        for (Object timeout : new Object[] {0L, -1L, 1.5, "5000", 9007199254740992L, Double.NaN}) {
            malformed.add(with(call, "timeoutMilliseconds", timeout));
        }
        // 128 bytes is the longest parent ID: 65 two-byte characters are 130.
        for (Object parent : new Object[] {17L, "", repeat('p', 129), repeat('é', 65)}) {
            malformed.add(with(call, "parentOperationID", parent));
        }
        for (Map<String, Object> bad : malformed) {
            assertEquals("sdk.functions.invalidCall", code(functions.invoke(CONTEXT, bad)), String.valueOf(bad));
        }
        assertEquals("sdk.functions.invalidCall", code(functions.invoke(CONTEXT, null)));
        for (String functionId : new String[] {null, "", "Not-Valid"}) {
            assertEquals("sdk.functions.invalidCall", code(functions.providers(functionId)), String.valueOf(functionId));
        }
        assertEquals("sdk.host.invalidContext", code(functions.invoke(null, call)));
        assertEquals("sdk.host.invalidContext", code(functions.invoke(map("contextID", "w1"), call)));
        assertEquals(before, host.messages().size());

        // What is well formed is sent as given: no cap on the timeout, which is Core's to apply.
        Set<Object> taken = new HashSet<>();
        Map<String, Object> bare = relayed(backend, host, taken, with(call, "params", null, "timeoutMilliseconds", null, "parentOperationID", null));
        assertEquals(map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings", "context", CONTEXT), bare);
        Map<String, Object> full = relayed(backend, host, taken, with(call, "params", map(), "timeoutMilliseconds", 700000L,
                "parentOperationID", repeat('é', 64)));
        assertEquals(map(), full.get("params"));
        assertEquals(700000L, full.get("timeoutMilliseconds"));
        assertEquals(repeat('é', 64), full.get("parentOperationID"));
        assertEquals(5000L, relayed(backend, host, taken, with(call, "timeoutMilliseconds", 5000.0)).get("timeoutMilliseconds"));
        assertEquals(5000L, relayed(backend, host, taken, with(call, "timeoutMilliseconds", 5000)).get("timeoutMilliseconds"));
        assertEquals(repeat('p', 128), relayed(backend, host, taken, with(call, "parentOperationID", repeat('p', 128))).get("parentOperationID"));
        // Only the fields of the method are sent; a caller cannot be chosen from here.
        Map<String, Object> only = relayed(backend, host, taken, with(call, "caller", map("pluginID", "dev.lithe.fixture.other"), "extra", true));
        assertEquals(map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings", "context", CONTEXT), only);
        host.close();
    }

    @Test
    void cancellingTheCallingFunctionCancelsItsRelayedCallOnTheWire() {
        CountDownLatch started = new CountDownLatch(1);
        AtomicReference<Throwable> outcome = new AtomicReference<>();
        Backend backend = Backend.create(null, null).function("relay.run", (arguments, context) -> {
            CompletableFuture<Map<String, Object>> relayed = context.host().functions().invoke(CONTEXT, object(arguments.get("call")));
            started.countDown();
            try {
                return relayed.join();
            } catch (CompletionException error) {
                outcome.set(error.getCause());
                throw error;
            }
        });
        TestHost host = session(backend);
        CompletableFuture<Map<String, Object>> call = host.requestAsync(Protocol.FUNCTION_INVOKE,
                map("functionID", "relay.run", "operationID", "c",
                        "params", map("call", map("pluginID", "dev.lithe.fixture.go", "functionID", "languageImport.settings"))), 51L);
        Fixtures.await(started, "the relayed call");
        Map<String, Object> request = nextRequest(host, Protocol.HOST_FUNCTIONS_INVOKE);
        assertEquals("c", object(request.get("params")).get("parentOperationID"));
        host.notify(Protocol.CANCEL_REQUEST, map("id", 51L));
        Map<String, Object> response = Fixtures.await(call, "the cancelled call");
        assertEquals((long) Protocol.REQUEST_CANCELLED, object(response.get("error")).get("code"));
        assertTrue(outcome.get() instanceof CancelledException, String.valueOf(outcome.get()));
        assertEquals(Arrays.asList(map("id", request.get("id"))),
                sent(host, Protocol.CANCEL_REQUEST).stream().map(message -> message.get("params")).collect(Collectors.toList()));
        // Core's late answer is dropped; the session continues.
        host.send(map("id", request.get("id"), "error",
                map("code", -32800L, "message", "Cancelled.", "data", map("code", "plugin.host.cancelled"))));
        assertEquals(1, host.close());
    }
}
