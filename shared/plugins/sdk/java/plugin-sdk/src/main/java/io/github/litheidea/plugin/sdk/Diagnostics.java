package io.github.litheidea.plugin.sdk;

import java.util.Arrays;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.Set;

/** Typed facts for the host's litheLogging 1 catalog; never accepts message or payload text. */
public final class Diagnostics {
    static final long MAX_INTEGER = 9007199254740991L;
    static final Set<String> OPERATIONS = new HashSet<>(Arrays.asList(
            "initialize", "shutdown", "notification", "function-call", "result-encoding",
            "lithe/build/model", "lithe/build/dependencies", "lithe/build/execute", "lithe/build/test",
            "lithe/build/discoverTests", "lithe/build/prepareRun", "lithe/build/prepareDebug",
            "lithe/vcs/status", "lithe/vcs/baseContent", "lithe/vcs/history", "lithe/vcs/fileHistory",
            "lithe/vcs/annotate", "lithe/vcs/checkin", "lithe/vcs/rollback", "lithe/vcs/update",
            "lithe/vcs/fileOperation", "lithe/vcs/resolve", "lithe/vcs/cleanup",
            "lithe/keymap/import"));
    private final Backend backend;
    private final String operationId;

    Diagnostics(Backend backend, String operationId) {
        this.backend = backend;
        this.operationId = operationId;
    }

    public void degraded(String operation, String code, Long count) { record("degraded", operation, code, count, null); }
    public void recovered(String operation, String code, Long count) { record("recovered", operation, code, count, null); }
    public void degraded(String operation, String code) { degraded(operation, code, null); }
    public void recovered(String operation, String code) { recovered(operation, code, null); }

    static boolean token(String value, int maximum) {
        return value != null && value.length() > 0 && value.length() <= maximum && value.matches("[A-Za-z0-9._:/\\-$]+");
    }

    static String requestOperationId(String operationId, Object requestId) {
        if (token(operationId, 128)) return operationId;
        String fallback = requestId instanceof String ? (String) requestId : null;
        if (requestId instanceof Number) {
            double number = ((Number) requestId).doubleValue();
            if (Double.isFinite(number) && Math.abs(number) <= MAX_INTEGER && number == Math.rint(number)) {
                fallback = Long.toString(((Number) requestId).longValue());
            }
        }
        return token(fallback, 128) ? fallback : null;
    }

    void terminal(String operation, Throwable failure, long started) {
        String code = null;
        if (failure != null) {
            Throwable error = Backend.unwrap(failure);
            code = error instanceof PluginError ? ((PluginError) error).code() : "internal_error";
        }
        record(failure == null ? "operationFinished" : "operationFailed", operation, code, null,
                Math.min(MAX_INTEGER, Math.max(0, (System.nanoTime() - started) / 1000000)));
    }

    private void record(String kind, String operation, String code, Long count, Long duration) {
        if (!backend.loggingOperation(operation)) throw new PluginSdkError("sdk.logging.invalidOperation", "Diagnostics need a registered operation.");
        if (code != null && !token(code, 64)) throw new PluginSdkError("sdk.logging.invalidCode", "Diagnostics need a controlled code.");
        if (count != null && (count < 0 || count > MAX_INTEGER)) throw new PluginSdkError("sdk.logging.invalidCount", "Diagnostics need a nonnegative safe count.");
        Map<String, Object> params = new LinkedHashMap<>();
        params.put("kind", kind);
        params.put("operation", operation.length() <= 64 ? operation : "function-call");
        if (code != null) params.put("code", code);
        if (count != null) params.put("count", count);
        if (duration != null) params.put("durationMs", duration);
        if (token(operationId, 128)) params.put("operationId", operationId);
        backend.logFact(params);
    }
}
