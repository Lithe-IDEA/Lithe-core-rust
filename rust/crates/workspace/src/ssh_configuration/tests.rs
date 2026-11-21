use super::*;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory removed when dropped.
struct Directory(PathBuf);

impl Directory {
    fn new(label: &str) -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "lithe-workspace-ssh-{label}-{}-{}",
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn write(&self, relative: &str, text: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Stand-in for OpenSSH that prints `ssh -G` output: `configured` for the
/// destination with the configuration files, `baseline` with `-F none`.
/// Both are shell `case` arms keyed on the destination, the last argument.
#[cfg(unix)]
fn fake_ssh(directory: &Directory, configured: &str, baseline: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let program = directory.path().join("ssh");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nfor last; do :; done\ncase \" $* \" in\n*\" -F none \"*)\ncase \"$last\" in\n{baseline}\nesac;;\n*)\ncase \"$last\" in\n{configured}\nesac;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program.display().to_string()
}

fn lines(values: &[&str]) -> String {
    values.join("\n")
}

const DEFAULTS: &[&str] = &[
    "user localdev",
    "port 22",
    "identityfile ~/.ssh/id_ed25519",
    "connecttimeout 15",
    "serveraliveinterval 15",
    "proxyjump none",
];

#[cfg(unix)]
#[test]
fn effective_settings_tell_where_each_value_comes_from() {
    let directory = Directory::new("effective");
    let configured = format!(
        "*) echo '{}';;",
        lines(&[
            "hostname 10.0.0.5",
            "user dev",
            "port 2222",
            "identityfile ~/.ssh/build_key",
            "connecttimeout 15",
            "serveraliveinterval 15",
            "proxyjump jump-box",
        ])
    );
    let baseline = format!("*) echo 'hostname build-box\n{}';;", lines(DEFAULTS));
    let ssh = fake_ssh(&directory, &configured, &baseline);
    let request: EffectiveRequest = serde_json::from_value(json!({
        "connection": { "host": "build-box", "user": "dev" }
    }))
    .unwrap();

    let settings = effective_with(request, Some(&ssh)).unwrap();

    assert_eq!(
        settings["hostName"],
        json!({ "value": "10.0.0.5", "source": "sshConfig" })
    );
    assert_eq!(
        settings["user"],
        json!({ "value": "dev", "source": "connection" })
    );
    assert_eq!(
        settings["port"],
        json!({ "value": 2222, "source": "sshConfig" })
    );
    assert_eq!(
        settings["identityFiles"],
        json!({ "value": ["~/.ssh/build_key"], "source": "sshConfig" })
    );
    assert_eq!(
        settings["proxyJump"],
        json!({ "value": "jump-box", "source": "sshConfig" })
    );
    assert_eq!(
        settings["connectTimeoutSeconds"],
        json!({ "value": 15, "source": "default" })
    );
    assert_eq!(
        settings["keepAliveSeconds"],
        json!({ "value": 15, "source": "default" })
    );
}

#[cfg(unix)]
#[test]
fn a_failing_client_reports_its_redacted_output() {
    let directory = Directory::new("failing");
    let ssh = fake_ssh(
        &directory,
        "*) echo 'bad configuration in /Users/someone/.ssh/config' >&2; exit 255;;",
        "*) exit 255;;",
    );
    let request: EffectiveRequest =
        serde_json::from_value(json!({ "connection": { "host": "build-box" } })).unwrap();

    let error = effective_with(request, Some(&ssh)).unwrap_err();

    assert!(matches!(error.code, ErrorCode::ProcessFailed));
    let details = error.details.unwrap();
    assert!(details.contains("<HOME>/.ssh/config"), "{details}");
    assert!(!details.contains("someone"), "{details}");
}

#[test]
fn a_missing_client_is_a_missing_runtime() {
    let directory = Directory::new("missing");
    let ssh = directory.path().join("no-ssh-here").display().to_string();
    let request: EffectiveRequest =
        serde_json::from_value(json!({ "connection": { "host": "build-box" } })).unwrap();

    let error = effective_with(request, Some(&ssh)).unwrap_err();

    assert!(matches!(error.code, ErrorCode::RuntimeMissing));
}

#[test]
fn a_malformed_connection_names_the_field() {
    let request: EffectiveRequest = serde_json::from_value(json!({
        "connection": { "host": "dev@build-box" }
    }))
    .unwrap();

    let error = effective_settings(request).unwrap_err();

    assert!(matches!(error.code, ErrorCode::InvalidRequest));
    assert!(error.details.unwrap().contains("\"host\""));
}

#[cfg(unix)]
#[test]
fn configuration_hosts_are_listed_with_what_they_resolve_to() {
    let home = Directory::new("hosts");
    home.write(
        ".ssh/config",
        "Include conf.d/*\nHost build-box gpu\n  HostName 10.0.0.5\nHost *.internal !bastion\n  User ops\nHost jump-box\n  HostName 203.0.113.7\n",
    );
    home.write(".ssh/conf.d/lab", "Host lab Build-Box\n  User lab\n");
    let ssh = fake_ssh(
        &home,
        &lines(&[
            "build-box) echo 'hostname 10.0.0.5\nuser dev\nport 22\nproxyjump jump-box';;",
            "gpu) echo 'hostname 10.0.0.9\nuser dev\nport 2200\nproxyjump admin@jump-box:2222';;",
            "jump-box) echo 'hostname 203.0.113.7\nuser dev\nport 22\nproxyjump none';;",
            "lab) echo 'no configuration' >&2; exit 255;;",
        ]),
        "*) exit 1;;",
    );
    let saved = crate::connections::edit(
        serde_json::from_value(json!({
            "document": null,
            "save": {
                "name": "GPU",
                "connection": { "host": "10.0.0.9", "user": "dev", "port": 2200 }
            },
        }))
        .unwrap(),
    )
    .unwrap();
    let saved_id = saved["id"].as_str().unwrap().to_owned();
    let document = saved["document"].as_str().unwrap().to_owned();
    let request = HostsRequest {
        home_directory: home.path().display().to_string(),
        document: Some(document),
    };

    let result = hosts_with(request, Some(&ssh)).unwrap();

    let files: Vec<&str> = result["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| file.as_str().unwrap())
        .collect();
    assert_eq!(files.len(), 2);
    assert!(files[0].ends_with("config") && files[1].ends_with("lab"));
    let hosts = result["hosts"].as_array().unwrap();
    let summary: Vec<(&str, &str)> = hosts
        .iter()
        .map(|host| {
            (
                host["alias"].as_str().unwrap(),
                host["status"].as_str().unwrap(),
            )
        })
        .collect();
    // `Build-Box` in the included file repeats `build-box` regardless of
    // case.
    assert_eq!(
        summary,
        [
            ("build-box", "importable"),
            ("gpu", "exists"),
            ("*.internal", "pattern"),
            ("!bastion", "pattern"),
            ("jump-box", "importable"),
            ("lab", "unreadable"),
        ]
    );
    let build = &hosts[0];
    assert_eq!(
        build["resolved"],
        json!({ "hostName": "10.0.0.5", "user": "dev", "port": 22, "proxyJump": "jump-box" })
    );
    assert_eq!(hosts[1]["existingId"], json!(saved_id));
    assert_eq!(hosts[1]["existingName"], "GPU");
    assert_eq!(hosts[4]["usedAsJumpBy"], json!(["build-box", "gpu"]));
    assert_eq!(build["usedAsJumpBy"], json!([]));
    assert_eq!(result["truncated"], false);
}

#[test]
fn a_home_without_configuration_lists_nothing() {
    let home = Directory::new("empty");
    let request = HostsRequest {
        home_directory: home.path().display().to_string(),
        document: None,
    };

    let result = hosts_with(request, Some("ssh-is-never-run")).unwrap();

    assert_eq!(
        result,
        json!({ "files": [], "hosts": [], "truncated": false })
    );
}

#[test]
fn a_relative_home_is_refused() {
    let request = HostsRequest {
        home_directory: "home".into(),
        document: None,
    };

    let error = ssh_config_hosts(request).unwrap_err();

    assert!(matches!(error.code, ErrorCode::InvalidRequest));
}

#[test]
fn aliases_stop_at_the_limit() {
    let text: String = (0..MAX_ALIASES + 3)
        .map(|index| format!("Host host-{index}\n"))
        .collect();
    let (names, truncated) = distinct_names(&[(PathBuf::from("config"), text)]);

    assert_eq!(names.len(), MAX_ALIASES);
    assert!(truncated);
}
