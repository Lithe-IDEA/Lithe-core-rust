use super::*;
use crate::client::bootstrap::probe_command;
use serde_json::json;

fn connection(value: serde_json::Value) -> SshConnection {
    serde_json::from_value::<SshConnection>(value)
        .expect("valid connection")
        .normalized()
}

/// The local command line the connection produces for the probe.
fn command_line(value: serde_json::Value) -> Vec<String> {
    let hop = connection(value).hop().expect("valid hop");
    TransportChain::new(vec![Hop::Ssh(hop)])
        .command(&probe_command())
        .unwrap()
}

fn option_value(command: &[String], key: &str) -> Option<String> {
    command
        .windows(2)
        .filter(|pair| pair[0] == "-o")
        .find_map(|pair| pair[1].strip_prefix(&format!("{key}=")).map(str::to_owned))
}

fn refusal(value: serde_json::Value) -> ConnectionError {
    connection(value.clone())
        .hop()
        .expect_err(&format!("{value} was accepted"))
}

/// Ids handed out in order, as Core's random generator would.
fn ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

/// Saves a connection to `host`, with an id derived from the host.
fn saved(connections: &mut Connections, name: &str, host: &str) -> String {
    connections
        .save(
            None,
            name,
            connection(json!({ "host": host })),
            Label::default(),
            || format!("id-{host}"),
        )
        .expect("saved")
}

#[test]
fn private_key_connections_offer_only_the_chosen_key() {
    let command = command_line(json!({
        "host": "devbox.example",
        "port": 2222,
        "user": "dev",
        "authentication": "privateKey",
        "identityFile": "~/.ssh/id_ed25519",
        "connectTimeoutSeconds": 20,
        "keepAliveSeconds": 30,
    }));
    assert!(command.windows(2).any(|pair| pair == ["-p", "2222"]));
    assert_eq!(&command[command.len() - 2], "dev@devbox.example");
    assert_eq!(
        option_value(&command, "IdentityFile").as_deref(),
        Some("~/.ssh/id_ed25519")
    );
    assert_eq!(
        option_value(&command, "IdentitiesOnly").as_deref(),
        Some("yes")
    );
    assert_eq!(
        option_value(&command, "PreferredAuthentications").as_deref(),
        Some("publickey")
    );
    assert_eq!(
        option_value(&command, "ConnectTimeout").as_deref(),
        Some("20")
    );
    // OpenSSH keeps the first value, so the connection's interval must come
    // before the transport's default.
    assert_eq!(
        option_value(&command, "ServerAliveInterval").as_deref(),
        Some("30")
    );
    assert_eq!(option_value(&command, "BatchMode").as_deref(), Some("no"));
}

#[test]
fn other_authentication_methods_select_matching_ssh_options() {
    let password = command_line(json!({ "host": "h", "authentication": "password" }));
    assert_eq!(
        option_value(&password, "PreferredAuthentications").as_deref(),
        Some("keyboard-interactive,password")
    );
    assert_eq!(
        option_value(&password, "PubkeyAuthentication").as_deref(),
        Some("no")
    );

    let agent = command_line(json!({ "host": "h", "authentication": "agent" }));
    assert_eq!(
        option_value(&agent, "PreferredAuthentications").as_deref(),
        Some("publickey")
    );
    assert_eq!(option_value(&agent, "IdentityFile"), None);

    // Automatic leaves the order to OpenSSH and the user's configuration,
    // and an alias without a user, port or jump host stays untouched.
    let automatic = command_line(json!({ "host": "devbox" }));
    assert_eq!(option_value(&automatic, "PreferredAuthentications"), None);
    assert_eq!(option_value(&automatic, "ProxyJump"), None);
    assert_eq!(&automatic[automatic.len() - 2], "devbox");
    assert!(!automatic.iter().any(|argument| argument == "-p"));
    assert_eq!(
        option_value(&automatic, "ConnectTimeout").as_deref(),
        Some(DEFAULT_CONNECT_TIMEOUT_SECONDS.to_string().as_str())
    );
}

#[test]
fn jump_hosts_and_quoted_key_paths_reach_ssh() {
    let command = command_line(json!({
        "host": "fe80::1%en0",
        "authentication": "privateKey",
        "identityFile": "~/My Keys/id_rsa",
        "proxyJump": "ops@bastion.example.com:2200,jump2",
    }));
    assert_eq!(
        option_value(&command, "ProxyJump").as_deref(),
        Some("ops@bastion.example.com:2200,jump2")
    );
    assert_eq!(
        option_value(&command, "IdentityFile").as_deref(),
        Some("\"~/My Keys/id_rsa\"")
    );
    assert_eq!(&command[command.len() - 2], "fe80::1%en0");

    let direct = command_line(json!({ "host": "box", "proxyJump": "none" }));
    assert_eq!(option_value(&direct, "ProxyJump").as_deref(), Some("none"));
}

#[test]
fn normalizing_trims_fields_and_keeps_a_key_only_for_key_authentication() {
    let trimmed = connection(json!({
        "host": "  box  ",
        "user": " ",
        "identityFile": "~/.ssh/id_ed25519",
        "proxyJump": "",
    }));
    assert_eq!(trimmed, connection(json!({ "host": "box" })));
    let key = connection(json!({
        "host": "box",
        "authentication": "privateKey",
        "identityFile": " ~/.ssh/id_ed25519 ",
    }));
    assert_eq!(key.identity_file.as_deref(), Some("~/.ssh/id_ed25519"));
    // Defaults are left out, so the document only shows what the user set.
    assert_eq!(
        serde_json::to_value(&trimmed).unwrap(),
        json!({ "host": "box" })
    );
}

#[test]
fn malformed_connections_name_the_field_before_ssh_starts() {
    use Reason::*;
    for (value, field, reason) in [
        (json!({ "host": "" }), "host", Required),
        (json!({ "host": "dev@box" }), "host", Invalid),
        (json!({ "host": "-oProxyCommand=x" }), "host", Invalid),
        (json!({ "host": "ssh://box" }), "host", Invalid),
        (json!({ "host": "box name" }), "host", Invalid),
        (json!({ "host": "a".repeat(1025) }), "host", TooLong),
        (json!({ "host": "box", "port": 0 }), "port", Invalid),
        (
            json!({ "host": "box", "user": "-oProxyCommand=x" }),
            "user",
            Invalid,
        ),
        (json!({ "host": "box", "user": "a b" }), "user", Invalid),
        (json!({ "host": "box", "user": "a:b" }), "user", Invalid),
        (
            json!({ "host": "box", "authentication": "privateKey" }),
            "identityFile",
            Required,
        ),
        (
            json!({ "host": "box", "authentication": "privateKey", "identityFile": "key\nProxyCommand x" }),
            "identityFile",
            Invalid,
        ),
        (
            json!({ "host": "box", "proxyJump": "-oProxyCommand=x" }),
            "proxyJump",
            Invalid,
        ),
        (
            json!({ "host": "box", "proxyJump": "bastion," }),
            "proxyJump",
            Invalid,
        ),
        (
            json!({ "host": "box", "proxyJump": "ssh://bastion" }),
            "proxyJump",
            Invalid,
        ),
        (
            json!({ "host": "box", "connectTimeoutSeconds": 0 }),
            "connectTimeoutSeconds",
            Invalid,
        ),
        (
            json!({ "host": "box", "connectTimeoutSeconds": 601 }),
            "connectTimeoutSeconds",
            Invalid,
        ),
        (
            json!({ "host": "box", "keepAliveSeconds": 0 }),
            "keepAliveSeconds",
            Invalid,
        ),
        (
            json!({ "host": "box", "keepAliveSeconds": 3601 }),
            "keepAliveSeconds",
            Invalid,
        ),
    ] {
        assert_eq!(
            refusal(value.clone()),
            ConnectionError { field, reason },
            "{value}"
        );
    }
    // Secrets and local commands have no field to arrive in.
    for value in [
        json!({ "host": "h", "password": "x" }),
        json!({ "host": "h", "proxyCommand": "nc %h %p" }),
    ] {
        assert!(serde_json::from_value::<SshConnection>(value).is_err());
    }
}

#[test]
fn saved_connections_round_trip_through_the_document() {
    let mut connections = Connections::parse(None).unwrap();
    assert!(connections.entries().is_empty());
    let id = connections
        .save(
            None,
            "  Build box ",
            connection(json!({ "host": "build.example", "user": "ci", "port": 2222 })),
            Label::default(),
            ids(),
        )
        .unwrap();
    connections.remember_folder(&id, "/srv/app").unwrap();
    let text = connections.text().unwrap();
    assert!(text.ends_with('\n'));
    let read = Connections::parse(Some(&text)).unwrap();
    assert_eq!(read, connections);
    let entry = read.get(&id).unwrap();
    assert_eq!(entry.name, "Build box");
    assert_eq!(entry.connection.host, "build.example");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        json!({
            "version": 1,
            "connections": [{
                "id": id,
                "name": "Build box",
                "connection": { "host": "build.example", "port": 2222, "user": "ci" },
                "recentFolders": ["/srv/app"],
            }],
        })
    );
}

#[test]
fn labels_are_saved_and_groups_become_sections_in_first_use_order() {
    let mut connections = Connections::default();
    assert!(connections.sections().is_empty());
    let label = |color: Option<Color>, group: &str| Label {
        color,
        group: Some(group.to_owned()),
    };
    let save = |connections: &mut Connections, name: &str, label: Label| {
        connections
            .save(None, name, connection(json!({ "host": "h" })), label, ids())
            .unwrap()
    };
    let web = save(
        &mut connections,
        "Web",
        label(Some(Color::Green), " Production "),
    );
    let lab = save(&mut connections, "Lab", label(Some(Color::Purple), "   "));
    let db = save(&mut connections, "Db", label(None, "PRODUCTION"));
    let ci = save(&mut connections, "Ci", label(None, "Build"));
    let section = |group: Option<&'static str>, ids: Vec<&str>| (group, ids.join(","));
    fn listed(connections: &Connections) -> Vec<(Option<&str>, String)> {
        connections
            .sections()
            .into_iter()
            .map(|section| (section.group, section.ids.join(",")))
            .collect()
    }
    // A blank group is no group; another case takes the first spelling.
    assert_eq!(
        listed(&connections),
        vec![
            section(Some("Production"), vec![&web, &db]),
            section(Some("Build"), vec![&ci]),
            section(None, vec![&lab]),
        ]
    );
    let text = connections.text().unwrap();
    let stored: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(stored["connections"][0]["color"], json!("green"));
    assert_eq!(stored["connections"][0]["group"], json!("Production"));
    assert_eq!(stored["connections"][1]["color"], json!("purple"));
    assert!(stored["connections"][1].get("group").is_none());
    assert!(stored["connections"][2].get("color").is_none());
    assert_eq!(stored["connections"][2]["group"], json!("Production"));
    assert_eq!(Connections::parse(Some(&text)).unwrap(), connections);

    // An entry alone in its group may change the group's case; saving
    // without a label clears both fields.
    connections
        .save(
            Some(&ci),
            "Ci",
            connection(json!({ "host": "h" })),
            label(None, "BUILD"),
            ids(),
        )
        .unwrap();
    connections
        .save(
            Some(&web),
            "Web",
            connection(json!({ "host": "h" })),
            Label::default(),
            ids(),
        )
        .unwrap();
    assert_eq!(connections.get(&web).unwrap().color, None);
    assert_eq!(
        listed(&connections),
        vec![
            section(Some("Production"), vec![&db]),
            section(Some("BUILD"), vec![&ci]),
            section(None, vec![&web, &lab]),
        ]
    );

    for (group, reason) in [
        ("tab\there".to_owned(), Reason::Invalid),
        ("组".repeat(65), Reason::TooLong),
    ] {
        assert_eq!(
            connections.save(
                None,
                "Other",
                connection(json!({ "host": "h" })),
                label(None, &group),
                ids()
            ),
            Err(ConnectionError::at("group", reason)),
            "{group:?}"
        );
    }
    assert!(connections
        .save(
            None,
            "Other",
            connection(json!({ "host": "h" })),
            label(None, &"组".repeat(64)),
            ids()
        )
        .is_ok());
}

#[test]
fn saving_refuses_names_that_only_differ_in_case_and_unknown_ids() {
    let mut connections = Connections::default();
    let first = saved(&mut connections, "Devbox", "devbox");
    assert_eq!(
        connections.save(
            None,
            "devbox",
            connection(json!({ "host": "other" })),
            Label::default(),
            ids()
        ),
        Err(ConnectionError::at("name", Reason::Duplicate))
    );
    // An entry keeps its own name when the user edits it.
    assert_eq!(
        connections.save(
            Some(&first),
            "DEVBOX",
            connection(json!({ "host": "devbox" })),
            Label::default(),
            ids()
        ),
        Ok(first.clone())
    );
    assert_eq!(
        connections.save(
            Some("missing"),
            "Other",
            connection(json!({ "host": "h" })),
            Label::default(),
            ids()
        ),
        Err(ConnectionError::at("id", Reason::Unknown))
    );
    for (name, reason) in [
        ("", Reason::Required),
        ("   ", Reason::Required),
        ("tab\there", Reason::Invalid),
    ] {
        assert_eq!(
            connections.save(
                None,
                name,
                connection(json!({ "host": "h" })),
                Label::default(),
                ids()
            ),
            Err(ConnectionError::at("name", reason)),
            "{name:?}"
        );
    }
    let long = "名".repeat(129);
    assert_eq!(
        connections.save(
            None,
            &long,
            connection(json!({ "host": "h" })),
            Label::default(),
            ids()
        ),
        Err(ConnectionError::at("name", Reason::TooLong))
    );
    assert_eq!(connections.entries().len(), 1);
}

#[test]
fn recent_folders_follow_the_target_not_the_name() {
    let mut connections = Connections::default();
    let id = saved(&mut connections, "Devbox", "devbox");
    for index in 0..12 {
        connections
            .remember_folder(&id, &format!("/srv/{index}"))
            .unwrap();
    }
    connections.remember_folder(&id, "/srv/5").unwrap();
    let folders = &connections.get(&id).unwrap().recent_folders;
    assert_eq!(folders.len(), 10);
    assert_eq!(folders[0], "/srv/5");
    assert_eq!(folders[1], "/srv/11");
    assert_eq!(
        folders.iter().filter(|folder| *folder == "/srv/5").count(),
        1
    );
    for path in ["srv", "/srv/../etc", "/srv/", ""] {
        assert_eq!(
            connections.remember_folder(&id, path),
            Err(ConnectionError::at("path", Reason::Invalid)),
            "{path:?}"
        );
    }
    assert_eq!(
        connections.remember_folder("missing", "/srv"),
        Err(ConnectionError::at("id", Reason::Unknown))
    );

    // Renaming or changing how to authenticate keeps the folders.
    connections
        .save(
            Some(&id),
            "Renamed",
            connection(
                json!({ "host": "devbox", "authentication": "agent", "keepAliveSeconds": 30 }),
            ),
            Label::default(),
            ids(),
        )
        .unwrap();
    assert_eq!(connections.get(&id).unwrap().recent_folders.len(), 10);
    // Another host, port, user or jump host forgets them.
    for target in [
        json!({ "host": "other" }),
        json!({ "host": "other", "port": 2222 }),
        json!({ "host": "other", "port": 2222, "user": "dev" }),
        json!({ "host": "other", "port": 2222, "user": "dev", "proxyJump": "bastion" }),
    ] {
        connections.remember_folder(&id, "/srv").unwrap();
        connections
            .save(
                Some(&id),
                "Renamed",
                connection(target.clone()),
                Label::default(),
                ids(),
            )
            .unwrap();
        assert!(
            connections.get(&id).unwrap().recent_folders.is_empty(),
            "{target}"
        );
    }
}

#[test]
fn removing_keeps_the_order_and_ignores_absent_ids() {
    let mut connections = Connections::default();
    let first = saved(&mut connections, "One", "one");
    let mut second_ids = || "second".to_owned();
    connections
        .save(
            None,
            "Two",
            connection(json!({ "host": "two" })),
            Label::default(),
            &mut second_ids,
        )
        .unwrap();
    let mut third_ids = || "third".to_owned();
    connections
        .save(
            None,
            "Three",
            connection(json!({ "host": "three" })),
            Label::default(),
            &mut third_ids,
        )
        .unwrap();
    connections.remove("second");
    let before = connections.clone();
    connections.remove("missing");
    assert_eq!(connections, before);
    let names: Vec<_> = connections
        .entries()
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, ["One", "Three"]);
    assert_eq!(connections.entries()[0].id, first);
}

#[test]
fn new_ids_are_drawn_until_one_is_valid_and_unused() {
    let mut connections = Connections::default();
    let mut first = || "taken".to_owned();
    connections
        .save(
            None,
            "One",
            connection(json!({ "host": "one" })),
            Label::default(),
            &mut first,
        )
        .unwrap();
    let mut candidates = ["taken", "bad id", "", "fresh"]
        .into_iter()
        .map(str::to_owned);
    let id = connections
        .save(
            None,
            "Two",
            connection(json!({ "host": "two" })),
            Label::default(),
            || candidates.next().unwrap(),
        )
        .unwrap();
    assert_eq!(id, "fresh");
}

#[test]
fn the_list_holds_at_most_128_connections() {
    let mut connections = Connections::default();
    for index in 0..128 {
        saved(
            &mut connections,
            &format!("Host {index}"),
            &format!("h{index}"),
        );
    }
    assert_eq!(
        connections.save(
            None,
            "One more",
            connection(json!({ "host": "h" })),
            Label::default(),
            ids()
        ),
        Err(ConnectionError::at("connections", Reason::Full))
    );
    // Editing an entry still works when the list is full.
    let id = connections.entries()[0].id.to_owned();
    assert!(connections
        .save(
            Some(&id),
            "Host 0",
            connection(json!({ "host": "h0", "port": 22 })),
            Label::default(),
            ids()
        )
        .is_ok());
    assert!(Connections::parse(Some(&connections.text().unwrap())).is_ok());
}

#[test]
fn stored_documents_are_checked_like_edits() {
    let entry =
        |id: &str, name: &str| json!({ "id": id, "name": name, "connection": { "host": "h" } });
    let document = |connections: serde_json::Value| {
        json!({ "version": 1, "connections": connections }).to_string()
    };
    for (text, field, reason) in [
        ("{".to_owned(), "document", Reason::Invalid),
        ("[]".to_owned(), "document", Reason::Invalid),
        (
            r#"{"version":1,"version":1,"connections":[]}"#.to_owned(),
            "document",
            Reason::Invalid,
        ),
        (
            json!({ "version": 1, "connections": [], "extra": true }).to_string(),
            "document",
            Reason::Invalid,
        ),
        (
            json!({ "version": 2 }).to_string(),
            "version",
            Reason::Invalid,
        ),
        (
            document(json!([entry("a", "One"), entry("a", "Two")])),
            "id",
            Reason::Duplicate,
        ),
        (
            document(json!([entry("a", "One"), entry("b", "ONE")])),
            "name",
            Reason::Duplicate,
        ),
        (
            document(json!([entry("a b", "One")])),
            "id",
            Reason::Invalid,
        ),
        (
            document(json!([entry("a", " One")])),
            "name",
            Reason::Invalid,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h", "port": 0 } }]),
            ),
            "port",
            Reason::Invalid,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h" }, "recentFolders": ["relative"] }]),
            ),
            "recentFolders",
            Reason::Invalid,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h" }, "recentFolders": ["/a", "/a"] }]),
            ),
            "recentFolders",
            Reason::Duplicate,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h", "password": "x" } }]),
            ),
            "document",
            Reason::Invalid,
        ),
        (
            document(json!((0..129)
                .map(|index| entry(&format!("i{index}"), &format!("n{index}")))
                .collect::<Vec<_>>())),
            "connections",
            Reason::Full,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h" }, "group": " Prod" }]),
            ),
            "group",
            Reason::Invalid,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h" }, "group": "" }]),
            ),
            "group",
            Reason::Required,
        ),
        (
            document(
                json!([{ "id": "a", "name": "One", "connection": { "host": "h" }, "color": "pink" }]),
            ),
            "document",
            Reason::Invalid,
        ),
        (" ".repeat(MAX_BYTES + 1), "document", Reason::TooLong),
    ] {
        assert_eq!(
            Connections::parse(Some(&text)),
            Err(ConnectionError { field, reason }),
            "{text:.80}"
        );
    }
}
