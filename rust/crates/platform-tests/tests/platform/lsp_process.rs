//! Local language servers that `SystemProcessLauncher` starts.

use crate::support::{OutputWatch, TempDir, DEADLINE, FIXTURE};
use lithe_lsp_transport::process::{
    LspProcessHandle, LspProcessLauncher, LspProcessSpec, SystemProcessLauncher,
};
use lithe_platform_tests::Mode;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

/// A launched server that is disposed when the test ends, also when an
/// assertion fails first.
struct Server {
    handle: Arc<dyn LspProcessHandle>,
    output: OutputWatch,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.close_input();
        let _ = self.handle.dispose(DEADLINE);
    }
}

fn launch(
    executable: PathBuf,
    arguments: &[&str],
    directory: &TempDir,
    environment: BTreeMap<String, String>,
) -> Server {
    let streams = SystemProcessLauncher
        .launch(LspProcessSpec {
            executable,
            remote_commands: Vec::new(),
            remote_executable: None,
            remote_path_prepend: Vec::new(),
            arguments: arguments
                .iter()
                .map(|argument| argument.to_string())
                .collect(),
            working_directory: directory.path().to_path_buf(),
            environment,
        })
        .expect("launch the language server");
    Server {
        handle: streams.handle,
        output: OutputWatch::new(streams.output),
    }
}

/// A server whose process starts the actual runtime, like a launcher script.
fn launch_parent(directory: &TempDir) -> Server {
    let server = launch(
        PathBuf::from(FIXTURE),
        &[Mode::Parent.name()],
        directory,
        BTreeMap::new(),
    );
    server.output.ready();
    server
}

#[test]
fn terminating_a_language_server_ends_the_processes_it_started() {
    let directory = TempDir::new("lithe lsp terminate");
    let server = launch_parent(&directory);

    server.handle.terminate();

    // The runtime shares the output pipe, so the stream closes only once the
    // runtime is gone too, not just the process that started it.
    server
        .output
        .wait_closed("terminating the server should end the process it started");
    assert!(server.handle.take_termination_failure().is_none());
    server
        .handle
        .dispose(DEADLINE)
        .expect("a terminated server is disposed");
}

#[test]
fn disposing_a_language_server_ends_the_processes_it_started() {
    let directory = TempDir::new("lithe lsp dispose");
    let server = launch_parent(&directory);

    server
        .handle
        .dispose(DEADLINE)
        .expect("every process of the server ends");

    server
        .output
        .wait_closed("disposing the server should end the process it started");
    assert!(server.handle.exit_status().is_some());
}

#[cfg(windows)]
mod batch {
    //! `cmd.exe` scripts that start the actual language server.

    use super::{launch, TempDir, FIXTURE};
    use lithe_platform_tests::{Mode, ECHO_FILE_ENV};
    use std::collections::BTreeMap;

    /// Variable through which a test script finds the fixture program.
    const FIXTURE_ENV: &str = "LITHE_TEST_FIXTURE";

    fn script(directory: &TempDir, name: &str, mode: Mode) -> std::path::PathBuf {
        let path = directory.path().join(name);
        std::fs::write(
            &path,
            format!("@echo off\r\n\"%{FIXTURE_ENV}%\" {} %*\r\n", mode.name()),
        )
        .expect("batch script");
        path
    }

    #[test]
    fn batch_language_server_preserves_spaced_paths_and_shell_metacharacters() {
        let directory = TempDir::new("lithe lsp batch");
        // The script forwards `%*` to a native Windows program, which parses
        // the command line the way a real native language server does.
        let executable = script(&directory, "scripted server.cmd", Mode::EchoArguments);
        let echo_file = directory.path().join("arguments.txt");
        let arguments = [
            "workspace&echo_injected",
            "percent%PATH%value",
            "bang!value",
            "caret^value",
            "paren(value)",
            "quoted\"value",
            r"C:\workspace data\",
        ];
        let server = launch(
            executable,
            &arguments,
            &directory,
            BTreeMap::from([
                (FIXTURE_ENV.to_string(), FIXTURE.to_string()),
                (
                    ECHO_FILE_ENV.to_string(),
                    echo_file.to_string_lossy().into_owned(),
                ),
            ]),
        );
        server.handle.close_input();
        server
            .output
            .wait_closed("the script and the program it starts should exit");

        let echoed = std::fs::read_to_string(&echo_file).expect("echoed arguments");
        assert_eq!(echoed, arguments.join("\n"));
    }

    #[test]
    fn terminating_a_batch_language_server_ends_the_process_it_started() {
        let directory = TempDir::new("lithe lsp batch tree");
        let executable = script(&directory, "holding server.cmd", Mode::Hold);
        let server = launch(
            executable,
            &[],
            &directory,
            BTreeMap::from([(FIXTURE_ENV.to_string(), FIXTURE.to_string())]),
        );
        server.output.ready();

        server.handle.terminate();

        // The program holds the stdout pipe; it closes only once that program
        // is gone too, not just the cmd.exe that started it.
        server
            .output
            .wait_closed("terminating the server should end the process its script started");
    }
}
