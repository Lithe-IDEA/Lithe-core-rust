//! The language-server process boundary.
//!
//! The engine owns a language server's process and stdio, but it reaches them
//! only through these traits. That lets the engine's lifecycle rules --
//! deadlines, crash handling, restart isolation -- be tested against a
//! scripted server instead of a real one.
//!
//! A local server and everything it starts form one
//! [`lithe_git_host::ProcessTree`], so ending the server also ends a runtime
//! that its launcher script started. On Windows the tree is a Job Object,
//! which also ends it when the host crashes; on Unix it is a process group.

use lithe_git_host::ProcessTree;
use lithe_platform::Platform;
use lithe_protocol::{CoreError, ErrorCode};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Everything needed to start a language server, after provider adaptation has
/// already rewritten the arguments.
pub struct LspProcessSpec {
    /// Program to start.
    pub executable: PathBuf,
    /// Bare commands to resolve on the remote login PATH when executable is empty.
    pub remote_commands: Vec<String>,
    /// An executable already resolved on the remote host, never mapped as a desktop path.
    pub remote_executable: Option<String>,
    /// POSIX search path prefixes from the selected remote toolchain.
    pub remote_path_prepend: Vec<String>,
    /// Arguments after the program, one entry per argument.
    pub arguments: Vec<String>,
    /// Directory the process starts in.
    pub working_directory: PathBuf,
    /// Variables added to the inherited environment, or to the login
    /// environment on a remote workspace host.
    pub environment: BTreeMap<String, String>,
}

/// A running language-server process.
///
/// Exit notification and verified disposal are separate: a dead root may
/// still own descendants. Disposal must succeed before releasing resources.
pub trait LspProcessHandle: Send + Sync {
    /// OS process identifier when the platform exposes one. Scripted test
    /// handles intentionally return no identifier.
    fn process_id(&self) -> Option<u32> {
        None
    }

    /// Accepts one complete frame in order. Native handles flush it directly;
    /// remote handles enqueue bounded delivery without blocking the UI caller.
    /// A delivery failure closes the streams and becomes a transport failure.
    fn write_input(&self, frame: &[u8]) -> Result<(), CoreError>;

    /// Drops the input stream so the server observes EOF on stdin.
    fn close_input(&self);

    /// `Some` once the process has exited. The inner value is absent when the
    /// platform reports no exit code, as it does for a signalled process.
    fn exit_status(&self) -> Option<Option<i32>>;

    /// Ends the process without waiting for it to agree.
    fn terminate(&self);

    /// Transfers a native cleanup failure to the engine that owns role routing.
    fn take_termination_failure(&self) -> Option<std::io::Error> {
        None
    }

    /// Confirms disposal before the engine forgets the owning session.
    /// Scripted handles have no descendants and report their own exit.
    fn dispose(&self, _timeout: Duration) -> Result<(), CoreError> {
        if self.exit_status().is_some() {
            Ok(())
        } else {
            Err(CoreError::new(
                ErrorCode::ProcessFailed,
                "Language-server cleanup is incomplete.",
            ))
        }
    }
}

/// A launched process and the two streams the engine reads on its own threads.
pub struct LspProcessStreams {
    /// Input, exit and disposal control of the process.
    pub handle: Arc<dyn LspProcessHandle>,
    /// The server's stdout, carrying framed LSP messages.
    pub output: Box<dyn Read + Send>,
    /// The server's stderr, read as log lines.
    pub errors: Box<dyn Read + Send>,
}

/// Factory boundary used by the engine to start real or scripted servers.
pub trait LspProcessLauncher: Send + Sync {
    /// Starts one process and transfers ownership of its handle and output streams.
    fn launch(&self, spec: LspProcessSpec) -> Result<LspProcessStreams, CoreError>;
}

/// Starts language servers as real child processes with piped stdio.
pub struct SystemProcessLauncher;

impl LspProcessLauncher for SystemProcessLauncher {
    fn launch(&self, spec: LspProcessSpec) -> Result<LspProcessStreams, CoreError> {
        if let Some(host) = lithe_workspace::current() {
            return crate::remote_process::launch(host, spec);
        }
        if !spec.remote_commands.is_empty()
            || spec.remote_executable.is_some()
            || !spec.remote_path_prepend.is_empty()
        {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "Remote process parameters require a remote workspace host",
            ));
        }
        let mut command = language_server_command(&spec);
        command
            .current_dir(&spec.working_directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        ProcessTree::prepare(&mut command);
        let mut child = command.spawn().map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessStartFailed,
                "Could not start the language-server process.",
            )
            .with_details(error.to_string())
        })?;
        let tree = ProcessTree::adopt(&mut child).map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessStartFailed,
                "Could not take ownership of the language-server process tree.",
            )
            .with_details(error.to_string())
        })?;
        let stdin = child.stdin.take().ok_or_else(|| missing_stream("stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| missing_stream("stdout"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| missing_stream("stderr"))?;
        Ok(LspProcessStreams {
            handle: Arc::new(SystemProcess {
                input: Mutex::new(Some(stdin)),
                child: Mutex::new(child),
                termination_failure: Mutex::new(None),
                tree,
            }),
            output: Box::new(stdout),
            errors: Box::new(stderr),
        })
    }
}

fn language_server_command(spec: &LspProcessSpec) -> Command {
    if let Some(batch) = BatchLaunch::new(spec, Platform::HOST) {
        return batch.into_command();
    }
    let mut command = Command::new(&spec.executable);
    command.args(&spec.arguments).envs(&spec.environment);
    command
}

/// Program that runs a Windows batch script.
const BATCH_INTERPRETER: &str = "cmd.exe";

/// Variable that carries the script path to [`BATCH_INTERPRETER`].
const BATCH_EXECUTABLE_ENV: &str = "LITHE_LSP_BATCH_EXECUTABLE";

/// How a Windows batch-script language server is started.
///
/// `cmd.exe` parses its command line with its own rules, so the script path
/// and arguments travel in environment variables and the command line only
/// references them. Environment expansion is single-pass, so the values
/// reach the script without `cmd.exe` interpreting path metacharacters or
/// percent pairs.
#[derive(Debug, PartialEq, Eq)]
struct BatchLaunch {
    /// Variables added to the inherited environment, in the order set.
    environment: Vec<(String, String)>,
    /// `cmd.exe` arguments, each passed exactly as written.
    arguments: Vec<String>,
}

impl BatchLaunch {
    /// The launch of `spec` when `platform` runs it as a batch script.
    fn new(spec: &LspProcessSpec, platform: Platform) -> Option<Self> {
        if !platform.is_windows() || !is_batch_script(&spec.executable) {
            return None;
        }
        let mut environment: Vec<_> = spec
            .environment
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        environment.push((
            BATCH_EXECUTABLE_ENV.to_string(),
            batch_env_value(&spec.executable.to_string_lossy()),
        ));
        for (index, argument) in spec.arguments.iter().enumerate() {
            environment.push((batch_argument_env(index), batch_env_value(argument)));
        }
        let arguments = ["/D", "/S", "/V:OFF", "/C"]
            .into_iter()
            .map(str::to_string)
            .chain([batch_command_line(spec.arguments.len())])
            .collect();
        Some(Self {
            environment,
            arguments,
        })
    }

    fn into_command(self) -> Command {
        let mut command = Command::new(BATCH_INTERPRETER);
        command.envs(self.environment);
        for argument in self.arguments {
            lithe_platform::process::verbatim_argument(&mut command, argument);
        }
        command
    }
}

fn is_batch_script(executable: &Path) -> bool {
    matches!(
        executable.extension().and_then(|extension| extension.to_str()),
        Some(extension) if extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd")
    )
}

fn batch_argument_env(index: usize) -> String {
    format!("LITHE_LSP_BATCH_ARGUMENT_{index}")
}

fn batch_command_line(argument_count: usize) -> String {
    let mut command_line = format!("\"%{BATCH_EXECUTABLE_ENV}%\"");
    for index in 0..argument_count {
        command_line.push_str(&format!(" \"%{}%\"", batch_argument_env(index)));
    }
    format!("\"{command_line}\"")
}

/// Escapes `value` so the Windows argument parser of the program the script
/// starts reads it back unchanged inside the quotes of [`batch_command_line`].
fn batch_env_value(value: &str) -> String {
    let mut escaped = String::new();
    let mut backslashes = 0;
    for character in value.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                escaped.push_str(&"\\".repeat(backslashes * 2 + 1));
                escaped.push('"');
                backslashes = 0;
            }
            _ => {
                escaped.push_str(&"\\".repeat(backslashes));
                escaped.push(character);
                backslashes = 0;
            }
        }
    }
    escaped.push_str(&"\\".repeat(backslashes * 2));
    escaped
}

/// Interval between exit checks while a native disposal waits for the root.
const DISPOSAL_POLL: Duration = Duration::from_millis(10);

struct SystemProcess {
    input: Mutex<Option<ChildStdin>>,
    child: Mutex<Child>,
    /// At most one pending cleanup category, transferred to the owning engine.
    termination_failure: Mutex<Option<std::io::Error>>,
    /// The server and its descendants. On Windows dropping the handle also
    /// ends all of them.
    tree: ProcessTree,
}

impl LspProcessHandle for SystemProcess {
    fn process_id(&self) -> Option<u32> {
        self.child.lock().ok().map(|child| child.id())
    }

    fn write_input(&self, frame: &[u8]) -> Result<(), CoreError> {
        let mut input = self.input.lock().map_err(|_| {
            CoreError::new(
                ErrorCode::Unknown,
                "Language-server stdin lock was poisoned.",
            )
        })?;
        let input = input.as_mut().ok_or_else(|| {
            CoreError::new(ErrorCode::ProcessFailed, "Language-server stdin is closed.")
        })?;
        input.write_all(frame).map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessFailed,
                "Could not write to language-server stdin.",
            )
            .with_details(error.to_string())
        })?;
        input.flush().map_err(|error| {
            CoreError::new(
                ErrorCode::ProcessFailed,
                "Could not flush language-server stdin.",
            )
            .with_details(error.to_string())
        })
    }

    fn close_input(&self) {
        if let Ok(mut input) = self.input.lock() {
            *input = None;
        }
    }

    fn exit_status(&self) -> Option<Option<i32>> {
        self.child
            .lock()
            .ok()
            .and_then(|mut child| child.try_wait().ok().flatten())
            .map(|status| status.code())
    }

    fn terminate(&self) {
        // Native I/O returns only a category; the engine decides WARN and role routing.
        let result = self.terminate_native();
        if let Err(error) = result {
            *self
                .termination_failure
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(error);
        }
    }

    fn take_termination_failure(&self) -> Option<std::io::Error> {
        self.termination_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    fn dispose(&self, timeout: Duration) -> Result<(), CoreError> {
        // A signalled child is reaped asynchronously, so an immediate status
        // read after termination can still report it running. Repeat the
        // idempotent kill and poll until the bounded disposal deadline. The
        // last kill failure explains a root that outlives the deadline.
        let deadline = Instant::now() + timeout;
        let mut kill_failure = None;
        while self.exit_status().is_none() && Instant::now() < deadline {
            if let Err(error) = self.terminate_native() {
                kill_failure = Some(error);
            }
            std::thread::sleep(DISPOSAL_POLL);
        }
        if self.exit_status().is_none() {
            let error = CoreError::new(
                ErrorCode::ProcessFailed,
                "Language-server root did not exit.",
            );
            return Err(match kill_failure {
                Some(cause) => error.with_details(cause.to_string()),
                None => error,
            });
        }
        // The root is reaped now, so a Unix process group counts only the
        // descendants that are still alive.
        self.tree
            .terminate(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| {
                CoreError::new(
                    ErrorCode::ProcessFailed,
                    "Language-server process tree did not exit.",
                )
                .with_details(error.to_string())
            })
    }
}

impl SystemProcess {
    fn terminate_native(&self) -> std::io::Result<()> {
        let tree_result = self.tree.kill();
        let child_result = match self.child.lock() {
            Ok(mut child) => match child.kill() {
                Ok(()) => Ok(()),
                // A process that exited before termination is expected absence.
                Err(_) if matches!(child.try_wait(), Ok(Some(_))) => Ok(()),
                Err(error) => Err(error),
            },
            Err(_) => Err(std::io::Error::other("Process lock poisoned")),
        };
        tree_result?;
        child_result
    }
}

fn missing_stream(stream: &str) -> CoreError {
    CoreError::new(
        ErrorCode::ProcessStartFailed,
        "A language-server standard stream was unavailable.",
    )
    .with_details(stream)
}

// Starting real servers needs a host of each platform; those tests live in
// the lithe-platform-tests package.
#[cfg(test)]
mod tests {
    use super::{
        batch_command_line, batch_env_value, language_server_command, BatchLaunch, LspProcessSpec,
        BATCH_EXECUTABLE_ENV,
    };
    use lithe_platform::Platform;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn spec(executable: &str, arguments: &[&str]) -> LspProcessSpec {
        LspProcessSpec {
            executable: PathBuf::from(executable),
            remote_commands: Vec::new(),
            remote_executable: None,
            remote_path_prepend: Vec::new(),
            arguments: arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect(),
            working_directory: PathBuf::from("workspace"),
            environment: BTreeMap::from([("JAVA_HOME".to_string(), "jdk".to_string())]),
        }
    }

    #[test]
    fn windows_starts_batch_scripts_through_cmd_exe() {
        let spec = spec(
            r"C:\Program Files\Lithe\language-server.bat",
            &["-data", r"C:\workspace data"],
        );

        let launch = BatchLaunch::new(&spec, Platform::Windows).expect("batch launch");

        assert_eq!(
            launch.arguments,
            [
                "/D",
                "/S",
                "/V:OFF",
                "/C",
                r#"""%LITHE_LSP_BATCH_EXECUTABLE%" "%LITHE_LSP_BATCH_ARGUMENT_0%" "%LITHE_LSP_BATCH_ARGUMENT_1%"""#,
            ]
        );
        assert_eq!(
            launch.environment,
            [
                ("JAVA_HOME".to_string(), "jdk".to_string()),
                (
                    BATCH_EXECUTABLE_ENV.to_string(),
                    r"C:\Program Files\Lithe\language-server.bat".to_string()
                ),
                (
                    "LITHE_LSP_BATCH_ARGUMENT_0".to_string(),
                    "-data".to_string()
                ),
                (
                    "LITHE_LSP_BATCH_ARGUMENT_1".to_string(),
                    r"C:\workspace data".to_string()
                ),
            ]
        );
    }

    #[test]
    fn batch_script_extensions_match_without_case() {
        for executable in ["server.cmd", "server.CMD", "server.Bat"] {
            assert!(
                BatchLaunch::new(&spec(executable, &[]), Platform::Windows).is_some(),
                "{executable}"
            );
        }
        for executable in ["server.exe", "server", "server.sh", "server.bat.txt"] {
            assert!(
                BatchLaunch::new(&spec(executable, &[]), Platform::Windows).is_none(),
                "{executable}"
            );
        }
    }

    #[test]
    fn unix_runs_scripts_named_like_batch_files_directly() {
        let spec = spec("/opt/server/language-server.cmd", &["--stdio"]);

        assert_eq!(BatchLaunch::new(&spec, Platform::Unix), None);
        if !Platform::HOST.is_windows() {
            let command = language_server_command(&spec);
            assert_eq!(
                command.get_program(),
                OsStr::new("/opt/server/language-server.cmd")
            );
            assert_eq!(command.get_args().collect::<Vec<_>>(), ["--stdio"]);
        }
    }

    #[test]
    fn the_command_line_only_references_environment_variables() {
        assert_eq!(batch_command_line(0), r#"""%LITHE_LSP_BATCH_EXECUTABLE%"""#);
        assert_eq!(
            batch_command_line(1),
            r#"""%LITHE_LSP_BATCH_EXECUTABLE%" "%LITHE_LSP_BATCH_ARGUMENT_0%"""#
        );
    }

    #[test]
    fn batch_values_escape_quotes_and_the_backslashes_before_them() {
        let cases = [
            ("plain", "plain"),
            ("percent%PATH%value", "percent%PATH%value"),
            ("quoted\"value", r#"quoted\"value"#),
            (r#"slash\"quote"#, r#"slash\\\"quote"#),
            (r"C:\workspace data\", r"C:\workspace data\\"),
            (r"inner\path", r"inner\path"),
        ];
        for (value, escaped) in cases {
            assert_eq!(batch_env_value(value), escaped, "{value}");
        }
    }
}
