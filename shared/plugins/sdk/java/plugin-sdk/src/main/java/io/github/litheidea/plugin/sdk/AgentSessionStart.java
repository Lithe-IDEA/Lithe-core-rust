package io.github.litheidea.plugin.sdk;

/** What {@code lithe/host/agents/didStart} says about a session Core started. */
public final class AgentSessionStart {
    private final String session;
    private final String agentId;
    private final String cwd;
    private final String resume;

    AgentSessionStart(String session, String agentId, String cwd, String resume) {
        this.session = session;
        this.agentId = agentId;
        this.cwd = cwd;
        this.resume = resume;
    }

    /** Core's ID of the session, such as {@code agent-1}. */
    public String session() {
        return session;
    }

    /** The {@code agents[].id} from the manifest. */
    public String agentId() {
        return agentId;
    }

    /** The working directory the CLI runs in. */
    public String cwd() {
        return cwd;
    }

    /** The CLI's own session ID to resume when the user resumes one, or null. */
    public String resume() {
        return resume;
    }
}
