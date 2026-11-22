use super::*;
use crate::client::transport::{ContainerEngine, ContainerHop, Hop, SshHop, TransportChain};
use serde_json::json;

fn target(hops: Vec<Hop>, user_id: Option<u32>) -> Target {
    Target::new(&TransportChain::new(hops), user_id)
}

fn devbox() -> Target {
    target(vec![Hop::Ssh(SshHop::new("dev@box"))], Some(1000))
}

fn row(host: Option<&str>, port: u16) -> PortRecord {
    PortRecord {
        host: host.map(Into::into),
        port,
        added: None,
        label: None,
        local_port: None,
        forward_target: None,
        confirmed_target: false,
    }
}

fn labelled(host: Option<&str>, port: u16, label: &str) -> PortRecord {
    PortRecord {
        label: Some(label.into()),
        ..row(host, port)
    }
}

fn forwarded(host: Option<&str>, port: u16, target: &str, confirmed: bool) -> PortRecord {
    PortRecord {
        forward_target: Some(target.into()),
        confirmed_target: confirmed,
        ..row(host, port)
    }
}

fn added(host: &str, scheme: Option<&str>, path: Option<&str>) -> AddedAddress {
    AddedAddress {
        host: host.into(),
        scheme: scheme.map(Into::into),
        path: path.map(Into::into),
    }
}

fn host_rule(listen_port: Option<u16>, target_host: &str, label: Option<&str>) -> Rule {
    Rule::Host {
        listen_port,
        target_host: target_host.into(),
        target_port: 5432,
        label: label.map(Into::into),
    }
}

fn reverse_rule(listen_port: Option<u16>, local_port: u16) -> Rule {
    Rule::Reverse {
        listen_port,
        local_port,
        label: None,
    }
}

fn ports(records: &PortRecords, target: &Target, path: &str) -> Vec<PortRecord> {
    records.records(target, path).unwrap().ports
}

/// The document after a round trip through its text, to show what is stored.
fn stored(records: &PortRecords) -> serde_json::Value {
    let text = records.text().unwrap();
    assert_eq!(PortRecords::parse(Some(&text)).unwrap(), *records);
    serde_json::from_str(&text).unwrap()
}

#[test]
fn records_never_carry_over_between_hosts_accounts_containers_or_folders() {
    let mut records = PortRecords::parse(None).unwrap();
    records
        .set_port(&devbox(), "/home/dev/app", labelled(None, 3000, "Web"))
        .unwrap();
    assert_eq!(
        ports(&records, &devbox(), "/home/dev/app"),
        [labelled(None, 3000, "Web")]
    );

    let container = Hop::Container(ContainerHop::new(ContainerEngine::Docker, "web"));
    let others = [
        (
            target(vec![Hop::Ssh(SshHop::new("dev@other"))], Some(1000)),
            "/home/dev/app",
        ),
        (
            target(vec![Hop::Ssh(SshHop::new("dev@box"))], Some(0)),
            "/home/dev/app",
        ),
        (
            target(
                vec![Hop::Ssh(SshHop::new("dev@box")), container],
                Some(1000),
            ),
            "/home/dev/app",
        ),
        (devbox(), "/home/dev/app/sub"),
        (devbox(), "/home/dev"),
    ];
    for (other, path) in &others {
        assert_eq!(
            records.records(other, path).unwrap(),
            WorkspaceRecords::default()
        );
    }
}

#[test]
fn rows_are_keyed_by_the_address_a_forward_reaches() {
    let mut records = PortRecords::parse(None).unwrap();
    // Every name of the host's loopback interface and of all its interfaces
    // is the same row.
    for host in [
        "localhost",
        "LOCALHOST",
        "127.0.0.1",
        "::1",
        "0.0.0.0",
        "::",
        "::ffff:127.0.0.1",
    ] {
        records
            .set_port(&devbox(), "/srv", labelled(Some(host), 3000, host))
            .unwrap();
        assert_eq!(
            ports(&records, &devbox(), "/srv"),
            [labelled(None, 3000, host)]
        );
    }
    // Other addresses are rows of their own, in canonical form.
    records
        .set_port(&devbox(), "/srv", labelled(Some("DB.Internal"), 3000, "db"))
        .unwrap();
    records
        .set_port(
            &devbox(),
            "/srv",
            labelled(Some("127.0.0.2"), 3000, "second"),
        )
        .unwrap();
    records
        .set_port(
            &devbox(),
            "/srv",
            labelled(Some("0:0:0:0:0:0:a00:1"), 22, "v6"),
        )
        .unwrap();
    assert_eq!(
        ports(&records, &devbox(), "/srv"),
        [
            labelled(Some("::a00:1"), 22, "v6"),
            labelled(None, 3000, "::ffff:127.0.0.1"),
            labelled(Some("127.0.0.2"), 3000, "second"),
            labelled(Some("db.internal"), 3000, "db"),
        ]
    );
    // Any spelling removes the row.
    records
        .remove_port(&devbox(), "/srv", Some("DB.INTERNAL"), 3000)
        .unwrap();
    records
        .remove_port(&devbox(), "/srv", Some("[::]"), 3000)
        .unwrap();
    assert_eq!(
        ports(&records, &devbox(), "/srv"),
        [
            labelled(Some("::a00:1"), 22, "v6"),
            labelled(Some("127.0.0.2"), 3000, "second"),
        ]
    );

    for (record, field) in [
        (row(None, 0), "port"),
        (labelled(Some("-oProxyCommand"), 1, "x"), "host"),
        (labelled(Some("a b"), 1, "x"), "host"),
        (labelled(Some(""), 1, "x"), "host"),
    ] {
        assert_eq!(
            records.set_port(&devbox(), "/srv", record),
            Err(PortRecordError { field })
        );
    }
}

#[test]
fn added_addresses_keep_scheme_and_path_but_never_the_query_or_fragment() {
    let mut records = PortRecords::parse(None).unwrap();
    let jupyter = PortRecord {
        added: Some(added(
            "localhost",
            Some("http"),
            Some("/lab?token=secret#cell"),
        )),
        ..row(None, 8888)
    };
    records.set_port(&devbox(), "/srv", jupyter).unwrap();
    let only_query = PortRecord {
        added: Some(added("db.internal", Some("https"), Some("/?token=secret"))),
        ..row(Some("db.internal"), 8443)
    };
    records.set_port(&devbox(), "/srv", only_query).unwrap();
    let without_scheme = PortRecord {
        added: Some(added("localhost", None, Some("/app#top"))),
        ..row(None, 3000)
    };
    records.set_port(&devbox(), "/srv", without_scheme).unwrap();

    assert_eq!(
        ports(&records, &devbox(), "/srv"),
        [
            PortRecord {
                added: Some(added("localhost", None, Some("/app"))),
                ..row(None, 3000)
            },
            PortRecord {
                added: Some(added("db.internal", Some("https"), None)),
                ..row(Some("db.internal"), 8443)
            },
            PortRecord {
                added: Some(added("localhost", Some("http"), Some("/lab"))),
                ..row(None, 8888)
            },
        ]
    );
    let text = records.text().unwrap();
    assert!(!text.contains("secret") && !text.contains('?') && !text.contains('#'));

    // A stored query is refused rather than read.
    let document = json!({
        "version": 1,
        "workspaces": [{
            "target": devbox(),
            "workspace": "/srv",
            "ports": [{
                "port": 8888,
                "added": {"host": "localhost", "scheme": "http", "path": "/lab?token=secret"}
            }]
        }]
    });
    assert_eq!(
        PortRecords::parse(Some(&document.to_string())),
        Err(PortRecordError { field: "ports" })
    );
}

#[test]
fn added_addresses_must_read_back_as_typed_on_their_row() {
    let mut records = PortRecords::parse(None).unwrap();
    let ipv6 = PortRecord {
        added: Some(added("::1", None, None)),
        ..row(None, 3000)
    };
    records.set_port(&devbox(), "/srv", ipv6).unwrap();
    let all_interfaces = PortRecord {
        added: Some(added("0.0.0.0", Some("http"), None)),
        ..row(Some("0.0.0.0"), 80)
    };
    records.set_port(&devbox(), "/srv", all_interfaces).unwrap();

    for added in [
        // Another row's host.
        added("db.internal", None, None),
        // Not as `remote.parseForwardAddress` reads it back.
        added("LOCALHOST", None, None),
        added("localhost", Some("HTTP"), None),
        added("localhost", Some("http"), Some("lab")),
        added("localhost", Some("http"), Some("/a b")),
        added("dev@localhost", None, None),
        added("[::1]", None, None),
        added("", None, None),
        added("localhost", Some("1http"), None),
    ] {
        let record = PortRecord {
            added: Some(added),
            ..row(None, 3000)
        };
        assert_eq!(
            records.set_port(&devbox(), "/srv", record),
            Err(PortRecordError { field: "added" })
        );
    }
}

#[test]
fn a_confirmation_is_kept_only_for_a_running_forward_outside_the_hosts_loopback() {
    let mut records = PortRecords::parse(None).unwrap();
    records
        .set_port(
            &devbox(),
            "/srv",
            forwarded(Some("db.internal"), 5432, "DB.internal", true),
        )
        .unwrap();
    // Loopback targets never need one.
    records
        .set_port(&devbox(), "/srv", forwarded(None, 3000, "0.0.0.0", true))
        .unwrap();
    records
        .set_port(
            &devbox(),
            "/srv",
            forwarded(Some("127.0.0.2"), 3000, "127.0.0.2", true),
        )
        .unwrap();
    // Without a running forward there is nothing to confirm.
    let stopped = PortRecord {
        confirmed_target: true,
        ..labelled(Some("10.0.0.5"), 80, "Proxy")
    };
    records.set_port(&devbox(), "/srv", stopped).unwrap();

    let restorable: Vec<_> = records.records(&devbox(), "/srv").unwrap().ports;
    assert_eq!(
        restorable,
        [
            labelled(Some("10.0.0.5"), 80, "Proxy"),
            forwarded(None, 3000, "127.0.0.1", false),
            forwarded(Some("127.0.0.2"), 3000, "127.0.0.2", false),
            forwarded(Some("db.internal"), 5432, "db.internal", true),
        ]
    );
    let workspace = records.records(&devbox(), "/srv").unwrap();
    let targets: Vec<_> = workspace
        .restorable()
        .map(|(record, target)| (record.port, target))
        .collect();
    assert_eq!(
        targets,
        [
            (3000, "127.0.0.1"),
            (3000, "127.0.0.2"),
            (5432, "db.internal")
        ]
    );

    // Stopping the forward forgets the confirmation.
    records
        .set_port(&devbox(), "/srv", labelled(Some("db.internal"), 5432, "DB"))
        .unwrap();
    assert_eq!(
        records.records(&devbox(), "/srv").unwrap().ports[3],
        labelled(Some("db.internal"), 5432, "DB")
    );
}

#[test]
fn a_forward_target_must_be_the_rows_address() {
    let mut records = PortRecords::parse(None).unwrap();
    for (record, field) in [
        (forwarded(None, 3000, "db.internal", true), "forwardTarget"),
        (
            forwarded(Some("db.internal"), 3000, "localhost", false),
            "forwardTarget",
        ),
        (
            forwarded(None, 3000, "-oProxyCommand", false),
            "forwardTarget",
        ),
        (
            PortRecord {
                local_port: Some(0),
                ..forwarded(None, 3000, "localhost", false)
            },
            "localPort",
        ),
    ] {
        assert_eq!(
            records.set_port(&devbox(), "/srv", record),
            Err(PortRecordError { field })
        );
    }
    assert_eq!(records, PortRecords::parse(None).unwrap());
}

#[test]
fn a_row_with_nothing_to_remember_is_removed() {
    let mut records = PortRecords::parse(None).unwrap();
    let chosen = PortRecord {
        local_port: Some(13000),
        ..row(None, 3000)
    };
    records.set_port(&devbox(), "/srv", chosen.clone()).unwrap();
    assert_eq!(ports(&records, &devbox(), "/srv"), [chosen]);
    let blank = PortRecord {
        confirmed_target: true,
        ..row(None, 3000)
    };
    records.set_port(&devbox(), "/srv", blank).unwrap();
    assert_eq!(records, PortRecords::parse(None).unwrap());
    assert_eq!(stored(&records), json!({"version": 1, "workspaces": []}));
    // Removing what is not there changes nothing.
    records.remove_port(&devbox(), "/srv", None, 3000).unwrap();
    records
        .remove_rule(&devbox(), "/srv", reverse_rule(None, 3000))
        .unwrap();
    assert_eq!(records, PortRecords::parse(None).unwrap());

    let long = "x".repeat(MAX_LABEL_CHARS + 1);
    for label in ["", "a\nb", long.as_str()] {
        assert_eq!(
            records.set_port(&devbox(), "/srv", labelled(None, 1, label)),
            Err(PortRecordError { field: "label" })
        );
    }
    records
        .set_port(
            &devbox(),
            "/srv",
            labelled(None, 1, &"界".repeat(MAX_LABEL_CHARS)),
        )
        .unwrap();
}

#[test]
fn rules_are_the_same_rule_whatever_their_labels_and_keep_their_order() {
    let mut records = PortRecords::parse(None).unwrap();
    records
        .set_rule(&devbox(), "/srv", host_rule(Some(0), "DB.internal", None))
        .unwrap();
    records
        .set_rule(&devbox(), "/srv", reverse_rule(Some(9000), 3000))
        .unwrap();
    records
        .set_rule(
            &devbox(),
            "/srv",
            host_rule(None, "db.internal", Some("Database")),
        )
        .unwrap();
    records
        .set_rule(
            &devbox(),
            "/srv",
            host_rule(Some(15432), "db.internal", None),
        )
        .unwrap();
    assert_eq!(
        records.records(&devbox(), "/srv").unwrap().rules,
        [
            host_rule(None, "db.internal", Some("Database")),
            reverse_rule(Some(9000), 3000),
            host_rule(Some(15432), "db.internal", None),
        ]
    );
    records
        .remove_rule(
            &devbox(),
            "/srv",
            host_rule(None, "DB.INTERNAL", Some("other")),
        )
        .unwrap();
    assert_eq!(
        records.records(&devbox(), "/srv").unwrap().rules,
        [
            reverse_rule(Some(9000), 3000),
            host_rule(Some(15432), "db.internal", None)
        ]
    );
    assert_eq!(
        stored(&records)["workspaces"][0]["rules"],
        json!([
            {"kind": "reverse", "listenPort": 9000, "localPort": 3000},
            {"kind": "host", "listenPort": 15432, "targetHost": "db.internal", "targetPort": 5432},
        ])
    );

    for (rule, field) in [
        (host_rule(None, "-oProxyCommand", None), "targetHost"),
        (
            Rule::Host {
                listen_port: None,
                target_host: "db".into(),
                target_port: 0,
                label: None,
            },
            "targetPort",
        ),
        (reverse_rule(None, 0), "localPort"),
        (host_rule(None, "db", Some("")), "label"),
    ] {
        assert_eq!(
            records.set_rule(&devbox(), "/srv", rule),
            Err(PortRecordError { field })
        );
    }
}

#[test]
fn a_rule_never_records_listening_on_every_host_interface() {
    let document = |rule: serde_json::Value| {
        json!({
            "version": 1,
            "workspaces": [{"target": devbox(), "workspace": "/srv", "rules": [rule]}]
        })
        .to_string()
    };
    let refused = [
        json!({"kind": "reverse", "localPort": 3000, "allInterfaces": true}),
        json!({"kind": "host", "targetHost": "db", "targetPort": 5432, "allInterfaces": false}),
        json!({"kind": "host", "targetHost": "db", "targetPort": 5432, "confirmed": true}),
        json!({"kind": "local", "localPort": 3000}),
    ];
    for rule in refused {
        assert_eq!(
            PortRecords::parse(Some(&document(rule))),
            Err(PortRecordError { field: "document" })
        );
    }
    PortRecords::parse(Some(&document(
        json!({"kind": "reverse", "localPort": 3000}),
    )))
    .unwrap();
}

#[test]
fn the_workspace_edited_last_comes_first_and_the_oldest_makes_room() {
    let mut records = PortRecords::parse(None).unwrap();
    for index in 0..=MAX_WORKSPACES {
        records
            .set_port(&devbox(), &format!("/w/{index}"), labelled(None, 3000, "x"))
            .unwrap();
    }
    let paths = |records: &PortRecords| -> Vec<String> {
        records
            .workspaces
            .iter()
            .map(|entry| entry.workspace.clone())
            .collect()
    };
    let listed = paths(&records);
    assert_eq!(listed.len(), MAX_WORKSPACES);
    assert_eq!(listed[0], format!("/w/{MAX_WORKSPACES}"));
    assert_eq!(listed[MAX_WORKSPACES - 1], "/w/1");

    // An edit that changes nothing keeps the order.
    records
        .set_port(&devbox(), "/w/1", labelled(None, 3000, "x"))
        .unwrap();
    assert_eq!(paths(&records), listed);
    records
        .set_port(&devbox(), "/w/1", labelled(None, 3000, "y"))
        .unwrap();
    assert_eq!(paths(&records)[0], "/w/1");

    // A failed edit changes nothing either.
    let before = records.clone();
    records
        .set_port(&devbox(), "/w/2", labelled(None, 0, "x"))
        .unwrap_err();
    assert_eq!(records, before);

    // Room for the document's size is made the same way. The full workspaces
    // are written directly, so that one edit makes room instead of a
    // thousand edits filling it.
    let label = "界".repeat(MAX_LABEL_CHARS);
    let full = |index: usize| WorkspaceEntry {
        target: devbox(),
        workspace: format!("/big/{index}"),
        ports: (1..=MAX_PORTS as u16)
            .map(|port| labelled(None, port, &label))
            .collect(),
        rules: Vec::new(),
    };
    let mut records = PortRecords::parse(None).unwrap();
    while records.text().is_ok() {
        records.workspaces.push(full(records.workspaces.len()));
    }
    let before = paths(&records);
    records
        .set_port(&devbox(), "/big/0", labelled(None, 1, "edited"))
        .unwrap();
    let after = paths(&records);
    assert!(records.text().is_ok());
    assert!(after.len() < before.len());
    assert_eq!(after[..], before[..after.len()]);
}

#[test]
fn a_workspace_holds_a_bounded_number_of_rows_and_rules() {
    let mut records = PortRecords::parse(None).unwrap();
    for port in 1..=MAX_PORTS as u16 {
        records
            .set_port(&devbox(), "/srv", labelled(None, port, "x"))
            .unwrap();
    }
    assert_eq!(
        records.set_port(&devbox(), "/srv", labelled(None, 60000, "x")),
        Err(PortRecordError { field: "ports" })
    );
    // Replacing a row still works.
    records
        .set_port(&devbox(), "/srv", labelled(None, 1, "y"))
        .unwrap();

    for port in 1..=MAX_RULES as u16 {
        records
            .set_rule(&devbox(), "/srv", reverse_rule(None, port))
            .unwrap();
    }
    assert_eq!(
        records.set_rule(&devbox(), "/srv", reverse_rule(None, 60000)),
        Err(PortRecordError { field: "rules" })
    );
}

#[test]
fn targets_and_paths_are_checked_like_trust() {
    let mut records = PortRecords::parse(None).unwrap();
    let option = target(vec![Hop::Ssh(SshHop::new("-oProxyCommand=x"))], None);
    assert_eq!(
        records.set_port(&option, "/srv", labelled(None, 1, "x")),
        Err(PortRecordError { field: "target" })
    );
    for path in ["srv", "/srv/../etc", "/srv/", ""] {
        assert_eq!(
            records.set_port(&devbox(), path, labelled(None, 1, "x")),
            Err(PortRecordError { field: "workspace" })
        );
        assert_eq!(
            records.records(&devbox(), path),
            Err(PortRecordError { field: "workspace" })
        );
    }
}

#[test]
fn parse_refuses_documents_an_edit_never_writes() {
    let entry =
        |ports: serde_json::Value| json!({"target": devbox(), "workspace": "/srv", "ports": ports});
    let cases = [
        ("{".to_owned(), "document"),
        (json!({"version": 2, "workspaces": []}).to_string(), "version"),
        (json!({"version": 1, "extra": true}).to_string(), "document"),
        (
            r#"{"version": 1, "version": 1, "workspaces": []}"#.to_owned(),
            "document",
        ),
        (
            json!({"version": 1, "workspaces": [entry(json!([{"host": "LOCALHOST", "port": 1, "label": "x"}]))]})
                .to_string(),
            "ports",
        ),
        (
            json!({"version": 1, "workspaces": [entry(json!([{"port": 1, "label": "x"}, {"port": 1, "label": "y"}]))]})
                .to_string(),
            "ports",
        ),
        (
            json!({"version": 1, "workspaces": [entry(json!([{"port": 1, "label": "x", "confirmedTarget": true}]))]})
                .to_string(),
            "ports",
        ),
        (
            json!({"version": 1, "workspaces": [entry(json!([{"port": 1, "label": "x"}])), entry(json!([{"port": 2, "label": "y"}]))]})
                .to_string(),
            "workspaces",
        ),
        (
            json!({"version": 1, "workspaces": [{"target": devbox(), "workspace": "srv", "ports": [{"port": 1, "label": "x"}]}]})
                .to_string(),
            "workspace",
        ),
        (
            json!({"version": 1, "workspaces": [{"target": {"hops": [{"ssh": {"destination": "-o"}}]}, "workspace": "/srv", "ports": [{"port": 1, "label": "x"}]}]})
                .to_string(),
            "target",
        ),
        (
            json!({"version": 1, "workspaces": [{"target": devbox(), "workspace": "/srv", "rules": [
                {"kind": "reverse", "localPort": 1},
                {"kind": "reverse", "localPort": 1, "label": "again"},
            ]}]})
            .to_string(),
            "rules",
        ),
        (" ".repeat(MAX_BYTES + 1), "document"),
    ];
    for (document, field) in cases {
        assert_eq!(
            PortRecords::parse(Some(&document)),
            Err(PortRecordError { field }),
            "{field}"
        );
    }

    // Rows out of order are sorted, blank rows and empty workspaces dropped.
    let document = json!({"version": 1, "workspaces": [
        entry(json!([{"port": 9, "label": "b"}, {"port": 3}, {"port": 1, "label": "a"}])),
        {"target": devbox(), "workspace": "/empty"},
    ]});
    let records = PortRecords::parse(Some(&document.to_string())).unwrap();
    assert_eq!(
        stored(&records),
        json!({"version": 1, "workspaces": [
            entry(json!([{"port": 1, "label": "a"}, {"port": 9, "label": "b"}])),
        ]})
    );
}
