use super::*;
use crate::client::connections::Label;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// A Motty database in a scratch directory, removed when dropped.
struct Database {
    directory: PathBuf,
    path: PathBuf,
}

impl Database {
    fn new(format: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "lithe-remote-motty-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.db");
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute_batch(
                "CREATE TABLE records (kind TEXT NOT NULL, id TEXT NOT NULL, stamp TEXT NOT NULL, \
                 deleted INTEGER NOT NULL DEFAULT 0, payload BLOB, PRIMARY KEY (kind, id)) WITHOUT ROWID;\
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;",
            )
            .unwrap();
        database
            .execute(
                "INSERT INTO meta (key, value) VALUES ('format', ?1)",
                [format],
            )
            .unwrap();
        Self { directory, path }
    }

    fn insert(&self, kind: &str, id: &str, deleted: bool, payload: serde_json::Value) {
        rusqlite::Connection::open(&self.path)
            .unwrap()
            .execute(
                "INSERT INTO records (kind, id, stamp, deleted, payload) VALUES (?1, ?2, '1', ?3, ?4)",
                rusqlite::params![kind, id, deleted, payload.to_string().into_bytes()],
            )
            .unwrap();
    }

    fn host(&self, id: &str, order: i64, data: serde_json::Value) {
        let mut data = data;
        data["id"] = json!(id);
        self.insert("host", id, false, json!({ "o": order, "d": data }));
    }

    fn report(&self, connections: &Connections) -> Report {
        candidates(&read(&self.path).expect("readable"), connections)
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn find<'a>(report: &'a Report, key: &str) -> &'a Candidate {
    report
        .candidates
        .iter()
        .find(|candidate| candidate.key == key)
        .unwrap_or_else(|| panic!("no candidate {key}"))
}

fn field<'a>(candidate: &'a Candidate, motty: &str) -> &'a Field {
    candidate
        .fields
        .iter()
        .find(|field| field.motty == motty)
        .unwrap_or_else(|| panic!("no field {motty}"))
}

#[test]
fn ssh_hosts_become_connections_in_mottys_order() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.insert(
        "group",
        "g1",
        false,
        json!({ "o": 0, "d": { "name": "Build machines" } }),
    );
    database.host(
        "b",
        2,
        json!({
            "label": "GPU box",
            "address": "gpu.example",
            "user": "ml",
            "port": 2222,
            "color": "#22c55e",
            "group": "g1",
            "system": "ubuntu",
            "ssh_options": { "server_alive_interval": 30, "identities_only": true, "identity_files": ["~/.ssh/gpu"] },
        }),
    );
    database.host(
        "a",
        1,
        json!({ "label": "", "address": "build.example", "port": 0, "group": "Build Machines", "color": "" }),
    );
    database.host(
        "gone",
        0,
        json!({ "label": "Old", "address": "old.example" }),
    );
    database.insert(
        "host",
        "deleted",
        true,
        json!({ "o": 0, "d": { "address": "deleted.example" } }),
    );

    let report = database.report(&Connections::default());
    assert_eq!(report.format, "28");
    let keys: Vec<&str> = report
        .candidates
        .iter()
        .map(|candidate| candidate.key.as_str())
        .collect();
    assert_eq!(keys, ["gone", "a", "b"]);
    assert_eq!(report.groups, ["Build Machines"]);

    let gpu = find(&report, "b");
    assert_eq!(gpu.status, Status::Importable);
    assert_eq!(gpu.name, "GPU box");
    assert_eq!(gpu.address, "ml@gpu.example:2222");
    assert_eq!(gpu.system.as_deref(), Some("ubuntu"));
    assert_eq!(gpu.color, Some(Color::Green));
    assert_eq!(gpu.group.as_deref(), Some("Build machines"));
    assert_eq!(
        serde_json::to_value(gpu.connection.as_ref().unwrap()).unwrap(),
        json!({
            "host": "gpu.example",
            "port": 2222,
            "user": "ml",
            "authentication": "privateKey",
            "identityFile": "~/.ssh/gpu",
            "keepAliveSeconds": 30,
        })
    );
    assert_eq!(
        field(gpu, "ssh_options.identity_files").lithe,
        Some("identityFile")
    );

    // A blank label takes the address, and port 0 is no port.
    let build = find(&report, "a");
    assert_eq!(build.name, "build.example");
    assert_eq!(build.address, "build.example");
    assert_eq!(build.color, None);
    assert_eq!(build.status, Status::Importable);
}

#[test]
fn settings_lithe_has_no_place_for_are_listed_with_the_reason() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.host(
        "h",
        0,
        json!({
            "label": "Partly",
            "address": "partly.example",
            "authentication": "kerberos",
            "identity": "motty-key-1",
            "credential": "stored",
            "forward_x11": true,
            "agent_forward": false,
            "encoding": "gbk",
            "terminal": "",
            "ssh_options": { "server_alive_count_max": 5, "identity_files": [] },
        }),
    );
    database.host(
        "k",
        1,
        json!({
            "address": "keys.example",
            "authentication": "key",
            "ssh_options": { "identity_files": ["~/.ssh/one", "~/.ssh/two"] },
        }),
    );
    let report = database.report(&Connections::default());
    let partly = find(&report, "h");
    assert_eq!(partly.status, Status::Partial);
    assert_eq!(field(partly, "authentication").skip, Some(Skip::Kerberos));
    assert_eq!(field(partly, "identity").skip, Some(Skip::Secret));
    assert_eq!(
        field(partly, "identity").value,
        "",
        "the key reference is not shown"
    );
    assert_eq!(field(partly, "credential").skip, Some(Skip::Secret));
    assert_eq!(field(partly, "forward_x11").skip, Some(Skip::NoSetting));
    assert_eq!(field(partly, "encoding").value, "gbk");
    assert_eq!(
        field(partly, "ssh_options.server_alive_count_max").skip,
        Some(Skip::NoSetting)
    );
    for unset in ["agent_forward", "terminal"] {
        assert!(
            partly.fields.iter().all(|field| field.motty != unset),
            "{unset}"
        );
    }
    assert_eq!(
        serde_json::to_value(partly.connection.as_ref().unwrap()).unwrap(),
        json!({ "host": "partly.example" })
    );

    let keys = find(&report, "k");
    assert_eq!(keys.status, Status::Partial);
    let files: Vec<(&str, Option<Skip>)> = keys
        .fields
        .iter()
        .filter(|field| field.motty == "ssh_options.identity_files")
        .map(|field| (field.value.as_str(), field.skip))
        .collect();
    assert_eq!(
        files,
        [("~/.ssh/one", None), ("~/.ssh/two", Some(Skip::ExtraKey))]
    );
}

#[test]
fn other_protocols_and_refused_settings_are_not_imported() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.host(
        "t",
        0,
        json!({ "label": "Router", "address": "router.example", "protocol": "telnet" }),
    );
    database.host(
        "bad",
        1,
        json!({ "label": "Bad", "address": "-oProxyCommand=x" }),
    );
    database.host(
        "user",
        2,
        json!({ "label": "Bad user", "address": "192.0.2.20", "user": "a b" }),
    );
    let report = database.report(&Connections::default());
    let telnet = find(&report, "t");
    assert_eq!(telnet.status, Status::Unsupported);
    assert_eq!(telnet.connection, None);
    assert_eq!(field(telnet, "protocol").skip, Some(Skip::Protocol));
    let bad = find(&report, "bad");
    assert_eq!(bad.status, Status::Invalid);
    assert_eq!(bad.connection, None);
    assert_eq!(field(bad, "address").skip, Some(Skip::Invalid));
    // The refusal names the setting that Lithe refuses, not the address.
    let user = find(&report, "user");
    assert_eq!(user.status, Status::Invalid);
    assert_eq!(user.connection, None);
    assert_eq!(field(user, "user").skip, Some(Skip::Invalid));
    assert_eq!(field(user, "user").lithe, None);
    assert_eq!(field(user, "address").skip, None);
    assert_eq!(field(user, "address").lithe, Some("host"));
}

#[test]
fn windows_domain_users_are_imported() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.host(
        "prefix",
        0,
        json!({ "label": "Prefix", "address": "192.0.2.30", "user": "CONTOSO\\admin" }),
    );
    database.host(
        "suffix",
        1,
        json!({ "label": "Suffix", "address": "192.0.2.31", "user": "admin@contoso.example" }),
    );
    let report = database.report(&Connections::default());
    for (key, user) in [
        ("prefix", "CONTOSO\\admin"),
        ("suffix", "admin@contoso.example"),
    ] {
        let candidate = find(&report, key);
        assert_eq!(candidate.status, Status::Importable, "host {key}");
        let connection = candidate.connection.as_ref().expect("imported connection");
        assert_eq!(connection.user.as_deref(), Some(user), "host {key}");
    }
}

#[test]
fn jump_routes_follow_proxy_records_and_refuse_commands() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.host(
        "edge",
        0,
        json!({ "address": "edge.example", "user": "ops", "port": 2200 }),
    );
    database.host("inner", 1, json!({ "address": "inner.example" }));
    database.host("target", 2, json!({ "address": "target.example" }));
    database.host("relayed", 3, json!({ "address": "relayed.example" }));
    database.host("paused", 4, json!({ "address": "paused.example" }));
    database.insert(
        "proxy",
        "p1",
        false,
        json!({ "o": 0, "d": { "target_id": "target", "jump_id": "inner" } }),
    );
    database.insert(
        "proxy",
        "p2",
        false,
        json!({ "o": 1, "d": { "target_id": "inner", "jump_id": "edge" } }),
    );
    database.insert(
        "proxy",
        "p3",
        false,
        json!({ "o": 2, "d": { "target_id": "relayed", "jump_address": "x.example", "relay_command": "nc %h %p" } }),
    );
    database.insert(
        "proxy",
        "p4",
        false,
        json!({ "o": 3, "d": { "target_id": "paused", "jump_address": "x.example", "stopped": true } }),
    );
    let report = database.report(&Connections::default());
    let target = find(&report, "target");
    assert_eq!(
        target.connection.as_ref().unwrap().proxy_jump(),
        Some("ops@edge.example:2200,inner.example")
    );
    assert_eq!(field(target, "proxy").lithe, Some("proxyJump"));
    let relayed = find(&report, "relayed");
    assert_eq!(relayed.status, Status::Partial);
    assert_eq!(field(relayed, "proxy").skip, Some(Skip::Command));
    assert_eq!(relayed.connection.as_ref().unwrap().proxy_jump(), None);
    assert!(find(&report, "paused")
        .fields
        .iter()
        .all(|field| field.motty != "proxy"));
}

#[test]
fn hosts_already_saved_are_marked_with_the_entry() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.host(
        "h",
        0,
        json!({ "label": "Build", "address": "Build.Example", "user": "ci" }),
    );
    let mut connections = Connections::default();
    let id = connections
        .save(
            None,
            "Mine",
            SshConnection::to_host("build.example"),
            Label::default(),
            || "saved-1".to_owned(),
        )
        .unwrap();
    let report = database.report(&connections);
    let build = find(&report, "h");
    assert_eq!(build.status, Status::Exists);
    assert_eq!(build.existing_id.as_deref(), Some(id.as_str()));
}

#[test]
fn colors_map_to_the_nearest_lithe_color() {
    assert_eq!(nearest_color("#ef4444"), Some(Color::Red));
    assert_eq!(nearest_color("#F97316"), Some(Color::Orange));
    assert_eq!(nearest_color("#eab308"), Some(Color::Yellow));
    assert_eq!(nearest_color("#3b82f6"), Some(Color::Blue));
    assert_eq!(nearest_color("#a855f7"), Some(Color::Purple));
    assert_eq!(nearest_color("#10b981"), Some(Color::Green));
    assert_eq!(nearest_color(""), None);
    assert_eq!(nearest_color("red"), None);
    assert_eq!(nearest_color("#12345"), None);
}

#[test]
fn unreadable_databases_say_why() {
    let database = Database::new("27");
    assert_eq!(read(&database.path), Err(ReadError::UnsupportedFormat));
    assert_eq!(
        read(&database.directory.join("absent.db")),
        Err(ReadError::Missing)
    );
    assert_eq!(read(&database.directory), Err(ReadError::Missing));
    let garbage = database.directory.join("garbage.db");
    std::fs::write(&garbage, b"not a database").unwrap();
    assert_eq!(read(&garbage), Err(ReadError::Unreadable));
}

#[test]
fn records_lithe_cannot_read_are_left_out() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.insert("host", "text", false, json!("not an object"));
    database.insert("host", "nodata", false, json!({ "o": 0 }));
    database.host("ok", 0, json!({ "address": "ok.example" }));
    let report = database.report(&Connections::default());
    let keys: Vec<&str> = report
        .candidates
        .iter()
        .map(|candidate| candidate.key.as_str())
        .collect();
    assert_eq!(keys, ["ok"]);
}

#[test]
fn reading_never_changes_the_database() {
    let database = Database::new(SUPPORTED_FORMAT);
    database.host("h", 0, json!({ "address": "h.example" }));
    let before = std::fs::read(&database.path).unwrap();
    read(&database.path).unwrap();
    assert_eq!(std::fs::read(&database.path).unwrap(), before);
}
