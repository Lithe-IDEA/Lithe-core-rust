//! Plugin functions: plugin-private actions a backend registers and the host
//! invokes on behalf of the same plugin, next to the typed domain protocols.
//!
//! The protocol version is the `backend` capability version, negotiated in
//! `initialize` under `capabilities.experimental.litheFunctions`,
//! independently of `litheBuild`, so a backend can speak either one. The
//! protocol covers function registration and invocation, the host services of
//! `host_request` (credentials, `lithe/host/getCapabilities`, subscriptions,
//! workspace contexts with a workspace context on invocations, typed host
//! failure codes and details, host notifications), and the cross-plugin
//! function relay (`lithe/host/functions/*`) with the Core-written `caller`
//! of an invocation. Functions never replace a domain
//! operation: a build model, dependency tree, or task still travels over its
//! typed `lithe/build/*` method and Core's plan and decode commands. This
//! crate holds the deterministic rules (identifiers, registration, invocation
//! planning, outcome and progress normalization, and the refusal of requests a
//! backend may not make); the session engine applies them to live traffic.
//! The rules live in the plugin contract
//! (`shared/plugins/contract/v1/README.md`, "Plugin functions"). The
//! [`host_api`] commands expose the same host-method catalog to native
//! dispatchers.

pub mod host_api;

use lithe_plugin_language_servers::is_protocol_name;
use lithe_plugin_model::capabilities::{
    offered_versions, versions as capability_versions, BACKEND,
};
use lithe_plugin_model::{is_function_id, MAX_FUNCTION_ID_BYTES};
use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

mod failures;
mod functions;
mod host_notifications;
mod host_params;
mod host_requests;

pub use failures::*;
pub use functions::*;
pub use host_notifications::*;
pub use host_params::*;
pub use host_requests::*;

/// Key under `capabilities.experimental` on both sides of the exchange.
const EXPERIMENTAL_KEY: &str = "litheFunctions";

/// Plugin-function protocol versions this client speaks: the versions of the
/// `backend` capability.
fn supported_versions() -> Vec<u64> {
    offered_versions(capability_versions(BACKEND))
}

/// Feature name recorded in the session's server capabilities once a backend
/// answered a supported version with a valid function list.
pub const FUNCTIONS_FEATURE: &str = "litheFunctions";

/// Host-to-backend request that runs one registered function.
pub const INVOKE_METHOD: &str = "lithe/function/invoke";

/// Backend-to-host request adding functions after `initialize`.
pub const REGISTER_METHOD: &str = "lithe/function/register";

/// Backend-to-host request withdrawing functions; running calls are revoked.
pub const UNREGISTER_METHOD: &str = "lithe/function/unregister";

/// Backend-to-host notification reporting progress of one running call.
pub const PROGRESS_METHOD: &str = "lithe/function/progress";

/// Upper bound of functions one session may have registered at a time.
const MAX_FUNCTIONS: usize = 256;

const MAX_PLUGIN_ID_BYTES: usize = 128;

const MAX_PLUGIN_ERROR_CODE_BYTES: usize = 64;

/// Bound of a backend-supplied failure or progress message, in UTF-8 bytes.
const MAX_MESSAGE_BYTES: usize = 1024;

/// Bound of one serialized function result. Larger documents belong in a
/// domain protocol with its own decoder, not in a plugin-private reply.
pub const MAX_RESULT_BYTES: usize = 1 << 20;

/// Upper bound of one invocation's deadline, matching build-provider requests.
pub const MAX_FUNCTION_TIMEOUT_MS: u64 = 60 * 60_000;

/// Host service that keeps secrets for the plugin, private to its ID.
pub const HOST_SERVICE_CREDENTIALS: &str = "credentials";

/// Host service listing open workspace contexts and their folders, granted
/// by `workspace.metadata` to a `backend` process.
pub const HOST_SERVICE_WORKSPACE: &str = "workspace";

/// Capability discovery. Every negotiated plugin-owned session has it;
/// Core answers it without involving the host.
const HOST_SERVICE_HOST: &str = "host";

/// Ending subscriptions other services created. Every negotiated
/// plugin-owned session has it, because only the session that holds a
/// subscription can name it.
const HOST_SERVICE_SUBSCRIPTIONS: &str = "subscriptions";

/// Calls into the functions other plugins export. The grant comes from the
/// manifest's `requires.pluginFunctions.calls`, not from a permission: the
/// caller's declaration plus the provider's `exports` is the consent, and
/// Core's relay checks both on every call.
pub const HOST_SERVICE_FUNCTIONS: &str = "functions";

/// Services granted as a whole by their aggregate grant. Every other host
/// method is granted individually, so adding a method to `workspace` or
/// `editor` never widens an existing permission.
pub(crate) const GRANTABLE_HOST_SERVICES: &[&str] = &[
    HOST_SERVICE_CREDENTIALS,
    HOST_SERVICE_WORKSPACE,
    HOST_SERVICE_FUNCTIONS,
];

/// Whether a launch grant names a service granted as a whole or an implemented
/// host method.
pub fn is_host_grant(grant: &str) -> bool {
    GRANTABLE_HOST_SERVICES.contains(&grant)
        || HOST_METHODS
            .iter()
            .any(|entry| entry.method == grant && entry.permission.is_some())
}

/// Methods admitted by one manifest permission. The platform intersects these
/// with its registered method handlers before starting the backend.
pub fn methods_for_permission(permission: &str) -> Vec<&'static str> {
    HOST_METHODS
        .iter()
        .filter(|entry| entry.permission == Some(permission))
        .map(|entry| entry.method)
        .collect()
}

/// Permissions for the additive host services.
pub const HOST_PERMISSIONS: &[&str] = &[
    "workspace.files.read",
    "workspace.files.watch",
    "workspace.search",
    "documents.read",
    "editor.edit",
    "editor.actions",
    "editor.providers",
    "editor.open",
    "preview.open",
    "browser.open",
    "window.messages",
    "terminal.create",
    "keymaps.readSelectedSources",
    "run.start",
    "debug.start",
    PERMISSION_LANGUAGE_SERVER_REQUEST,
    PERMISSION_DOCUMENTS_CONTENT_PROVIDER,
];

/// Permission of the `languageServer` host service (plan API-25): a backend
/// may send requests and notifications to, and report the status of, the
/// language servers its own plugin declares.
pub const PERMISSION_LANGUAGE_SERVER_REQUEST: &str = "languageServer.request";

/// Permission of `documents/registerContentProvider` (plan API-12): the
/// backend supplies the read-only text of URIs of a scheme it registers.
pub(crate) const PERMISSION_DOCUMENTS_CONTENT_PROVIDER: &str = "documents.contentProvider";

const CREDENTIALS_GET: &str = "lithe/host/credentials/get";

const CREDENTIALS_SET: &str = "lithe/host/credentials/set";

const CREDENTIALS_DELETE: &str = "lithe/host/credentials/delete";

/// Host method that reports the protocol version and granted host services.
pub const GET_CAPABILITIES: &str = "lithe/host/getCapabilities";

const SUBSCRIPTIONS_DISPOSE: &str = "lithe/host/subscriptions/dispose";

const WORKSPACE_LIST_CONTEXTS: &str = "lithe/host/workspace/listContexts";

const WORKSPACE_GET_CONTEXT: &str = "lithe/host/workspace/getContext";

const WORKSPACE_GET_FOLDERS: &str = "lithe/host/workspace/getFolders";

/// Backend request listing the plugins that export a function it may call.
pub const FUNCTIONS_PROVIDERS: &str = "lithe/host/functions/providers";

/// Backend request calling a function another plugin exports.
pub const FUNCTIONS_INVOKE: &str = "lithe/host/functions/invoke";

const MAX_CREDENTIAL_KEY_BYTES: usize = 128;

/// Bound of one stored secret, in UTF-8 bytes; enough for a password, a
/// token, or a private key, not for documents.
pub(crate) const MAX_CREDENTIAL_SECRET_BYTES: usize = 16 * 1024;

/// Host requests one session may have awaiting the host's answer.
pub const MAX_PENDING_HOST_REQUESTS: usize = 16;

/// Bound of the serialized `details` object of one host failure; enough
/// for a few numbers and identifiers, not for documents.
pub(crate) const MAX_HOST_DETAILS_BYTES: usize = 4 * 1024;

/// Bound of the serialized params of one host-to-backend notification,
/// the same as one function result.
pub const MAX_HOST_NOTIFICATION_BYTES: usize = MAX_RESULT_BYTES;

/// Stable codes of the plugin-function contract. Synchronous refusals carry
/// them in `CoreError::details`; asynchronous outcomes in the runtime error's
/// `code`; refused backend requests in the JSON-RPC error's `data.code`.
pub mod codes {
    /// The session cannot serve plugin functions: it belongs to no plugin, or
    /// its backend did not negotiate the protocol.
    pub const UNAVAILABLE: &str = "plugin.function.unavailable";
    /// The function is not registered on the session.
    pub const UNKNOWN: &str = "plugin.function.unknown";
    /// The reference names another plugin than the one owning the session.
    pub const WRONG_PLUGIN: &str = "plugin.function.wrongPlugin";
    /// The plugin or function ID of the reference is malformed.
    pub const INVALID_REFERENCE: &str = "plugin.function.invalidReference";
    /// Invocation params are not a JSON object.
    pub const INVALID_PARAMS: &str = "plugin.function.invalidParams";
    /// A register or unregister request was refused as a whole.
    pub const INVALID_REGISTRATION: &str = "plugin.function.invalidRegistration";
    /// The backend answered the invocation with an error.
    pub const FAILED: &str = "plugin.function.failed";
    /// The backend's answer has no result or exceeds the result bound.
    pub const INVALID_RESULT: &str = "plugin.function.invalidResult";
    /// The function was unregistered while the call was running.
    pub const REVOKED: &str = "plugin.function.revoked";
    /// The backend asked the host to dispatch a domain operation or function,
    /// which could route back to the backend itself.
    pub const RECURSIVE_CALL: &str = "plugin.function.recursiveCall";
    /// The backend asked for a host capability this host does not offer.
    pub const HOST_UNAVAILABLE: &str = "plugin.host.unavailable";
    /// The host offers the service, but the plugin's permissions do not
    /// grant it to this session.
    pub const HOST_PERMISSION_DENIED: &str = "plugin.host.permissionDenied";
    /// A host-service request has malformed or unknown params.
    pub const HOST_INVALID_PARAMS: &str = "plugin.host.invalidParams";
    /// The host could not perform the service.
    pub const HOST_FAILED: &str = "plugin.host.failed";
    /// The request names a workspace context that was closed or replaced
    /// by a newer generation.
    pub const HOST_CONTEXT_EXPIRED: &str = "plugin.host.contextExpired";
    /// The context's authority (for example a remote workspace) does not
    /// implement the requested file or process operation. The host never
    /// falls back to the local placeholder directory.
    pub const HOST_AUTHORITY_UNSUPPORTED: &str = "plugin.host.authorityUnsupported";
    /// A versioned write found a newer version than the one it names.
    pub const HOST_VERSION_CONFLICT: &str = "plugin.host.versionConflict";
    /// The request exceeds a host limit on size or count. Retrying the same
    /// request fails again; rate limits are `HOST_RATE_LIMITED`.
    pub const HOST_LIMIT_EXCEEDED: &str = "plugin.host.limitExceeded";
    /// The backend cancelled the request, or the host stopped it.
    pub const HOST_CANCELLED: &str = "plugin.host.cancelled";
    /// The host's own deadline for the operation passed.
    pub const HOST_TIMEOUT: &str = "plugin.host.timeout";
    /// The resource the request names (a file, a document, a window) does
    /// not exist or is no longer open.
    pub const HOST_NOT_FOUND: &str = "plugin.host.notFound";
    /// The resource exists but may not be changed.
    pub const HOST_READ_ONLY: &str = "plugin.host.readOnly";
    /// The params are well formed, but this host does not implement an
    /// option they ask for.
    pub const HOST_UNSUPPORTED_OPTION: &str = "plugin.host.unsupportedOption";
    /// The resource is not in the state the operation needs, for example a
    /// document with unsaved changes or of another kind; the backend changes
    /// the state or the request, not merely retries.
    pub const HOST_PRECONDITION_FAILED: &str = "plugin.host.preconditionFailed";
    /// The host cannot serve the request at the moment, for example while
    /// the user composes text or an index loads; the same request may
    /// succeed later.
    pub const HOST_BUSY: &str = "plugin.host.busy";
    /// The plugin made too many requests of this kind recently; the same
    /// request may succeed after a while.
    pub const HOST_RATE_LIMITED: &str = "plugin.host.rateLimited";
    /// A tool definition the request references resolves to no executable
    /// for this workspace; `details` carries `{tool, reason}`.
    pub const HOST_TOOL_UNRESOLVED: &str = "plugin.host.toolUnresolved";
    /// The language server a `languageServer` request names is not running
    /// in the context: it is starting, stopped, crashed, or not started.
    pub const HOST_LANGUAGE_SERVER_UNAVAILABLE: &str = "plugin.host.languageServerUnavailable";
    /// The language server answered a forwarded request with an error; the
    /// failure's `details` carry the server's `code` and `message`.
    pub const HOST_LANGUAGE_SERVER_ERROR: &str = "plugin.host.languageServerError";
    /// The method or command is not in the language server's declared
    /// `requestMethods`, `commands`, or allowed notifications.
    pub const HOST_METHOD_NOT_DECLARED: &str = "plugin.host.methodNotDeclared";
    /// The request names a contribution, such as a language server, that
    /// another plugin owns.
    pub const HOST_FOREIGN_CONTRIBUTION: &str = "plugin.host.foreignContribution";
    /// A page's call or a host operation ended because the backend process
    /// exited. Reported by hosts to pages, never sent to a backend.
    pub const BACKEND_EXITED: &str = "plugin.backend.exited";

    // Cross-plugin relay (`lithe/host/functions/*`). Each
    // refusal has its own code and number, so a caller can branch on either.

    /// The caller's manifest does not declare a call to this function.
    pub const NOT_DECLARED: &str = "plugin.function.notDeclared";
    /// The plugin that should provide the function does not export it, or
    /// has not registered it with its running backend.
    pub const NOT_EXPORTED: &str = "plugin.function.notExported";
    /// The providing plugin is not installed or not enabled, or was disabled
    /// or removed while the call was running.
    pub const PROVIDER_DISABLED: &str = "plugin.function.providerDisabled";
    /// The caller declared another contract version than the provider exports.
    pub const VERSION_MISMATCH: &str = "plugin.function.versionMismatch";
    /// The provider is already on the call chain, so the call would come
    /// back to a plugin that is waiting for it (including a plugin calling
    /// itself).
    pub const CALL_CYCLE: &str = "plugin.function.callCycle";
    /// The call chain is longer than the depth limit.
    pub const DEPTH_EXCEEDED: &str = "plugin.function.depthExceeded";
    /// The calling plugin already has the most relayed calls it may have
    /// running at once.
    pub const TOO_MANY_CALLS: &str = "plugin.function.tooManyCalls";
    /// The relayed params or result exceed the relay's byte limit.
    pub const PAYLOAD_TOO_LARGE: &str = "plugin.function.payloadTooLarge";
    /// The provider's backend could not serve the call: it did not start in
    /// time, exited, or withdrew the function while the call ran.
    pub const PROVIDER_UNAVAILABLE: &str = "plugin.function.providerUnavailable";
}

#[cfg(test)]
mod tests {
    //! Golden cases from `shared/plugins/contract/v1/fixtures/backend/function-protocol.json`.
    //! Timing behavior (cancellation, deadlines, late replies, revocation of a
    //! running call) is covered against a scripted backend in the engine tests.

    use super::*;

    const FIXTURE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../shared/plugins/contract/v1/fixtures/backend/function-protocol.json"
    ));

    fn fixture() -> Value {
        serde_json::from_str(FIXTURE).expect("the function-protocol fixture is JSON")
    }

    fn cases(section: &str) -> Vec<Value> {
        fixture()[section]
            .as_array()
            .unwrap_or_else(|| panic!("fixture section {section} is missing"))
            .clone()
    }

    fn id_set(value: &Value) -> BTreeSet<String> {
        value
            .as_array()
            .expect("an ID list")
            .iter()
            .map(|id| id.as_str().expect("a string ID").to_string())
            .collect()
    }

    fn name(case: &Value) -> &str {
        case["name"].as_str().expect("every case is named")
    }

    #[test]
    fn the_offer_matches_the_fixture() {
        let fixture = fixture();
        let (key, offer) = client_experimental_capabilities();
        assert_eq!(fixture["protocol"], key);
        assert_eq!(fixture["offer"], offer);
    }

    #[test]
    fn negotiation_follows_the_fixture() {
        for case in cases("negotiation") {
            let negotiated = negotiate(&case["capabilities"]);
            let expected = &case["expected"];
            match expected["functions"].as_array() {
                Some(_) => assert_eq!(
                    negotiated,
                    Some((
                        expected["version"].as_u64().unwrap_or(0),
                        id_set(&expected["functions"])
                    )),
                    "{}",
                    name(&case)
                ),
                None => assert_eq!(negotiated, None, "{}", name(&case)),
            }
        }
    }

    #[test]
    fn registration_follows_the_fixture() {
        for case in cases("registration") {
            let current = id_set(&case["registered"]);
            let params = case.get("params");
            let expected = &case["expected"];
            let outcome = match case["method"].as_str() {
                Some(REGISTER_METHOD) => {
                    apply_registration(&current, params).map(|registry| (registry, Vec::new()))
                }
                Some(UNREGISTER_METHOD) => apply_unregistration(&current, params),
                other => panic!("{}: unknown method {other:?}", name(&case)),
            };
            if expected["status"] == "accepted" {
                let (registry, removed) =
                    outcome.unwrap_or_else(|reason| panic!("{}: {reason}", name(&case)));
                assert_eq!(registry, id_set(&expected["functions"]), "{}", name(&case));
                let expected_removed: Vec<String> =
                    id_set(&expected["removed"]).into_iter().collect();
                assert_eq!(removed, expected_removed, "{}", name(&case));
            } else {
                assert_eq!(expected["code"], codes::INVALID_REGISTRATION);
                assert!(outcome.is_err(), "{} should be refused", name(&case));
            }
        }
    }

    #[test]
    fn invocation_planning_follows_the_fixture() {
        for case in cases("invocations") {
            let session = &case["session"];
            let functions = id_set(&session["functions"]);
            let reference: FunctionReference =
                serde_json::from_value(case["request"]["function"].clone())
                    .unwrap_or_else(|error| panic!("{}: {error}", name(&case)));
            let planned = plan_invocation(
                &FunctionSession {
                    plugin_id: session["pluginID"].as_str(),
                    negotiated: session["negotiated"].as_bool().unwrap_or(false),
                    functions: &functions,
                },
                &reference,
                "operation-1",
                &case["request"]["params"],
            );
            let expected = &case["expected"];
            if expected["status"] == "sent" {
                let params = planned.unwrap_or_else(|error| panic!("{}: {error:?}", name(&case)));
                assert_eq!(expected["method"], INVOKE_METHOD, "{}", name(&case));
                assert_eq!(params, expected["params"], "{}", name(&case));
            } else {
                let error = planned.expect_err(name(&case));
                assert_eq!(
                    error.details.as_deref(),
                    expected["code"].as_str(),
                    "{}",
                    name(&case)
                );
            }
        }
    }

    #[test]
    fn outcomes_follow_the_fixture() {
        for case in cases("outcomes") {
            let expected = &case["expected"];
            match invocation_outcome(&case["response"]) {
                Ok(result) => assert_eq!(result, expected["result"], "{}", name(&case)),
                Err(failure) => {
                    let error = &expected["error"];
                    assert_eq!(failure.code, error["code"], "{}", name(&case));
                    assert_eq!(failure.message, error["message"], "{}", name(&case));
                    assert_eq!(
                        failure.plugin_error_code.as_deref(),
                        error["pluginErrorCode"].as_str(),
                        "{}",
                        name(&case)
                    );
                }
            }
        }
    }

    #[test]
    fn progress_follows_the_fixture() {
        for case in cases("progress") {
            let parsed = parse_progress(case.get("params"));
            let expected = &case["expected"];
            if expected.is_null() {
                assert_eq!(parsed, None, "{}", name(&case));
            } else {
                assert_eq!(
                    parsed,
                    Some(FunctionProgress {
                        operation_id: expected["operationID"].as_str().unwrap().to_string(),
                        message: expected["message"].as_str().map(ToString::to_string),
                        percentage: expected["percentage"].as_u64(),
                    }),
                    "{}",
                    name(&case)
                );
            }
        }
    }

    #[test]
    fn backend_requests_follow_the_fixture() {
        for case in cases("backendRequests") {
            assert_eq!(
                refused_backend_request(case["method"].as_str().unwrap()),
                case["expected"]["code"].as_str(),
                "{}",
                name(&case)
            );
        }
    }

    #[test]
    fn host_requests_follow_the_fixture() {
        for case in cases("hostRequests") {
            let granted: BTreeSet<String> = id_set(&case["hostServices"]);
            let outcome = host_request(
                case["method"].as_str().unwrap(),
                case.get("params"),
                Some(0),
                &granted,
            );
            let expected = &case["expected"];
            if expected["relay"].as_bool() == Some(true) {
                assert_eq!(outcome, Ok(HostRoute::Relay), "{}", name(&case));
                continue;
            }
            if let Some(service) = expected["forward"].as_str() {
                assert_eq!(
                    outcome,
                    Ok(HostRoute::Forward(
                        HOST_METHODS
                            .iter()
                            .map(|entry| entry.service)
                            .find(|known| *known == service)
                            .expect("a known service")
                    )),
                    "{}",
                    name(&case)
                );
                continue;
            }
            match expected.get("answer") {
                Some(answer) => {
                    assert_eq!(
                        outcome,
                        Ok(HostRoute::Answer(answer.clone())),
                        "{}",
                        name(&case)
                    )
                }
                None => {
                    let refusal = outcome.expect_err(name(&case));
                    assert_eq!(refusal.code, expected["code"], "{}", name(&case));
                    assert_eq!(refusal.rpc_code, expected["rpcCode"], "{}", name(&case));
                    assert_eq!(
                        refusal.details.as_ref(),
                        expected.get("details"),
                        "{}",
                        name(&case)
                    );
                }
            }
        }
    }

    #[test]
    fn invocation_contexts_follow_the_fixture() {
        for case in cases("invocationContexts") {
            let mut params = case["params"].clone();
            let outcome = attach_context(&mut params, case.get("context"));
            let expected = &case["expected"];
            match expected["code"].as_str() {
                Some(code) => assert_eq!(
                    outcome.expect_err(name(&case)).details.as_deref(),
                    Some(code),
                    "{}",
                    name(&case)
                ),
                None => {
                    outcome.unwrap_or_else(|error| panic!("{}: {error:?}", name(&case)));
                    assert_eq!(params, expected["params"], "{}", name(&case));
                }
            }
        }
    }

    #[test]
    fn invocation_callers_follow_the_fixture() {
        for case in cases("invocationCallers") {
            let mut params = case["params"].clone();
            attach_caller(&mut params, case["caller"].as_str());
            assert_eq!(params, case["expected"]["params"], "{}", name(&case));
        }
    }

    #[test]
    fn relay_errors_follow_the_fixture() {
        for case in cases("relayErrors") {
            let code = case["code"].as_str().unwrap();
            assert_eq!(host_rpc_code(code), case["rpcCode"], "{}", name(&case));
        }
    }

    /// Every failure a relayed call can end with has its own number, so a
    /// caller can branch on either the string or the number.
    #[test]
    fn relay_refusals_have_distinct_numbers() {
        let relay_codes = [
            codes::FAILED,
            codes::INVALID_RESULT,
            codes::NOT_DECLARED,
            codes::NOT_EXPORTED,
            codes::PROVIDER_DISABLED,
            codes::VERSION_MISMATCH,
            codes::CALL_CYCLE,
            codes::DEPTH_EXCEEDED,
            codes::TOO_MANY_CALLS,
            codes::PAYLOAD_TOO_LARGE,
            codes::PROVIDER_UNAVAILABLE,
        ];
        let numbers: BTreeSet<i64> = relay_codes.iter().map(|code| host_rpc_code(code)).collect();
        assert_eq!(numbers.len(), relay_codes.len());
        assert!(!numbers.contains(&-32603));
    }

    /// The relay's discovery and invocation requests are not the recursive
    /// call: a backend cannot send `lithe/function/invoke`.
    #[test]
    fn relay_methods_do_not_weaken_the_recursive_call_refusal() {
        assert_eq!(
            refused_backend_request(INVOKE_METHOD),
            Some(codes::RECURSIVE_CALL)
        );
        assert_eq!(
            refused_backend_request("lithe/build/model"),
            Some(codes::RECURSIVE_CALL)
        );
    }

    #[test]
    fn host_failures_follow_the_fixture() {
        for case in cases("hostFailures") {
            let (code, rpc_code) = host_failure(case["code"].as_str());
            assert_eq!(code, case["expected"]["code"], "{}", name(&case));
            assert_eq!(rpc_code, case["expected"]["rpcCode"], "{}", name(&case));
        }
    }

    #[test]
    fn host_notifications_follow_the_fixture() {
        for case in cases("hostNotifications") {
            let granted: BTreeSet<String> = case["hostServices"]
                .as_array()
                .unwrap()
                .iter()
                .map(|service| service.as_str().unwrap().to_string())
                .collect();
            let outcome = host_notification(
                case["method"].as_str().unwrap(),
                &case["params"],
                Some(0),
                &granted,
            );
            let expected = &case["expected"];
            match expected["code"].as_str() {
                Some(code) => assert_eq!(
                    outcome.expect_err(name(&case)).details.as_deref(),
                    Some(code),
                    "{}",
                    name(&case)
                ),
                None => assert_eq!(
                    outcome.unwrap_or_else(|error| panic!("{}: {error:?}", name(&case))),
                    expected["delivered"].as_bool().unwrap(),
                    "{}",
                    name(&case)
                ),
            }
        }
    }

    /// Details pass only as a bounded object, so a host cannot hand a
    /// backend an unbounded or untyped payload through a failure.
    #[test]
    fn host_failure_details_are_a_bounded_object() {
        let details = json!({ "currentVersion": 7 });
        assert_eq!(bounded_host_details(Some(&details)), Some(details));
        assert_eq!(bounded_host_details(Some(&json!("text"))), None);
        let oversized = json!({ "text": "x".repeat(MAX_HOST_DETAILS_BYTES) });
        assert_eq!(bounded_host_details(Some(&oversized)), None);
        assert_eq!(bounded_host_details(None), None);
    }

    /// An oversized notification is refused before it reaches a backend.
    #[test]
    fn host_notifications_are_bounded() {
        let params = json!({
            "subscriptionID": "s1",
            "sequence": 1,
            "event": { "text": "x".repeat(MAX_HOST_NOTIFICATION_BYTES) }
        });
        let refusal = host_notification(SUBSCRIPTIONS_NOTIFY, &params, Some(0), &BTreeSet::new())
            .expect_err("the notification is too large");
        assert_eq!(refusal.details.as_deref(), Some(codes::HOST_INVALID_PARAMS));
    }

    /// A backend that negotiated no protocol version is offered no host
    /// service and receives no host notification, whatever it was granted.
    #[test]
    fn an_un_negotiated_backend_gets_no_host_services() {
        let granted = BTreeSet::from([HOST_SERVICE_WORKSPACE.to_string()]);
        let refusal = host_request(GET_CAPABILITIES, None, None, &granted)
            .expect_err("no negotiated version");
        assert_eq!(refusal.code, codes::HOST_UNAVAILABLE);
        assert_eq!(refusal.rpc_code, -32601);
        assert!(host_request(GET_CAPABILITIES, None, Some(0), &granted).is_ok());
        let params = json!({ "subscriptionID": "s1", "sequence": 1, "event": {} });
        assert!(
            !host_notification(SUBSCRIPTIONS_NOTIFY, &params, None, &granted)
                .expect("a well-formed notification is skipped, not refused")
        );
        assert!(
            host_notification(SUBSCRIPTIONS_NOTIFY, &params, Some(0), &granted)
                .expect("a negotiated backend receives it")
        );
    }

    /// Capability discovery lists only what the session may call.
    #[test]
    fn capabilities_list_only_granted_services() {
        let granted = BTreeSet::from([HOST_SERVICE_WORKSPACE.to_string()]);
        let answer = capabilities_answer(0, &granted);
        let services = answer["services"].as_object().expect("services");
        assert!(services.contains_key(HOST_SERVICE_WORKSPACE));
        assert!(services.contains_key(HOST_SERVICE_HOST));
        assert!(services.contains_key(HOST_SERVICE_SUBSCRIPTIONS));
        assert!(!services.contains_key(HOST_SERVICE_CREDENTIALS));
    }

    /// Every grantable service has at least one method, so a grant never
    /// admits nothing.
    #[test]
    fn grantable_services_have_methods() {
        for service in GRANTABLE_HOST_SERVICES {
            assert!(
                HOST_METHODS.iter().any(|entry| entry.service == *service),
                "{service}"
            );
        }
    }

    /// The secret bound is inclusive and measured in UTF-8 bytes.
    #[test]
    fn credential_secrets_are_bounded() {
        let granted = BTreeSet::from([HOST_SERVICE_CREDENTIALS.to_string()]);
        let set = |secret: String| {
            host_request(
                CREDENTIALS_SET,
                Some(&json!({ "key": "k", "secret": secret })),
                Some(0),
                &granted,
            )
        };
        assert!(set("x".repeat(MAX_CREDENTIAL_SECRET_BYTES)).is_ok());
        assert_eq!(
            set("x".repeat(MAX_CREDENTIAL_SECRET_BYTES + 1))
                .unwrap_err()
                .code,
            codes::HOST_INVALID_PARAMS
        );
    }

    /// Snapshot reads are separately granted and may name only bounded opaque references.
    #[test]
    fn keymap_snapshot_host_requests_require_grants_and_bounded_references() {
        let method = "lithe/host/keymapImport/readChunk";
        let granted = BTreeSet::from([method.to_string()]);
        let valid = json!({ "snapshotID": "abc-123", "providerID": "editor", "fileID": "file-1", "offset": 0, "maximumBytes": 131072 });
        assert!(matches!(
            host_request(method, Some(&valid), Some(0), &granted),
            Ok(HostRoute::Forward("keymapImport"))
        ));
        assert!(host_request(method, Some(&valid), Some(0), &BTreeSet::new()).is_err());
        for invalid in [
            json!({ "snapshotID": "../private", "providerID": "editor", "fileID": "file-1", "offset": 0, "maximumBytes": 1 }),
            json!({ "snapshotID": "abc", "providerID": "editor", "fileID": "file-1", "offset": 0, "maximumBytes": 131073 }),
            json!({ "snapshotID": "abc", "providerID": "editor", "fileID": "file-1", "offset": -1, "maximumBytes": 1 }),
            json!({ "snapshotID": "abc", "providerID": "editor", "fileID": "file-1", "offset": 0, "maximumBytes": 1, "path": "/private" }),
        ] {
            assert!(host_request(method, Some(&invalid), Some(0), &granted).is_err());
        }
        let list = "lithe/host/keymapImport/listInputs";
        let list_grant = BTreeSet::from([list.to_string()]);
        assert!(host_request(
            list,
            Some(&json!({ "snapshotID": "abc", "providerID": "editor" })),
            Some(0),
            &list_grant
        )
        .is_ok());
        assert!(host_request(
            list,
            Some(&json!({ "snapshotID": "abc", "providerID": "editor", "path": "secret" })),
            Some(0),
            &list_grant
        )
        .is_err());
    }

    /// A message over the bound is cut at a character boundary, never inside
    /// a multi-byte character.
    #[test]
    fn long_failure_messages_are_truncated_on_character_boundaries() {
        let message = format!("{}é", "a".repeat(MAX_MESSAGE_BYTES - 1));
        let failure = invocation_outcome(&json!({ "error": { "code": 1, "message": message } }))
            .expect_err("an error response fails");
        assert_eq!(failure.message, "a".repeat(MAX_MESSAGE_BYTES - 1));
    }

    #[test]
    fn oversized_results_are_invalid() {
        let result = "x".repeat(MAX_RESULT_BYTES);
        let failure = invocation_outcome(&json!({ "result": result }))
            .expect_err("a result over the bound is refused");
        assert_eq!(failure.code, codes::INVALID_RESULT);
    }

    /// A status message is cut at the progress bound before it reaches the
    /// host, at a character boundary; other methods pass untouched.
    #[test]
    fn forwarded_status_messages_are_bounded() {
        let message = format!("{}é", "a".repeat(MAX_MESSAGE_BYTES - 1));
        let params = json!({ "serverID": "metals", "state": "preparing", "message": message });
        let forwarded = forwarded_host_params(LANGUAGE_SERVER_SET_STATUS, Some(&params));
        assert_eq!(forwarded["message"], "a".repeat(MAX_MESSAGE_BYTES - 1));
        assert_eq!(forwarded["state"], "preparing");
        let request = json!({ "method": "x", "message": "m".repeat(MAX_MESSAGE_BYTES + 1) });
        assert_eq!(
            forwarded_host_params(LANGUAGE_SERVER_REQUEST, Some(&request)),
            request
        );
        assert_eq!(
            forwarded_host_params(LANGUAGE_SERVER_REQUEST, None),
            Value::Null
        );
    }

    /// A content scheme is a lowercase RFC 3986 scheme the host does not own.
    #[test]
    fn content_schemes_exclude_host_owned_ones() {
        for accepted in ["metals-library", "rust-std", "a", "x1+y.z"] {
            assert!(is_content_scheme(accepted), "{accepted}");
        }
        for refused in [
            "", "File", "1abc", "-abc", "a b", "a_b", "file", "https", "data", "lithe", "untitled",
        ] {
            assert!(!is_content_scheme(refused), "{refused}");
        }
        assert!(is_content_scheme(&"a".repeat(64)));
        assert!(!is_content_scheme(&"a".repeat(65)));
    }

    /// Only a session granted the language server methods lists them, and a
    /// session without the grant still lists nothing of the service.
    #[test]
    fn capabilities_list_language_server_methods_only_when_granted() {
        let services = |granted: &[&str]| {
            let granted = granted.iter().map(|method| method.to_string()).collect();
            capabilities_answer(0, &granted)["services"].clone()
        };
        assert!(services(&[]).get(HOST_SERVICE_LANGUAGE_SERVER).is_none());
        let granted = services(&[LANGUAGE_SERVER_REQUEST, LANGUAGE_SERVER_SET_STATUS]);
        let entry = &granted[HOST_SERVICE_LANGUAGE_SERVER];
        assert_eq!(
            entry["methods"],
            json!([LANGUAGE_SERVER_REQUEST, LANGUAGE_SERVER_SET_STATUS])
        );
        assert_eq!(
            entry["notifications"],
            json!([
                LANGUAGE_SERVER_DID_CHANGE_STATE,
                LANGUAGE_SERVER_DID_RECEIVE_NOTIFICATION
            ])
        );
    }

    #[test]
    fn the_registry_is_bounded() {
        let current: BTreeSet<String> = (0..MAX_FUNCTIONS)
            .map(|index| format!("f{index}"))
            .collect();
        let error =
            apply_registration(&current, Some(&json!({ "functions": [{ "id": "extra" }] })))
                .expect_err("the session limit holds");
        assert!(error.contains("Too many"));
    }
}
