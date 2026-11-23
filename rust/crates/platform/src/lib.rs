//! Host platform rules expressed as data.
//!
//! Windows and Unix differ in file names, path comparison, permissions and in
//! how a background process starts. Code that depends on such a rule takes a
//! [`Platform`] value instead of compiling one branch per target, so every
//! rule is unit-tested on every build machine. Production code passes
//! [`Platform::HOST`]; tests pass the platform whose rule they check.
//!
//! The few calls that need an operating-system API, such as Unix permission
//! bits, the account behind a Unix socket or Windows process creation flags,
//! stay behind `cfg` inside this crate. Tests that need a real Windows or Unix
//! host to observe them live in the `lithe-platform-tests` package.

pub mod cli;
pub mod entries;
pub mod fs;
pub mod names;
pub mod paths;
pub mod process;
#[cfg(unix)]
pub mod socket;

/// A family of host operating systems that share the rules in this crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Platform {
    /// Linux, macOS and every other target that is not Windows.
    Unix,
    /// Windows.
    Windows,
}

impl Platform {
    /// Every platform, for rules that must hold on each of them.
    pub const ALL: [Self; 2] = [Self::Unix, Self::Windows];

    /// The platform this binary was compiled for.
    pub const HOST: Self = if cfg!(windows) {
        Self::Windows
    } else {
        Self::Unix
    };

    /// Whether this is [`Platform::Windows`].
    pub const fn is_windows(self) -> bool {
        matches!(self, Self::Windows)
    }

    /// File name of the executable called `stem`, as a `PATH` search finds it.
    pub fn executable_name(self, stem: &str) -> String {
        match self {
            Self::Windows => format!("{stem}.exe"),
            Self::Unix => stem.to_owned(),
        }
    }

    /// The device that discards writes and reads as empty.
    pub const fn null_device(self) -> &'static str {
        match self {
            Self::Windows => "NUL",
            Self::Unix => "/dev/null",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Platform;

    #[test]
    fn host_matches_the_compilation_target() {
        assert_eq!(Platform::HOST.is_windows(), cfg!(windows));
    }

    #[test]
    fn executable_names_follow_the_platform() {
        assert_eq!(Platform::Windows.executable_name("git"), "git.exe");
        assert_eq!(Platform::Unix.executable_name("git"), "git");
    }

    #[test]
    fn null_devices_follow_the_platform() {
        assert_eq!(Platform::Windows.null_device(), "NUL");
        assert_eq!(Platform::Unix.null_device(), "/dev/null");
    }
}
