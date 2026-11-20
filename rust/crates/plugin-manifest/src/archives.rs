//! Pinned download archives: the platform target, source URLs, digest,
//! format and layout paths that `managedRuntimes` dependencies and
//! `commandInstallers` downloads share. Core validates the declaration and
//! selects the archive for the host's platform; the host downloads, checks the
//! digest and extracts.

use super::*;
use lithe_platform::Platform;
use url::Url;

/// Most platform archives one dependency or download declares: one per
/// supported operating system and architecture.
pub(crate) const MAX_ARTIFACTS: usize = 6;

/// Most fallback URLs one archive declares.
pub(crate) const MAX_MIRRORS: usize = 8;

/// Operating systems an archive may target.
pub(crate) const ARCHIVE_OPERATING_SYSTEMS: &[&str] = &["macos", "windows", "linux"];

/// Longest archive URL, in bytes.
const MAX_URL_BYTES: usize = 4096;

/// Longest layout path inside an archive, in bytes.
const MAX_LAYOUT_PATH_BYTES: usize = 512;

/// Text that is not empty, at most `maximum` bytes, without surrounding
/// whitespace or control characters.
pub(crate) fn bounded_text(value: &str, maximum: usize) -> bool {
    !value.is_empty()
        && value.len() <= maximum
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

/// A layout path inside an archive: empty, or `/`-separated parts without an
/// empty, `.` or `..` part, backslash or colon, so it means the same on every
/// host. Native extraction also checks filesystem aliases and link targets.
pub(crate) fn is_archive_layout_path(value: &str) -> bool {
    value.is_empty()
        || (bounded_text(value, MAX_LAYOUT_PATH_BYTES)
            && !value.contains(['\\', ':'])
            && value
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}

/// Whether `os` and `arch` name a platform an archive may target.
pub(crate) fn is_archive_platform(os: &str, arch: &str) -> bool {
    ARCHIVE_OPERATING_SYSTEMS.contains(&os) && KNOWN_ARCHITECTURES.contains(&arch)
}

/// The host platform a request selects an archive for.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ArchivePlatform {
    /// macos, windows or linux.
    pub(crate) os: String,
    /// arm64 or x86_64.
    pub(crate) arch: String,
}

impl ArchivePlatform {
    /// Whether an archive may target this platform.
    pub(crate) fn is_supported(&self) -> bool {
        is_archive_platform(&self.os, &self.arch)
    }

    /// Whether an archive declared for `os` and `arch` is this platform's.
    pub(crate) fn matches(&self, os: &str, arch: &str) -> bool {
        self.os == os && self.arch == arch
    }

    /// The path rules of this platform's filesystem.
    pub(crate) fn native_paths(&self) -> Platform {
        if self.os == "windows" {
            Platform::Windows
        } else {
            Platform::Unix
        }
    }
}

/// Where one archive comes from and how to read it.
pub(crate) struct ArchiveSource<'a> {
    pub(crate) url: &'a str,
    pub(crate) mirrors: &'a [String],
    pub(crate) sha256: &'a mut String,
    pub(crate) archive: &'a str,
}

/// Checks one archive's URL, mirrors, digest and format, and lowercases the
/// digest. `mirrors_allowed` runs only when the archive declares mirrors and
/// rejects them when the manifest's capability version does not have them.
pub(crate) fn validate_archive_source(
    field: &str,
    source: ArchiveSource<'_>,
    mirrors_allowed: impl FnOnce() -> Validation<()>,
) -> Validation<()> {
    if !is_https_source(source.url) {
        return Err(invalid(
            format!("{field}.url"),
            "An archive URL uses HTTPS without credentials or fragments.",
        ));
    }
    if !source.mirrors.is_empty() {
        mirrors_allowed()?;
        if source.mirrors.len() > MAX_MIRRORS {
            return Err(invalid(
                format!("{field}.mirrors"),
                format!("An archive declares at most {MAX_MIRRORS} mirror URLs."),
            ));
        }
        let mut seen = BTreeSet::new();
        seen.insert(source.url);
        for (index, mirror) in source.mirrors.iter().enumerate() {
            let entry = format!("{field}.mirrors[{index}]");
            if !is_https_source(mirror) {
                return Err(invalid(
                    entry,
                    "A mirror URL uses HTTPS without credentials or fragments.",
                ));
            }
            if !seen.insert(mirror.as_str()) {
                return Err(invalid(
                    entry,
                    "Mirror URLs are unique and differ from the primary URL.",
                ));
            }
        }
    }
    if source.sha256.len() != 64 || !source.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid(
            format!("{field}.sha256"),
            "An archive pins its complete SHA-256 digest.",
        ));
    }
    source.sha256.make_ascii_lowercase();
    if !["zip", "tar.gz"].contains(&source.archive) {
        return Err(invalid(
            format!("{field}.archive"),
            "An archive format is zip or tar.gz.",
        ));
    }
    Ok(())
}

/// An HTTPS URL with a host, without credentials or a fragment.
fn is_https_source(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    bounded_text(value, MAX_URL_BYTES)
        && url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}
