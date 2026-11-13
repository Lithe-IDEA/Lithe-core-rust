//! Start facts, arguments, connections and path mappings of a debug session.

use super::*;

/// Facts of one debug start that the manifest cannot know.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartFacts {
    pub(crate) registration: Generation,
    #[serde(default)]
    pub(crate) runtime_selection: Option<Generation>,
    pub(crate) target: TargetFacts,
}

/// A generation the plan was made against and the one current at start.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Generation {
    pub(crate) generation: u64,
    pub(crate) current_generation: u64,
}

impl Generation {
    pub(crate) fn is_stale(self) -> bool {
        self.generation != self.current_generation
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TargetFacts {
    /// Host-validated process arguments and environment; absent differs from empty.
    #[serde(default)]
    pub(crate) arguments: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) environment: Option<std::collections::BTreeMap<String, String>>,
    // Selection by kind happens before a start; the host still states it so a
    // start names what it was prepared for. A `connection` target is also
    // checked against the adapter, see `plan`.
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default, rename = "processID")]
    pub(crate) process_id: Option<u64>,
    #[serde(default)]
    pub(crate) working_directory: Option<String>,
    /// The program the host found for a source file, an absolute path.
    #[serde(default)]
    pub(crate) executable: Option<String>,
    /// Where a remote debuggee listens: a host name or IP literal without
    /// brackets, port, or scheme.
    #[serde(default)]
    pub(crate) host: Option<String>,
    #[serde(default)]
    pub(crate) port: Option<u64>,
    /// Local source roots and where the remote machine has them.
    #[serde(default)]
    pub(crate) path_mappings: Vec<PathMapping>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PathMapping {
    pub(crate) local: String,
    pub(crate) remote: String,
}

/// What a start asks of the adapter: the request and the facts around it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StartRequest {
    pub(crate) request: DebugRequest,
    pub(crate) facts: StartFacts,
    /// A plugin backend's own DAP arguments from `lithe/host/debug/start`, as
    /// `debug.decodeBackendConfiguration` accepted them. They go on top of the
    /// adapter's arguments, so the host sends the result unchanged.
    #[serde(default)]
    pub(crate) configuration: Option<Map<String, Value>>,
}

/// The remote endpoint a start connects to, as the adapter will receive it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartConnection {
    pub(crate) host: String,
    pub(crate) port: u16,
    /// False only when `host` is `localhost` or a loopback IP literal. A name
    /// that resolves to loopback still counts as remote, so a host asks the
    /// user about every endpoint Core cannot prove stays on this machine.
    pub(crate) remote: bool,
}

/// What a start gives the host: the DAP arguments and, for a connection
/// target, the endpoint they carry.
#[derive(Debug, Clone)]
pub struct StartArguments {
    /// DAP arguments to send unchanged with the request.
    pub arguments: Map<String, Value>,
    /// The remote endpoint, present only for a connection target.
    pub connection: Option<StartConnection>,
}

/// The default DAP arguments of a request with each binding filling its
/// argument from the target, unless the defaults already set it, and then a
/// backend's configuration replacing any argument it names.
pub fn start_arguments(
    adapter: &DebugAdapterDeclaration,
    start: &StartRequest,
) -> Validation<StartArguments> {
    let mut started = bound_arguments(adapter, start)?;
    if let Some(configuration) = &start.configuration {
        crate::backend_start::reject_host_keys(configuration)?;
        for (key, value) in configuration {
            started.arguments.insert(key.clone(), value.clone());
        }
    }
    Ok(started)
}

/// The adapter's own arguments for a start: its defaults and target bindings.
fn bound_arguments(
    adapter: &DebugAdapterDeclaration,
    start: &StartRequest,
) -> Validation<StartArguments> {
    plan(adapter, start.request, &start.facts)?;
    let configuration =
        adapter
            .configurations
            .as_ref()
            .and_then(|configurations| match start.request {
                DebugRequest::Launch => configurations.launch.as_ref(),
                DebugRequest::Attach => configurations.attach.as_ref(),
            });
    let mut arguments = configuration
        .and_then(|configuration| configuration.arguments.clone())
        .unwrap_or_default();
    let Some(bindings) =
        configuration.and_then(|configuration| configuration.target_bindings.as_ref())
    else {
        return Ok(StartArguments {
            arguments,
            connection: None,
        });
    };
    let target = &start.facts.target;
    // A binding is the fact path that fills the argument of the same name.
    let mut bind = |name: &str, binding: &Option<String>, value: Option<Value>| {
        if let (Some(_), Some(value)) = (binding, value) {
            arguments.entry(name.to_string()).or_insert(value);
        }
    };
    let program = match bindings.program.as_deref() {
        Some(TARGET_EXECUTABLE) => target.executable.clone(),
        _ => target.path.clone(),
    };
    bind("program", &bindings.program, program.map(Value::from));
    bind(
        "processId",
        &bindings.process_id,
        target.process_id.map(Value::from),
    );
    bind(
        "cwd",
        &bindings.cwd,
        target.working_directory.clone().map(Value::from),
    );
    bind(
        "args",
        &bindings.args,
        target.arguments.as_ref().map(|args| json!(args)),
    );
    bind(
        "env",
        &bindings.env,
        target.environment.as_ref().map(|env| json!(env)),
    );
    let connection = match &bindings.connection {
        Some(binding) => Some(bind_connection(binding, target, &mut arguments)?),
        None => None,
    };
    Ok(StartArguments {
        arguments,
        connection,
    })
}

/// Writes the target's endpoint, and its path mappings when the adapter takes
/// them, to the destinations the binding names.
fn bind_connection(
    binding: &DebugConnectionBinding,
    target: &TargetFacts,
    arguments: &mut Map<String, Value>,
) -> Validation<StartConnection> {
    let endpoint = connection_endpoint(target)?;
    if let (Some(host), Some(port)) = (&binding.host, &binding.port) {
        set_argument(arguments, host, Value::from(endpoint.host.clone()))?;
        set_argument(arguments, port, Value::from(endpoint.port))?;
    }
    if let Some(address) = &binding.address {
        let joined = if endpoint.host.contains(':') {
            format!("[{}]:{}", endpoint.host, endpoint.port)
        } else {
            format!("{}:{}", endpoint.host, endpoint.port)
        };
        set_argument(arguments, address, Value::from(joined))?;
    }
    // `plan` has already checked the mappings against this shape: their number
    // fits it, and a dictionary has no key twice.
    if let (Some(mappings), false) = (&binding.path_mappings, target.path_mappings.is_empty()) {
        let roots = |mapping: &PathMapping, order: MappingOrder| match order {
            MappingOrder::RemoteLocal => (mapping.remote.clone(), mapping.local.clone()),
            MappingOrder::LocalRemote => (mapping.local.clone(), mapping.remote.clone()),
        };
        match mappings {
            DebugPathMappingBinding::Objects {
                argument,
                local_key,
                remote_key,
            } => {
                let entries = target
                    .path_mappings
                    .iter()
                    .map(|mapping| {
                        let mut entry = Map::new();
                        entry.insert(local_key.clone(), Value::from(mapping.local.clone()));
                        entry.insert(remote_key.clone(), Value::from(mapping.remote.clone()));
                        Value::Object(entry)
                    })
                    .collect();
                set_argument(arguments, argument, Value::Array(entries))?;
            }
            DebugPathMappingBinding::First { local, remote } => {
                for first in target.path_mappings.iter().take(1) {
                    set_argument(arguments, local, Value::from(first.local.clone()))?;
                    set_argument(arguments, remote, Value::from(first.remote.clone()))?;
                }
            }
            DebugPathMappingBinding::Dictionary { argument, keyed_by } => {
                let order = match keyed_by {
                    MappingRoot::Remote => MappingOrder::RemoteLocal,
                    MappingRoot::Local => MappingOrder::LocalRemote,
                };
                let entries = target
                    .path_mappings
                    .iter()
                    .map(|mapping| {
                        let (key, value) = roots(mapping, order);
                        (key, Value::from(value))
                    })
                    .collect();
                set_argument(arguments, argument, Value::Object(entries))?;
            }
            DebugPathMappingBinding::Pairs { argument, order } => {
                let entries = target
                    .path_mappings
                    .iter()
                    .map(|mapping| {
                        let (first, second) = roots(mapping, *order);
                        Value::Array(vec![Value::from(first), Value::from(second)])
                    })
                    .collect();
                set_argument(arguments, argument, Value::Array(entries))?;
            }
        }
    }
    Ok(endpoint)
}

/// Sets the argument at a dotted path, creating the objects on the way. A
/// declaration cannot leave anything at the path (see
/// `DebugConnectionBinding::validate`), so an occupied path is a bug in the
/// declaration that slipped past it and is refused rather than overwritten.
fn set_argument(arguments: &mut Map<String, Value>, path: &str, value: Value) -> Validation<()> {
    let occupied = || {
        PluginRejection::at(
            "debug.arguments.invalid",
            path,
            "The adapter's arguments already hold something at this path.",
        )
    };
    let mut current = arguments;
    let mut names = path.split('.').peekable();
    while let Some(name) = names.next() {
        if names.peek().is_none() {
            return match current.entry(name.to_string()) {
                serde_json::map::Entry::Vacant(slot) => {
                    slot.insert(value);
                    Ok(())
                }
                serde_json::map::Entry::Occupied(_) => Err(occupied()),
            };
        }
        let next = current
            .entry(name.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(next) = next else {
            return Err(occupied());
        };
        current = next;
    }
    Ok(())
}

/// The endpoint a connection target names, checked.
pub(crate) fn connection_endpoint(target: &TargetFacts) -> Validation<StartConnection> {
    let invalid_target = |field: &'static str, message: &str| {
        PluginRejection::at("debug.target.invalid", field, message)
    };
    let Some(host) = target.host.as_deref() else {
        return Err(invalid_target(
            "target.host",
            "The target has no host, which the adapter's configuration reads.",
        ));
    };
    if let Some(problem) = host_problem(host) {
        return Err(invalid_target("target.host", problem));
    }
    let port = target
        .port
        .ok_or_else(|| {
            invalid_target(
                "target.port",
                "The target has no port, which the adapter's configuration reads.",
            )
        })
        .and_then(|port| {
            u16::try_from(port)
                .ok()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid_target("target.port", "A port is 1-65535."))
        })?;
    Ok(StartConnection {
        host: host.to_string(),
        port,
        remote: !is_loopback_host(host),
    })
}

/// Why a string is not a host to connect to, or `None` when it is one: a DNS
/// name or an IP literal, without scheme, credentials, port, path, brackets,
/// or whitespace. Non-ASCII names are written in punycode; an IPv6 zone is not
/// supported.
pub(crate) fn host_problem(host: &str) -> Option<&'static str> {
    if host.is_empty() || host.len() > MAX_HOST {
        return Some("A host is 1-253 characters.");
    }
    if host.parse::<IpAddr>().is_ok() {
        return None;
    }
    if host.contains(':') {
        return Some("A host is a name or an IP address, without a scheme, port, or brackets.");
    }
    if host
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return Some("A numeric host is a dotted IPv4 address.");
    }
    let labels_valid = host.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= MAX_HOST_LABEL
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    });
    if labels_valid {
        None
    } else {
        Some("A host name is dot-separated labels of letters, digits, and hyphens.")
    }
}

/// Whether Core can tell from the text alone that the host is this machine.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// Checks the path mappings the host sends along with a remote target against
/// the shape the adapter takes them in.
pub(crate) fn check_path_mappings(
    mappings: &[PathMapping],
    shape: &DebugPathMappingBinding,
) -> Validation<()> {
    let invalid_mapping = |message: String| {
        PluginRejection::at("debug.target.invalid", "target.pathMappings", message)
    };
    // A shape that holds fewer mappings than the user gave would start a
    // session with some source lines silently unmapped.
    if mappings.len() > shape.limit() {
        return Err(invalid_mapping(match shape.limit() {
            1 => "The adapter takes one path mapping.".to_string(),
            limit => format!("A target has up to {limit} path mappings."),
        }));
    }
    for mapping in mappings {
        let sound = |root: &str| {
            !root.is_empty()
                && root.chars().count() <= MAX_PATH_MAPPING_ROOT
                && !root.chars().any(char::is_control)
        };
        if !sound(&mapping.local) || !sound(&mapping.remote) {
            return Err(invalid_mapping(format!(
                "A path mapping root is one line of 1-{MAX_PATH_MAPPING_ROOT} characters."
            )));
        }
        // The local root is on this machine, so it must be a full path; the
        // remote root is the other machine's and may follow other conventions.
        if !std::path::Path::new(&mapping.local).is_absolute() {
            return Err(invalid_mapping(
                "A path mapping's local root is an absolute path.".to_string(),
            ));
        }
    }
    // A dictionary holds one value per key; two mappings from the same root
    // cannot both be sent, and keeping either would drop the other unseen.
    if let DebugPathMappingBinding::Dictionary { keyed_by, .. } = shape {
        let mut keys = HashSet::new();
        for mapping in mappings {
            let key = match keyed_by {
                MappingRoot::Local => &mapping.local,
                MappingRoot::Remote => &mapping.remote,
            };
            if !keys.insert(key.as_str()) {
                return Err(invalid_mapping(format!(
                    "The adapter takes one path mapping per {} root.",
                    match keyed_by {
                        MappingRoot::Local => "local",
                        MappingRoot::Remote => "remote",
                    }
                )));
            }
        }
    }
    Ok(())
}
