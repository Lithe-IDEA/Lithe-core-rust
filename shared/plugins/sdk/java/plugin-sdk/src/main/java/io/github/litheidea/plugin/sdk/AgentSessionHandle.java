package io.github.litheidea.plugin.sdk;

import java.util.List;
import java.util.concurrent.CompletableFuture;

/** The host methods of one native agent session. */
public final class AgentSessionHandle {
    private final String id;
    private final Host host;

    AgentSessionHandle(String id, Host host) {
        this.id = id;
        this.host = host;
    }

    /** Core's ID of the session. */
    public String id() {
        return id;
    }

    /** The host services outside the session, such as other plugins' functions. */
    public Host host() {
        return host;
    }

    /**
     * Sends {@code lithe/host/agents/write}: 1-256 lines for the CLI's
     * standard input, without line breaks; Core adds the newline.
     */
    public CompletableFuture<Void> write(List<String> lines) {
        return host.agents().write(id, lines);
    }

    /** Sends {@code lithe/host/agents/report}: 1-256 session events, applied together. */
    public CompletableFuture<Void> report(List<?> events) {
        return host.agents().report(id, events);
    }
}
