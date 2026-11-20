//! The account on the other end of a Unix socket, read through
//! `lithe_platform::socket` with the host's own system call.

#[cfg(unix)]
mod unix {
    use crate::support::TempDir;
    use lithe_platform::socket::{effective_uid, peer_uid};
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};

    #[test]
    fn both_ends_of_a_socket_see_the_account_that_opened_the_other() {
        let directory = TempDir::new("lithe peer");
        let path = directory.path().join("peer.sock");
        let listener = UnixListener::bind(&path).expect("listening socket");
        let client = UnixStream::connect(&path).expect("connect");
        // A Unix-domain connect completes once the connection is queued, so a
        // non-blocking accept takes it without waiting.
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let (server, _) = match listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                panic!("the queued connection was not ready to accept")
            }
            Err(error) => panic!("accept: {error}"),
        };

        assert_eq!(
            peer_uid(&server).expect("peer of the server end"),
            effective_uid()
        );
        assert_eq!(
            peer_uid(&client).expect("peer of the client end"),
            effective_uid()
        );
    }
}
