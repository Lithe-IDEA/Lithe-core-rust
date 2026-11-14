//! `debugAdapters` declarations: consoles, requests, configurations and their bindings.

use super::*;

/// One debug adapter a plugin implements.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugAdapterDeclaration {
    /// Plugin-local ID. The Registry identity is qualified by the plugin ID.
    pub id: String,
    pub(crate) display_name: String,
    /// DAP `initialize.adapterID`; the local ID when absent.
    #[serde(default, rename = "adapterID", skip_serializing_if = "Option::is_none")]
    pub(crate) adapter_id: Option<String>,
    /// Requests the adapter serves; `launch` only when absent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<DebugRequest>,
    /// Automatic matching; absent means explicit selection only.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selectors: Vec<DebugSelector>,
    /// Imported `launch.json` `type` values this adapter answers to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) configuration_types: Vec<String>,
    /// DAP argument defaults and target bindings per request; absent, a start
    /// begins from empty arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configurations: Option<DebugConfigurations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) capabilities: Option<DebugCapabilities>,
    /// What the Debug Console offers while typing a debugger command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) console: Option<DebugConsole>,
    /// DAP extensions the adapter implements; `debugAdapters` 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) features: Option<DebugAdapterFeatures>,
    /// Values the adapter starts with when the user set none; `debugAdapters` 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) defaults: Option<DebugAdapterDefaults>,
    /// How the host starts and reaches the adapter.
    pub launch: DebugLaunch,
}

/// Debugger commands the adapter's console understands. Adapters that answer
/// the DAP `completions` request complete live; this list is for the ones
/// that do not, and a host shows both together.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DebugConsole {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) commands: Vec<DebugConsoleCommand>,
    /// Text the adapter's console reads as "what follows is a debugger
    /// command, not an expression". A host puts it before the typed line when
    /// it asks the adapter to complete, so the adapter proposes commands
    /// instead of expression keywords it would offer for a bare word.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command_prefix: Option<String>,
}

/// One completable command word or phrase, for example `breakpoint set`.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DebugConsoleCommand {
    /// The text inserted when the command is chosen.
    pub(crate) name: String,
    /// One line shown beside the name. Adapter-owned text, shown as written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
}

/// The DAP request that starts a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DebugRequest {
    /// The adapter starts the program it debugs.
    Launch,
    /// The adapter connects to a program that is already running.
    Attach,
}

/// Optional automatic matching: OR between selectors, AND between fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugSelector {
    /// Target kinds that select this adapter, matched exactly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub target_kinds: Vec<String>,
    #[serde(default, rename = "languageIDs", skip_serializing_if = "Vec::is_empty")]
    pub(crate) language_ids: Vec<String>,
}

/// Argument defaults and bindings for each request the adapter serves.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugConfigurations {
    /// Defaults for a `launch` request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<DebugConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) attach: Option<DebugConfiguration>,
}

/// Defaults for the DAP arguments of one request.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugConfiguration {
    /// Adapter-owned default DAP arguments, ordinary JSON data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Map<String, Value>>,
    /// Packaged JSON schema of the final merged arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parameters_schema: Option<String>,
    /// Which target fact fills which missing DAP argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_bindings: Option<DebugTargetBindings>,
    /// Where the program to debug comes from when the target is a source
    /// file. Launch only, and exactly when `program` binds to
    /// `target.executable`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<DebugExecutable>,
}

/// How a host finds the program a source file builds into. The user's own
/// choice in a `file` setting always wins; the candidates are what a host
/// looks for when the user chose nothing, so the setting can show it.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugExecutable {
    /// ID of a `file` setting of the same plugin.
    pub setting: String,
    /// Paths to try in order. Each starts with `${workspaceFolder}/` or
    /// `${fileDirectory}/` and may use `${fileBasenameNoExtension}`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) candidates: Vec<String>,
}

/// Typed projections from the target into known DAP fields. Each field has
/// exactly one legal source, so a binding is a fact, not an expression.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DebugTargetBindings {
    /// Source of the `args` argument: only `target.arguments`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    /// Source of the `env` argument: only `target.environment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Source of the `program` argument: `target.path` or `target.executable`. Launch only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Source of the `processId` argument: only `target.processID`. Attach only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<String>,
    /// Source of the `cwd` argument: only `target.workingDirectory`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Where a remote `host:port` goes. Attach only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) connection: Option<DebugConnectionBinding>,
}

/// Where each fact of a remote target lands in the adapter's attach
/// arguments. Adapters name and nest these differently (`hostName` and `port`,
/// `connect.host`, a single `host:port` string), so unlike the fixed names
/// above each value here is the destination path, not the source fact: `host`
/// reads `target.host`, `port` reads `target.port`, `address` reads both.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DebugConnectionBinding {
    /// Argument that receives the host name or IP literal. Pairs with `port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) host: Option<String>,
    /// Argument that receives the port, as an integer. Pairs with `host`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) port: Option<String>,
    /// Argument that receives `host:port` as one string, with an IPv6 host in
    /// brackets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) address: Option<String>,
    /// How local and remote source roots reach the adapter. Absent when the
    /// adapter has no such argument; the host then does not offer the choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) path_mappings: Option<DebugPathMappingBinding>,
}

/// The ways adapters take source roots: where they go and in what form.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "shape", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum DebugPathMappingBinding {
    /// `[{ <localKey>: …, <remoteKey>: … }, …]` at `argument`.
    #[serde(rename_all = "camelCase")]
    Objects {
        argument: String,
        local_key: String,
        remote_key: String,
    },
    /// The first mapping only, as two strings: `local` and `remote` are the
    /// destination arguments.
    First { local: String, remote: String },
    /// `{ <root>: <other root>, … }` at `argument`, keyed by one side.
    #[serde(rename_all = "camelCase")]
    Dictionary {
        argument: String,
        keyed_by: MappingRoot,
    },
    /// `[[<root>, <other root>], …]` at `argument`, in the declared order.
    Pairs {
        argument: String,
        order: MappingOrder,
    },
}

/// Which side of a mapping a dictionary is keyed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MappingRoot {
    Local,
    Remote,
}

/// Which root comes first in a pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MappingOrder {
    RemoteLocal,
    LocalRemote,
}

impl DebugPathMappingBinding {
    /// Destination paths the binding writes, with the field each belongs to.
    fn destinations(&self) -> Vec<(&'static str, &str)> {
        match self {
            Self::Objects { argument, .. }
            | Self::Dictionary { argument, .. }
            | Self::Pairs { argument, .. } => vec![("pathMappings.argument", argument)],
            Self::First { local, remote } => {
                vec![
                    ("pathMappings.local", local),
                    ("pathMappings.remote", remote),
                ]
            }
        }
    }

    fn key_fields(&self) -> Vec<(&'static str, &str)> {
        match self {
            Self::Objects {
                local_key,
                remote_key,
                ..
            } => vec![
                ("pathMappings.localKey", local_key),
                ("pathMappings.remoteKey", remote_key),
            ],
            Self::First { .. } | Self::Dictionary { .. } | Self::Pairs { .. } => Vec::new(),
        }
    }

    /// How many mappings the adapter can take. A shape that holds one mapping
    /// refuses more rather than keeping the first and dropping the rest.
    pub(crate) fn limit(&self) -> usize {
        match self {
            Self::First { .. } => 1,
            Self::Objects { .. } | Self::Dictionary { .. } | Self::Pairs { .. } => {
                MAX_PATH_MAPPINGS
            }
        }
    }
}

impl DebugConnectionBinding {
    /// Every destination argument path with the binding field it comes from.
    fn destinations(&self) -> Vec<(&'static str, &str)> {
        let mut destinations = Vec::new();
        for (name, path) in [
            ("host", &self.host),
            ("port", &self.port),
            ("address", &self.address),
        ] {
            if let Some(path) = path {
                destinations.push((name, path.as_str()));
            }
        }
        if let Some(mappings) = &self.path_mappings {
            destinations.extend(mappings.destinations());
        }
        destinations
    }

    pub(crate) fn validate(
        &self,
        field: &str,
        defaults: Option<&Map<String, Value>>,
        fixed_names: &[&str],
    ) -> Validation<()> {
        if self.host.is_some() != self.port.is_some() {
            return Err(invalid(
                format!("{field}.host"),
                "host and port are declared together.",
            ));
        }
        if self.host.is_none() && self.address.is_none() {
            return Err(invalid(
                field,
                "A connection binding receives the target through host and port, or address.",
            ));
        }
        let destinations = self.destinations();
        for (name, path) in &destinations {
            if !is_argument_path(path) {
                return Err(invalid(
                    format!("{field}.{name}"),
                    format!(
                        "An argument path is up to {MAX_ARGUMENT_PATH_DEPTH} names joined by dots, each a letter or underscore followed by letters, digits, underscores, or hyphens."
                    ),
                ));
            }
        }
        if let Some(mappings) = &self.path_mappings {
            for (name, key) in mappings.key_fields() {
                if !is_argument_name(key) {
                    return Err(invalid(
                        format!("{field}.{name}"),
                        "A key is a letter or underscore followed by letters, digits, underscores, or hyphens.",
                    ));
                }
            }
            if let DebugPathMappingBinding::Objects {
                local_key,
                remote_key,
                ..
            } = mappings
            {
                if local_key == remote_key {
                    return Err(invalid(
                        format!("{field}.pathMappings.remoteKey"),
                        "The local and remote keys differ.",
                    ));
                }
            }
        }
        // Two facts can never share a destination, and a destination the
        // plugin's own defaults already set would make the default win over
        // the host the user confirmed: the adapter would connect somewhere the
        // confirmation did not name.
        for (index, (name, path)) in destinations.iter().enumerate() {
            if let Some((other, _)) = destinations[..index]
                .iter()
                .find(|(_, other)| paths_overlap(path, other))
            {
                return Err(invalid(
                    format!("{field}.{name}"),
                    format!("The argument path overlaps {other}."),
                ));
            }
            if fixed_names.iter().any(|fixed| paths_overlap(path, fixed)) {
                return Err(invalid(
                    format!("{field}.{name}"),
                    "The argument path overlaps another binding.",
                ));
            }
            if defaults.is_some_and(|defaults| default_occupies(defaults, path)) {
                return Err(invalid(
                    format!("{field}.{name}"),
                    "The argument path is already set by the configuration's own arguments.",
                ));
            }
        }
        Ok(())
    }
}
