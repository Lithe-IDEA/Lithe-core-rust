package io.github.litheidea.plugin.sdk;

import java.util.List;

/**
 * The plugin's translation of one native agent session (plan API-45). Core
 * runs the agent's CLI; the SDK calls these methods on the session's own
 * worker, one at a time and in the order Core sent the notifications, so a
 * method may wait for host requests.
 *
 * <p>A failing {@link #prompt} ends its turn: the SDK reports
 * {@code turnEnded} with reason {@code failed}. Other failures are only logged.
 */
public interface AgentSession {
    /** Lines the CLI wrote to standard output, without line breaks. */
    void output(List<String> lines) throws Exception;

    /** The user's prompt for {@code turn}; write what the CLI expects. */
    void prompt(long turn, String text) throws Exception;

    /** The user cancelled {@code turn}. Does nothing unless overridden. */
    default void cancel(long turn) throws Exception {}

    /** The user answered permission request {@code permission} with {@code option}. */
    void decidePermission(long turn, String permission, String option) throws Exception;

    /**
     * The CLI ended, with its exit code when it had one; the session gets no
     * further calls. Does nothing unless overridden.
     */
    default void exited(Long exitCode) {}
}
