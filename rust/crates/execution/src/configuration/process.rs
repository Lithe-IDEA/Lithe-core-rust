//! Built-in process runs: shell, PowerShell and batch scripts and programs.
//!
//! Starting a script or a program is something every platform does without a
//! plugin, so these providers live in Core and keep working when no plugin is
//! installed or a plugin fails. Interpreters of other languages and build
//! tools stay with the plugins that declare them.
//!
//! An entry keeps its choices in `extensions.process` ([`ProcessOptions`]).
//! `args` holds the script options or the program arguments, and `cwd` and
//! `env` keep their usual meaning. A project-relative path is saved with `/`
//! so every platform reads it the same way; an absolute path is native text.

use super::*;
use lithe_platform::paths::{is_absolute_path, join_native};

/// The built-in shell script provider.
pub(crate) const SHELL: &str = "process.shell";
/// The built-in PowerShell script provider.
pub(crate) const POWERSHELL: &str = "process.powershell";
/// The built-in Windows batch script provider.
pub(crate) const BATCH: &str = "process.batch";
/// The built-in program provider.
pub(crate) const EXECUTABLE: &str = "process.executable";

pub(crate) const MAXIMUM_NAME_BYTES: usize = 256;
const MAXIMUM_PATH_BYTES: usize = 4096;
const MAXIMUM_TEXT_BYTES: usize = 1024 * 1024;
const MAXIMUM_ARGUMENTS: usize = 256;
const MAXIMUM_ARGUMENT_BYTES: usize = 32 * 1024;
/// How much of a script is read to find its `#!` line.
const SHEBANG_BYTES: usize = 1024;

/// What a built-in process run starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    /// A shell script, [`SHELL`].
    Shell,
    /// A PowerShell script, [`POWERSHELL`].
    PowerShell,
    /// A batch script run by `cmd`, [`BATCH`]; Windows only.
    Batch,
    /// A program started directly, [`EXECUTABLE`].
    Executable,
}

impl Kind {
    /// Every kind, in the order a host lists them.
    pub(crate) const ALL: [Self; 4] =
        [Self::Shell, Self::PowerShell, Self::Batch, Self::Executable];

    pub(crate) fn from_provider(provider: &str) -> Option<Self> {
        match provider {
            SHELL => Some(Self::Shell),
            POWERSHELL => Some(Self::PowerShell),
            BATCH => Some(Self::Batch),
            EXECUTABLE => Some(Self::Executable),
            _ => None,
        }
    }

    pub(crate) fn provider(self) -> &'static str {
        match self {
            Self::Shell => SHELL,
            Self::PowerShell => POWERSHELL,
            Self::Batch => BATCH,
            Self::Executable => EXECUTABLE,
        }
    }

    /// Whether the kind runs a script through an interpreter.
    pub(crate) fn runs_script(self) -> bool {
        self != Self::Executable
    }

    /// Whether `platform` can run the kind: `cmd` exists only on Windows.
    pub(crate) fn supported_on(self, platform: Platform) -> bool {
        self != Self::Batch || platform.is_windows()
    }

    /// Editor field, and error field, of the entry's `args`.
    pub(crate) fn arguments_field(self) -> &'static str {
        if self.runs_script() {
            "scriptOptions"
        } else {
            "programArguments"
        }
    }

    /// The family of shell that runs scripts of the kind, which an editor's
    /// interpreter field names so a host can offer the installed shells of
    /// that family: `posix` (sh, bash, zsh and the like), `powershell`
    /// (Windows PowerShell and `pwsh`) or `cmd`. A program has no interpreter.
    pub(crate) fn shell_family(self) -> Option<&'static str> {
        match self {
            Self::Shell => Some("posix"),
            Self::PowerShell => Some("powershell"),
            Self::Batch => Some("cmd"),
            Self::Executable => None,
        }
    }

    /// File name suffix of the temporary file an inline script is run from;
    /// PowerShell runs only `.ps1` files and `cmd` only `.cmd` or `.bat`.
    fn script_suffix(self) -> &'static str {
        match self {
            Self::Shell => ".sh",
            Self::PowerShell => ".ps1",
            Self::Batch => ".cmd",
            Self::Executable => "",
        }
    }
}

/// Whether `provider` names a built-in process run, including kinds a later
/// release added: those are invalid here instead of belonging to a plugin.
pub(crate) fn is_process_provider(provider: &str) -> bool {
    provider.starts_with("process.")
}

/// Why `platform` cannot run entries of `provider`, absent when it can.
/// Providers that are not built-in process runs are not judged here.
pub(crate) fn provider_availability(provider: &str, platform: Platform) -> Option<Availability> {
    platform_availability(Kind::from_provider(provider)?, platform)
}

/// Why `platform` cannot run `kind`, absent when it can: `unsupportedPlatform`
/// for a batch script outside Windows. Such an entry is still created, edited
/// and saved, so a project shared with Windows keeps it.
pub(crate) fn platform_availability(kind: Kind, platform: Platform) -> Option<Availability> {
    (!kind.supported_on(platform)).then(|| {
        Availability::unavailable(
            "unsupportedPlatform",
            format!("{} runs only on Windows", kind.provider()),
        )
    })
}

/// The `extensions.process` choices of a built-in process run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessOptions {
    /// Script file: project-relative with `/`, or absolute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) script: Option<String>,
    /// Script text run from a temporary file, instead of `script`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) text: Option<String>,
    /// Program to start: a bare name looked up on `PATH`, a project-relative
    /// path containing `/`, or an absolute path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) program: Option<String>,
    /// Interpreter of the script, named like `program`; absent picks one from
    /// the kind, the script's `#!` line and the platform.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interpreter: Option<String>,
    /// Interpreter options placed before the script; absent uses the kind's
    /// defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) interpreter_arguments: Option<Vec<String>>,
    /// Whether a PowerShell run on Windows passes `-ExecutionPolicy Bypass`,
    /// so a script that is not signed runs. Saved only when turned off.
    #[serde(default = "enabled", skip_serializing_if = "is_enabled")]
    pub(crate) execution_policy_bypass: bool,
    /// Keys a later release added, kept so saving here does not drop them.
    #[serde(flatten)]
    pub(crate) other: serde_json::Map<String, Value>,
}

fn enabled() -> bool {
    true
}

fn is_enabled(value: &bool) -> bool {
    *value
}

impl Default for ProcessOptions {
    fn default() -> Self {
        Self {
            script: None,
            text: None,
            program: None,
            interpreter: None,
            interpreter_arguments: None,
            execution_policy_bypass: true,
            other: serde_json::Map::new(),
        }
    }
}

impl ProcessOptions {
    /// The options saved in `configuration`, defaults when it has none.
    pub(crate) fn of(configuration: &RunConfiguration) -> Result<Self, CoreError> {
        match configuration.extensions.get("process") {
            None => Ok(Self::default()),
            Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
                CoreError::new(ErrorCode::InvalidRequest, "Invalid process run options")
                    .with_details(error.to_string())
            }),
        }
    }

    /// The script file, when one is named. Blank text names none.
    pub(crate) fn script(&self) -> Option<&str> {
        present(self.script.as_deref())
    }

    /// The inline script text, when there is any.
    pub(crate) fn text(&self) -> Option<&str> {
        present(self.text.as_deref())
    }

    /// The program, when one is named.
    pub(crate) fn program(&self) -> Option<&str> {
        present(self.program.as_deref())
    }

    /// The interpreter the entry names; `None` leaves it automatic.
    pub(crate) fn interpreter(&self) -> Option<&str> {
        present(self.interpreter.as_deref())
    }
}

fn present(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !value.trim().is_empty())
}

/// One invalid value of a process run, named by its editor field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct FieldError {
    /// Editor field id, such as `script` or `workingDirectory`.
    pub(crate) field: &'static str,
    /// Why the value is invalid: `required`, `conflict`, `notAllowed`,
    /// `tooLong`, `invalidCharacter`, `invalidPath`, `notFound`,
    /// `outsideProject` or `invalidName`.
    pub(crate) code: &'static str,
}

impl FieldError {
    pub(crate) fn new(field: &'static str, code: &'static str) -> Self {
        Self { field, code }
    }
}

/// Rejects a process run whose saved values cannot start. Files are not
/// looked up: a script that a build creates later is valid until a launch.
pub(crate) fn validate(
    configuration: &RunConfiguration,
    platform: Platform,
) -> Result<(), CoreError> {
    let errors = field_errors(configuration, platform)?;
    if errors.is_empty() {
        return Ok(());
    }
    Err(
        CoreError::new(ErrorCode::InvalidRequest, "Invalid process run").with_details(
            errors
                .iter()
                .map(|error| format!("{}: {}", error.field, error.code))
                .collect::<Vec<_>>()
                .join(", "),
        ),
    )
}

/// The invalid values of a process run, each named by its editor field. Fails
/// when the entry is not a known process run or holds fields that no process
/// run has.
pub(crate) fn field_errors(
    configuration: &RunConfiguration,
    platform: Platform,
) -> Result<Vec<FieldError>, CoreError> {
    let kind = Kind::from_provider(&configuration.provider).ok_or_else(|| {
        CoreError::new(ErrorCode::NotSupported, "Unsupported process run")
            .with_details(configuration.provider.clone())
    })?;
    if configuration.command.is_some()
        || !configuration.toolchains.is_empty()
        || configuration.debug.is_some()
        || !configuration.members.is_empty()
        || configuration.execution == Execution::Group
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Process runs cannot store commands, tool bindings, debug adapters or group members",
        ));
    }
    let options = ProcessOptions::of(configuration)?;
    let mut errors = Vec::new();
    if let Some(code) = text_error(&configuration.name, MAXIMUM_NAME_BYTES) {
        errors.push(FieldError::new("name", code));
    }
    if kind.runs_script() {
        match (options.script(), options.text()) {
            (Some(_), Some(_)) => errors.push(FieldError::new("source", "conflict")),
            (None, None) => errors.push(FieldError::new("script", "required")),
            (Some(script), None) => errors.extend(script_error(script, platform)),
            (None, Some(text)) => errors.extend(
                text_error(text, MAXIMUM_TEXT_BYTES).map(|code| FieldError::new("text", code)),
            ),
        }
        if options.program.is_some() {
            errors.push(FieldError::new("program", "notAllowed"));
        }
        if let Some(interpreter) = options.interpreter() {
            errors.extend(program_error("interpreter", interpreter, platform));
        }
        if let Some(arguments) = &options.interpreter_arguments {
            errors.extend(arguments_error("interpreterArguments", arguments));
        }
    } else {
        match options.program() {
            Some(program) => errors.extend(program_error("program", program, platform)),
            None => errors.push(FieldError::new("program", "required")),
        }
        for (field, saved) in [
            ("script", options.script.is_some()),
            ("text", options.text.is_some()),
            ("interpreter", options.interpreter.is_some()),
            (
                "interpreterArguments",
                options.interpreter_arguments.is_some(),
            ),
        ] {
            if saved {
                errors.push(FieldError::new(field, "notAllowed"));
            }
        }
    }
    if !options.execution_policy_bypass && kind != Kind::PowerShell {
        errors.push(FieldError::new("executionPolicyBypass", "notAllowed"));
    }
    errors.extend(arguments_error(kind.arguments_field(), &configuration.args));
    if configuration
        .env
        .iter()
        .any(|(name, value)| name.is_empty() || name.contains(['=', '\0']) || value.contains('\0'))
    {
        errors.push(FieldError::new("environment", "invalidName"));
    }
    Ok(errors)
}

pub(crate) fn text_error(value: &str, maximum: usize) -> Option<&'static str> {
    if value.len() > maximum {
        Some("tooLong")
    } else if value.contains('\0') {
        Some("invalidCharacter")
    } else {
        None
    }
}

/// A script is absolute, or project-relative with `/` and no `..`.
fn script_error(value: &str, platform: Platform) -> Option<FieldError> {
    if let Some(code) = text_error(value, MAXIMUM_PATH_BYTES) {
        return Some(FieldError::new("script", code));
    }
    (!is_absolute_path(value, platform) && invalid_portable_relative_path(value))
        .then(|| FieldError::new("script", "invalidPath"))
}

/// A program is a bare name, absolute, or project-relative with `/` and no `..`.
fn program_error(field: &'static str, value: &str, platform: Platform) -> Option<FieldError> {
    if let Some(code) = text_error(value, MAXIMUM_PATH_BYTES) {
        return Some(FieldError::new(field, code));
    }
    (is_path(value) && !is_absolute_path(value, platform) && invalid_portable_relative_path(value))
        .then(|| FieldError::new(field, "invalidPath"))
}

fn arguments_error(field: &'static str, arguments: &[String]) -> Option<FieldError> {
    if arguments.len() > MAXIMUM_ARGUMENTS {
        return Some(FieldError::new(field, "tooLong"));
    }
    arguments
        .iter()
        .find_map(|argument| text_error(argument, MAXIMUM_ARGUMENT_BYTES))
        .map(|code| FieldError::new(field, code))
}

/// Whether a program value is a path rather than a name looked up on `PATH`.
/// Both separators count on every platform, so the same text means the same
/// thing wherever the document is read.
pub(crate) fn is_path(value: &str) -> bool {
    value.contains(['/', '\\'])
}

/// The program a launch starts: a name the host looks up on `PATH`, or a
/// native absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Executable {
    Command(String),
    Path(String),
}

impl Executable {
    fn plan(&self) -> Value {
        match self {
            Self::Command(command) => json!({ "command": command }),
            Self::Path(path) => json!({ "path": path }),
        }
    }

    /// The text a user would type for this program.
    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Command(text) | Self::Path(text) => text,
        }
    }
}

/// An interpreter with the options placed before the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interpreter {
    pub(crate) executable: Executable,
    pub(crate) arguments: Vec<String>,
}

/// The `#!` line of a script: the interpreter, then everything after it as
/// at most one argument, the way Linux reads it. `#!/usr/bin/env -S a b`
/// therefore passes `-S a b` to `env`, which splits it again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shebang {
    pub(crate) interpreter: String,
    pub(crate) argument: Option<String>,
}

impl Shebang {
    /// The `#!` line at the start of `head`, after an optional UTF-8 BOM.
    pub(crate) fn parse(head: &[u8]) -> Option<Self> {
        let head = head.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(head);
        let rest = head.strip_prefix(b"#!")?;
        let end = rest
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(rest.len());
        let line = std::str::from_utf8(&rest[..end]).ok()?.trim();
        let (interpreter, argument) = match line.split_once([' ', '\t']) {
            Some((interpreter, argument)) => (interpreter, Some(argument.trim())),
            None => (line, None),
        };
        if interpreter.is_empty() {
            return None;
        }
        Some(Self {
            interpreter: interpreter.to_string(),
            argument: argument
                .filter(|argument| !argument.is_empty())
                .map(str::to_string),
        })
    }

    /// The `#!` line of the script file at `path`; `None` when there is
    /// none or the file cannot be read.
    pub(crate) fn of_file(path: &Path) -> Option<Self> {
        use std::io::Read;
        let mut head = Vec::with_capacity(SHEBANG_BYTES);
        fs::File::open(path)
            .ok()?
            .take(SHEBANG_BYTES as u64)
            .read_to_end(&mut head)
            .ok()?;
        Self::parse(&head)
    }
}

/// The interpreter a script of `kind` runs with when the entry names none.
///
/// A shell script on Unix uses its `#!` line, else `/bin/sh`. Windows has no
/// `#!` handling and no `/usr/bin`, so a shell script there runs with the
/// program the line names, looked up on `PATH`, else `bash`. PowerShell is
/// `powershell` on Windows, which every Windows has, and `pwsh` elsewhere.
/// `cmd` skips its AutoRun commands with `/d`. Programs have no interpreter;
/// [`Kind::Executable`] returns an empty command.
pub(crate) fn automatic_interpreter(
    kind: Kind,
    shebang: Option<&Shebang>,
    platform: Platform,
) -> Interpreter {
    let interpreter = |executable, arguments: &[&str]| Interpreter {
        executable,
        arguments: arguments
            .iter()
            .map(|argument| argument.to_string())
            .collect(),
    };
    match kind {
        Kind::Shell => match (shebang, platform) {
            (None, Platform::Unix) => interpreter(Executable::Path("/bin/sh".into()), &[]),
            (None, Platform::Windows) => interpreter(Executable::Command("bash".into()), &[]),
            (Some(shebang), Platform::Unix) => Interpreter {
                executable: if is_absolute_path(&shebang.interpreter, platform) {
                    Executable::Path(shebang.interpreter.clone())
                } else {
                    Executable::Command(shebang.interpreter.clone())
                },
                arguments: shebang.argument.iter().cloned().collect(),
            },
            (Some(shebang), Platform::Windows) => windows_shebang_interpreter(shebang),
        },
        Kind::PowerShell => interpreter(
            Executable::Command(
                if platform.is_windows() {
                    "powershell"
                } else {
                    "pwsh"
                }
                .into(),
            ),
            &["-NoLogo", "-NoProfile"],
        ),
        Kind::Batch => interpreter(Executable::Command("cmd".into()), &["/d"]),
        Kind::Executable => interpreter(Executable::Command(String::new()), &[]),
    }
}

/// `#!/bin/bash -e` becomes `bash -e`, and `#!/usr/bin/env [-S] bash -e`
/// becomes `bash -e`, so a Windows `PATH` finds the program.
fn windows_shebang_interpreter(shebang: &Shebang) -> Interpreter {
    let name = shebang
        .interpreter
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_string();
    if name != "env" {
        return Interpreter {
            executable: Executable::Command(name),
            arguments: shebang.argument.iter().cloned().collect(),
        };
    }
    let mut words = shebang
        .argument
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
        .skip_while(|word| *word == "-S")
        .map(str::to_string);
    match words.next() {
        Some(program) => Interpreter {
            executable: Executable::Command(program),
            arguments: words.collect(),
        },
        None => Interpreter {
            executable: Executable::Command("bash".into()),
            arguments: Vec::new(),
        },
    }
}

/// The interpreter a run uses: the entry's own, else the automatic one. The
/// automatic options apply unless the entry names options; a shell script
/// with a named interpreter has none, since its `#!` line is not used.
pub(crate) fn effective_interpreter(
    kind: Kind,
    options: &ProcessOptions,
    shebang: Option<&Shebang>,
    root: &Path,
    platform: Platform,
) -> Interpreter {
    let automatic = automatic_interpreter(kind, shebang, platform);
    let named = options
        .interpreter()
        .map(|interpreter| program_reference(root, interpreter, platform));
    let arguments = match (&options.interpreter_arguments, &named) {
        (Some(arguments), _) => arguments.clone(),
        (None, Some(_)) if kind == Kind::Shell => Vec::new(),
        (None, _) => automatic.arguments,
    };
    Interpreter {
        executable: named.unwrap_or(automatic.executable),
        arguments,
    }
}

/// What a program value names, without looking at the file system.
pub(crate) fn program_reference(root: &Path, value: &str, platform: Platform) -> Executable {
    if !is_path(value) {
        Executable::Command(value.to_string())
    } else if is_absolute_path(value, platform) {
        Executable::Path(value.to_string())
    } else {
        Executable::Path(native_project_path(root, value, platform))
    }
}

/// The native text of project-relative `/`-separated `value`.
fn native_project_path(root: &Path, value: &str, platform: Platform) -> String {
    join_native(&root.to_string_lossy(), value, platform)
}

/// The file a path value names, for reading it on this machine.
pub(crate) fn file_path(root: &Path, value: &str, platform: Platform) -> PathBuf {
    if is_absolute_path(value, platform) {
        PathBuf::from(value)
    } else {
        root.join(value)
    }
}

/// The automatic display name of an entry: the script's or program's file
/// name without its extension.
pub(crate) fn default_name(options: &ProcessOptions) -> Option<String> {
    let value = options.script().or(options.program())?;
    let file = value.trim().rsplit(['/', '\\']).next()?;
    let stem = match file.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => file,
    };
    (!stem.is_empty()).then(|| stem.to_string())
}

/// The launch plan of a resolved built-in process run.
///
/// Besides the keys of every plan, it may carry:
/// - `inlineScript`, `{path, content}`: the host writes `content` to `path`
///   before starting and removes the file once the process exits.
///   `arguments` already names `path`, which is `inline_script_base` with the
///   kind's suffix.
/// - `commandLine`, for batch scripts: the exact Windows command line. `cmd`
///   does not parse quotes the way other programs do, so the host passes it
///   unchanged instead of quoting `arguments` itself.
pub(crate) fn launch_plan(
    root: &Path,
    configuration: &Value,
    platform: Platform,
    inline_script_base: Option<&str>,
) -> Result<Value, CoreError> {
    let configuration: RunConfiguration = serde_json::from_value(configuration.clone())
        .map_err(|_| CoreError::new(ErrorCode::ParseFailed, "Invalid process run"))?;
    validate(&configuration, platform)?;
    let kind = Kind::from_provider(&configuration.provider).expect("validated process runs");
    if !kind.supported_on(platform) {
        return Err(CoreError::new(
            ErrorCode::NotSupported,
            "Batch scripts run only on Windows",
        ));
    }
    let options = ProcessOptions::of(&configuration)?;
    let mut plan = json!({
        "workingDirectory": configuration.cwd,
        "environment": {},
        "env": configuration.env,
    });
    if !kind.runs_script() {
        let program = options.program().expect("validated programs are named");
        let executable = program_reference(root, program, platform);
        require_program(root, program, &executable, platform)?;
        plan["executable"] = executable.plan();
        plan["arguments"] = json!(configuration.args);
        return Ok(plan);
    }
    let (script, shebang) = match (options.script(), options.text()) {
        (Some(script), _) => {
            let file = file_path(root, script, platform);
            if !file.is_file() {
                return Err(
                    CoreError::new(ErrorCode::InvalidRequest, "Script file was not found")
                        .with_details(script),
                );
            }
            let native = if is_absolute_path(script, platform) {
                script.to_string()
            } else {
                native_project_path(root, script, platform)
            };
            (native, Shebang::of_file(&file))
        }
        (None, Some(text)) => {
            let base = inline_script_base
                .filter(|base| is_absolute_path(base, platform) && !base.contains('\0'))
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::InvalidRequest,
                        "Inline scripts need an absolute temporary script location",
                    )
                })?;
            let path = format!("{base}{}", kind.script_suffix());
            let content = inline_content(kind, text, platform);
            let shebang = Shebang::parse(content.as_bytes());
            plan["inlineScript"] = json!({ "path": path, "content": content });
            (path, shebang)
        }
        (None, None) => unreachable!("validated scripts name a file or text"),
    };
    let interpreter = effective_interpreter(kind, &options, shebang.as_ref(), root, platform);
    if let Some(value) = options.interpreter() {
        require_program(root, value, &interpreter.executable, platform)?;
    }
    let mut arguments = interpreter.arguments.clone();
    match kind {
        Kind::PowerShell => {
            if platform.is_windows() && options.execution_policy_bypass {
                arguments.extend(["-ExecutionPolicy".into(), "Bypass".into()]);
            }
            arguments.extend(["-File".into(), script]);
        }
        Kind::Batch => {
            let command = std::iter::once(script.as_str())
                .chain(configuration.args.iter().map(String::as_str))
                .map(cmd_quote)
                .collect::<Vec<_>>()
                .join(" ");
            let line = std::iter::once(interpreter.executable.text())
                .chain(interpreter.arguments.iter().map(String::as_str))
                .map(cmd_quote)
                .chain(["/s".into(), "/c".into(), format!("\"{command}\"")])
                .collect::<Vec<_>>()
                .join(" ");
            plan["commandLine"] = json!(line);
            arguments.extend(["/s".into(), "/c".into(), script]);
        }
        Kind::Shell | Kind::Executable => arguments.push(script),
    }
    arguments.extend(configuration.args.iter().cloned());
    plan["executable"] = interpreter.executable.plan();
    plan["arguments"] = json!(arguments);
    Ok(plan)
}

/// Fails when a program named by a path is not a file. A name looked up on
/// `PATH` is left to the host, which owns the environment it is looked up in.
fn require_program(
    root: &Path,
    value: &str,
    executable: &Executable,
    platform: Platform,
) -> Result<(), CoreError> {
    if matches!(executable, Executable::Path(_)) && !file_path(root, value, platform).is_file() {
        return Err(
            CoreError::new(ErrorCode::InvalidRequest, "Program was not found").with_details(value),
        );
    }
    Ok(())
}

/// The text an inline script is saved as: LF lines for shells, CRLF lines
/// for `cmd`, which misreads labels in LF files, and a UTF-8 BOM for
/// PowerShell on Windows, which otherwise reads the file in the ANSI code page.
fn inline_content(kind: Kind, text: &str, platform: Platform) -> String {
    let mut content = text.replace("\r\n", "\n");
    if !content.ends_with('\n') {
        content.push('\n');
    }
    match kind {
        Kind::Batch => content.replace('\n', "\r\n"),
        Kind::PowerShell if platform.is_windows() => format!("\u{feff}{content}"),
        _ => content,
    }
}

/// Quotes one word of a `cmd` command line. `cmd` has no escape inside
/// quotes, so a quote is doubled, which batch scripts read back as written;
/// `%` still expands variables, as it does in any `cmd` command.
pub(crate) fn cmd_quote(argument: &str) -> String {
    if !argument.is_empty()
        && !argument
            .chars()
            .any(|character| character.is_whitespace() || "\"&|<>^(),;=%!".contains(character))
    {
        return argument.to_string();
    }
    format!("\"{}\"", argument.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh project directory, removed when dropped.
    struct Project(PathBuf);

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(name: &str) -> Project {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "lithe-process-run-{name}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Project(root)
    }

    fn entry(provider: &str, process: Value) -> Value {
        json!({
            "id": "user:test",
            "name": "Test",
            "provider": provider,
            "extensions": { "process": process },
        })
    }

    fn typed(value: &Value) -> RunConfiguration {
        serde_json::from_value(value.clone()).unwrap()
    }

    fn codes(value: &Value, platform: Platform) -> Vec<String> {
        field_errors(&typed(value), platform)
            .unwrap()
            .into_iter()
            .map(|error| format!("{}: {}", error.field, error.code))
            .collect()
    }

    #[test]
    fn shebang_lines_follow_linux_semantics() {
        let shebang = Shebang::parse(b"#!/usr/bin/env bash -e\necho\n").unwrap();
        assert_eq!(shebang.interpreter, "/usr/bin/env");
        assert_eq!(shebang.argument.as_deref(), Some("bash -e"));
        let shebang = Shebang::parse(b"\xEF\xBB\xBF#!/bin/zsh\r\n").unwrap();
        assert_eq!(shebang.interpreter, "/bin/zsh");
        assert_eq!(shebang.argument, None);
        assert!(Shebang::parse(b"echo hi\n").is_none());
        assert!(Shebang::parse(b"#!   \n").is_none());
    }

    #[test]
    fn automatic_interpreters_differ_by_platform() {
        let automatic = |kind, line: Option<&[u8]>, platform| {
            let shebang = line.and_then(Shebang::parse);
            let interpreter = automatic_interpreter(kind, shebang.as_ref(), platform);
            (interpreter.executable, interpreter.arguments)
        };
        assert_eq!(
            automatic(Kind::Shell, None, Platform::Unix),
            (Executable::Path("/bin/sh".into()), vec![])
        );
        assert_eq!(
            automatic(Kind::Shell, Some(b"#!/bin/bash -e"), Platform::Unix),
            (Executable::Path("/bin/bash".into()), vec!["-e".into()])
        );
        assert_eq!(
            automatic(
                Kind::Shell,
                Some(b"#!/usr/bin/env -S bash -e"),
                Platform::Windows
            ),
            (Executable::Command("bash".into()), vec!["-e".into()])
        );
        assert_eq!(
            automatic(Kind::Shell, Some(b"#!/bin/zsh"), Platform::Windows),
            (Executable::Command("zsh".into()), vec![])
        );
        assert_eq!(
            automatic(Kind::Shell, None, Platform::Windows),
            (Executable::Command("bash".into()), vec![])
        );
        let no_profile = vec!["-NoLogo".to_string(), "-NoProfile".to_string()];
        assert_eq!(
            automatic(Kind::PowerShell, None, Platform::Windows),
            (Executable::Command("powershell".into()), no_profile.clone())
        );
        assert_eq!(
            automatic(Kind::PowerShell, None, Platform::Unix),
            (Executable::Command("pwsh".into()), no_profile)
        );
        assert_eq!(
            automatic(Kind::Batch, None, Platform::Windows),
            (Executable::Command("cmd".into()), vec!["/d".into()])
        );
    }

    #[test]
    fn a_named_interpreter_replaces_the_automatic_options() {
        let project = project("interpreter");
        let shebang = Shebang::parse(b"#!/bin/bash -e");
        let mut options = ProcessOptions {
            interpreter: Some("zsh".into()),
            ..ProcessOptions::default()
        };
        let interpreter = effective_interpreter(
            Kind::Shell,
            &options,
            shebang.as_ref(),
            &project.0,
            Platform::Unix,
        );
        assert_eq!(interpreter.executable, Executable::Command("zsh".into()));
        assert!(interpreter.arguments.is_empty());
        options.interpreter_arguments = Some(vec!["-x".into()]);
        let interpreter = effective_interpreter(
            Kind::Shell,
            &options,
            shebang.as_ref(),
            &project.0,
            Platform::Unix,
        );
        assert_eq!(interpreter.arguments, ["-x"]);
        let interpreter = effective_interpreter(
            Kind::PowerShell,
            &ProcessOptions {
                interpreter: Some("tools/pwsh".into()),
                ..ProcessOptions::default()
            },
            None,
            &project.0,
            Platform::Unix,
        );
        assert_eq!(
            interpreter.executable,
            Executable::Path(project.0.join("tools/pwsh").to_string_lossy().into_owned())
        );
        assert_eq!(interpreter.arguments, ["-NoLogo", "-NoProfile"]);
    }

    #[test]
    fn invalid_values_are_named_by_field() {
        let unix = Platform::Unix;
        assert_eq!(
            codes(
                &entry(SHELL, json!({ "script": "a.sh", "text": "echo" })),
                unix
            ),
            ["source: conflict"]
        );
        assert_eq!(codes(&entry(SHELL, json!({})), unix), ["script: required"]);
        assert_eq!(
            codes(&entry(SHELL, json!({ "script": "../a.sh" })), unix),
            ["script: invalidPath"]
        );
        assert_eq!(
            codes(
                &entry(
                    SHELL,
                    json!({ "script": "a.sh", "program": "x", "executionPolicyBypass": false })
                ),
                unix
            ),
            ["program: notAllowed", "executionPolicyBypass: notAllowed"]
        );
        assert_eq!(
            codes(&entry(EXECUTABLE, json!({ "script": "a.sh" })), unix),
            ["program: required", "script: notAllowed"]
        );
        assert!(codes(
            &entry(
                POWERSHELL,
                json!({ "script": "a.ps1", "executionPolicyBypass": false })
            ),
            unix
        )
        .is_empty());
        let mut environment = entry(EXECUTABLE, json!({ "program": "ls" }));
        environment["env"] = json!({ "A=B": "1" });
        assert_eq!(codes(&environment, unix), ["environment: invalidName"]);
        let mut arguments = entry(EXECUTABLE, json!({ "program": "ls" }));
        arguments["args"] = json!(vec!["x"; MAXIMUM_ARGUMENTS + 1]);
        assert_eq!(codes(&arguments, unix), ["programArguments: tooLong"]);
        let mut command = entry(EXECUTABLE, json!({ "program": "ls" }));
        command["command"] = json!("ls");
        let error = field_errors(&typed(&command), unix).unwrap_err();
        assert!(matches!(error.code, ErrorCode::InvalidRequest));
        let error = field_errors(&typed(&entry("process.python", json!({}))), unix).unwrap_err();
        assert!(matches!(error.code, ErrorCode::NotSupported));
    }

    #[test]
    fn shell_scripts_start_with_their_shebang_interpreter() {
        let project = project("shell");
        fs::create_dir_all(project.0.join("scripts")).unwrap();
        fs::write(project.0.join("scripts/build.sh"), "#!/bin/bash -e\necho\n").unwrap();
        let mut value = entry(SHELL, json!({ "script": "scripts/build.sh" }));
        value["args"] = json!(["--fast", "a b"]);
        value["cwd"] = json!("scripts");
        let plan = launch_plan(&project.0, &value, Platform::Unix, None).unwrap();
        let script = project
            .0
            .join("scripts/build.sh")
            .to_string_lossy()
            .into_owned();
        assert_eq!(plan["executable"], json!({ "path": "/bin/bash" }));
        assert_eq!(plan["arguments"], json!(["-e", script, "--fast", "a b"]));
        assert_eq!(plan["workingDirectory"], "scripts");
        assert!(plan.get("inlineScript").is_none());
        assert!(plan.get("commandLine").is_none());
    }

    #[test]
    fn a_missing_script_or_program_fails_the_launch() {
        let project = project("missing");
        let error = launch_plan(
            &project.0,
            &entry(SHELL, json!({ "script": "gone.sh" })),
            Platform::Unix,
            None,
        )
        .unwrap_err();
        assert_eq!(error.message, "Script file was not found");
        let error = launch_plan(
            &project.0,
            &entry(EXECUTABLE, json!({ "program": "./bin/tool" })),
            Platform::Unix,
            None,
        )
        .unwrap_err();
        assert_eq!(error.message, "Program was not found");
    }

    #[test]
    fn programs_start_by_name_or_path() {
        let project = project("program");
        fs::write(project.0.join("tool"), "").unwrap();
        let mut value = entry(EXECUTABLE, json!({ "program": "ls" }));
        value["args"] = json!(["-l"]);
        let plan = launch_plan(&project.0, &value, Platform::Unix, None).unwrap();
        assert_eq!(plan["executable"], json!({ "command": "ls" }));
        assert_eq!(plan["arguments"], json!(["-l"]));
        let plan = launch_plan(
            &project.0,
            &entry(EXECUTABLE, json!({ "program": "./tool" })),
            Platform::Unix,
            None,
        )
        .unwrap();
        let path = project.0.join("./tool").to_string_lossy().into_owned();
        assert_eq!(plan["executable"], json!({ "path": path }));
    }

    #[test]
    fn powershell_bypasses_the_execution_policy_only_on_windows_and_when_asked() {
        let project = project("powershell");
        fs::write(project.0.join("deploy.ps1"), "Write-Host hi\n").unwrap();
        let arguments = |process: Value, platform| {
            launch_plan(&project.0, &entry(POWERSHELL, process), platform, None).unwrap()
                ["arguments"]
                .clone()
        };
        let windows_script = format!("{}\\deploy.ps1", project.0.to_string_lossy());
        assert_eq!(
            arguments(json!({ "script": "deploy.ps1" }), Platform::Windows),
            json!([
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                windows_script
            ])
        );
        assert_eq!(
            arguments(
                json!({ "script": "deploy.ps1", "executionPolicyBypass": false }),
                Platform::Windows
            ),
            json!(["-NoLogo", "-NoProfile", "-File", windows_script])
        );
        let unix_script = project.0.join("deploy.ps1").to_string_lossy().into_owned();
        assert_eq!(
            arguments(json!({ "script": "deploy.ps1" }), Platform::Unix),
            json!(["-NoLogo", "-NoProfile", "-File", unix_script])
        );
    }

    #[test]
    fn batch_scripts_carry_the_exact_cmd_command_line() {
        let project = project("batch");
        fs::write(project.0.join("run.cmd"), "@echo off\r\n").unwrap();
        let mut value = entry(BATCH, json!({ "script": "run.cmd" }));
        value["args"] = json!(["a b", "say \"hi\"", "plain"]);
        let plan = launch_plan(&project.0, &value, Platform::Windows, None).unwrap();
        let script = format!("{}\\run.cmd", project.0.to_string_lossy());
        assert_eq!(plan["executable"], json!({ "command": "cmd" }));
        assert_eq!(
            plan["arguments"],
            json!(["/d", "/s", "/c", script, "a b", "say \"hi\"", "plain"])
        );
        assert_eq!(
            plan["commandLine"],
            format!(
                "cmd /d /s /c \"{} \"a b\" \"say \"\"hi\"\"\" plain\"",
                cmd_quote(&script)
            )
        );
        let error = launch_plan(&project.0, &value, Platform::Unix, None).unwrap_err();
        assert!(matches!(error.code, ErrorCode::NotSupported));
    }

    #[test]
    fn cmd_words_are_quoted_only_when_cmd_would_split_them() {
        assert_eq!(cmd_quote("plain"), "plain");
        assert_eq!(cmd_quote(r"C:\Tools\run.cmd"), r"C:\Tools\run.cmd");
        assert_eq!(cmd_quote(""), "\"\"");
        assert_eq!(cmd_quote("a b"), "\"a b\"");
        assert_eq!(cmd_quote("a&b"), "\"a&b\"");
        assert_eq!(cmd_quote("50%"), "\"50%\"");
        assert_eq!(cmd_quote("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn inline_scripts_are_written_in_the_form_their_interpreter_reads() {
        let project = project("inline");
        let plan = |provider, platform, base| {
            launch_plan(
                &project.0,
                &entry(provider, json!({ "text": "#!/bin/bash\r\necho hi" })),
                platform,
                Some(base),
            )
        };
        let shell = plan(SHELL, Platform::Unix, "/tmp/lithe-run-1").unwrap();
        assert_eq!(
            shell["inlineScript"],
            json!({ "path": "/tmp/lithe-run-1.sh", "content": "#!/bin/bash\necho hi\n" })
        );
        assert_eq!(shell["executable"], json!({ "path": "/bin/bash" }));
        assert_eq!(shell["arguments"], json!(["/tmp/lithe-run-1.sh"]));
        let batch = plan(BATCH, Platform::Windows, r"C:\Temp\lithe-run-1").unwrap();
        assert_eq!(batch["inlineScript"]["path"], r"C:\Temp\lithe-run-1.cmd");
        assert_eq!(
            batch["inlineScript"]["content"],
            "#!/bin/bash\r\necho hi\r\n"
        );
        let powershell = plan(POWERSHELL, Platform::Windows, r"C:\Temp\lithe-run-1").unwrap();
        assert!(powershell["inlineScript"]["content"]
            .as_str()
            .unwrap()
            .starts_with('\u{feff}'));
        let powershell = plan(POWERSHELL, Platform::Unix, "/tmp/lithe-run-1").unwrap();
        assert!(!powershell["inlineScript"]["content"]
            .as_str()
            .unwrap()
            .starts_with('\u{feff}'));
        let error = plan(SHELL, Platform::Unix, "relative/base").unwrap_err();
        assert_eq!(
            error.message,
            "Inline scripts need an absolute temporary script location"
        );
    }

    #[test]
    fn default_names_come_from_the_file_name() {
        let name = |script: Option<&str>, program: Option<&str>| {
            default_name(&ProcessOptions {
                script: script.map(str::to_string),
                program: program.map(str::to_string),
                ..ProcessOptions::default()
            })
        };
        assert_eq!(
            name(Some("scripts/build.sh"), None).as_deref(),
            Some("build")
        );
        assert_eq!(
            name(None, Some(r"C:\Tools\app.exe")).as_deref(),
            Some("app")
        );
        assert_eq!(name(None, Some(".env")).as_deref(), Some(".env"));
        assert_eq!(name(None, None), None);
    }
}
