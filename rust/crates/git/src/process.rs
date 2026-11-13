//! Git subprocesses: local and remote invocation, captured streams, the per-operation
//! invocation trace and the generic `git.command` entry points.

use super::*;

/// Raw Git process streams kept separate for machine-readable consumers.
pub(crate) struct GitProcessOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: i32,
}

impl GitProcessOutput {
    pub(crate) fn into_command_response(self, arguments: &[String]) -> GitCommandResponse {
        let stdout = String::from_utf8_lossy(&self.stdout).to_string();
        let stderr = String::from_utf8_lossy(&self.stderr).to_string();
        let invocation = GitCommandInvocation {
            arguments: arguments.to_vec(),
            stdout: stdout.clone(),
            stderr: stderr.clone(),
            exit_code: self.exit_code,
        };
        let output = format!("{stdout}{stderr}");
        GitCommandResponse {
            arguments: arguments.to_vec(),
            output,
            stdout,
            stderr,
            exit_code: self.exit_code,
            invocations: vec![invocation],
            operation_error: None,
            stash_restore: None,
            tag_deletion: None,
            branch_deletion: None,
            history_rewrite: None,
            outcome_authoritative: false,
            warnings: Vec::new(),
        }
    }
}

thread_local! {
    static GIT_INVOCATION_TRACE: RefCell<Option<Vec<GitCommandInvocation>>> = const {
        RefCell::new(None)
    };
}

pub(crate) fn with_git_invocation_trace(
    operation: impl FnOnce() -> Result<GitCommandResponse, CoreError>,
) -> Result<GitCommandResponse, CoreError> {
    let previous = GIT_INVOCATION_TRACE.with(|trace| trace.replace(Some(Vec::new())));
    let result = operation();
    let invocations = GIT_INVOCATION_TRACE
        .with(|trace| trace.replace(previous))
        .unwrap_or_default();

    match result {
        Ok(mut response) => {
            response.invocations = invocations;
            synchronize_final_invocation(&mut response);
            Ok(response)
        }
        Err(error) if !invocations.is_empty() => {
            // A composite Git operation may complete subprocesses before a
            // follow-up probe fails. Preserve those diagnostics as command data
            // instead of replacing them with an empty error result.
            let mut response = failed_git_result(error);
            response.invocations = invocations;
            synchronize_final_invocation(&mut response);
            Ok(response)
        }
        Err(error) => Err(error),
    }
}

pub(crate) fn synchronize_final_invocation(response: &mut GitCommandResponse) {
    // Recovery creation and post-mutation cleanup are traced subprocesses, but
    // must not replace the authoritative history mutation's success or failure.
    if response.history_rewrite.is_some() || response.outcome_authoritative {
        return;
    }
    if response.exit_code == 0 && !response.warnings.is_empty() {
        // The mutation already succeeded and a later reconciliation step only
        // produced a warning. Keep the authoritative success summary while the
        // failed follow-up remains available in `invocations` and `warnings`.
        return;
    }
    let Some(final_invocation) = response.invocations.last() else {
        return;
    };
    response.arguments = final_invocation.arguments.clone();
    response.stdout = final_invocation.stdout.clone();
    response.stderr = final_invocation.stderr.clone();
    response.output = format!("{}{}", response.stdout, response.stderr);
    response.exit_code = final_invocation.exit_code;
}

pub(crate) fn record_git_invocation(response: &GitCommandResponse) {
    let Some(invocation) = response.invocations.first().cloned() else {
        return;
    };
    GIT_INVOCATION_TRACE.with(|trace| {
        if let Some(invocations) = trace.borrow_mut().as_mut() {
            invocations.push(invocation);
        }
    });
}

/// Executes an argument-based Git command after validating the workspace root.
pub fn command(request: GitCommandRequest) -> Result<GitCommandResponse, CoreError> {
    with_git_invocation_trace(|| {
        let root = validate_root(&request.root)?;
        // Raw compatibility commands can also mutate refs and configuration.
        // Share the typed writers' fail-fast lease instead of waiting on a mutex
        // that cannot observe the request's cancellation or deadline.
        let _lease = rewrite::RewriteLease::acquire(&root)?;
        execute_git(&root, &request.arguments, request.input)
    })
}

pub(crate) fn readonly_command(
    request: GitCommandRequest,
) -> Result<GitCommandResponse, CoreError> {
    let root = validate_root(&request.root)?;
    execute_git_readonly(&root, &request.arguments, request.input)
}

/// Resolves the containing repository root without participating in write coordination.
pub fn discover_repository_root(
    request: GitRepositoryRootRequest,
) -> Result<Option<String>, CoreError> {
    let root = validate_root(&request.root)?;
    Ok(discover_containing_repository(Path::new(&root))?
        .map(|path| path.to_string_lossy().replace('\\', "/")))
}

/// Runs one Git invocation the user sees in the console, under the request's
/// execution policy and cancellation scope.
pub fn execute_git(
    root: &str,
    arguments: &[String],
    input: Option<String>,
) -> Result<GitCommandResponse, CoreError> {
    execute_git_with_options(root, arguments, input, false)
}

/// Runs one internal read-only Git query with optional locks disabled. It never
/// prompts, and reaches the console only when the request sets `reportQueries`.
pub fn execute_git_readonly(
    root: &str,
    arguments: &[String],
    input: Option<String>,
) -> Result<GitCommandResponse, CoreError> {
    execute_git_readonly_with_environment(root, arguments, input, &[])
}

pub(crate) fn execute_git_readonly_with_environment(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    environment: &[(String, String)],
) -> Result<GitCommandResponse, CoreError> {
    // Internal queries follow IDEA's silent handler policy. Keep their result for
    // the owning workflow without filling the user's console with plumbing output.
    let arguments = execution_policy::arguments(arguments);
    let response = capture_git_with_environment(root, &arguments, input, true, environment)?
        .into_command_response(&arguments);
    record_git_invocation(&response);
    Ok(response)
}

fn execute_git_with_options(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    disable_optional_locks: bool,
) -> Result<GitCommandResponse, CoreError> {
    execute_git_with_environment(root, arguments, input, disable_optional_locks, &[])
}

pub(crate) fn execute_git_with_environment(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    disable_optional_locks: bool,
    environment: &[(String, String)],
) -> Result<GitCommandResponse, CoreError> {
    let arguments = execution_policy::arguments(arguments);
    let mut retry_scope = None;
    for attempt in 0..3 {
        let output = capture_git_process(
            root,
            &arguments,
            input.clone(),
            disable_optional_locks,
            environment,
            true,
        )?;
        let response = output.into_command_response(&arguments);
        record_git_invocation(&response);
        let authentication_failed = [
            "Authentication failed",
            "could not read Username",
            "could not read Password",
            "Permission denied (publickey",
        ]
        .iter()
        .any(|marker| response.stderr.contains(marker));
        let transfer = execution_policy::command_index(&arguments).is_some_and(|index| {
            matches!(
                arguments[index].as_str(),
                "fetch" | "pull" | "push" | "clone"
            )
        });
        if response.exit_code == 0
            || !authentication_failed
            || !transfer
            || !execution_policy::current().interactive
            || attempt == 2
        {
            return Ok(response);
        }
        let confirmation =
            lithe_git_host::authentication::Confirmation::new().map_err(|error| {
                CoreError::new(
                    ErrorCode::ProcessFailed,
                    "Could not request Git authentication retry",
                )
                .with_details(error.to_string())
            })?;
        execution_events::emit(
            serde_json::json!({ "type": "authentication", "requestId": confirmation.request_id,
            "prompt": "Git authentication failed. Retry with credentials entered in Lithe, bypassing the credential helper for this retry?",
            "secret": false, "retry": true, "attempt": attempt + 2, "workingDirectory": root }),
        );
        let retry = confirmation
            .wait(|| lithe_protocol::cancellation::check().is_err())
            .map_err(|_| CoreError::new(ErrorCode::TimedOut, "Git authentication retry expired"))?;
        lithe_protocol::cancellation::check()?;
        if !retry {
            return Ok(response);
        }
        drop(retry_scope.take());
        let mut options = execution_policy::current();
        options.use_credential_helper = false;
        retry_scope = Some(execution_policy::Scope::begin(Some(options))?);
    }
    unreachable!("bounded Git attempts always return a response")
}

pub(crate) fn capture_git_with_options(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    disable_optional_locks: bool,
) -> Result<GitProcessOutput, CoreError> {
    capture_git_with_environment(root, arguments, input, disable_optional_locks, &[])
}

pub(crate) fn capture_git_with_environment(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    disable_optional_locks: bool,
    environment: &[(String, String)],
) -> Result<GitProcessOutput, CoreError> {
    capture_git_process(
        root,
        arguments,
        input,
        disable_optional_locks,
        environment,
        false,
    )
}

pub(crate) fn capture_git_process(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    disable_optional_locks: bool,
    environment: &[(String, String)],
    visible: bool,
) -> Result<GitProcessOutput, CoreError> {
    validate_windows_git_path(Path::new(root)).map_err(git_path_error)?;
    lithe_protocol::cancellation::check()?;
    let mut trace = command_trace::Trace::begin(root, arguments, visible);
    let result = match remote_process::capture(
        root,
        arguments,
        input.as_deref(),
        disable_optional_locks,
        environment,
        visible,
        &mut trace,
    ) {
        Some(result) => result,
        None => capture_local_git_process(
            root,
            arguments,
            input,
            disable_optional_locks,
            environment,
            visible,
            &mut trace,
        ),
    };
    trace.finish(result.as_ref().err());
    result
}

fn capture_local_git_process(
    root: &str,
    arguments: &[String],
    input: Option<String>,
    disable_optional_locks: bool,
    environment: &[(String, String)],
    visible: bool,
    trace: &mut command_trace::Trace,
) -> Result<GitProcessOutput, CoreError> {
    let mut process = git_process();
    let version = lithe_git_host::configuration::version(process.get_program(), || {
        lithe_protocol::cancellation::check().is_err()
    });
    lithe_protocol::cancellation::check()?;
    execution_policy::validate_version(&version.map_err(|error| {
        CoreError::new(
            ErrorCode::ProcessStartFailed,
            "Could not inspect Git executable",
        )
        .with_details(error.to_string())
    })?)?;
    if visible && execution_policy::current().interactive && !execution_events::has_sink() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Interactive Git requires an execution event receiver",
        ));
    }
    process
        .args(arguments)
        .current_dir(root)
        .envs(environment.iter().map(|(key, value)| (key, value)));
    if disable_optional_locks {
        process.env("GIT_OPTIONAL_LOCKS", "0");
    }
    if execution_policy::command_index(arguments).is_some_and(|index| arguments[index] != "config")
    {
        lithe_git_host::configuration::configure(
            &mut process,
            &execution_policy::temporary_config(),
        )
        .map_err(|error| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "Could not apply temporary Git configuration",
            )
            .with_details(error.to_string())
        })?;
    }
    let invocation = RefCell::new(execution_events::Invocation::reported(visible));
    let mut authentication = if visible && execution_policy::current().interactive {
        let session = lithe_git_host::authentication::Session::new().map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessStartFailed,
                "Could not prepare Git authentication",
            )
            .with_details(error.to_string())
        })?;
        session.configure(&mut process).map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessStartFailed,
                "Could not configure Git authentication",
            )
            .with_details(error.to_string())
        })?;
        Some(session)
    } else {
        None
    };
    let mut cancellation_error = None;
    let output = lithe_git_host::run(
        &mut process,
        input.as_deref().map(str::as_bytes),
        || {
            cancellation_error = lithe_protocol::cancellation::check().err();
            if cancellation_error.is_none() {
                if let Some(session) = &mut authentication {
                    match session.poll() {
                        Ok(challenges) => {
                            for challenge in challenges {
                                execution_events::emit(
                                    serde_json::json!({ "type": "authentication", "requestId": challenge.request_id,
                                "prompt": execution_events::redact(&challenge.prompt), "secret": challenge.secret,
                                "attempt": challenge.attempt, "workingDirectory": root }),
                                );
                            }
                        }
                        Err(error) => {
                            cancellation_error = Some(
                                CoreError::new(
                                    ErrorCode::ProcessFailed,
                                    "Git authentication transport failed",
                                )
                                .with_details(error.to_string()),
                            )
                        }
                    }
                }
            }
            cancellation_error.is_some()
        },
        || {
            if let Some(invocation) = invocation.borrow().as_ref() {
                invocation.started(root, arguments);
            }
        },
        |stream, bytes| {
            if let Some(invocation) = invocation.borrow_mut().as_mut() {
                invocation.output(stream, bytes);
            }
        },
    );
    let error = output.failure.map(|failure| match failure {
        lithe_git_host::Failure::Start(error) => {
            CoreError::new(ErrorCode::ProcessStartFailed, "Could not start Git")
                .with_details(error.to_string())
        }
        lithe_git_host::Failure::Io(error) => {
            CoreError::new(ErrorCode::ProcessFailed, "Could not read Git output")
                .with_details(error.to_string())
        }
        lithe_git_host::Failure::Cancelled => cancellation_error
            .unwrap_or_else(|| CoreError::new(ErrorCode::Cancelled, "Operation was cancelled")),
        lithe_git_host::Failure::OutputLimit => CoreError::new(
            ErrorCode::ProcessFailed,
            "Git output exceeded the capture limit",
        ),
        lithe_git_host::Failure::Cleanup => CoreError::new(
            ErrorCode::ProcessFailed,
            "Git process cleanup did not complete within its deadline",
        ),
    });
    let exit_code = output
        .status
        .as_ref()
        .and_then(std::process::ExitStatus::code);
    trace.observe(exit_code, &output.stderr);
    if let Some(invocation) = invocation.into_inner() {
        // A query that answers with a nonzero exit, such as a missing config key, keeps
        // its actual code without presenting a normal result as failure.
        let expected_exit = error.is_none()
            && execution_events::Invocation::answered(visible, arguments, exit_code);
        invocation.finished_with_expected_exit(exit_code, error.as_ref(), expected_exit);
    }
    if let Some(error) = error {
        return Err(error);
    }
    Ok(GitProcessOutput {
        stdout: output.stdout,
        stderr: output.stderr,
        exit_code: exit_code.unwrap_or(1),
    })
}

pub(crate) fn git_process() -> Command {
    let options = execution_policy::current();
    let executable = lithe_git_host::configuration::executable(options.executable.as_deref())
        .unwrap_or_else(|| options.executable.unwrap_or_else(|| "git".into()).into());
    #[cfg(target_os = "windows")]
    {
        let mut process = Command::new(executable);
        process.creation_flags(git_process_creation_flags());
        process
    }

    #[cfg(not(target_os = "windows"))]
    {
        Command::new(executable)
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn git_process_creation_flags() -> u32 {
    // Git runs as an IDE background task; attaching a console can briefly open
    // the user's default terminal whenever status or repository data refreshes.
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    CREATE_NO_WINDOW
}

pub(crate) fn successful_git_result() -> GitCommandResponse {
    GitCommandResponse {
        arguments: Vec::new(),
        output: String::new(),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        invocations: Vec::new(),
        operation_error: None,
        stash_restore: None,
        tag_deletion: None,
        branch_deletion: None,
        history_rewrite: None,
        outcome_authoritative: false,
        warnings: Vec::new(),
    }
}

pub(crate) fn failed_git_result(error: CoreError) -> GitCommandResponse {
    GitCommandResponse {
        arguments: Vec::new(),
        output: String::new(),
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
        invocations: Vec::new(),
        operation_error: Some(error),
        stash_restore: None,
        tag_deletion: None,
        branch_deletion: None,
        history_rewrite: None,
        outcome_authoritative: false,
        warnings: Vec::new(),
    }
}

pub(crate) fn read_git_config_value(root: &str, key: &str) -> Result<Option<String>, CoreError> {
    // This is an internal preflight, not a user command. A missing optional
    // value returns 1 normally and must not appear as a failed console action.
    let arguments = ["config".into(), "--get".into(), key.to_string()];
    let configured =
        capture_git_with_options(root, &arguments, None, true)?.into_command_response(&arguments);
    if configured.exit_code == 1 {
        return Ok(None);
    }
    if configured.exit_code != 0 {
        return Err(
            CoreError::new(ErrorCode::ProcessFailed, "Could not read Git configuration")
                .with_details(configured.output),
        );
    }
    let value = configured.output.trim();
    if value.is_empty() {
        return Ok(None);
    }
    validate_push_component(value)?;
    Ok(Some(value.to_string()))
}

pub(crate) fn run_git(
    directory: &Path,
    arguments: &[&str],
) -> Result<std::process::Output, CoreError> {
    let arguments = arguments
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    if lithe_workspace::is_remote() {
        let output = capture_git_process(
            &directory.to_string_lossy(),
            &arguments,
            None,
            true,
            &[],
            false,
        )?;
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(output.exit_code << 8)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(output.exit_code as u32)
        };
        return Ok(std::process::Output {
            status,
            stdout: output.stdout,
            stderr: output.stderr,
        });
    }
    // Status and path discovery are read-only from Lithe's point of view. Git
    // may otherwise refresh its optional index data while answering a query,
    // which emits `.git/index` events into the native watcher and can trigger
    // another status refresh.
    let root = directory.to_string_lossy();
    let mut trace = command_trace::Trace::begin(&root, &arguments, false);
    let mut invocation = execution_events::Invocation::reported(false);
    if let Some(invocation) = &invocation {
        invocation.started(&root, &arguments);
    }
    let result = git_process()
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(&arguments)
        .current_dir(directory)
        .output()
        .map_err(|error| {
            CoreError::new(ErrorCode::ProcessStartFailed, "Could not start Git")
                .with_details(error.to_string())
        });
    let exit_code = result.as_ref().ok().and_then(|output| output.status.code());
    if let Ok(output) = &result {
        trace.observe(exit_code, &output.stderr);
        if let Some(invocation) = &mut invocation {
            invocation.output(lithe_git_host::Stream::Stdout, &output.stdout);
            invocation.output(lithe_git_host::Stream::Stderr, &output.stderr);
        }
    }
    if let Some(invocation) = invocation {
        let expected_exit =
            result.is_ok() && execution_events::Invocation::answered(false, &arguments, exit_code);
        invocation.finished_with_expected_exit(exit_code, result.as_ref().err(), expected_exit);
    }
    trace.finish(result.as_ref().err());
    result
}
