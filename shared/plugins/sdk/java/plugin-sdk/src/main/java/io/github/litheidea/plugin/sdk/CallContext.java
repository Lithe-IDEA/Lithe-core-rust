package io.github.litheidea.plugin.sdk;

import java.io.IOException;
import java.util.List;
import java.util.Map;

/**
 * What a handler gets besides its params: the call's cancellation, tools and
 * host requests that end with the call, the backend's log, and progress for
 * plugin functions or bounded build operations.
 */
public final class CallContext {
    /** Sends one progress report; absent for model, dependency and discovery requests. */
    interface ProgressSink {
        void report(String message, Double percentage);
        default void report(String message, Double percentage, String output) { report(message, percentage); }
    }

    private final java.util.concurrent.atomic.AtomicBoolean quietSuccess = new java.util.concurrent.atomic.AtomicBoolean();
    private final Cancellation cancellation;
    private final Backend backend;
    private final String functionId;
    private final String operationId;
    private final ProgressSink progress;
    private final Host host;
    private final Map<String, Object> workspaceContext;
    private final Map<String, Object> caller;

    CallContext(Cancellation cancellation, Backend backend, String functionId, String operationId, ProgressSink progress,
            Host host, Map<String, Object> workspaceContext, Map<String, Object> caller) {
        this.cancellation = cancellation;
        this.backend = backend;
        this.functionId = functionId;
        this.operationId = operationId;
        this.progress = progress;
        this.host = host;
        this.workspaceContext = workspaceContext;
        this.caller = caller;
    }

    /**
     * Cancelled when the host cancels the call, the backend shuts down, or the
     * call ends; work a handler leaves running after it returned is stopped.
     */
    public Cancellation cancellation() {
        return cancellation;
    }

    public boolean isCancelled() {
        return cancellation.isCancelled();
    }

    /** Throws {@link CancelledException} once the call was cancelled. */
    public void throwIfCancelled() {
        cancellation.throwIfCancelled();
    }

    /**
     * Runs a tool that belongs to this call: cancelling the call or ending it
     * stops the tool, and the backend shutting down ends it.
     */
    public Processes.Result run(List<String> command, Processes.Options options) throws IOException {
        Processes.Options settings = options == null ? new Processes.Options() : options.copy();
        settings.listenerFailure(() -> diagnostics().degraded("notification", "output_listener_failed"));
        Cancellation own = settings.cancellation();
        settings.cancellation(own == null ? cancellation : cancellation.combinedWith(own));
        return Processes.run(command, settings);
    }

    /** Runs a tool that belongs to this call with default options. */
    public Processes.Result run(List<String> command) throws IOException {
        return run(command, null);
    }

    public void suppressSuccessDiagnostics() { quietSuccess.set(true); }
    boolean quietSuccess() { return quietSuccess.get(); }

    public Diagnostics diagnostics() { return new Diagnostics(backend, operationId); }

    /**
     * Reports progress of a function or bounded build call. Function messages
     * are cut to 1024 bytes, build messages to 4096; percentages are rounded to
     * 0-100. Reports after cancellation or completion are dropped.
     */
    public void progress(String message, Double percentage) { progress(message, percentage, null); }

    /** Build calls may additionally stream up to 64 KiB of output per report. */
    public void progress(String message, Double percentage, String output) {
        if (progress != null && !cancellation.isCancelled()) progress.report(message, percentage, output);
    }

    /** The function this call runs, or null for a build or LSP request. */
    public String functionId() {
        return functionId;
    }

    /**
     * The wire operation ID of a function or bounded build call, otherwise null.
     * It identifies the call in diagnostics and, when the request has no usable
     * operation ID, falls back to the JSON-RPC request ID: do not use it as a
     * {@code parentOperationID}. {@link #host()} already sends the right one.
     */
    public String operationId() {
        return operationId;
    }

    /**
     * Host services that belong to this call: requests still unanswered when
     * the call is cancelled or ends fail with {@link CancelledException} and
     * are cancelled on the wire. A subscription created here outlives the call
     * until it is disposed.
     */
    public Host host() {
        return host;
    }

    /**
     * The {@code { contextID, generation }} of the workspace a function was
     * invoked for, or null: for other calls and for an invocation outside a
     * workspace. Pass it to the
     * {@link Host} methods that name a context.
     */
    public Map<String, Object> workspaceContext() {
        return workspaceContext;
    }

    /**
     * The plugin that called this function through the relay: an
     * unmodifiable {@code { pluginID }}, written by Core from the calling
     * session. Null when the host started the call, and for calls that are not
     * function invocations.
     * Decide by who asks from this, never from a plugin ID in the params,
     * which any caller can write.
     */
    public Map<String, Object> caller() {
        return caller;
    }

    /** The {@code pluginID} of {@link #caller()}, or null when there is no calling plugin. */
    public String callerPluginId() {
        return caller == null ? null : (String) caller.get("pluginID");
    }
}
