//! Account-local identity of the actual server state, before any session token.

use super::{daemon::private_directory, platform};
use crate::protocol::methods::encode_hex;
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

/// Prints a non-secret stable identity of this account's server state. A
/// new host, default account or container state gets a different identity.
/// The file is published without replacement so concurrent launches agree.
pub fn print(root: &Path) -> io::Result<()> {
    private_directory(root)?;
    let identity_path = root.join("target-identity-v1");
    let temporary = root.join(format!(".target-identity-{}", std::process::id()));
    let mut created_temporary = false;
    let result = (|| {
        if !identity_path.exists() {
            let mut random = [0u8; 32];
            fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&temporary)?;
            created_temporary = true;
            file.write_all(encode_hex(&random).as_bytes())?;
            file.sync_all()?;
            match fs::hard_link(&temporary, &identity_path) {
                Ok(()) => {}
                // log-exempt: Expected absence: the publication slot is already occupied; all launchers read that same identity below.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&identity_path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != platform::current_uid()
            || metadata.mode() & 0o077 != 0
            || metadata.len() != 64
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "invalid target identity file",
            ));
        }
        let mut bytes = [0u8; 64];
        file.read_exact(&mut bytes)?;
        if !bytes.iter().all(u8::is_ascii_hexdigit) {
            return Err(io::Error::other("invalid target identity"));
        }
        let mut digest = Sha256::new();
        digest.update(b"lithe.server.target.v1\0");
        digest.update(bytes);
        digest.update(platform::current_uid().to_be_bytes());
        digest.update(fs::canonicalize(root)?.as_os_str().as_encoded_bytes());
        writeln!(io::stdout(), "{}", encode_hex(&digest.finalize()))
    })();
    if created_temporary {
        fs::remove_file(&temporary)?;
    }
    result
}
