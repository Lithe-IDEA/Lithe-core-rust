//! Windows AskPass channel: a local named pipe that only the current user can
//! open.
//!
//! The pipe's security descriptor makes the current user its owner and
//! grants access to nobody else, and the kernel refuses clients on other
//! machines. Names are random and the first instance is created exclusively,
//! so a name that another process already holds is skipped rather than
//! shared. The helper checks the pipe's owner before it sends the session
//! token, so a pipe that another account created never receives it.

use super::{random_hex, Accept};
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::null_mut;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_BUSY,
    ERROR_PIPE_CONNECTED, ERROR_PIPE_LISTENING, ERROR_PIPE_NOT_CONNECTED, ERROR_SUCCESS,
    GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TokenUser, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, READ_CONTROL, SECURITY_IDENTIFICATION,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PeekNamedPipe, WaitNamedPipeW, PIPE_NOWAIT,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Every local named pipe lives under this prefix.
const PREFIX: &str = r"\\.\pipe\";
/// Each direction buffers one maximum-size frame.
const BUFFER_SIZE: u32 = 16 * 1024;
/// Accepted prompts, the listening instance, and the replacement that is
/// created before a connected instance is handed over.
const MAX_INSTANCES: u32 = 8;
/// Name collisions are retried with a fresh random suffix this many times.
const CREATE_ATTEMPTS: usize = 4;
/// How long the helper waits for a free pipe instance.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Creates a directory with the same protected owner-only DACL as AskPass.
pub(crate) fn private_directory(path: &std::path::Path) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::CreateDirectoryW;
    let descriptor = Descriptor::owner_only(&CurrentUser::query()?)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0 .0,
        bInheritHandle: 0,
    };
    let path = wide(path.as_os_str());
    // SAFETY: both the NUL-terminated path and security descriptor outlive
    // this call; CreateDirectoryW copies the descriptor into the directory.
    if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Listening pipe for one session. Dropping it closes the last listening
/// instance, which removes the pipe name once accepted instances close too.
pub(super) struct Listener {
    listening: OwnedHandle,
    name: Vec<u16>,
    address: OsString,
    descriptor: Descriptor,
}

impl Listener {
    /// Creates the first instance of a pipe with a fresh random name.
    pub(super) fn bind() -> io::Result<Self> {
        let descriptor = Descriptor::owner_only(&CurrentUser::query()?)?;
        let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
        for _ in 0..CREATE_ATTEMPTS {
            let address = OsString::from(format!("{PREFIX}lithe-askpass-{}", random_hex::<8>()?));
            let name = wide(&address);
            match instance(&name, &descriptor, true) {
                Ok(listening) => {
                    return Ok(Self {
                        listening,
                        name,
                        address,
                        descriptor,
                    })
                }
                // The first-instance flag reports a name in use as access denied.
                Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32) => {
                    last = error
                }
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    /// The pipe name the helper connects to.
    pub(super) fn address(&self) -> &OsStr {
        &self.address
    }

    /// Takes the connected instance, if any, without blocking.
    pub(super) fn accept(&mut self) -> io::Result<Accept<Stream>> {
        // SAFETY: the handle is a live pipe instance; nonblocking mode never
        // uses the overlapped structure.
        if unsafe { ConnectNamedPipe(self.listening.as_raw_handle(), null_mut()) } != 0 {
            // Nonzero only reports that the instance is ready for a client.
            return Ok(Accept::Empty);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error().map(|code| code as u32) {
            Some(ERROR_PIPE_LISTENING) => Ok(Accept::Empty),
            Some(ERROR_PIPE_CONNECTED) => {
                // Create the replacement first so the name keeps listening.
                let next = instance(&self.name, &self.descriptor, false)?;
                let connected = std::mem::replace(&mut self.listening, next);
                Ok(Accept::Peer(Stream(File::from(connected))))
            }
            Some(ERROR_NO_DATA) => {
                // A client connected and left before it was accepted; this
                // instance cannot serve another one.
                self.listening = instance(&self.name, &self.descriptor, false)?;
                Ok(Accept::Empty)
            }
            _ => Err(error),
        }
    }
}

/// The server end of one accepted connection, in nonblocking mode.
pub(super) struct Stream(File);

impl Read for Stream {
    /// Reads only what is already in the pipe, so the call never waits.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut available = 0u32;
        // SAFETY: only the count out-pointer is supplied and it references a
        // live local.
        let peeked = unsafe {
            PeekNamedPipe(
                self.0.as_raw_handle(),
                null_mut(),
                0,
                null_mut(),
                &mut available,
                null_mut(),
            )
        };
        if peeked == 0 {
            let error = io::Error::last_os_error();
            return match error.raw_os_error().map(|code| code as u32) {
                Some(ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_NO_DATA) => Ok(0),
                _ => Err(error),
            };
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let limit = buffer.len().min(available as usize);
        self.0.read(&mut buffer[..limit])
    }
}

impl Write for Stream {
    /// Writes what fits in the pipe buffer; a full buffer reports
    /// `WouldBlock` like a nonblocking socket.
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self.0.write(buffer)? {
            0 if !buffer.is_empty() => Err(io::ErrorKind::WouldBlock.into()),
            written => Ok(written),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Opens the helper's end of the pipe and checks that the current user owns
/// it before any byte is sent. Reads then wait until the session answers or
/// closes the pipe.
pub(super) fn connect(address: &OsStr) -> io::Result<File> {
    let local = address
        .to_str()
        .and_then(|text| text.strip_prefix(PREFIX))
        .is_some_and(|name| !name.is_empty() && !name.contains(['\\', '/']));
    if !local {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let name = wide(address);
    let deadline = Instant::now() + CONNECT_TIMEOUT;
    let pipe = loop {
        let opened = OpenOptions::new()
            .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL)
            // The server may identify this process but never act as it.
            .security_qos_flags(SECURITY_IDENTIFICATION)
            .open(address);
        match opened {
            Ok(pipe) => break pipe,
            Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                let wait = u32::try_from(remaining.as_millis())
                    .unwrap_or(u32::MAX)
                    .max(1);
                // SAFETY: the name is NUL-terminated. The next open reports
                // whatever made the wait fail.
                unsafe { WaitNamedPipeW(name.as_ptr(), wait) };
            }
            Err(error) => return Err(error),
        }
    };
    verify_owner(&pipe)?;
    Ok(pipe)
}

fn verify_owner(pipe: &File) -> io::Result<()> {
    let user = CurrentUser::query()?;
    let mut owner: PSID = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the handle is live and every out-pointer references a live
    // local; `owner` points into `descriptor`.
    let status = unsafe {
        GetSecurityInfo(
            pipe.as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let descriptor = Local(descriptor);
    // SAFETY: both SIDs stay valid while their buffers live.
    let same = !owner.is_null() && unsafe { EqualSid(owner, user.sid()) } != 0;
    drop(descriptor);
    if same {
        Ok(())
    } else {
        Err(io::ErrorKind::PermissionDenied.into())
    }
}

fn instance(name: &[u16], descriptor: &Descriptor, first: bool) -> io::Result<OwnedHandle> {
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0 .0,
        bInheritHandle: 0,
    };
    let open_mode = if first {
        PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE
    } else {
        PIPE_ACCESS_DUPLEX
    };
    // SAFETY: the name is NUL-terminated and the attributes and descriptor
    // outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
            MAX_INSTANCES,
            BUFFER_SIZE,
            BUFFER_SIZE,
            0,
            &attributes,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateNamedPipeW returned a new handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain([0]).collect()
}

/// The process token's user, kept in the buffer that `GetTokenInformation`
/// filled, because the SID points into it.
struct CurrentUser {
    buffer: Vec<usize>,
}

impl CurrentUser {
    fn query() -> io::Result<Self> {
        let mut token: HANDLE = null_mut();
        // SAFETY: the pseudo handle needs no closing and the out-pointer
        // references a live local.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: OpenProcessToken returned a new handle that nothing else owns.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut length = 0u32;
        // SAFETY: a null buffer of size zero only asks for the needed length;
        // the expected insufficient-buffer failure leaves `length` set.
        unsafe {
            GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length)
        };
        if length == 0 {
            return Err(io::Error::last_os_error());
        }
        let word = std::mem::size_of::<usize>();
        let mut buffer = vec![0usize; (length as usize).div_ceil(word)];
        // SAFETY: the buffer is pointer-aligned and at least `length` bytes.
        let filled = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * word) as u32,
                &mut length,
            )
        };
        if filled == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn sid(&self) -> PSID {
        // SAFETY: the buffer holds the TOKEN_USER that query() read.
        unsafe { (*self.buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }

    fn sid_string(&self) -> io::Result<String> {
        let mut text = null_mut();
        // SAFETY: the SID is valid while `self` lives and the out-pointer
        // references a live local.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let allocation = Local(text.cast());
        // SAFETY: the system returned a NUL-terminated string.
        let length = (0..)
            .take_while(|&index| unsafe { *text.add(index) } != 0)
            .count();
        // SAFETY: `length` characters precede the terminator.
        let value = String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
            .map_err(io::Error::other);
        drop(allocation);
        value
    }
}

/// A security descriptor whose owner is the current user and whose protected
/// DACL grants that user full access and nobody else any.
struct Descriptor(Local);

impl Descriptor {
    fn owner_only(user: &CurrentUser) -> io::Result<Self> {
        let sid = user.sid_string()?;
        let sddl: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})")
            .encode_utf16()
            .chain([0])
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the SDDL string is NUL-terminated and the out-pointer
        // references a live local.
        let converted = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        };
        if converted == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(Local(descriptor)))
    }
}

/// Memory the system allocated with `LocalAlloc`, freed once when dropped.
struct Local(*mut core::ffi::c_void);

impl Drop for Local {
    fn drop(&mut self) {
        // SAFETY: the pointer came from a system allocation owned only here.
        unsafe { LocalFree(self.0) };
    }
}
