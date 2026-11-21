//! Tunnels, local port forwarding and listening-port discovery.

mod support;

use lithe_remote::client::api::TunnelEvent;
use lithe_remote::client::forward_set::MAX_FORWARDS;
use lithe_remote::client::{
    ForwardError, ForwardKind, ForwardSet, ForwardSpec, HostForwardSpec, PortForward,
    RemoteSession, ReverseForwardSpec, TargetField,
};
use lithe_remote::protocol::methods::TunnelTarget;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use support::{connect, STEP_TIMEOUT};

/// A one-connection echo server that uppercases what it reads, so the test
/// can tell the server's answer from its own request.
fn spawn_tcp_echo() -> (u16, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
        let mut buffer = [0_u8; 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    let upper = buffer[..read].to_ascii_uppercase();
                    if stream.write_all(&upper).is_err() {
                        return;
                    }
                }
            }
        }
    });
    (port, server)
}

#[test]
fn tunnels_reach_tcp_ports_on_the_remote() {
    let (port, server) = spawn_tcp_echo();
    let (session, _events) = connect();
    let tunnel = session
        .open_tunnel(TunnelTarget::Tcp {
            host: "127.0.0.1".into(),
            port,
        })
        .unwrap();
    tunnel.write(b"hello").unwrap();
    let mut received = Vec::new();
    while received.len() < 5 {
        match tunnel.next_event(STEP_TIMEOUT).unwrap() {
            Some(TunnelEvent::Data(bytes)) => received.extend(bytes),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(received, b"HELLO");
    tunnel.close().unwrap();
    loop {
        match tunnel.next_event(STEP_TIMEOUT).unwrap() {
            Some(TunnelEvent::Closed(closed)) => {
                assert!(closed.error.is_none(), "{closed:?}");
                break;
            }
            Some(TunnelEvent::Data(_)) => {}
            None => panic!("the subscription ended without a closed event"),
        }
    }
    server.join().unwrap();
    drop(tunnel);
    session.shutdown(STEP_TIMEOUT);
}

// Docker and database sockets are Unix sockets on the remote.
/// A private directory directly under `/tmp`, removed on drop.
struct ShortDir(std::path::PathBuf);

impl ShortDir {
    fn new() -> Self {
        let path = std::path::PathBuf::from(format!("/tmp/lithe-tunnel-{}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for ShortDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn tunnels_reach_unix_sockets_on_the_remote() {
    // Socket paths are limited to about 100 bytes, which the temporary
    // directory on macOS already nearly exhausts.
    let directory = ShortDir::new();
    let path = directory.0.join("service.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.write_all(b"greeting").unwrap();
    });
    let (session, _events) = connect();
    let tunnel = session
        .open_tunnel(TunnelTarget::Unix {
            path: path.to_str().unwrap().into(),
        })
        .unwrap();
    let mut received = Vec::new();
    loop {
        match tunnel.next_event(STEP_TIMEOUT).unwrap() {
            Some(TunnelEvent::Data(bytes)) => received.extend(bytes),
            Some(TunnelEvent::Closed(_)) => break,
            None => panic!("the subscription ended without a closed event"),
        }
    }
    assert_eq!(received, b"greeting");
    server.join().unwrap();
    drop(tunnel);
    session.shutdown(STEP_TIMEOUT);
}

#[test]
fn connecting_to_a_closed_port_fails_with_a_stable_code() {
    // Binding then dropping a listener yields a port nobody listens on.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let (session, _events) = connect();
    match session.open_tunnel(TunnelTarget::Tcp {
        host: "127.0.0.1".into(),
        port,
    }) {
        Err(lithe_remote::client::ClientError::Remote(error)) => {
            assert_eq!(error.code, lithe_remote::protocol::ErrorCode::ConnectFailed)
        }
        Err(other) => panic!("unexpected {other:?}"),
        Ok(_) => panic!("connected to a closed port"),
    }
    session.shutdown(STEP_TIMEOUT);
}

// The full path a browser takes: local port → tunnel → remote port.
#[test]
fn port_forwards_bridge_local_connections_to_the_remote() {
    let (port, server) = spawn_tcp_echo();
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forward = PortForward::start(
        Arc::clone(&session),
        "127.0.0.1:0".parse().unwrap(),
        TunnelTarget::Tcp {
            host: "127.0.0.1".into(),
            port,
        },
    )
    .unwrap();

    let mut local = TcpStream::connect(forward.local_address()).unwrap();
    local.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
    local.write_all(b"through the tunnel").unwrap();
    let mut answer = vec![0_u8; b"through the tunnel".len()];
    local.read_exact(&mut answer).unwrap();
    assert_eq!(answer, b"THROUGH THE TUNNEL");
    drop(local);
    server.join().unwrap();
    drop(forward);
    // Bridge threads finish asynchronously once the remote side reports the
    // tunnel closed, and may still hold the session; shut it down through
    // the shared handle instead of waiting for sole ownership.
    session
        .request_value(
            lithe_remote::protocol::methods::SERVER_SHUTDOWN,
            serde_json::Value::Null,
            STEP_TIMEOUT,
        )
        .unwrap();
}

// A forward on every interface would expose the remote service to the local
// network, so only loopback addresses are accepted.
#[test]
fn port_forwards_refuse_addresses_other_than_loopback() {
    let (session, _events) = connect();
    let session = Arc::new(session);
    for bind in ["0.0.0.0:0", "[::]:0"] {
        let error = PortForward::start(
            Arc::clone(&session),
            bind.parse().unwrap(),
            TunnelTarget::Tcp {
                host: "127.0.0.1".into(),
                port: 1,
            },
        )
        .err()
        .unwrap_or_else(|| panic!("{bind} was accepted"));
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{bind}");
    }
    let session = Arc::into_inner(session).expect("no forward holds the session");
    session.shutdown(STEP_TIMEOUT);
}

// Regression: a local client that half-closes after its request, as
// HTTP/1.0 clients and `nc -N` do, had the tunnel aborted. Queued data was
// lost and the reply never arrived.
#[test]
fn half_closed_requests_still_get_their_reply() {
    // The server answers only after reading the whole request to its end.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
        let mut request = Vec::new();
        stream.read_to_end(&mut request).unwrap();
        stream
            .write_all(format!("received {} bytes", request.len()).as_bytes())
            .unwrap();
    });
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forward = PortForward::start(
        Arc::clone(&session),
        "127.0.0.1:0".parse().unwrap(),
        TunnelTarget::Tcp {
            host: "127.0.0.1".into(),
            port,
        },
    )
    .unwrap();

    let mut local = TcpStream::connect(forward.local_address()).unwrap();
    local.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
    // Several chunks, so some are still queued when the half-close arrives.
    let request = vec![b'x'; 1 << 20];
    local.write_all(&request).unwrap();
    local.shutdown(std::net::Shutdown::Write).unwrap();
    let mut reply = String::new();
    local.read_to_string(&mut reply).unwrap();
    assert_eq!(reply, format!("received {} bytes", request.len()));
    server.join().unwrap();
    drop(forward);
    session
        .request_value(
            lithe_remote::protocol::methods::SERVER_SHUTDOWN,
            serde_json::Value::Null,
            STEP_TIMEOUT,
        )
        .unwrap();
}

#[test]
fn listening_ports_include_a_local_listener() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (session, _events) = connect();
    let ports = session.listening_ports().unwrap();
    let own = ports
        .iter()
        .find(|listening| listening.port == port && listening.address == "127.0.0.1")
        .unwrap_or_else(|| panic!("{port} not in {ports:?}"));
    // The server runs as the same user, so it can name the owner.
    assert_eq!(own.pid, Some(std::process::id()), "{own:?}");
    assert!(own.process.is_some(), "{own:?}");
    drop(listener);
    session.shutdown(STEP_TIMEOUT);
}

/// A forward to `host:port` on a port the system chooses, unconfirmed.
fn forward_to(host: &str, port: u16) -> ForwardSpec {
    ForwardSpec {
        target_host: host.into(),
        target_port: port,
        local_port: None,
        allow_non_loopback_target: false,
    }
}

/// Ends the server through a shared session handle; forwards may still hold
/// other handles.
fn shut_down(session: &RemoteSession) {
    session
        .request_value(
            lithe_remote::protocol::methods::SERVER_SHUTDOWN,
            serde_json::Value::Null,
            STEP_TIMEOUT,
        )
        .unwrap();
}

// The ports page names the address a remote program listens on, often the
// wildcard address; the forward reaches it through the host's loopback
// address, and a second click returns the same forward.
#[test]
fn forward_sets_bridge_a_wildcard_listener_once_per_target() {
    let (port, server) = spawn_tcp_echo();
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));

    let (info, created) = forwards.start(forward_to("0.0.0.0", port)).unwrap();
    assert!(created);
    assert_eq!(info.target_host.as_deref(), Some("127.0.0.1"));
    assert!(info.loopback_target);
    assert_eq!(info.local_host.as_deref(), Some("127.0.0.1"));
    assert_eq!(info.requested_local_port, None);
    assert!(!info.local_port_changed);
    let (again, created) = forwards.start(forward_to("127.0.0.1", port)).unwrap();
    assert!(!created);
    assert_eq!(again.id, info.id);
    assert_eq!(again.local_port, info.local_port);

    let local_port = info.local_port.unwrap();
    let mut local = TcpStream::connect(("127.0.0.1", local_port)).unwrap();
    local.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
    local.write_all(b"ports page").unwrap();
    let mut answer = vec![0_u8; b"ports page".len()];
    local.read_exact(&mut answer).unwrap();
    assert_eq!(answer, b"PORTS PAGE");
    drop(local);
    server.join().unwrap();

    assert_eq!(forwards.list().unwrap().len(), 1);
    assert!(forwards.stop(info.id));
    assert!(!forwards.stop(info.id), "a stopped forward stops only once");
    assert!(forwards.list().unwrap().is_empty());
    shut_down(&session);
}

// Design S28: a target outside the host's loopback interface reaches another
// machine through the host, so it starts only once the user confirmed it.
#[test]
fn forward_sets_start_targets_outside_the_host_only_when_confirmed() {
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));
    // A documentation address (RFC 5737). Nothing connects to the forward,
    // so no packet is ever sent to it.
    let elsewhere = forward_to("192.0.2.10", 5432);

    assert!(matches!(
        forwards.start(elsewhere.clone()),
        Err(ForwardError::TargetNotConfirmed)
    ));
    assert!(forwards.list().unwrap().is_empty());
    let (info, created) = forwards
        .start(ForwardSpec {
            allow_non_loopback_target: true,
            ..elsewhere
        })
        .unwrap();
    assert!(created);
    assert!(!info.loopback_target);
    assert_eq!(info.target_host.as_deref(), Some("192.0.2.10"));

    for (spec, field) in [
        (forward_to("127.0.0.1", 0), TargetField::Port),
        (forward_to("-oProxyCommand", 22), TargetField::Host),
    ] {
        assert!(
            matches!(forwards.start(spec.clone()), Err(ForwardError::InvalidTarget(found)) if found == field),
            "{spec:?}"
        );
    }
    shut_down(&session);
}

// A local program already using the requested port keeps it; the forward
// moves to a free port and says so.
#[test]
fn forward_sets_move_off_a_local_port_another_program_uses() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));

    let (info, created) = forwards
        .start(ForwardSpec {
            local_port: Some(port),
            ..forward_to("localhost", 8080)
        })
        .unwrap();
    assert!(created);
    assert_eq!(info.requested_local_port, Some(port));
    assert!(info.local_port_changed);
    assert_ne!(info.local_port, Some(port));
    drop(taken);
    shut_down(&session);
}

#[test]
fn forward_sets_refuse_more_forwards_than_the_limit() {
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));
    let limit = u16::try_from(MAX_FORWARDS).unwrap();
    for port in 1..=limit {
        forwards.start(forward_to("127.0.0.1", port)).unwrap();
    }
    assert!(matches!(
        forwards.start(forward_to("127.0.0.1", limit + 1)),
        Err(ForwardError::LimitReached)
    ));
    // A target that already has a forward still answers at the limit.
    let (_, created) = forwards.start(forward_to("127.0.0.1", 1)).unwrap();
    assert!(!created);
    shut_down(&session);
}

// Design S29: when the connection ends, every local port closes, so a port
// never keeps accepting connections that lead nowhere.
#[test]
fn forward_sets_release_their_local_ports_when_the_session_ends() {
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));
    let (info, _) = forwards.start(forward_to("127.0.0.1", 1)).unwrap();
    let local_port = info.local_port.unwrap();

    shut_down(&session);
    // The release runs on a thread of its own once the session sees the
    // stream end; the closed listener is its only observable result.
    let deadline = Instant::now() + STEP_TIMEOUT;
    while TcpStream::connect(("127.0.0.1", local_port)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "local port {} still accepts connections after the session ended",
            local_port
        );
        // test-stability: allow(rust-real-sleep) reason: the listener closes on a background thread without a waitable signal; the loop has a monotonic deadline and a diagnostic.
        thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(forwards.list(), Err(ForwardError::SessionClosed)));
    assert!(matches!(
        forwards.start(forward_to("127.0.0.1", 2)),
        Err(ForwardError::SessionClosed)
    ));
}

/// A host forward on a port the host chooses, on its loopback interface,
/// unconfirmed.
fn host_forward_to(host: &str, port: u16) -> HostForwardSpec {
    HostForwardSpec {
        listen_port: None,
        all_interfaces: false,
        target_host: host.into(),
        target_port: port,
        allow_non_loopback_target: false,
    }
}

/// Sends `request` over `port` on this machine's loopback interface and
/// reads the echo server's uppercased answer.
fn exchange(port: u16, request: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(STEP_TIMEOUT)).unwrap();
    stream.write_all(request).unwrap();
    let mut answer = vec![0_u8; request.len()];
    stream.read_exact(&mut answer).unwrap();
    answer
}

// The test server runs on this machine, so the host's loopback interface is
// this machine's too.
#[test]
fn host_forwards_relay_a_host_port_to_another_server() {
    let (port, server) = spawn_tcp_echo();
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));

    let (info, created) = forwards
        .start_host(host_forward_to("127.0.0.1", port))
        .unwrap();
    assert!(created);
    assert_eq!(info.kind, ForwardKind::Host);
    assert_eq!(info.host_listen_host.as_deref(), Some("127.0.0.1"));
    assert!(!info.all_interfaces);
    assert_eq!(info.local_port, None);
    let host_port = info.host_listen_port.unwrap();
    let (again, created) = forwards
        .start_host(host_forward_to("127.0.0.1", port))
        .unwrap();
    assert!(!created);
    assert_eq!(again.id, info.id);

    assert_eq!(exchange(host_port, b"jump host"), b"JUMP HOST");
    server.join().unwrap();
    let listed = forwards.list().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].connections, 1);
    assert_eq!(listed[0].failed_connections, 0);

    assert!(forwards.stop(info.id));
    // Closing waits until the host released the port.
    assert!(TcpStream::connect(("127.0.0.1", host_port)).is_err());
    shut_down(&session);
}

#[test]
fn host_forwards_report_a_taken_host_port_and_check_their_targets() {
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));

    let error = forwards
        .start_host(HostForwardSpec {
            listen_port: Some(port),
            ..host_forward_to("127.0.0.1", 8080)
        })
        .unwrap_err();
    assert!(matches!(error, ForwardError::HostPortInUse), "{error:?}");
    // A documentation address (RFC 5737); nothing connects to it.
    assert!(matches!(
        forwards.start_host(host_forward_to("192.0.2.10", 5432)),
        Err(ForwardError::TargetNotConfirmed)
    ));
    assert!(matches!(
        forwards.start_host(host_forward_to("127.0.0.1", 0)),
        Err(ForwardError::InvalidTarget(TargetField::Port))
    ));
    assert!(forwards.list().unwrap().is_empty());
    drop(taken);
    shut_down(&session);
}

#[test]
fn reverse_forwards_bring_host_connections_back_to_a_local_port() {
    let (local_port, server) = spawn_tcp_echo();
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));

    assert!(matches!(
        forwards.start_reverse(ReverseForwardSpec {
            listen_port: None,
            local_port: 0,
        }),
        Err(ForwardError::InvalidTarget(TargetField::LocalPort))
    ));
    let (info, created) = forwards
        .start_reverse(ReverseForwardSpec {
            listen_port: None,
            local_port,
        })
        .unwrap();
    assert!(created);
    assert_eq!(info.kind, ForwardKind::Reverse);
    assert_eq!(info.host_listen_host.as_deref(), Some("127.0.0.1"));
    assert_eq!(info.local_port, Some(local_port));
    assert_eq!(info.target_host, None);

    let host_port = info.host_listen_port.unwrap();
    assert_eq!(exchange(host_port, b"reverse"), b"REVERSE");
    server.join().unwrap();
    assert_eq!(forwards.list().unwrap()[0].connections, 1);
    assert!(forwards.stop(info.id));
    assert!(TcpStream::connect(("127.0.0.1", host_port)).is_err());
    shut_down(&session);
}

// One limit covers every kind, so host listeners cannot exceed what the
// ports page shows.
#[test]
fn forward_kinds_share_one_limit() {
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));
    let local = u16::try_from(MAX_FORWARDS - 1).unwrap();
    for port in 1..=local {
        forwards.start(forward_to("127.0.0.1", port)).unwrap();
    }
    forwards
        .start_host(host_forward_to("127.0.0.1", 1))
        .unwrap();
    assert!(matches!(
        forwards.start_reverse(ReverseForwardSpec {
            listen_port: None,
            local_port: 1,
        }),
        Err(ForwardError::LimitReached)
    ));
    shut_down(&session);
}

// The host closes its listeners when the client disconnects; nothing
// reopens them.
#[test]
fn host_ports_close_when_the_session_ends() {
    let (session, _events) = connect();
    let session = Arc::new(session);
    let forwards = ForwardSet::new(Arc::clone(&session));
    let (info, _) = forwards
        .start_host(host_forward_to("127.0.0.1", 1))
        .unwrap();
    let host_port = info.host_listen_port.unwrap();

    shut_down(&session);
    let deadline = Instant::now() + STEP_TIMEOUT;
    // The host can exit before this client reads the end of its stream.
    while !session.is_closed() || TcpStream::connect(("127.0.0.1", host_port)).is_ok() {
        assert!(
            Instant::now() < deadline,
            "host port {host_port} still accepts connections after the session ended"
        );
        // test-stability: allow(rust-real-sleep) reason: the host closes the listener when its process exits, without a signal to this test; the loop has a monotonic deadline and a diagnostic.
        thread::sleep(Duration::from_millis(20));
    }
    assert!(matches!(
        forwards.start_host(host_forward_to("127.0.0.1", 2)),
        Err(ForwardError::SessionClosed)
    ));
}
