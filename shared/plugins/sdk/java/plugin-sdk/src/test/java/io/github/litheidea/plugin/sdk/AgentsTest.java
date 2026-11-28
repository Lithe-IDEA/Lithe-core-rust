package io.github.litheidea.plugin.sdk;

import static io.github.litheidea.plugin.sdk.Fixtures.map;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.CompletableFuture;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;

/**
 * Native agent sessions and the agents host service (plan API-45), driven by
 * {@code contract/v1/fixtures/agents/native-session.json}; mirrors
 * {@code sdk/node/test/agents.test.mjs}.
 */
@Timeout(value = 30, unit = TimeUnit.SECONDS)
class AgentsTest {
    private static final Map<String, Object> FIXTURE = loadFixture();

    private static Map<String, Object> loadFixture() {
        try {
            return object(Json.parse(new String(Files.readAllBytes(
                    Paths.get("../../../contract/v1/fixtures/agents/native-session.json")), StandardCharsets.UTF_8)));
        } catch (IOException error) {
            throw new UncheckedIOException(error);
        }
    }

    @SuppressWarnings("unchecked")
    private static Map<String, Object> object(Object value) {
        return (Map<String, Object>) value;
    }

    @SuppressWarnings("unchecked")
    private static List<Object> list(Object value) {
        return (List<Object>) value;
    }

    /**
     * A backend whose handlers record every call as {@code [session, call,
     * args...]}, as the fixture lists them. It fails to start sessions in
     * /work/broken and fails prompts that say fail. agent-1's first output
     * waits until agent-2 exited, so sessions sharing one worker never get
     * past it.
     */
    private static final class Recorder {
        private final List<List<Object>> calls = new ArrayList<>();
        private final CountDownLatch otherExited = new CountDownLatch(1);
        final Backend backend = Backend.create("agents-test", null);

        Recorder() {
            backend.onAgentSessions((start, session) -> {
                if (!session.id().equals(start.session())) {
                    throw new PluginError("test.wrongHandle", "The handle belongs to another session.");
                }
                if ("/work/broken".equals(start.cwd())) {
                    throw new PluginError("test.startFailed", "The start failed.");
                }
                String id = start.session();
                record(id, "start", map("session", id, "agentID", start.agentId(), "cwd", start.cwd(),
                        "resume", start.resume()));
                return new AgentSession() {
                    @Override
                    public void output(List<String> lines) {
                        if (id.equals("agent-1") && lines.contains("one")) {
                            Fixtures.await(otherExited, "agent-2 to exit while agent-1 waits");
                        }
                        record(id, "output", lines);
                    }

                    @Override
                    public void prompt(long turn, String text) {
                        record(id, "prompt", turn, text);
                        if (text.equals("fail")) {
                            throw new PluginError("test.promptFailed", "The prompt failed.");
                        }
                    }

                    @Override
                    public void cancel(long turn) {
                        record(id, "cancel", turn);
                    }

                    @Override
                    public void decidePermission(long turn, String permission, String option) {
                        record(id, "decidePermission", turn, permission, option);
                    }

                    @Override
                    public void setConfigOption(String option, String value) {
                        record(id, "setConfigOption", option, value);
                    }

                    @Override
                    public void exited(Long exitCode) {
                        record(id, "exited", exitCode);
                        if (id.equals("agent-2")) {
                            otherExited.countDown();
                        }
                    }
                };
            });
        }

        private void record(Object... call) {
            synchronized (calls) {
                calls.add(Arrays.asList(call));
                calls.notifyAll();
            }
        }

        /** The calls of every session, once {@code wanted} calls arrived. */
        Map<String, Object> bySession(int wanted) {
            long deadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(TestHost.WAIT_MILLIS);
            synchronized (calls) {
                while (calls.size() < wanted) {
                    long remaining = TimeUnit.NANOSECONDS.toMillis(deadline - System.nanoTime());
                    if (remaining <= 0) {
                        throw new AssertionError("Timed out waiting for " + wanted + " session calls; got " + calls);
                    }
                    try {
                        calls.wait(remaining);
                    } catch (InterruptedException error) {
                        Thread.currentThread().interrupt();
                        throw new AssertionError("Interrupted waiting for session calls.", error);
                    }
                }
                Map<String, Object> sessions = new LinkedHashMap<>();
                for (List<Object> call : calls) {
                    list(sessions.computeIfAbsent((String) call.get(0), session -> new ArrayList<>()))
                            .add(call.subList(1, call.size()));
                }
                return sessions;
            }
        }
    }

    private static int count(Object expected) {
        int count = 0;
        for (Object calls : object(expected).values()) {
            count += list(calls).size();
        }
        return count;
    }

    private static void sendAll(TestHost host, Object notifications) {
        for (Object notification : list(notifications)) {
            Map<String, Object> entry = object(notification);
            host.notify((String) entry.get("method"), entry.get("params"));
        }
    }

    private static List<Object> failureCodes(TestHost host) {
        List<Object> codes = new ArrayList<>();
        for (Map<String, Object> message : host.messages()) {
            if ("lithe/log".equals(message.get("method"))
                    && "operationFailed".equals(object(message.get("params")).get("kind"))) {
                codes.add(object(message.get("params")).get("code"));
            }
        }
        return codes;
    }

    private static TestHost session(Backend backend) {
        TestHost host = new TestHost(backend);
        host.initialize();
        return host;
    }

    @Test
    void eachAgentSessionGetsItsNotificationsInOrderFromStartToExit() {
        Recorder recorder = new Recorder();
        TestHost host = session(recorder.backend);
        Map<String, Object> sessions = object(FIXTURE.get("sessions"));
        sendAll(host, sessions.get("notifications"));
        // Every session's last call is its exit, so nothing follows the count.
        assertEquals(sessions.get("calls"), recorder.bySession(count(sessions.get("calls"))));
        assertEquals(1, host.close());
        // agent-9 never started, so its output reaches no handler.
        assertEquals(Collections.singletonList("agent_session_unknown"), failureCodes(host));
    }

    @Test
    void aFailedAgentStartOrPromptEndsTheTurnAsFailed() {
        Recorder recorder = new Recorder();
        TestHost host = session(recorder.backend);
        Map<String, Object> failures = object(FIXTURE.get("failures"));
        sendAll(host, failures.get("notifications"));
        // Each worker waits for Core's answer, so answer the reports as they come.
        Set<Object> answered = new HashSet<>();
        Set<Object> reports = new HashSet<>();
        for (int index = 0; index < list(failures.get("reports")).size(); index++) {
            Map<String, Object> report = host.waitFor(message -> "lithe/host/agents/report".equals(message.get("method"))
                    && !answered.contains(message.get("id")), "a turnEnded report");
            answered.add(report.get("id"));
            reports.add(report.get("params"));
            host.send(map("id", report.get("id"), "result", map()));
        }
        assertEquals(new HashSet<>(list(failures.get("reports"))), reports);
        assertEquals(failures.get("calls"), recorder.bySession(count(failures.get("calls"))));
        assertEquals(1, host.close());
    }

    @Test
    void agentHostMethodsSendTheFixtureRequests() {
        Backend backend = Backend.create("agents-test", null);
        TestHost host = session(backend);
        Host.Agents agents = backend.host().agents();
        for (Object raw : list(FIXTURE.get("methods"))) {
            Map<String, Object> entry = object(raw);
            String call = (String) entry.get("call");
            String session = (String) entry.get("session");
            CompletableFuture<?> pending;
            if (call.equals("write")) {
                List<String> lines = new ArrayList<>();
                for (Object line : list(entry.get("lines"))) {
                    lines.add((String) line);
                }
                pending = agents.write(session, lines);
            } else if (call.equals("report")) {
                pending = agents.report(session, list(entry.get("events")));
            } else {
                pending = agents.usage(object(entry.get("params")));
            }
            Map<String, Object> request = object(entry.get("request"));
            Map<String, Object> sent = host.waitFor(message -> request.get("method").equals(message.get("method")),
                    (String) request.get("method"));
            assertEquals(request.get("params"), sent.get("params"));
            Map<String, Object> answer = call.equals("usage")
                    ? map("sinceMs", 1L, "untilMs", 2L, "summaries", new ArrayList<>(), "rateLimits", new ArrayList<>())
                    : map();
            host.send(map("id", sent.get("id"), "result", answer));
            Object result = Fixtures.await(pending, call);
            if (call.equals("usage")) {
                assertEquals(answer, result);
            } else {
                assertNull(result);
            }
        }
        assertEquals(1, host.close());
    }

    @Test
    void agentNotificationsAreRoutedOnlyThroughAgentSessions() {
        Backend backend = Backend.create("agents-test", null);
        for (String method : Arrays.asList("lithe/host/agents/didStart", "lithe/host/agents/didOutput",
                "lithe/host/agents/didExit")) {
            assertEquals("sdk.method.reserved", assertThrows(PluginSdkError.class,
                    () -> backend.onHostNotification(method, params -> {})).code());
        }
        // Without a registration a started session is only logged.
        TestHost host = session(backend);
        host.notify("lithe/host/agents/didStart", map("session", "agent-1", "agentID", "example", "cwd", "/work/orders"));
        host.notify("lithe/host/agents/didOutput", map("session", "agent-1", "lines", Collections.singletonList("one")));
        // The reader logs both before it reads the end of the input.
        assertEquals(1, host.close());
        assertEquals(Collections.singletonList("agent_unhandled"), failureCodes(host));
    }
}
