//! Adapter capabilities, launch, transport, endpoint and process declarations.

use super::*;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DebugCapabilities {
    /// Standard DAP capability names the host treats as absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) disabled: Vec<String>,
}

/// How the adapter process is started and reached.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugLaunch {
    /// Absent exactly for `tcpLanguageServer`, whose adapter lives inside a
    /// language server the plugin already declares and has no process of its
    /// own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<DebugProcess>,
    pub(crate) transport: DebugTransport,
    /// Required by, and only allowed for, the TCP transports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<DebugEndpoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DebugTransport {
    Stdio,
    TcpAnnounced,
    TcpPort,
    /// The adapter runs inside the plugin's language server, which opens a
    /// loopback port when asked by a command.
    TcpLanguageServer,
}

/// Where a TCP adapter listens. The fields of the other transports must stay
/// absent, which `validate` checks.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugEndpoint {
    /// `tcpLanguageServer`: language ID of a language server of this plugin.
    #[serde(
        default,
        rename = "languageID",
        skip_serializing_if = "Option::is_none"
    )]
    pub language_id: Option<String>,
    /// `tcpLanguageServer`: the `workspace/executeCommand` command that starts
    /// the adapter and answers with its loopback port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    /// `tcpAnnounced`: the stream the adapter prints its endpoint on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stream: Option<String>,
    /// `tcpAnnounced`: the text before the endpoint on that line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prefix: Option<String>,
    /// `tcpAnnounced`: `port` or `hostPort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
    /// `tcpPort`: a fixed loopback port of the adapter process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) port: Option<u16>,
}

/// The adapter process: the launch fields every plugin server process shares,
/// or the runtime Run selected for the target.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum DebugProcess {
    /// One of the plugin's own server processes.
    Server(ServerProcess),
    /// The runtime the selected Run uses for the target.
    SelectedRuntime(SelectedRuntimeProcess),
}

/// Start the adapter on the runtime Run selected for the target, not on a
/// runtime the plugin names a version of.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectedRuntimeProcess {
    pub(crate) launch: SelectedRuntimeLaunch,
    #[serde(default)]
    pub(crate) arguments: Vec<String>,
    #[serde(default)]
    pub(crate) validation_arguments: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SelectedRuntimeLaunch {
    kind: SelectedRuntimeKind,
    pub(crate) tool: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
enum SelectedRuntimeKind {
    #[serde(rename = "runSelectedRuntime")]
    RunSelectedRuntime,
}

impl<'de> Deserialize<'de> for DebugProcess {
    /// Chooses the branch by `launch.kind` and refuses fields a DAP adapter
    /// cannot use, so `ServerProcess` keeps one shape for every owner.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let value = Value::deserialize(deserializer)?;
        let object = value
            .as_object()
            .ok_or_else(|| D::Error::custom("a debug adapter process is an object"))?;
        let selected = object
            .get("launch")
            .and_then(|launch| launch.get("kind"))
            .and_then(Value::as_str)
            == Some("runSelectedRuntime");
        if selected {
            return serde_json::from_value(value)
                .map(Self::SelectedRuntime)
                .map_err(D::Error::custom);
        }
        if let Some(key) = object
            .keys()
            .find(|key| !PROCESS_KEYS.contains(&key.as_str()))
        {
            return Err(D::Error::custom(format!(
                "unknown field `{key}` in a debug adapter process"
            )));
        }
        serde_json::from_value(value)
            .map(Self::Server)
            .map_err(D::Error::custom)
    }
}

impl DebugProcess {
    pub(crate) fn validate(&self, field: &str, context: &DeclarationContext<'_>) -> Validation<()> {
        match self {
            Self::Server(process) => process.validate(
                field,
                context.declared_settings,
                ServerRole::DebugAdapter,
                context.runtime_tools,
            ),
            Self::SelectedRuntime(process) => {
                let tool = process.launch.tool.as_str();
                context
                    .runtime_tools
                    .check(tool, format!("{field}.launch.tool"))?;
                validate_process_arguments(
                    &process.arguments,
                    &process.validation_arguments,
                    context.declared_settings,
                    field,
                )
            }
        }
    }

    /// The runtime tool this adapter needs Run to have selected.
    pub(crate) fn selected_runtime_tool(&self) -> Option<&str> {
        match self {
            Self::SelectedRuntime(process) => Some(process.launch.tool.as_str()),
            Self::Server(_) => None,
        }
    }
}
