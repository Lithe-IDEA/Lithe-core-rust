package io.github.litheidea.plugin.sdk;

import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CompletableFuture;
import java.util.function.BooleanSupplier;

/**
 * Host services ({@code lithe/host/*}): credentials; workspace contexts,
 * files, documents, the editor, previews, the browser, messages, and
 * capability discovery; calls to other plugins' functions. Every method
 * answers with a future and never blocks; failures the host reports arrive as
 * {@link HostError}.
 *
 * <p>{@link Backend#host()} is usable once the session is ready. A handler's
 * {@link CallContext#host()} belongs to its call: requests still unanswered
 * when the call is cancelled or ends fail with {@link CancelledException} and
 * are cancelled on the wire. At most 16 requests are on the wire at once; the
 * rest wait their turn.
 *
 * <p>Contract: {@code contract/v1/README.md}, "Host services".
 */
public final class Host {
    /** Receives a subscription's events, one at a time and in order. */
    @FunctionalInterface
    public interface Listener {
        void onEvent(Event event) throws Exception;
    }

    /** One {@code lithe/host/subscriptions/notify}. */
    public static final class Event {
        private final String subscriptionId;
        private final long sequence;
        private final boolean gap;
        private final Object event;

        Event(String subscriptionId, long sequence, boolean gap, Object event) {
            this.subscriptionId = subscriptionId;
            this.sequence = sequence;
            this.gap = gap;
            this.event = event;
        }

        public String subscriptionId() {
            return subscriptionId;
        }

        /** Starts at 1 and grows by one per event of the subscription. */
        public long sequence() {
            return sequence;
        }

        /**
         * True when events before this one were lost, so state built from
         * earlier events should be refreshed, for example by rescanning.
         */
        public boolean gap() {
            return gap;
        }

        /** The event, usually an object. */
        public Object event() {
            return event;
        }
    }

    /** A host subscription, such as a file watch or registered editor actions. */
    public static final class Subscription {
        private final String id;
        private final BooleanSupplier live;
        private final HostChannel channel;

        Subscription(String id, BooleanSupplier live, HostChannel channel) {
            this.id = id;
            this.live = live;
            this.channel = channel;
        }

        public String id() {
            return id;
        }

        /** True once disposed here or the session ended. */
        public boolean isDisposed() {
            return !live.getAsBoolean();
        }

        /** Stops routing events at once and asks the host to dispose the subscription. */
        public CompletableFuture<Void> dispose() {
            return channel.dispose(id);
        }
    }

    private final HostChannel channel;
    private final List<Cancellation> scope;
    /** The wire operation ID of the function call this Host belongs to, or null; the default parent of {@link Functions#invoke}. */
    private final String parentOperationId;
    private final Credentials credentials = new Credentials();
    private final Subscriptions subscriptions = new Subscriptions();
    private final Workspace workspace = new Workspace();
    private final Documents documents = new Documents();
    private final Editor editor = new Editor();
    private final Preview preview = new Preview();
    private final Browser browser = new Browser();
    private final Window window = new Window();
    private final Functions functions = new Functions();

    Host(HostChannel channel, List<Cancellation> scope, String parentOperationId) {
        this.channel = channel;
        this.scope = Collections.unmodifiableList(new ArrayList<>(scope));
        this.parentOperationId = parentOperationId;
    }

    /** Whether the host offered plugin functions ({@code litheFunctions}); false before {@code initialize}. */
    public boolean negotiated() {
        return channel.negotiated();
    }

    /**
     * The same services, with requests also cancelled by {@code cancellation},
     * for work a plugin may stop for its own reasons. Calls to other plugins
     * keep the parent call of this Host.
     */
    public Host withCancellation(Cancellation cancellation) {
        if (cancellation == null) {
            throw new IllegalArgumentException("A cancellation must not be null.");
        }
        List<Cancellation> combined = new ArrayList<>(scope);
        combined.add(cancellation);
        return new Host(channel, combined, parentOperationId);
    }

    /**
     * Sends any {@code lithe/host/*} request, including methods a later host
     * adds; {@link #supports} says whether the session may call it.
     */
    public CompletableFuture<Object> request(String method, Object params) {
        return channel.request(method, params, scope);
    }

    /**
     * Sends a request that answers {@code { subscriptionID }} and routes the
     * subscription's events to {@code listener}.
     */
    public CompletableFuture<Subscription> subscribe(String method, Object params, Listener listener) {
        return channel.subscribe(method, params, listener, scope);
    }

    /**
     * {@code { protocolVersion, backendAuthority, services, limits }}, asked
     * once per session.
     */
    public CompletableFuture<Map<String, Object>> capabilities() {
        return channel.capabilities();
    }

    /** Whether this session may call {@code method}, as {@link #capabilities} lists it. */
    public CompletableFuture<Boolean> supports(String method) {
        return channel.supports(method);
    }

    public Credentials credentials() {
        return credentials;
    }

    public Subscriptions subscriptions() {
        return subscriptions;
    }

    public Workspace workspace() {
        return workspace;
    }

    public Documents documents() {
        return documents;
    }

    public Editor editor() {
        return editor;
    }

    public Preview preview() {
        return preview;
    }

    public Browser browser() {
        return browser;
    }

    public Terminal terminal() {
        return new Terminal();
    }

    public Window window() {
        return window;
    }

    /** Calls to functions other plugins export. */
    public Functions functions() {
        return functions;
    }

    /**
     * {@code { contextID, generation }} of a context reference or of a full
     * context from {@code listContexts} or {@code getContext}, so either can be
     * passed on.
     *
     * @throws PluginSdkError {@code sdk.host.invalidContext} for anything else
     */
    public static Map<String, Object> contextReference(Map<String, ?> context) {
        Object id = context == null ? null : context.get("contextID");
        Object generation = context == null ? null : context.get("generation");
        if (!(id instanceof String) || !Protocol.isCredentialKey((String) id) || !Protocol.isGeneration(generation)) {
            throw new PluginSdkError("sdk.host.invalidContext",
                    "A context is { contextID, generation }: an ID of 1-128 ASCII letters, digits, \".\", \"_\", \":\" or \"-\", and a positive generation.");
        }
        Map<String, Object> reference = new LinkedHashMap<>();
        reference.put("contextID", id);
        reference.put("generation", ((Number) generation).longValue());
        return Collections.unmodifiableMap(reference);
    }

    /** Secrets in the operating system's credential store, scoped to the plugin. */
    public final class Credentials {
        private Credentials() {}

        /** The stored secret, or null when nothing is stored under {@code key}. */
        public CompletableFuture<String> get(String key) {
            if (!Protocol.isCredentialKey(key)) {
                return failed(invalidKey());
            }
            return channel.request(Protocol.HOST_CREDENTIALS_GET, params("key", key), scope).thenApply(result -> {
                if (result == null) {
                    return null;
                }
                Object secret = result instanceof Map ? ((Map<?, ?>) result).get("secret") : null;
                if (!(secret instanceof String)) {
                    throw new PluginSdkError("sdk.host.invalidResult", "The host answered credentials/get without a secret.");
                }
                return (String) secret;
            });
        }

        public CompletableFuture<Void> set(String key, String secret) {
            if (!Protocol.isCredentialKey(key)) {
                return failed(invalidKey());
            }
            if (secret == null || secret.getBytes(StandardCharsets.UTF_8).length > Protocol.MAX_CREDENTIAL_SECRET_BYTES) {
                return failed(new PluginSdkError("sdk.credentials.invalidSecret", "A secret is a string of at most 16 KiB."));
            }
            Map<String, Object> params = params("key", key);
            params.put("secret", secret);
            return channel.request(Protocol.HOST_CREDENTIALS_SET, params, scope).thenApply(result -> null);
        }

        /** Deletes the secret; also succeeds when nothing was stored. */
        public CompletableFuture<Void> delete(String key) {
            if (!Protocol.isCredentialKey(key)) {
                return failed(invalidKey());
            }
            return channel.request(Protocol.HOST_CREDENTIALS_DELETE, params("key", key), scope).thenApply(result -> null);
        }
    }

    public final class Subscriptions {
        private Subscriptions() {}

        /** Disposes a subscription by ID; also succeeds for an unknown or disposed one. */
        public CompletableFuture<Void> dispose(String subscriptionId) {
            return channel.dispose(subscriptionId);
        }
    }

    /**
     * Workspace contexts and files. Every method but
     * {@link #listContexts} names a context: a reference from
     * {@link CallContext#workspaceContext()}, or a context from
     * {@link #listContexts}.
     */
    public final class Workspace {
        private Workspace() {}

        /** {@code { revision, contexts }}. */
        public CompletableFuture<Map<String, Object>> listContexts() {
            return object(Protocol.HOST_WORKSPACE_LIST_CONTEXTS, channel.request(
                    Protocol.HOST_WORKSPACE_LIST_CONTEXTS, new LinkedHashMap<String, Object>(), scope));
        }

        public CompletableFuture<Map<String, Object>> getContext(Map<String, ?> context) {
            return scoped(Protocol.HOST_WORKSPACE_GET_CONTEXT, context, null);
        }

        /** {@code { folders }}. */
        public CompletableFuture<Map<String, Object>> getFolders(Map<String, ?> context) {
            return scoped(Protocol.HOST_WORKSPACE_GET_FOLDERS, context, null);
        }

        /** {@code params}: {@code resource, cursor?, limit?}. */
        public CompletableFuture<Map<String, Object>> readDirectory(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WORKSPACE_READ_DIRECTORY, context, params);
        }

        /** {@code params}: {@code resource}. */
        public CompletableFuture<Map<String, Object>> stat(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WORKSPACE_STAT, context, params);
        }

        /** {@code params}: {@code resource, maximumBytes?}. */
        public CompletableFuture<Map<String, Object>> readFile(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WORKSPACE_READ_FILE, context, params);
        }

        /**
         * Watches {@code resource, recursive?}; events are
         * {@code { rootID, changedPaths, requiresRescan }}.
         */
        public CompletableFuture<Subscription> watch(Map<String, ?> context, Map<String, ?> params, Listener listener) {
            Map<String, Object> message;
            try {
                message = withContext(context, params);
            } catch (PluginSdkError error) {
                return failed(error);
            }
            return channel.subscribe(Protocol.HOST_WORKSPACE_WATCH, message, listener, scope);
        }

        /** {@code params}: root {@code resource, query, fileMask?, cursor?, limit?}. */
        public CompletableFuture<Map<String, Object>> findFiles(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WORKSPACE_FIND_FILES, context, params);
        }

        /** {@code params}: root {@code resource, pattern}, and optional search options. */
        public CompletableFuture<Map<String, Object>> findText(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WORKSPACE_FIND_TEXT, context, params);
        }
    }

    /** Open documents, read from their live buffers. */
    public final class Documents {
        private Documents() {}

        /** {@code { documents }}. */
        public CompletableFuture<Map<String, Object>> list(Map<String, ?> context) {
            return scoped(Protocol.HOST_DOCUMENTS_LIST, context, null);
        }

        /** {@code params}: {@code documentID}; answers {@code { document, text }}. */
        public CompletableFuture<Map<String, Object>> getSnapshot(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_DOCUMENTS_GET_SNAPSHOT, context, params);
        }
    }

    /** The text editor. */
    public final class Editor {
        private Editor() {}

        /** The foreground text tab's document summary, or null. */
        public CompletableFuture<Map<String, Object>> getActive(Map<String, ?> context) {
            return scoped(Protocol.HOST_EDITOR_GET_ACTIVE, context, null);
        }

        /** {@code params}: {@code documentID}. */
        public CompletableFuture<Map<String, Object>> getSelections(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_EDITOR_GET_SELECTIONS, context, params);
        }

        /** {@code params}: {@code documentID, expectedVersion, edits}. */
        public CompletableFuture<Map<String, Object>> applyEdits(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_EDITOR_APPLY_EDITS, context, params);
        }

        /** {@code params}: {@code documentID, expectedVersion, selections, replacements}. */
        public CompletableFuture<Map<String, Object>> replaceSelections(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_EDITOR_REPLACE_SELECTIONS, context, params);
        }

        /** {@code params}: {@code documentID, expectedVersion, at, text}. */
        public CompletableFuture<Map<String, Object>> insertText(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_EDITOR_INSERT_TEXT, context, params);
        }

        /** {@code params}: {@code resource, activation?, range?, tab?}. */
        public CompletableFuture<Map<String, Object>> open(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_EDITOR_OPEN, context, params);
        }

        /** {@code params}: {@code resource, activation?, range?, tab?}. */
        public CompletableFuture<Map<String, Object>> reveal(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_EDITOR_REVEAL, context, params);
        }

        /**
         * Registers editor actions ({@code id, function, selector}) that invoke
         * one of the backend's functions; disposing the subscription removes them.
         */
        public CompletableFuture<Subscription> registerActions(Map<String, ?> context, Map<String, ?> params) {
            Map<String, Object> message;
            try {
                message = withContext(context, params);
            } catch (PluginSdkError error) {
                return failed(error);
            }
            return channel.subscribe(Protocol.HOST_EDITOR_REGISTER_ACTIONS, message, null, scope);
        }
    }

    /** HTML previews. */
    public final class Preview {
        private Preview() {}

        /** {@code params}: HTML {@code resource, presentation?, range?}. */
        public CompletableFuture<Map<String, Object>> openHtml(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_PREVIEW_OPEN_HTML, context, params);
        }
    }

    /** The system browser. */
    public final class Browser {
        private Browser() {}

        /** {@code params}: exactly one of an HTTP(S) {@code url} or an HTML {@code resource}. */
        public CompletableFuture<Map<String, Object>> openExternal(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_BROWSER_OPEN_EXTERNAL, context, params);
        }
    }

    /** Native terminals private to the backend session and workspace. */
    public final class Terminal {
        private Terminal() {}
        /** Params: executable, arguments?, title?, cwd?, environment?, show?. */
        public CompletableFuture<Map<String, Object>> create(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_TERMINAL_CREATE, context, params);
        }
        public CompletableFuture<Map<String, Object>> show(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_TERMINAL_SHOW, context, params);
        }
        public CompletableFuture<Map<String, Object>> close(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_TERMINAL_CLOSE, context, params);
        }
        /** Restarts the saved argv, restoring only the chosen plain display suffix. */
        public CompletableFuture<Map<String, Object>> reopen(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_TERMINAL_REOPEN, context, params);
        }
        /** Forgets the handle and its retained display history. */
        public CompletableFuture<Map<String, Object>> dispose(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_TERMINAL_DISPOSE, context, params);
        }
        /** Params: terminalID; disposing the subscription does not close the terminal. */
        public CompletableFuture<Subscription> onExit(Map<String, ?> context, Map<String, ?> params) {
            Map<String, Object> message;
            try { message = withContext(context, params); }
            catch (PluginSdkError error) { return failed(error); }
            return channel.subscribe(Protocol.HOST_TERMINAL_ON_EXIT, message, null, scope);
        }
    }

    /**
     * Messages in the window. {@code params}: {@code text,
     * actions?}; answers {@code { selectedIndex }}, null on dismissal.
     */
    public final class Window {
        private Window() {}

        public CompletableFuture<Map<String, Object>> showInformation(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WINDOW_SHOW_INFORMATION, context, params);
        }

        public CompletableFuture<Map<String, Object>> showWarning(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WINDOW_SHOW_WARNING, context, params);
        }

        public CompletableFuture<Map<String, Object>> showError(Map<String, ?> context, Map<String, ?> params) {
            return scoped(Protocol.HOST_WINDOW_SHOW_ERROR, context, params);
        }
    }

    /**
     * Calls to functions other plugins export, relayed by Core. The plugin's
     * manifest must declare each function it calls. Both methods fail with a
     * {@link HostError} {@code plugin.host.unavailable} when the host did not
     * offer plugin functions, without sending anything, and a malformed
     * argument fails with {@code sdk.functions.invalidCall} (a malformed
     * context with {@code sdk.host.invalidContext}), also without sending
     * anything.
     *
     * <p>A provider's own failure arrives as a {@link HostError} with code
     * {@code plugin.function.failed}; {@link HostError#pluginErrorCode()} is
     * its error code. Other relay failures have their own
     * {@code plugin.function.*} codes, see {@link HostError#CODES}.
     */
    public final class Functions {
        private Functions() {}

        /**
         * {@code { providers: [{ pluginID, version }] }}: the enabled plugins
         * other than this one that export {@code functionId}, sorted by plugin
         * ID. A listed provider's backend need not be running.
         */
        public CompletableFuture<Map<String, Object>> providers(String functionId) {
            if (!Protocol.isFunctionId(functionId)) {
                return failed(invalidCall("A functionID is dot-separated lowerCamelCase segments of at most 128 bytes."));
            }
            return object(Protocol.HOST_FUNCTIONS_PROVIDERS, channel.request(
                    Protocol.HOST_FUNCTIONS_PROVIDERS, params("functionID", functionId), scope));
        }

        /**
         * Calls one provider's exported function in the workspace {@code context}
         * (a reference from {@link CallContext#workspaceContext()}, or a context
         * from {@code workspace().listContexts()}) and answers {@code { value }},
         * the provider's result, where {@code value} may be null. Only the
         * fields listed here are sent. {@code call}:
         * <ul>
         * <li>{@code pluginID}: the provider, a non-empty string; required;</li>
         * <li>{@code functionID}: the exported function; required;</li>
         * <li>{@code params}: an object, omitted when absent;</li>
         * <li>{@code timeoutMilliseconds}: a positive integer; Core defaults to
         * 60 000 and caps it at 600 000;</li>
         * <li>{@code parentOperationID}: a string of 1-128 bytes. When absent
         * and this Host belongs to a function call, that call's wire operation
         * ID is sent, so Core knows the new call belongs to it; otherwise the
         * field is omitted and Core counts the new call as a child of every
         * relayed call this backend is serving.</li>
         * </ul>
         * When the call this Host belongs to is cancelled or ends, the relayed
         * call is cancelled on the wire and the future fails with
         * {@link CancelledException}.
         */
        public CompletableFuture<Map<String, Object>> invoke(Map<String, ?> context, Map<String, ?> call) {
            Map<String, Object> message;
            try {
                message = relayParams(context, call);
            } catch (PluginSdkError error) {
                return failed(error);
            }
            return object(Protocol.HOST_FUNCTIONS_INVOKE, channel.request(Protocol.HOST_FUNCTIONS_INVOKE, message, scope));
        }
    }

    /** The wire params of a relayed call: the listed fields of {@code call}, never another one. */
    private Map<String, Object> relayParams(Map<String, ?> context, Map<String, ?> call) {
        if (call == null) {
            throw invalidCall("A call is { pluginID, functionID, params?, timeoutMilliseconds?, parentOperationID? }.");
        }
        Object pluginId = call.get("pluginID");
        if (!(pluginId instanceof String) || ((String) pluginId).isEmpty()) {
            throw invalidCall("A call's pluginID is a non-empty string.");
        }
        Object functionId = call.get("functionID");
        if (!(functionId instanceof String) || !Protocol.isFunctionId((String) functionId)) {
            throw invalidCall("A functionID is dot-separated lowerCamelCase segments of at most 128 bytes.");
        }
        Map<String, Object> message = new LinkedHashMap<>();
        message.put("pluginID", pluginId);
        message.put("functionID", functionId);
        message.put("context", contextReference(context));
        Object params = call.get("params");
        if (params != null) {
            if (!(params instanceof Map)) {
                throw invalidCall("A call's params is an object.");
            }
            message.put("params", params);
        }
        Object timeout = call.get("timeoutMilliseconds");
        if (timeout != null) {
            if (!Protocol.isGeneration(timeout)) {
                throw invalidCall("A call's timeoutMilliseconds is a positive integer.");
            }
            message.put("timeoutMilliseconds", ((Number) timeout).longValue());
        }
        Object parent = call.get("parentOperationID");
        if (parent == null) {
            parent = parentOperationId;
        } else if (!Protocol.isParentOperationId(parent)) {
            throw invalidCall("A call's parentOperationID is a string of 1-128 bytes.");
        }
        if (parent != null) {
            message.put("parentOperationID", parent);
        }
        return message;
    }

    private static PluginSdkError invalidCall(String message) {
        return new PluginSdkError("sdk.functions.invalidCall", message);
    }

    /** A method whose params name a workspace context. */
    private CompletableFuture<Map<String, Object>> scoped(String method, Map<String, ?> context, Map<String, ?> params) {
        Map<String, Object> message;
        try {
            message = withContext(context, params);
        } catch (PluginSdkError error) {
            return failed(error);
        }
        return object(method, channel.request(method, message, scope));
    }

    private static Map<String, Object> withContext(Map<String, ?> context, Map<String, ?> params) {
        Map<String, Object> message = params == null ? new LinkedHashMap<String, Object>() : new LinkedHashMap<String, Object>(params);
        message.put("context", contextReference(context));
        return message;
    }

    /** The result as an object, or null; anything else is a host bug. */
    private static CompletableFuture<Map<String, Object>> object(String method, CompletableFuture<Object> response) {
        return response.thenApply(result -> {
            if (result == null) {
                return null;
            }
            if (!(result instanceof Map)) {
                throw new PluginSdkError("sdk.host.invalidResult", method + " answered with something other than an object.");
            }
            return Json.object(result);
        });
    }

    private static Map<String, Object> params(String key, Object value) {
        Map<String, Object> params = new LinkedHashMap<>();
        params.put(key, value);
        return params;
    }

    private static PluginSdkError invalidKey() {
        return new PluginSdkError("sdk.credentials.invalidKey",
                "A credential key is 1-128 ASCII letters, digits, \".\", \"_\", \":\" or \"-\".");
    }

    private static <T> CompletableFuture<T> failed(Throwable error) {
        CompletableFuture<T> future = new CompletableFuture<>();
        future.completeExceptionally(error);
        return future;
    }
}
