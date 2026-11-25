//! Owner-only, bounded evidence from the actual local OpenSSH process.

use lithe_git_host::private_directory::PrivateDirectory;
use lithe_remote::client::ssh_identity::{self, Identity};
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::path::PathBuf;

/// The helper only writes a private log; it is never parsed as shell code.
///
/// No open write handle is retained after creation: Win32-OpenSSH opens its
/// `-E` log file without `FILE_SHARE_WRITE`, so holding a writable handle in
/// the parent causes `ssh.exe` to fail with `ERROR_SHARING_VIOLATION` (32,
/// reported by the CRT as `Broken pipe`).
pub(crate) struct Evidence {
    path: PathBuf,
    _directory: PrivateDirectory,
}

impl Evidence {
    pub(crate) fn new() -> io::Result<Self> {
        let directory = PrivateDirectory::new()?;
        let path = directory.path().join("ssh.log");
        // Pre-create with owner-only permissions and close immediately so the
        // child OpenSSH process is the sole writer.
        drop(
            lithe_platform::fs::owner_only_file(OpenOptions::new().write(true).create_new(true))
                .open(&path)?,
        );
        Ok(Self {
            path,
            _directory: directory,
        })
    }

    /// Inserted before all hop options. OpenSSH reads the user's routing and
    /// host-key policy normally; these flags only capture its own evidence.
    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut arguments = vec!["-E".into(), self.path.to_string_lossy().into_owned()];
        for option in ssh_identity::OPTIONS {
            arguments.extend(["-o".into(), (*option).into()]);
        }
        arguments
    }

    /// Opens the evidence log read-only and reads at most `limit` bytes.
    ///
    /// On Windows, `OpenOptions::new().read(true)` defaults to
    /// `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, which allows
    /// reading while `ssh.exe` still holds the log open for writing.
    fn read_bounded(&self, limit: u64) -> io::Result<Vec<u8>> {
        let mut file = OpenOptions::new().read(true).open(&self.path)?;
        let mut bytes = Vec::new();
        file.by_ref().take(limit).read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    pub(crate) fn identity(&self, pid: u32) -> Option<Identity> {
        match self.read_bounded(ssh_identity::MAX_BYTES as u64 + 1) {
            Ok(bytes) => ssh_identity::parse(&bytes, pid),
            Err(error) => {
                lithe_diagnostics::core_events::recovery_failed(
                    "remote.session.diagnostics_failed",
                    "identity_read",
                    &format!("{:?}", error.kind()),
                    "io",
                );
                None
            }
        }
    }

    pub(crate) fn diagnostics(&self) -> Vec<u8> {
        match self.read_bounded(ssh_identity::MAX_BYTES as u64) {
            Ok(bytes) => bytes,
            Err(error) => {
                lithe_diagnostics::core_events::recovery_failed(
                    "remote.session.diagnostics_failed",
                    "identity_diagnostics",
                    &format!("{:?}", error.kind()),
                    "io",
                );
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn evidence_reads_diagnostics_and_identity_written_by_another_handle() {
        let evidence = Evidence::new().expect("evidence");
        let log_line = format!(
            "debug1: sshconnect.c:ssh_connect_direct():10 (pid=42): Connecting to box [127.0.0.1] port 22.\n\
             debug1: sshconnect.c:ssh_login():20 (pid=42): Authenticating to box:22 as 'dev'\n\
             debug1: sshconnect.c:verify_host_key():30 (pid=42): Server host key: ssh-ed25519 SHA256:{}\n\
             sshconnect2.c:ssh_userauth2():40 (pid=42): Authenticated to box ([127.0.0.1]:22) using \"publickey\".\n",
            "a".repeat(43)
        );
        let mut writer = OpenOptions::new()
            .append(true)
            .open(&evidence.path)
            .expect("open evidence log for append");
        writer
            .write_all(log_line.as_bytes())
            .expect("write evidence");
        writer.flush().expect("flush evidence");

        // Read while the writer handle is still open, matching a live ssh.exe.
        assert!(evidence.identity(42).is_some());
        assert_eq!(evidence.diagnostics(), log_line.as_bytes());
    }
}
