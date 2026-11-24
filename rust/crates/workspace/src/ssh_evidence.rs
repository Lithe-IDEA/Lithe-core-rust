//! Owner-only, bounded evidence from the actual local OpenSSH process.

use lithe_git_host::private_directory::PrivateDirectory;
use lithe_remote::client::ssh_identity::{self, Identity};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek};
use std::path::PathBuf;

/// The helper only writes a private log; it is never parsed as shell code.
pub(crate) struct Evidence {
    file: File,
    path: PathBuf,
    _directory: PrivateDirectory,
}

impl Evidence {
    pub(crate) fn new() -> io::Result<Self> {
        let directory = PrivateDirectory::new()?;
        let path = directory.path().join("ssh.log");
        let file = lithe_platform::fs::owner_only_file(
            OpenOptions::new().read(true).write(true).create_new(true),
        )
        .open(&path)?;
        Ok(Self {
            file,
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

    pub(crate) fn identity(&mut self, pid: u32) -> Option<Identity> {
        let read = (|| -> io::Result<Vec<u8>> {
            self.file.rewind()?;
            let mut bytes = Vec::new();
            self.file
                .by_ref()
                .take(ssh_identity::MAX_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        })();
        match read {
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

    pub(crate) fn diagnostics(&mut self) -> Vec<u8> {
        let result = (|| -> io::Result<Vec<u8>> {
            self.file.rewind()?;
            let mut bytes = Vec::new();
            self.file
                .by_ref()
                .take(ssh_identity::MAX_BYTES as u64)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        })();
        match result {
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
