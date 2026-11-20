//! Threads started before the resource they serve.
//!
//! Creating a thread fails when the host reaches its thread or memory limit.
//! Processes, terminals, tunnels, watches and walks need threads, and once a
//! child runs, a socket is connected or a resource is registered, a thread
//! that cannot start leaves a child nobody waits for, a stream nobody reads or
//! an entry nobody removes. Every such thread is therefore started first and
//! parked until its value arrives: a failure answers `BUSY` while nothing
//! needs cleaning up yet, and handing the value over afterwards cannot fail.

use crate::protocol::{ErrorCode, RemoteError};
use serde_json::json;
use std::io;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

/// A started thread waiting for the value it works on. Dropping it without
/// [`Parked::run`] ends the thread without doing any work.
pub(crate) struct Parked<T> {
    value: SyncSender<T>,
    thread: JoinHandle<()>,
}

impl<T: Send + 'static> Parked<T> {
    /// Starts a thread named `name` that runs `work` once it receives a value.
    pub(crate) fn start(
        name: &str,
        work: impl FnOnce(T) + Send + 'static,
    ) -> Result<Self, RemoteError> {
        let (value, received) = mpsc::sync_channel::<T>(1);
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                if let Ok(value) = received.recv() {
                    work(value);
                }
            })
            .map_err(unavailable)?;
        Ok(Self { value, thread })
    }

    /// Hands `value` to the thread, which then runs its work. The returned
    /// handle joins the thread; dropping it leaves the thread running.
    pub(crate) fn run(self, value: T) -> JoinHandle<()> {
        // The thread waits in `recv` until it receives this value or the
        // sender is dropped, and the one-slot buffer takes the value without
        // blocking. A failure would mean the thread ended on its own.
        if self.value.send(value).is_err() {
            eprintln!("lithe-remote-server: a parked thread ended before its work arrived");
        }
        self.thread
    }
}

/// The answer to a request whose threads cannot start. Like the server's own
/// limits it is a capacity limit, so clients may retry later.
pub(crate) fn unavailable(error: io::Error) -> RemoteError {
    RemoteError::new(ErrorCode::Busy, format!("cannot start a thread: {error}"))
        .with_details(json!({ "osError": error.raw_os_error() }))
}

#[cfg(test)]
mod tests {
    use super::Parked;
    use std::sync::mpsc;
    use std::time::Duration;

    const STEP: Duration = Duration::from_secs(5);

    #[test]
    fn parked_threads_run_their_value_or_end_without_one() {
        let (done, finished) = mpsc::channel();
        let parked = Parked::start("test-parked", move |value: u32| {
            let _ = done.send(value);
        })
        .unwrap();
        let thread = parked.run(7);
        assert_eq!(finished.recv_timeout(STEP), Ok(7));
        // Sending is the work's last step, so the join returns at once.
        thread.join().unwrap();

        // The work closure is dropped when the thread ends, which also drops
        // its sender: the receiver then reports that the thread is gone.
        let (gone, ended) = mpsc::channel::<u32>();
        let unused = Parked::start("test-parked", move |value: u32| {
            let _ = gone.send(value);
        })
        .unwrap();
        drop(unused);
        assert_eq!(
            ended.recv_timeout(STEP),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
    }
}
