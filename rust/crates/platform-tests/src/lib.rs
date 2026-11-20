//! Tests that need a real Windows or Unix host.
//!
//! Platform rules that can be written as data live in `lithe-platform` and
//! are unit-tested on every build machine. What only a real operating system
//! can show, such as process trees, `cmd.exe` argument parsing and permission
//! bits, is tested in this package. A build machine for one platform can run
//! `cargo test -p lithe-platform -p lithe-platform-tests` instead of the whole
//! workspace.
//!
//! The tests start only the `lithe-platform-fixture` program that this
//! package builds, never an installed tool. This library holds the protocol
//! the fixture and the tests share.

use std::time::Duration;

/// What the fixture program does, selected by its first argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Writes [`READY_PREFIX`] and its process ID as one line, then runs
    /// until its standard input closes, like a language server.
    Hold,
    /// Starts a [`Mode::Hold`] child that shares its standard streams, then
    /// waits for it: a two-level tree like `cmd.exe` starting `java`. The
    /// ready line therefore comes from the child, and the output stream
    /// closes only once both processes have exited.
    Parent,
    /// Writes [`FAILURE_OUTPUT`] and [`FAILURE_ERROR`] to its standard
    /// streams and exits with [`FAILURE_EXIT_CODE`].
    Failure,
    /// Writes [`STREAM_LINE`] lines until writing fails or
    /// [`STREAM_DURATION`] passes.
    Stream,
    /// Writes its remaining arguments, one per line, to the file named by
    /// [`ECHO_FILE_ENV`], exactly as the platform passed them.
    EchoArguments,
    /// Runs the Git AskPass helper for the prompt in its next argument, as
    /// Git or SSH would, and exits with the helper's exit code.
    AskPass,
}

impl Mode {
    /// Every mode, in declaration order.
    pub const ALL: [Self; 6] = [
        Self::Hold,
        Self::Parent,
        Self::Failure,
        Self::Stream,
        Self::EchoArguments,
        Self::AskPass,
    ];

    /// The argument that selects this mode.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Hold => "hold",
            Self::Parent => "parent",
            Self::Failure => "failure",
            Self::Stream => "stream",
            Self::EchoArguments => "echo-arguments",
            Self::AskPass => "askpass",
        }
    }

    /// The mode that `name` selects.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == name)
    }
}

/// Starts the line a [`Mode::Hold`] fixture writes once it runs.
pub const READY_PREFIX: &str = "ready ";

/// Standard output of a [`Mode::Failure`] fixture.
pub const FAILURE_OUTPUT: &str = "fixture output";

/// Standard error of a [`Mode::Failure`] fixture.
pub const FAILURE_ERROR: &str = "fixture error";

/// Exit code of a [`Mode::Failure`] fixture.
pub const FAILURE_EXIT_CODE: i32 = 23;

/// The line a [`Mode::Stream`] fixture repeats.
pub const STREAM_LINE: &str = "fixture-ready";

/// How long a [`Mode::Stream`] fixture writes when nobody stops it.
pub const STREAM_DURATION: Duration = Duration::from_secs(3);

/// Environment variable that names the file a [`Mode::EchoArguments`]
/// fixture writes.
pub const ECHO_FILE_ENV: &str = "LITHE_PLATFORM_FIXTURE_ECHO_FILE";

/// Longest time any fixture runs. A test that fails to end its fixture
/// therefore leaves no process behind for longer than this.
pub const LIFETIME_LIMIT: Duration = Duration::from_secs(30);

/// Exit code of a fixture that reached [`LIFETIME_LIMIT`].
pub const LIFETIME_EXIT_CODE: i32 = 124;

/// Parses the process ID from a line that a [`Mode::Hold`] fixture wrote.
pub fn ready_process_id(line: &str) -> Option<u32> {
    line.trim_end().strip_prefix(READY_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{ready_process_id, Mode, READY_PREFIX};

    #[test]
    fn every_mode_is_selected_by_its_own_name() {
        for mode in Mode::ALL {
            assert_eq!(Mode::from_name(mode.name()), Some(mode));
        }
        assert_eq!(Mode::from_name("unknown"), None);
    }

    #[test]
    fn ready_lines_carry_the_process_id() {
        assert_eq!(ready_process_id(&format!("{READY_PREFIX}42\r\n")), Some(42));
        assert_eq!(ready_process_id("ready"), None);
        assert_eq!(ready_process_id("other 42"), None);
    }
}
