//! Typed handler inputs and outputs for the versioned toolchain provider protocol.

use crate::Cancelled;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub(crate) const METHOD: &str = "toolchains/query";
pub(crate) const CAPABILITY: &str = "litheToolchains";
pub(crate) const MAX_PROVIDERS: usize = 8;
pub(crate) const MAX_RUNNING_QUERIES: usize = 8;
pub(crate) const MAX_REPLY_BYTES: usize = 1024 * 1024;
const MAX_INSTALLATIONS: usize = 512;
const MAX_STATEMENTS: usize = 256;
const MAX_WATCH_PATHS: usize = 256;

/// The `requires.toolchainProviders` version declared by this backend's manifest.
/// Version 0 remains the default so upgrading the SDK does not change a plugin's
/// protocol. Version 1 separates toolchain failures from relayed function errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolchainProviderVersion {
    /// The published protocol, including the legacy -32070/-32071 errors.
    V0,
    /// The same query protocol with permission/authority errors -32080/-32081.
    V1,
}

impl ToolchainProviderVersion {
    pub(crate) fn number(self) -> u64 {
        match self {
            Self::V0 => 0,
            Self::V1 => 1,
        }
    }
}

/// A query routed to a provider selected during initialization.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolchainQuery {
    #[serde(rename = "operationID")]
    pub operation_id: String,
    pub generation: u64,
    #[serde(rename = "providerID")]
    pub provider_id: String,
    authority: Authority,
    pub context: Option<ToolchainQueryContext>,
    pub workspace: Option<ToolchainWorkspace>,
    pub tools: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Authority {
    kind: String,
    id: Option<String>,
}

/// Identity and generation of the host workspace context.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainQueryContext {
    #[serde(rename = "contextID")]
    pub context_id: String,
    pub generation: u64,
}

/// The opened root and the workspace-relative directory being resolved.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainWorkspace {
    #[serde(rename = "rootID")]
    pub root_id: String,
    pub root: String,
    pub directory: String,
}

/// Facts a provider reports. The host obtains its own filesystem observations
/// and Core validates them before using these facts for resolution.
#[derive(Debug, Default, Serialize)]
pub struct ToolchainQueryResult {
    pub installations: Vec<ToolchainInstallation>,
    pub statements: Vec<ToolchainStatement>,
    pub watch: Vec<String>,
}

/// A discovered installation, with a native home path.
#[derive(Debug, Serialize)]
pub struct ToolchainInstallation {
    pub tool: String,
    pub version: String,
    pub home: String,
}

/// An environment or version requirement. Use `installation: None` to report
/// a requirement that could not be satisfied.
#[derive(Debug, Serialize)]
pub struct ToolchainStatement {
    pub tool: String,
    pub scope: ToolchainScope,
    pub source: String,
    pub request: String,
    pub installation: Option<String>,
    pub kind: ToolchainStatementKind,
    pub reader: String,
}

/// Scope of a version selection or declaration.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolchainScope {
    Project,
    Global,
}

/// Whether the provider read an environment selection or a version declaration.
#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolchainStatementKind {
    Environment,
    Declaration,
}

/// A toolchain query failure with the protocol's stable error code. Missing
/// installations and unmet versions belong in a successful query result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolchainQueryError {
    MethodUnavailable,
    InvalidParams,
    Cancelled,
    PermissionDenied,
    AuthorityUnsupported,
    Timeout,
    ProviderUnavailable,
    InvalidReply,
    LimitExceeded,
    UnsafePath,
    OperationExpired,
}

impl From<Cancelled> for ToolchainQueryError {
    fn from(_: Cancelled) -> Self {
        Self::Cancelled
    }
}

impl ToolchainQueryError {
    pub(crate) fn response(
        self,
        operation: &str,
        provider: &str,
        version: ToolchainProviderVersion,
    ) -> Value {
        let (number, code, message) = match self {
            Self::MethodUnavailable => (
                -32601,
                "methodUnavailable",
                "The provider was not negotiated.",
            ),
            Self::InvalidParams => (
                -32602,
                "invalidParams",
                "Invalid toolchain query parameters.",
            ),
            Self::Cancelled => (-32800, "cancelled", "The toolchain query was cancelled."),
            Self::PermissionDenied => (
                match version {
                    ToolchainProviderVersion::V0 => -32070,
                    ToolchainProviderVersion::V1 => -32080,
                },
                "permissionDenied",
                "The toolchain query was not permitted.",
            ),
            Self::AuthorityUnsupported => (
                match version {
                    ToolchainProviderVersion::V0 => -32071,
                    ToolchainProviderVersion::V1 => -32081,
                },
                "authorityUnsupported",
                "The provider accepts only local queries.",
            ),
            Self::Timeout => (-32072, "timeout", "The toolchain query timed out."),
            Self::ProviderUnavailable => (
                -32073,
                "providerUnavailable",
                "The toolchain provider is unavailable.",
            ),
            Self::InvalidReply => (-32074, "invalidReply", "The toolchain response is invalid."),
            Self::LimitExceeded => (
                -32075,
                "limitExceeded",
                "The toolchain query exceeded its limits.",
            ),
            Self::UnsafePath => (-32076, "unsafePath", "The toolchain path is unsafe."),
            Self::OperationExpired => (
                -32077,
                "operationExpired",
                "The toolchain query is no longer current.",
            ),
        };
        json!({
            "code": number,
            "message": message,
            "data": {
                "code": format!("plugin.toolchain.{code}"),
                "operationID": identity(operation).then_some(operation),
                "providerID": provider_id(provider).then_some(provider),
            },
        })
    }
}

impl ToolchainQuery {
    pub(crate) fn parse(params: &Value) -> Result<Self, ToolchainQueryError> {
        let query = Self::deserialize(params).map_err(|_| ToolchainQueryError::InvalidParams)?;
        if !params
            .as_object()
            .is_some_and(|p| p.contains_key("context") && p.contains_key("workspace"))
            || !identity(&query.operation_id)
            || !provider_id(&query.provider_id)
            || query.generation == 0
            || query.tools.is_empty()
            || query.tools.len() > 64
            || !query.tools.iter().all(|tool| tool_id(tool))
            || !query.tools.windows(2).all(|pair| pair[0] < pair[1])
            || query.workspace.is_some() && query.context.is_none()
            || query
                .context
                .as_ref()
                .is_some_and(|context| context.generation == 0 || !identity(&context.context_id))
        {
            return Err(ToolchainQueryError::InvalidParams);
        }
        if query.authority.kind == "remote" && query.authority.id.as_deref().is_some_and(identity) {
            return Err(ToolchainQueryError::AuthorityUnsupported);
        }
        if query.authority.kind != "local" || params["authority"].get("id").is_some() {
            return Err(ToolchainQueryError::InvalidParams);
        }
        if let Some(workspace) = &query.workspace {
            let directory = &workspace.directory;
            if !identity(&workspace.root_id)
                || !bounded(&workspace.root, 4096)
                || !bounded(directory, 2048)
                || (directory != "."
                    && (directory.contains(['\\', ':'])
                        || directory
                            .split('/')
                            .any(|part| matches!(part, "" | "." | ".."))))
            {
                return Err(ToolchainQueryError::InvalidParams);
            }
        }
        Ok(query)
    }

    pub(crate) fn answer(&self, facts: ToolchainQueryResult) -> Result<Value, ToolchainQueryError> {
        if facts.installations.len() > MAX_INSTALLATIONS
            || facts.statements.len() > MAX_STATEMENTS
            || facts.watch.len() > MAX_WATCH_PATHS
        {
            return Err(ToolchainQueryError::LimitExceeded);
        }
        let mut result =
            serde_json::to_value(facts).map_err(|_| ToolchainQueryError::InvalidReply)?;
        result["operationID"] = json!(self.operation_id);
        result["generation"] = json!(self.generation);
        result["providerID"] = json!(self.provider_id);
        Ok(result)
    }
}

fn bounded(value: &str, maximum: usize) -> bool {
    !value.is_empty() && value.len() <= maximum && !value.chars().any(char::is_control)
}

fn identity(value: &str) -> bool {
    bounded(value, 128)
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

pub(crate) fn provider_id(value: &str) -> bool {
    bounded(value, 64)
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn tool_id(value: &str) -> bool {
    value.len() <= 192
        && match value.split_once(':') {
            Some((namespace, local)) => {
                provider_id(local)
                    && namespace.split(['.', '-']).all(|part| {
                        !part.is_empty()
                            && part
                                .bytes()
                                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                    })
            }
            None => provider_id(value),
        }
}
