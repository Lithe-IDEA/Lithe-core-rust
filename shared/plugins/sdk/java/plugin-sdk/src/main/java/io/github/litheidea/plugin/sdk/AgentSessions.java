package io.github.litheidea.plugin.sdk;

import java.util.ArrayList;
import java.util.Arrays;
import java.util.Collections;
import java.util.HashMap;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.Executor;
import java.util.concurrent.LinkedBlockingQueue;

/**
 * Native agent sessions (plan API-45). Core starts an agent's CLI and relays
 * it through the plugin backend: the CLI's standard output arrives as
 * {@code lithe/host/agents/didOutput}, the user's prompts, cancellations and
 * permission decisions as {@code didPrompt}, {@code didCancel} and
 * {@code didDecidePermission}, and the backend answers with
 * {@code lithe/host/agents/write} (the CLI's standard input) and
 * {@code lithe/host/agents/report} (session events). A translation is only
 * right when it sees the CLI's lines in order, so every session gets one
 * worker that runs its handler's methods one at a time, in the order Core
 * sent the notifications.
 */
final class AgentSessions {
    static final Set<String> NOTIFICATIONS = Collections.unmodifiableSet(new HashSet<>(Arrays.asList(
            Protocol.HOST_AGENTS_DID_START, Protocol.HOST_AGENTS_DID_OUTPUT, Protocol.HOST_AGENTS_DID_PROMPT,
            Protocol.HOST_AGENTS_DID_CANCEL, Protocol.HOST_AGENTS_DID_DECIDE_PERMISSION,
            Protocol.HOST_AGENTS_DID_EXIT)));
    private static final String OPERATION = "notification";

    private final Backend backend;
    private final Map<String, Worker> sessions = new HashMap<>();
    private Backend.AgentSessionStarter starter;

    AgentSessions(Backend backend) {
        this.backend = backend;
    }

    synchronized void starter(Backend.AgentSessionStarter starter) {
        this.starter = starter;
    }

    /**
     * Routes one notification. It runs on the reader thread, so it only
     * queues work and never waits for a handler.
     */
    void route(String method, Map<String, Object> params, Executor executor) {
        String session = string(params, "session");
        boolean start = method.equals(Protocol.HOST_AGENTS_DID_START);
        Backend.AgentSessionStarter registered;
        Worker worker;
        synchronized (this) {
            registered = starter;
            if (start) {
                worker = registered == null ? null : new Worker();
                if (worker != null) {
                    sessions.put(session, worker);
                }
            } else if (method.equals(Protocol.HOST_AGENTS_DID_EXIT)) {
                worker = sessions.remove(session);
            } else {
                worker = sessions.get(session);
            }
        }
        if (start) {
            if (worker == null) {
                backend.diagnostics().failed(OPERATION, "agent_unhandled");
                return;
            }
            Object resume = params.get("resume");
            AgentSessionStart started = new AgentSessionStart(session, string(params, "agentID"),
                    string(params, "cwd"), resume instanceof String ? (String) resume : null);
            Worker queue = worker;
            executor.execute(() -> run(registered, started, queue));
            return;
        }
        if (worker == null) {
            if (registered != null) {
                backend.diagnostics().failed(OPERATION, "agent_session_unknown");
            }
            return;
        }
        worker.push(notice(method, params));
        if (method.equals(Protocol.HOST_AGENTS_DID_EXIT)) {
            worker.close();
        }
    }

    /** Ends every open session; its worker stops after the notices already queued. */
    void closeAll() {
        List<Worker> open;
        synchronized (this) {
            open = new ArrayList<>(sessions.values());
            sessions.clear();
        }
        for (Worker worker : open) {
            worker.close();
        }
    }

    /** Starts the session's handler, then runs its notifications in order. */
    private void run(Backend.AgentSessionStarter starter, AgentSessionStart start, Worker worker) {
        AgentSessionHandle handle = new AgentSessionHandle(start.session(), backend.host());
        AgentSession handler = null;
        try {
            handler = starter.start(start, handle);
            if (handler == null) {
                backend.diagnostics().failed(OPERATION, "internal_error");
            }
        } catch (Throwable error) {
            failed(error);
        }
        for (;;) {
            Notice notice;
            try {
                notice = worker.take();
            } catch (InterruptedException error) {
                Thread.currentThread().interrupt();
                return;
            }
            if (notice == Notice.CLOSED) {
                return;
            }
            boolean handled = false;
            if (handler != null) {
                try {
                    deliver(handler, notice);
                    handled = true;
                } catch (Throwable error) {
                    failed(error);
                }
            }
            if (!handled && notice.method.equals(Protocol.HOST_AGENTS_DID_PROMPT)) {
                endTurn(handle, notice.turn);
            }
        }
    }

    private static void deliver(AgentSession handler, Notice notice) throws Exception {
        switch (notice.method) {
            case Protocol.HOST_AGENTS_DID_OUTPUT:
                handler.output(notice.lines);
                return;
            case Protocol.HOST_AGENTS_DID_PROMPT:
                handler.prompt(notice.turn, notice.text);
                return;
            case Protocol.HOST_AGENTS_DID_CANCEL:
                handler.cancel(notice.turn);
                return;
            case Protocol.HOST_AGENTS_DID_DECIDE_PERMISSION:
                handler.decidePermission(notice.turn, notice.permission, notice.option);
                return;
            default:
                handler.exited(notice.exitCode);
        }
    }

    /**
     * No handler ran the turn, so it ends here. The handler may have ended it
     * already; Core then refuses the second end, which changes nothing.
     */
    private static void endTurn(AgentSessionHandle handle, long turn) {
        Map<String, Object> event = new LinkedHashMap<>();
        event.put("kind", "turnEnded");
        event.put("turn", turn);
        event.put("reason", "failed");
        try {
            handle.report(Collections.singletonList(event)).join();
        } catch (RuntimeException refused) {
            // Core refused the end or the session closed; the turn is over either way.
        }
    }

    private void failed(Throwable error) {
        if (!(Backend.unwrap(error) instanceof CancelledException)) {
            backend.diagnostics().terminal(OPERATION, error, System.nanoTime());
        }
    }

    private static Notice notice(String method, Map<String, Object> params) {
        List<String> lines = new ArrayList<>();
        Object raw = params.get("lines");
        if (raw instanceof List) {
            for (Object line : (List<?>) raw) {
                lines.add(line instanceof String ? (String) line : "");
            }
        }
        Object turn = params.get("turn");
        Object exit = params.get("exitCode");
        return new Notice(method, Collections.unmodifiableList(lines),
                turn instanceof Long && (Long) turn >= 0 ? (Long) turn : 0L,
                string(params, "text"), string(params, "permission"), string(params, "option"),
                exit instanceof Long ? (Long) exit : null);
    }

    private static String string(Map<String, Object> params, String key) {
        Object value = params.get(key);
        return value instanceof String ? (String) value : "";
    }

    /** One notification for a session's worker. */
    private static final class Notice {
        static final Notice CLOSED = new Notice("", Collections.<String>emptyList(), 0, "", "", "", null);

        final String method;
        final List<String> lines;
        final long turn;
        final String text;
        final String permission;
        final String option;
        final Long exitCode;

        Notice(String method, List<String> lines, long turn, String text, String permission, String option,
                Long exitCode) {
            this.method = method;
            this.lines = lines;
            this.turn = turn;
            this.text = text;
            this.permission = permission;
            this.option = option;
            this.exitCode = exitCode;
        }
    }

    /** An unbounded queue of one session's notifications, so the reader never waits for a handler. */
    private static final class Worker {
        private final LinkedBlockingQueue<Notice> queue = new LinkedBlockingQueue<>();

        void push(Notice notice) {
            queue.add(notice);
        }

        void close() {
            queue.add(Notice.CLOSED);
        }

        Notice take() throws InterruptedException {
            return queue.take();
        }
    }
}
