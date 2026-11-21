use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

fn connection(value: serde_json::Value) -> SshConnection {
    serde_json::from_value::<SshConnection>(value)
        .expect("valid connection")
        .normalized()
}

/// `ssh -G` output with the values the tests compare, as OpenSSH prints
/// them; `extra` lines replace or add keys.
fn output(extra: &[&str]) -> Output {
    let mut lines = vec![
        "user dev",
        "hostname build.example",
        "port 22",
        "connecttimeout 15",
        "serveraliveinterval 15",
        "identityfile ~/.ssh/id_rsa",
        "identityfile ~/.ssh/id_ed25519",
        "identitiesonly no",
    ];
    for line in extra {
        let key = line.split(' ').next().unwrap();
        if key != "identityfile" {
            lines.retain(|existing| !existing.starts_with(&format!("{key} ")));
        }
        lines.push(line);
    }
    Output::parse(&lines.join("\n"))
}

/// A scratch home directory, removed when dropped.
struct ScratchHome(PathBuf);

impl ScratchHome {
    fn new(label: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lithe-remote-ssh-config-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join(".ssh")).unwrap();
        Self(path)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn file_names(&self) -> Vec<String> {
        configuration_files(&self.0)
            .into_iter()
            .map(|(path, _)| {
                path.strip_prefix(&self.0)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }
}

impl Drop for ScratchHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn values_the_connection_sets_come_from_the_connection() {
    let saved = connection(json!({
        "host": "build",
        "port": 2222,
        "user": "ci",
        "proxyJump": "bastion.example",
        "connectTimeoutSeconds": 30,
        "keepAliveSeconds": 60,
    }));
    let configured = output(&[
        "hostname build.internal.example",
        "port 2222",
        "user ci",
        "proxyjump bastion.example",
        "connecttimeout 30",
        "serveraliveinterval 60",
    ]);
    let baseline = output(&[
        "hostname build",
        "port 2222",
        "user ci",
        "proxyjump bastion.example",
        "connecttimeout 30",
        "serveraliveinterval 60",
    ]);
    let settings = effective_settings(&saved, &configured, &baseline).unwrap();
    assert_eq!(
        settings.host_name,
        Effective {
            value: "build.internal.example".into(),
            source: Source::SshConfig,
        }
    );
    assert_eq!(settings.port.source, Source::Connection);
    assert_eq!(settings.user.source, Source::Connection);
    assert_eq!(
        settings.proxy_jump,
        Effective {
            value: Some("bastion.example".into()),
            source: Source::Connection,
        }
    );
    assert_eq!(settings.connect_timeout_seconds.value, 30);
    assert_eq!(settings.connect_timeout_seconds.source, Source::Connection);
    assert_eq!(settings.keep_alive_seconds.value, 60);
    assert_eq!(settings.keep_alive_seconds.source, Source::Connection);
}

#[test]
fn values_left_to_ssh_come_from_the_configuration_or_a_default() {
    let saved = connection(json!({ "host": "build.example" }));
    let configured = output(&[
        "user ops",
        "port 2200",
        "identityfile ~/.ssh/work_ed25519",
        "proxyjump jump.example",
    ]);
    let settings = effective_settings(&saved, &configured, &output(&[])).unwrap();
    assert_eq!(settings.host_name.source, Source::Connection);
    assert_eq!(
        settings.user,
        Effective {
            value: "ops".into(),
            source: Source::SshConfig,
        }
    );
    assert_eq!(settings.port.value, 2200);
    assert_eq!(settings.port.source, Source::SshConfig);
    assert_eq!(settings.identity_files.source, Source::SshConfig);
    assert_eq!(
        settings.identity_files.value.last().map(String::as_str),
        Some("~/.ssh/work_ed25519")
    );
    assert_eq!(settings.proxy_jump.value.as_deref(), Some("jump.example"));
    assert_eq!(settings.proxy_jump.source, Source::SshConfig);
    // Lithe passes both on the command line, so the files cannot set them.
    assert_eq!(settings.connect_timeout_seconds.value, 15);
    assert_eq!(settings.connect_timeout_seconds.source, Source::Default);
    assert_eq!(settings.keep_alive_seconds.source, Source::Default);

    let plain = effective_settings(&saved, &output(&[]), &output(&[])).unwrap();
    assert_eq!(plain.user.source, Source::Default);
    assert_eq!(plain.port.source, Source::Default);
    assert_eq!(plain.identity_files.source, Source::Default);
    assert_eq!(
        plain.proxy_jump,
        Effective {
            value: None,
            source: Source::Default,
        }
    );
}

#[test]
fn authentication_decides_the_key_files_shown() {
    let configured = output(&["identityfile ~/.ssh/chosen"]);
    let password = connection(json!({ "host": "h", "authentication": "password" }));
    let settings = effective_settings(&password, &configured, &output(&[])).unwrap();
    assert!(settings.identity_files.value.is_empty());
    assert_eq!(settings.identity_files.source, Source::Connection);

    let key = connection(json!({
        "host": "h",
        "authentication": "privateKey",
        "identityFile": "~/.ssh/chosen",
    }));
    let settings = effective_settings(&key, &configured, &output(&[])).unwrap();
    assert_eq!(settings.identity_files.source, Source::Connection);
    assert!(settings
        .identity_files
        .value
        .contains(&"~/.ssh/chosen".to_owned()));
}

#[test]
fn output_without_required_keys_is_refused() {
    let saved = connection(json!({ "host": "h" }));
    let missing = Output::parse("user dev\nport 22\n");
    assert_eq!(
        effective_settings(&saved, &missing, &missing).unwrap_err(),
        OutputError { key: "hostname" }
    );
    let bad_port = output(&["port many"]);
    assert_eq!(
        effective_settings(&saved, &bad_port, &bad_port).unwrap_err(),
        OutputError { key: "port" }
    );
    assert_eq!(
        configured_host(&output(&["proxyjump none"])).unwrap(),
        ConfiguredHost {
            host_name: "build.example".into(),
            user: "dev".into(),
            port: 22,
            proxy_jump: None,
        }
    );
}

#[test]
fn settings_serialize_with_their_sources() {
    let saved = connection(json!({ "host": "h", "port": 2222 }));
    let settings = effective_settings(&saved, &output(&["port 2222"]), &output(&[])).unwrap();
    let value = serde_json::to_value(settings).unwrap();
    assert_eq!(
        value["port"],
        json!({ "value": 2222, "source": "connection" })
    );
    assert_eq!(
        value["proxyJump"],
        json!({ "value": null, "source": "default" })
    );
    assert_eq!(value["hostName"]["source"], "connection");
    assert_eq!(value["keepAliveSeconds"]["value"], 15);
}

#[test]
fn host_lines_list_aliases_patterns_and_includes() {
    let (names, includes) = host_aliases(
        "# Build machines\n\
         Host build gpu-1 \"two words\"\n\
         \tHostName build.example\n\
         host=*.lab !secret dev?\n\
         Match host other exec \"true\"\n\
         \x20 Include config.d/* ~/extra\n\
         HOST staging\n",
    );
    assert_eq!(
        names,
        vec![
            HostName::Alias("build".into()),
            HostName::Alias("gpu-1".into()),
            HostName::Alias("two words".into()),
            HostName::Pattern("*.lab".into()),
            HostName::Pattern("!secret".into()),
            HostName::Pattern("dev?".into()),
            HostName::Alias("staging".into()),
        ]
    );
    assert_eq!(includes, vec!["config.d/*", "~/extra"]);
}

#[test]
fn jump_lists_name_their_hosts() {
    assert_eq!(
        jump_hosts("ops@bastion:2200,[fe80::1]:22, inner"),
        vec!["bastion", "fe80::1", "inner"]
    );
    assert!(jump_hosts("").is_empty());
}

#[test]
fn includes_are_read_in_order_below_the_home_directory() {
    let home = ScratchHome::new("includes");
    home.write(
        ".ssh/config",
        "Include config.d/*.conf\nInclude ~/other/ssh\nInclude /etc/ssh/ssh_config\nInclude ../escape\nHost main\n",
    );
    home.write(".ssh/config.d/b.conf", "Host b\n");
    home.write(".ssh/config.d/a.conf", "Host a\nInclude config\n");
    home.write(".ssh/config.d/.hidden.conf", "Host hidden\n");
    home.write(".ssh/config.d/c.txt", "Host c\n");
    home.write("other/ssh", "Host other\n");
    assert_eq!(
        home.file_names(),
        vec![
            ".ssh/config",
            ".ssh/config.d/a.conf",
            ".ssh/config.d/b.conf",
            "other/ssh"
        ]
    );
}

#[test]
fn missing_large_and_deep_configurations_stop_reading() {
    let home = ScratchHome::new("bounds");
    assert!(home.file_names().is_empty());

    home.write(".ssh/config", "Include big\nInclude level1\n");
    home.write(".ssh/big", &"#".repeat(MAX_FILE_BYTES as usize + 1));
    for level in 1..=20 {
        home.write(
            &format!(".ssh/level{level}"),
            &format!("Include level{}\n", level + 1),
        );
    }
    let names = home.file_names();
    assert!(!names.contains(&".ssh/big".to_owned()));
    assert_eq!(names.len(), 1 + MAX_INCLUDE_DEPTH);
}

#[test]
fn wildcards_match_within_one_name() {
    assert!(wildcard_match("*.conf", "a.conf"));
    assert!(wildcard_match("a?c", "abc"));
    assert!(wildcard_match("*", "anything"));
    assert!(!wildcard_match("*.conf", "a.conf.bak"));
    assert!(!wildcard_match("a?c", "ac"));
}
