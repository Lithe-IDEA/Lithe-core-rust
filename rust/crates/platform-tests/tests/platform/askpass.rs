//! The Git AskPass helper as Git and SSH start it: a separate process that
//! finds its session through the environment the session put on the command,
//! and prints the answer on its standard output.

use crate::support::{fixture, DEADLINE};
use lithe_git_host::authentication::{respond, Challenge, Session};
use lithe_platform_tests::Mode;
use std::ffi::OsString;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::time::Instant;

/// The prompt Git shows for an HTTPS password.
const PROMPT: &str = "Password for 'https://example.invalid': ";
const CREDENTIAL: &str = "test-only-private-credential";
/// Variables that `Session::configure` sets for the helper.
const ADDRESS_VARIABLE: &str = "LITHE_GIT_ASKPASS_ADDRESS";
const TOKEN_VARIABLE: &str = "LITHE_GIT_ASKPASS_TOKEN";

/// The value `session` gives `variable` in a configured command.
fn configured(session: &Session, variable: &str) -> OsString {
    let mut command = fixture(Mode::AskPass);
    session.configure(&mut command).expect("configure");
    command
        .get_envs()
        .find(|(name, _)| *name == variable)
        .and_then(|(_, value)| value)
        .expect("configured variable")
        .to_owned()
}

/// A running helper process, killed and reaped when dropped, so a failed
/// assertion leaves no process behind.
struct Helper(Child);

impl Helper {
    /// Starts the helper with the environment `session` gives Git, changed
    /// by `adjust`.
    fn start(session: &Session, adjust: impl FnOnce(&mut Command)) -> Self {
        let mut command = fixture(Mode::AskPass);
        command
            .arg(PROMPT)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        session.configure(&mut command).expect("configure");
        adjust(&mut command);
        Self(command.spawn().expect("start the helper"))
    }

    /// Polls `session` until the helper's prompt arrives.
    fn challenge(&mut self, session: &mut Session) -> Challenge {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let mut ready = session.poll().expect("poll");
            if let Some(challenge) = ready.pop() {
                assert!(ready.is_empty(), "unexpected second prompt");
                return challenge;
            }
            if let Some(status) = self.0.try_wait().expect("helper status") {
                panic!("the helper ended with {status} before it prompted");
            }
            assert!(Instant::now() < deadline, "no prompt before the deadline");
            std::thread::yield_now();
        }
    }

    /// Runs `poll` until the helper exits, then returns its exit code and
    /// everything it printed.
    fn exit(&mut self, mut poll: impl FnMut()) -> (Option<i32>, Vec<u8>) {
        let deadline = Instant::now() + DEADLINE;
        let status = loop {
            if let Some(status) = self.0.try_wait().expect("helper status") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "the helper did not exit before the deadline"
            );
            poll();
            std::thread::yield_now();
        };
        let mut output = Vec::new();
        self.0
            .stdout
            .take()
            .expect("helper output")
            .read_to_end(&mut output)
            .expect("read helper output");
        (status.code(), output)
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        // Both fail harmlessly once the helper has exited and been reaped.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn answer_reaches_git_through_the_helper_process() {
    let mut session = Session::new().expect("session");
    let mut helper = Helper::start(&session, |_| {});

    let challenge = helper.challenge(&mut session);
    assert_eq!(challenge.prompt, PROMPT);
    assert!(challenge.secret);
    assert_eq!(challenge.attempt, 1);
    assert!(respond(&challenge.request_id, Some(CREDENTIAL.into())));

    let (code, output) = helper.exit(|| {
        session.poll().expect("poll");
    });
    assert_eq!(code, Some(0));
    assert_eq!(output, format!("{CREDENTIAL}\n").into_bytes());
}

#[test]
fn cancelled_prompt_fails_the_helper_process() {
    let mut session = Session::new().expect("session");
    let mut helper = Helper::start(&session, |_| {});

    let challenge = helper.challenge(&mut session);
    assert!(respond(&challenge.request_id, None));

    let (code, output) = helper.exit(|| {
        session.poll().expect("poll");
    });
    assert_eq!(code, Some(1));
    assert!(output.is_empty());
}

#[test]
fn helper_with_another_token_never_reaches_the_user() {
    let mut session = Session::new().expect("session");
    let mut helper = Helper::start(&session, |command| {
        command.env(TOKEN_VARIABLE, "0".repeat(64));
    });

    let (code, output) = helper.exit(|| {
        assert!(
            session.poll().expect("poll").is_empty(),
            "a request with another token reached the user"
        );
    });
    assert_eq!(code, Some(1));
    assert!(output.is_empty());
}

#[test]
fn helper_fails_once_its_session_is_gone() {
    let mut session = Session::new().expect("session");
    let mut helper = Helper::start(&session, |_| {});
    let challenge = helper.challenge(&mut session);

    drop(session);

    let (code, output) = helper.exit(|| {});
    assert_eq!(code, Some(1));
    assert!(output.is_empty());
    assert!(!respond(&challenge.request_id, Some(CREDENTIAL.into())));
}

#[test]
fn helper_without_a_live_session_fails() {
    let stale = configured(&Session::new().expect("session"), ADDRESS_VARIABLE);
    let mut session = Session::new().expect("session");
    // No address at all, then the address of a session that has closed.
    for (case, address) in [("no address", None), ("closed session", Some(stale))] {
        let mut helper = Helper::start(&session, |command| match address {
            Some(address) => {
                command.env(ADDRESS_VARIABLE, address);
            }
            None => {
                command.env_remove(ADDRESS_VARIABLE);
            }
        });
        let (code, output) = helper.exit(|| {
            assert!(session.poll().expect("poll").is_empty(), "{case}: prompt");
        });
        assert_eq!(code, Some(1), "{case}");
        assert!(output.is_empty(), "{case}");
    }
}

#[cfg(windows)]
mod windows {
    //! What the kernel enforces on the session's named pipe.

    use super::{configured, ADDRESS_VARIABLE};
    use lithe_git_host::authentication::Session;
    use std::ffi::c_void;
    use std::fs::OpenOptions;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::{addr_of_mut, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HANDLE};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        GetTokenInformation, TokenUser, ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::READ_CONTROL;
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// This process's `TOKEN_USER`, in a pointer-aligned buffer the SID
    /// points into.
    fn current_user() -> Vec<usize> {
        let mut token: HANDLE = null_mut();
        // SAFETY: the pseudo handle needs no closing and the out-pointer
        // references a live local.
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
            0
        );
        // SAFETY: OpenProcessToken returned a new handle that nothing else owns.
        let token = unsafe { OwnedHandle::from_raw_handle(token) };
        let mut buffer = vec![0usize; 64];
        let mut length = 0u32;
        // SAFETY: the buffer is pointer-aligned and as long as the size passed.
        let filled = unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * size_of::<usize>()) as u32,
                &mut length,
            )
        };
        assert_ne!(filled, 0, "token user");
        buffer
    }

    #[test]
    fn pipe_belongs_to_the_current_user_and_admits_nobody_else() {
        let session = Session::new().expect("session");
        let pipe = OpenOptions::new()
            .access_mode(READ_CONTROL)
            .open(configured(&session, ADDRESS_VARIABLE))
            .expect("open the pipe");
        let user = current_user();
        // SAFETY: the buffer holds the TOKEN_USER that current_user() read.
        let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };

        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: the handle is live and every out-pointer references a live
        // local; `owner` and `dacl` point into `descriptor`.
        let status = unsafe {
            GetSecurityInfo(
                pipe.as_raw_handle(),
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        assert_eq!(status, ERROR_SUCCESS);
        let mut control = 0u16;
        let mut revision = 0u32;
        let mut size = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        let mut ace: *mut c_void = null_mut();
        // SAFETY: the descriptor and everything pointing into it stay valid
        // until LocalFree below; every out-pointer references a live local.
        unsafe {
            assert_ne!(EqualSid(owner, user_sid), 0, "the owner is the user");
            assert_ne!(
                GetSecurityDescriptorControl(descriptor, &mut control, &mut revision),
                0
            );
            assert!(!dacl.is_null(), "a null DACL admits everyone");
            assert_ne!(
                GetAclInformation(
                    dacl,
                    addr_of_mut!(size).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                ),
                0
            );
            assert_eq!(size.AceCount, 1, "one entry, for the user");
            assert_ne!(GetAce(dacl, 0, &mut ace), 0);
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            assert_eq!(u32::from((*ace).Header.AceType), ACCESS_ALLOWED_ACE_TYPE);
            let ace_sid: PSID = addr_of_mut!((*ace).SidStart).cast();
            assert_ne!(EqualSid(ace_sid, user_sid), 0, "the entry is the user's");
            LocalFree(descriptor);
        }
        assert_ne!(
            control & SE_DACL_PROTECTED,
            0,
            "inherited entries could widen the DACL"
        );
    }
}
