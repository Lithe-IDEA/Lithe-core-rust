package io.github.litheidea.plugin.sdk;

import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.regex.Pattern;

/**
 * Wire constants and validation shared with Lithe's Rust Core
 * ({@code rust/crates/plugin-rpc/src/} in the Lithe
 * repository) and the Node.js
 * SDK. The SDK checks the same rules locally so a plugin author sees a mistake
 * when registering a handler instead of as a refused session at runtime.
 */
public final class Protocol {
    /** The version of each protocol this SDK speaks; the host offers it in {@code initialize}. */
    public static final int FUNCTIONS_VERSION = 0;
    public static final int BUILD_VERSION = 0;
    public static final int LOGGING_VERSION = 0;
    public static final int KEYMAP_IMPORT_VERSION = 0;

    public static final String INITIALIZE = "initialize";
    public static final String INITIALIZED = "initialized";
    public static final String SHUTDOWN = "shutdown";
    public static final String EXIT = "exit";
    public static final String CANCEL_REQUEST = "$/cancelRequest";
    public static final String FUNCTION_INVOKE = "lithe/function/invoke";
    public static final String FUNCTION_REGISTER = "lithe/function/register";
    public static final String FUNCTION_UNREGISTER = "lithe/function/unregister";
    public static final String FUNCTION_PROGRESS = "lithe/function/progress";
    public static final String BUILD_MODEL = "lithe/build/model";
    public static final String BUILD_DEPENDENCIES = "lithe/build/dependencies";
    public static final String BUILD_EXECUTE = "lithe/build/execute";
    public static final String BUILD_TEST = "lithe/build/test";
    public static final String BUILD_DISCOVER_TESTS = "lithe/build/discoverTests";
    public static final String BUILD_PREPARE_DEBUG = "lithe/build/prepareDebug";
    public static final String BUILD_PREPARE_RUN = "lithe/build/prepareRun";
    public static final String BUILD_PROGRESS = "lithe/build/progress";

    /**
     * Backend-to-host requests. The host may add methods;
     * {@link Host#request} sends any {@code lithe/host/*} method, and
     * {@code getCapabilities} says which ones the session may call.
     */
    public static final String HOST_TERMINAL_CREATE = "lithe/host/terminal/create";
    public static final String HOST_TERMINAL_SHOW = "lithe/host/terminal/show";
    public static final String HOST_TERMINAL_CLOSE = "lithe/host/terminal/close";
    public static final String HOST_TERMINAL_REOPEN = "lithe/host/terminal/reopen";
    public static final String HOST_TERMINAL_DISPOSE = "lithe/host/terminal/dispose";
    public static final String HOST_TERMINAL_ON_EXIT = "lithe/host/terminal/onExit";
    public static final String HOST_PREFIX = "lithe/host/";
    public static final String HOST_CREDENTIALS_GET = "lithe/host/credentials/get";
    public static final String HOST_CREDENTIALS_SET = "lithe/host/credentials/set";
    public static final String HOST_CREDENTIALS_DELETE = "lithe/host/credentials/delete";
    public static final String HOST_GET_CAPABILITIES = "lithe/host/getCapabilities";
    public static final String HOST_SUBSCRIPTIONS_DISPOSE = "lithe/host/subscriptions/dispose";
    public static final String HOST_WORKSPACE_LIST_CONTEXTS = "lithe/host/workspace/listContexts";
    public static final String HOST_WORKSPACE_GET_CONTEXT = "lithe/host/workspace/getContext";
    public static final String HOST_WORKSPACE_GET_FOLDERS = "lithe/host/workspace/getFolders";
    public static final String HOST_WORKSPACE_READ_DIRECTORY = "lithe/host/workspace/readDirectory";
    public static final String HOST_WORKSPACE_STAT = "lithe/host/workspace/stat";
    public static final String HOST_WORKSPACE_READ_FILE = "lithe/host/workspace/readFile";
    public static final String HOST_WORKSPACE_WATCH = "lithe/host/workspace/watch";
    public static final String HOST_WORKSPACE_FIND_FILES = "lithe/host/workspace/findFiles";
    public static final String HOST_WORKSPACE_FIND_TEXT = "lithe/host/workspace/findText";
    public static final String HOST_DOCUMENTS_LIST = "lithe/host/documents/list";
    public static final String HOST_DOCUMENTS_GET_SNAPSHOT = "lithe/host/documents/getSnapshot";
    public static final String HOST_EDITOR_REGISTER_ACTIONS = "lithe/host/editor/registerActions";
    public static final String HOST_EDITOR_GET_ACTIVE = "lithe/host/editor/getActive";
    public static final String HOST_EDITOR_GET_SELECTIONS = "lithe/host/editor/getSelections";
    public static final String HOST_EDITOR_APPLY_EDITS = "lithe/host/editor/applyEdits";
    public static final String HOST_EDITOR_REPLACE_SELECTIONS = "lithe/host/editor/replaceSelections";
    public static final String HOST_EDITOR_INSERT_TEXT = "lithe/host/editor/insertText";
    public static final String HOST_EDITOR_OPEN = "lithe/host/editor/open";
    public static final String HOST_EDITOR_REVEAL = "lithe/host/editor/reveal";
    public static final String HOST_PREVIEW_OPEN_HTML = "lithe/host/preview/openHTML";
    public static final String HOST_BROWSER_OPEN_EXTERNAL = "lithe/host/browser/openExternal";
    public static final String HOST_WINDOW_SHOW_INFORMATION = "lithe/host/window/showInformation";
    public static final String HOST_WINDOW_SHOW_WARNING = "lithe/host/window/showWarning";
    public static final String HOST_WINDOW_SHOW_ERROR = "lithe/host/window/showError";

    /** Cross-plugin calls Core relays. */
    public static final String HOST_FUNCTIONS_PROVIDERS = "lithe/host/functions/providers";
    public static final String HOST_FUNCTIONS_INVOKE = "lithe/host/functions/invoke";

    /** Host-to-backend notifications. */
    public static final String HOST_DID_CHANGE_CONTEXTS = "lithe/host/workspace/didChangeContexts";
    public static final String HOST_SUBSCRIPTION_NOTIFY = "lithe/host/subscriptions/notify";
    public static final String HOST_COMMAND_DID_RUN = "lithe/host/commands/didRun";

    /** JSON-RPC and LSP error codes the SDK answers with. */
    public static final int PARSE_ERROR = -32700;
    public static final int INVALID_REQUEST = -32600;
    public static final int METHOD_NOT_FOUND = -32601;
    public static final int INVALID_PARAMS = -32602;
    public static final int INTERNAL_ERROR = -32603;
    public static final int SERVER_NOT_INITIALIZED = -32002;
    public static final int REQUEST_FAILED = -32000;
    public static final int REQUEST_CANCELLED = -32800;

    public static final int MAX_FUNCTIONS = 256;
    public static final int MAX_FUNCTION_ID_BYTES = 128;
    public static final int MAX_PLUGIN_ERROR_CODE_BYTES = 64;
    public static final int MAX_MESSAGE_BYTES = 1024;
    public static final int MAX_RESULT_BYTES = 1 << 20;
    public static final int MAX_BUILD_RESULT_BYTES = 32 * 1024 * 1024;
    public static final int MAX_BUILD_MESSAGE_BYTES = 4096;
    public static final int MAX_BUILD_OUTPUT_BYTES = 65536;
    /** Unanswered host requests Core allows a session; the SDK queues the rest. */
    public static final int MAX_PENDING_HOST_REQUESTS = 16;
    public static final int MAX_CREDENTIAL_SECRET_BYTES = 16 * 1024;
    /** Timeout Core gives a relayed call that names none. */
    public static final int RELAY_DEFAULT_TIMEOUT_MILLISECONDS = 60000;
    /** Longest timeout of a relayed call; the SDK sends a longer one unchanged and leaves the cap to Core. */
    public static final int RELAY_MAX_TIMEOUT_MILLISECONDS = 600000;
    public static final int MAX_PARENT_OPERATION_ID_BYTES = 128;

    static boolean buildProgress(String method) {
        return BUILD_EXECUTE.equals(method) || BUILD_TEST.equals(method) || BUILD_PREPARE_RUN.equals(method) || BUILD_PREPARE_DEBUG.equals(method);
    }

    private static final Pattern FUNCTION_SEGMENT = Pattern.compile("[a-z][A-Za-z0-9]*");
    private static final Pattern PLUGIN_ERROR_CODE = Pattern.compile("[A-Za-z][A-Za-z0-9._-]*");
    private static final Pattern KEY = Pattern.compile("[A-Za-z0-9._:-]{1,128}");
    private static final Pattern COMMAND_ID = Pattern.compile("[a-z0-9.-]{1,128}");
    private static final long MAX_SAFE_INTEGER = 9007199254740991L;

    private Protocol() {}

    /** Dot-separated lowerCamelCase segments, for example {@code snapshot.refresh}. */
    public static boolean isFunctionId(String value) {
        if (value == null || value.isEmpty() || utf8Length(value) > MAX_FUNCTION_ID_BYTES) {
            return false;
        }
        for (String segment : value.split("\\.", -1)) {
            if (!FUNCTION_SEGMENT.matcher(segment).matches()) {
                return false;
            }
        }
        return true;
    }

    /** The plugin error code alphabet Core forwards as {@code pluginErrorCode}. */
    public static boolean isPluginErrorCode(String value) {
        return value != null
                && utf8Length(value) <= MAX_PLUGIN_ERROR_CODE_BYTES
                && PLUGIN_ERROR_CODE.matcher(value).matches();
    }

    /**
     * The key alphabet of host services: 1-128 ASCII letters, digits,
     * {@code .}, {@code _}, {@code :}, or {@code -}. Credential keys, context
     * IDs, and subscription IDs use it.
     */
    public static boolean isCredentialKey(String value) {
        return value != null && KEY.matcher(value).matches();
    }

    /** {@code { contextID, generation }}, exactly, naming one generation of one workspace context. */
    public static boolean isContextReference(Object value) {
        if (!(value instanceof Map) || ((Map<?, ?>) value).size() != 2) {
            return false;
        }
        Map<?, ?> reference = (Map<?, ?>) value;
        Object id = reference.get("contextID");
        return id instanceof String && isCredentialKey((String) id) && isGeneration(reference.get("generation"));
    }

    /** A {@code parentOperationID}: a string of 1-128 UTF-8 bytes. */
    static boolean isParentOperationId(Object value) {
        if (!(value instanceof String)) {
            return false;
        }
        int bytes = utf8Length((String) value);
        return bytes >= 1 && bytes <= MAX_PARENT_OPERATION_ID_BYTES;
    }

    /** A command ID from the plugin's manifest: 1-128 of {@code [a-z0-9.-]}. */
    public static boolean isCommandId(String value) {
        return value != null && COMMAND_ID.matcher(value).matches();
    }

    /** A positive integer JSON can carry exactly. */
    static boolean isGeneration(Object value) {
        if (value instanceof Long || value instanceof Integer || value instanceof Short || value instanceof Byte) {
            long number = ((Number) value).longValue();
            return number >= 1 && number <= MAX_SAFE_INTEGER;
        }
        if (value instanceof Double || value instanceof Float) {
            double number = ((Number) value).doubleValue();
            return number >= 1 && number <= MAX_SAFE_INTEGER && number == Math.rint(number);
        }
        return false;
    }

    /** Cuts {@code value} to {@link #MAX_MESSAGE_BYTES} UTF-8 bytes without splitting a character. */
    public static String boundedText(String value) {
        return boundedText(value, MAX_MESSAGE_BYTES);
    }

    /** Cuts {@code value} to {@code limit} UTF-8 bytes without splitting a character. */
    public static String boundedText(String value, int limit) {
        String text = value == null ? "" : value;
        int used = 0;
        int index = 0;
        while (index < text.length()) {
            int codePoint = text.codePointAt(index);
            int size = codePoint < 0x80 ? 1 : codePoint < 0x800 ? 2 : codePoint < 0x10000 ? 3 : 4;
            if (used + size > limit) {
                return text.substring(0, index);
            }
            used += size;
            index += Character.charCount(codePoint);
        }
        return text;
    }

    /**
     * Methods reserved for the SDK. A plugin cannot install its own handler for
     * these, or send them to the host: the lifecycle belongs to the host's
     * session management, and {@code lithe/*} belongs to Lithe's typed contracts.
     */
    public static boolean isReservedMethod(String method) {
        return method.equals(INITIALIZE)
                || method.equals(INITIALIZED)
                || method.equals(SHUTDOWN)
                || method.equals(EXIT)
                || method.equals(CANCEL_REQUEST)
                || method.startsWith("lithe/");
    }

    static int utf8Length(String value) {
        return value.getBytes(StandardCharsets.UTF_8).length;
    }
}
