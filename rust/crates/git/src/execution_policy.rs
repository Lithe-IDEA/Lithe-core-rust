//! Request-scoped execution choices and deterministic temporary configuration.

use super::console::QueryCategory;
use super::fetch::GitFetchOptions;
use lithe_protocol::{CoreError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

/// Native execution preferences supplied by the host, never copied into a project file.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct GitExecutionOptions {
    /// Null uses Git from the host PATH. A selected executable is an absolute native path.
    pub executable: Option<String>,
    /// Host can answer AskPass challenges for this user-initiated operation.
    pub interactive: bool,
    /// Explicit helper preference; noninteractive callers fail if credentials are unavailable.
    pub use_credential_helper: bool,
    /// Application defaults; explicit invocation choices and repository overrides take priority.
    pub fetch_defaults: GitFetchOptions,
    /// Request per-remote Fetch execution and structured outcomes.
    pub detailed_fetch: bool,
    /// Explicit operation provenance; unknown requests must never be deduplicated.
    pub source: super::console::Source,
    /// The query categories a console records: their internal read-only queries emit
    /// execution events like visible commands. Empty by default, as IDEA leaves every
    /// read-only handler silent. Authentication and interactivity stay with visible commands;
    /// a query never prompts.
    pub console_queries: Vec<QueryCategory>,
    /// What the request's queries read, named by the host for reads the request command
    /// alone does not tell apart, such as a reload after an operation.
    pub query_category: Option<QueryCategory>,
    /// The request command's own category, which the dispatcher installs with
    /// [`GitExecutionOptions::for_command`]; it is never read from the request.
    #[serde(skip)]
    pub command_category: Option<QueryCategory>,
    /// Whether the request's workspace is trusted. Any Git command reads the repository's
    /// own configuration, which can name programs Git runs (`core.fsmonitor`, hooks,
    /// filters, `diff.external`), so `false` refuses to start Git, locally or on a remote
    /// host, with [`ErrorCode::GitUntrustedWorkspace`]. A request outside any workspace,
    /// such as a clone, has no trust state, so the field defaults to `true`; a host that
    /// tracks workspace trust sends `false` with every request of an untrusted workspace,
    /// including file requests whose ignore checks ask Git.
    pub project_code_allowed: bool,
}
impl Default for GitExecutionOptions {
    fn default() -> Self {
        Self {
            executable: None,
            interactive: false,
            use_credential_helper: true,
            fetch_defaults: GitFetchOptions::default(),
            detailed_fetch: false,
            source: super::console::Source::Unknown,
            console_queries: Vec::new(),
            query_category: None,
            command_category: None,
            project_code_allowed: true,
        }
    }
}
impl GitExecutionOptions {
    /// These options for a request of `command`, whose queries default to its category.
    pub fn for_command(mut self, command: &str) -> Self {
        self.command_category = super::console::category::of_command(command);
        self
    }
}

thread_local! { static OPTIONS: RefCell<GitExecutionOptions> = RefCell::new(GitExecutionOptions::default()); }

/// Restores the previous policy even if a nested request fails.
pub struct Scope(GitExecutionOptions);
impl Scope {
    /// Installs `options` (or the defaults) for the current thread until the scope drops.
    /// Rejects a relative executable path and invalid fetch defaults.
    pub fn begin(options: Option<GitExecutionOptions>) -> Result<Self, CoreError> {
        let options = options.unwrap_or_default();
        if options.executable.as_ref().is_some_and(|path| {
            !std::path::Path::new(path).is_absolute() || path.contains(['\0', '\r', '\n'])
        }) {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Git executable must be an absolute path",
            ));
        }
        super::fetch::arguments(&options.fetch_defaults)?;
        Ok(Self(OPTIONS.with(|current| current.replace(options))))
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        OPTIONS.with(|current| {
            current.replace(self.0.clone());
        });
    }
}

pub(super) fn current() -> GitExecutionOptions {
    OPTIONS.with(|current| current.borrow().clone())
}

/// Refuses to start Git for a request of an untrusted workspace. Every local Git process
/// is built by `process::git_process` and every remote one by `remote_process::parameters`;
/// both call this first, so no path starts Git in safe mode.
pub(super) fn require_project_code() -> Result<(), CoreError> {
    if OPTIONS.with(|current| current.borrow().project_code_allowed) {
        return Ok(());
    }
    Err(CoreError::new(
        ErrorCode::GitUntrustedWorkspace,
        "Git does not run in safe mode; trust the workspace to use Git",
    ))
}

/// The provenance of the running request's Git processes: the Git policy's, or the request
/// envelope's when the policy names none.
pub(super) fn source() -> super::console::Source {
    match current().source {
        super::console::Source::Unknown => lithe_request_context::trace::current().source,
        source => source,
    }
}

/// The category of the running request's internal queries: the one the host named, or else
/// the request command's. A `background` request without a named category is an automatic
/// refresh or watcher, whose queries belong to no category.
pub(super) fn query_category() -> Option<QueryCategory> {
    let options = current();
    options.query_category.or_else(|| {
        (source() != super::console::Source::Background)
            .then_some(options.command_category)
            .flatten()
    })
}

/// Whether the running request's internal queries are recorded: their category is one the
/// console records. Queries of no category, those of an automatic refresh or watcher, run too
/// often to record.
pub(super) fn reports_queries() -> bool {
    query_category().is_some_and(|category| current().console_queries.contains(&category))
}

/// Whether the running request's unrecorded queries still report a failure: a request that
/// is not `background`, as every such query did before the console chose categories.
/// Background refreshes and watchers stay silent even when a read fails.
pub(super) fn reports_query_failures() -> bool {
    source() != super::console::Source::Background
}

/// Environment configuration and scope provenance require Git 2.31 or newer.
pub(super) fn validate_version(version: &str) -> Result<(), CoreError> {
    let supported = version
        .strip_prefix("git version ")
        .and_then(|number| {
            let mut parts = number.split('.');
            Some((
                parts.next()?.parse::<u32>().ok()?,
                parts.next()?.parse::<u32>().ok()?,
            ))
        })
        .is_some_and(|number| number >= (2, 31));
    if supported {
        Ok(())
    } else {
        Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Git 2.31 or newer is required; select a newer executable in Git settings",
        ))
    }
}

/// Temporary values are scoped to a subprocess and visible in execution diagnostics.
pub(super) fn temporary_config() -> Vec<(String, String)> {
    let mut values = vec![
        ("color.ui".into(), "false".into()),
        ("core.quotepath".into(), "false".into()),
        ("log.showSignature".into(), "false".into()),
    ];
    if !current().use_credential_helper {
        values.push(("credential.helper".into(), String::new()));
    }
    values
}

/// Transfer commands emit progress even when their stderr is a pipe.
pub(super) fn arguments(arguments: &[String]) -> Vec<String> {
    let mut result = arguments.to_vec();
    if let Some(index) = command_index(arguments) {
        if matches!(
            arguments[index].as_str(),
            "fetch" | "pull" | "push" | "clone"
        ) && !arguments
            .iter()
            .any(|value| value == "--progress" || value == "--no-progress")
        {
            result.insert(index + 1, "--progress".into());
        }
    }
    result
}

pub(super) fn command_index(arguments: &[String]) -> Option<usize> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        if matches!(
            argument.as_str(),
            "-c" | "-C" | "--git-dir" | "--work-tree" | "--config-env"
        ) {
            index += 2;
        } else if argument.starts_with('-') {
            index += 1;
        } else {
            return Some(index);
        }
    }
    None
}

/// Separates the readable subcommand from the global options folded by native consoles.
/// Raw process arguments remain unchanged in the event and command response.
pub(super) fn console_arguments(
    arguments: &[String],
    temporary_config: &[(String, String)],
) -> (Vec<String>, Vec<String>) {
    let command = command_index(arguments).unwrap_or(0);
    let mut global = temporary_config
        .iter()
        .flat_map(|(key, value)| ["-c".into(), format!("{key}={value}")])
        .collect::<Vec<String>>();
    // Include every original global argument in the disclosure. In particular,
    // do not mistake a commit message containing "-c" for a configuration flag.
    global.extend_from_slice(&arguments[..command]);
    (arguments[command..].to_vec(), global)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_projection_preserves_raw_arguments_and_folds_only_global_options() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/git/console-command-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let arguments: Vec<String> = serde_json::from_value(case["arguments"].clone()).unwrap();
            let configuration: Vec<(String, String)> =
                serde_json::from_value(case["temporaryConfig"].clone()).unwrap();
            let (display, global) = console_arguments(&arguments, &configuration);
            assert_eq!(
                serde_json::json!(display),
                case["displayArguments"],
                "{}",
                case["name"]
            );
            assert_eq!(
                serde_json::json!(global),
                case["globalArguments"],
                "{}",
                case["name"]
            );
            assert_eq!(serde_json::json!(arguments), case["arguments"]);
        }
    }

    #[test]
    fn version_capabilities_reject_old_or_unknown_executables() {
        for version in [
            "git version 2.31.0",
            "git version 2.50.1.windows.1",
            "git version 2.39.5 (Apple Git-154)",
        ] {
            assert!(validate_version(version).is_ok());
        }
        for version in ["git version 2.30.9", "unknown", "git version 2.bad"] {
            assert!(validate_version(version).is_err());
        }
    }

    #[test]
    fn named_defaults_are_scoped_and_do_not_duplicate_transfer_progress() {
        let before = current();
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/git/execution-policy-v1.json"
        ))
        .unwrap();
        assert_eq!(
            serde_json::to_value(temporary_config()).unwrap(),
            fixture["temporaryConfig"]
        );
        {
            let _scope = Scope::begin(Some(GitExecutionOptions {
                interactive: true,
                use_credential_helper: false,
                ..Default::default()
            }))
            .unwrap();
            assert_eq!(temporary_config().last().unwrap().0, "credential.helper");
            let args =
                ["-c", "core.editor=true", "push", "--progress", "origin"].map(str::to_string);
            assert_eq!(arguments(&args), args);
            assert_eq!(
                arguments(&["clone".into(), "--".into(), "remote".into()])[1],
                "--progress"
            );
        }
        assert_eq!(
            current().use_credential_helper,
            before.use_credential_helper
        );
        assert!(Scope::begin(Some(GitExecutionOptions {
            executable: Some("relative/git".into()),
            ..Default::default()
        }))
        .is_err());
    }

    /// An untrusted request builds no Git process; without the field, as for a clone that
    /// belongs to no workspace, Git runs.
    #[test]
    fn untrusted_requests_build_no_git_process() {
        let options: GitExecutionOptions = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(options.project_code_allowed);
        assert!(crate::process::git_process().is_ok());
        {
            let _scope = Scope::begin(Some(
                serde_json::from_value(serde_json::json!({ "projectCodeAllowed": false })).unwrap(),
            ))
            .unwrap();
            let refused = crate::process::git_process().unwrap_err();
            assert!(matches!(refused.code, ErrorCode::GitUntrustedWorkspace));
            assert!(
                crate::ignored_paths(std::path::Path::new("."), &["x".into()])
                    .unwrap()
                    .is_none()
            );
        }
        assert!(crate::process::git_process().is_ok());
    }

    /// The shared cases fix each command's category and which queries a console records.
    #[test]
    fn console_queries_follow_the_named_or_command_category() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/git/console-queries-v1.json"
        ))
        .unwrap();
        // Hosts read the whole list from the fixture. The match fails to compile when a
        // category is added, so the list cannot fall behind the enum.
        use super::super::console::QueryCategory::*;
        let categories: Vec<super::super::console::QueryCategory> =
            serde_json::from_value(fixture["queryCategories"].clone()).unwrap();
        let positions = categories.iter().map(|category| match category {
            References => 0,
            Reload => 1,
            DiffRefresh => 2,
            Diff => 3,
            Log => 4,
            LogPathFilter => 5,
            Annotate => 6,
            Panels => 7,
            Checks => 8,
        });
        assert!(positions.eq(0..9), "{categories:?}");
        for (command, category) in fixture["categories"].as_object().unwrap() {
            assert_eq!(
                serde_json::json!(super::super::console::category::of_command(command)),
                *category,
                "{command}"
            );
        }
        for case in fixture["cases"].as_array().unwrap() {
            let options: GitExecutionOptions =
                serde_json::from_value(case["gitExecution"].clone()).unwrap();
            let _scope =
                Scope::begin(Some(options.for_command(case["command"].as_str().unwrap()))).unwrap();
            assert_eq!(
                reports_queries(),
                case["reported"].as_bool().unwrap(),
                "{}",
                case["name"]
            );
        }
        assert!(!reports_queries());
    }
}
