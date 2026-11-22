//! Running containers as remote workspaces.
//!
//! A container on this machine, on an SSH host or inside a WSL distribution
//! is reached through its engine's `exec -i`. [`ContainerConnection`] names
//! the engine, the container and, optionally, the user, and becomes the last
//! hop of a transport chain; the server is then probed, installed and
//! started inside the container exactly as on an SSH host. Nothing is saved:
//! the connection window lists the running containers each time it asks,
//! with [`list_command`] and [`parse_containers`].
//!
//! Lithe only runs `exec` in a container that already runs. It does not
//! start, create or reconfigure containers, mount directories or ask for
//! elevated rights: whoever may use the engine may already do all of that,
//! so the engine is not a security boundary (S32).
//!
//! Note: .agents/notes/proposed/architecture/2026-10-24-remote-development-workspace-host.md

use crate::client::connections::{ConnectionError, Reason};
use crate::client::transport::{
    is_container_name, is_container_user, ContainerEngine, ContainerHop, MAX_CONTAINER_NAME_BYTES,
    MAX_CONTAINER_USER_BYTES,
};
use serde::{Deserialize, Serialize};

/// Containers one listing returns at most; later ones are dropped.
pub const MAX_CONTAINERS: usize = 256;

/// Bytes of `ps` output read at most; the rest is ignored.
pub const MAX_LIST_BYTES: usize = 256 * 1024;

/// Longest image or status text a listing keeps, in bytes; longer text is
/// cut at a character boundary.
const MAX_TEXT_BYTES: usize = 256;

/// `remote.connect`'s `container` field: the engine, the running container
/// and the user. On its own it names a container on this machine; next to
/// `connection` or `wsl` a container on that host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerConnection {
    /// Engine whose CLI runs `exec`.
    pub engine: ContainerEngine,
    /// Container name or id, as `ps` lists it.
    pub container: String,
    /// `user`, `uid` or `user:group` passed as `--user`; the image's default
    /// user when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

impl ContainerConnection {
    /// The hop into the container through the engine CLI on `PATH`. Both
    /// fields are trimmed and an empty `user` means the image's default user.
    ///
    /// Refused, with the field and the reason, when the container is empty,
    /// longer than 128 bytes or not a name or id the engine CLI cannot read
    /// as an option, or the user is longer than 64 bytes or not a name, uid
    /// or `user:group`.
    pub fn hop(&self) -> Result<ContainerHop, ConnectionError> {
        let container = self.container.trim();
        if container.is_empty() {
            return Err(ConnectionError::at("container", Reason::Required));
        }
        if container.len() > MAX_CONTAINER_NAME_BYTES {
            return Err(ConnectionError::at("container", Reason::TooLong));
        }
        if !is_container_name(container) {
            return Err(ConnectionError::at("container", Reason::Invalid));
        }
        let user = self
            .user
            .as_deref()
            .map(str::trim)
            .filter(|user| !user.is_empty());
        if let Some(user) = user {
            if user.len() > MAX_CONTAINER_USER_BYTES {
                return Err(ConnectionError::at("user", Reason::TooLong));
            }
            if !is_container_user(user) {
                return Err(ConnectionError::at("user", Reason::Invalid));
            }
        }
        let mut hop = ContainerHop::new(self.engine, container);
        hop.user = user.map(Into::into);
        Ok(hop)
    }
}

/// One running container in a listing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListedContainer {
    /// Full container id.
    pub id: String,
    /// The container's name; its first one when the engine lists several.
    pub name: String,
    /// Image the container runs, as the engine prints it.
    pub image: String,
    /// The engine's own status text, for example `Up 2 hours`.
    pub status: String,
}

/// Command line, starting with the engine CLI's name, that prints the
/// running containers one per line: full id, names, image and status,
/// separated by tabs. Docker and Podman both read this template; their JSON
/// output differs.
pub fn list_command(engine: ContainerEngine) -> Vec<String> {
    vec![
        engine.name().into(),
        "ps".into(),
        "--no-trunc".into(),
        "--format".into(),
        "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}".into(),
    ]
}

/// The containers in the output of [`list_command`], in the order the
/// engine printed them.
///
/// Leaves out lines without four fields, ids or names `exec` cannot be given,
/// and repeated ids. Image and status are kept as display text without
/// control characters, at most 256 bytes each. At most [`MAX_LIST_BYTES`] of
/// output and [`MAX_CONTAINERS`] containers are read.
pub fn parse_containers(output: &[u8]) -> Vec<ListedContainer> {
    let output = &output[..output.len().min(MAX_LIST_BYTES)];
    let text = String::from_utf8_lossy(output);
    let mut containers: Vec<ListedContainer> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        let [id, names, image, status] = fields.as_slice() else {
            continue;
        };
        let id = id.trim();
        // Docker lists links as extra comma-separated names.
        let name = names.split(',').next().unwrap_or_default().trim();
        if !is_container_name(id)
            || !is_container_name(name)
            || containers.iter().any(|seen| seen.id == id)
        {
            continue;
        }
        containers.push(ListedContainer {
            id: id.into(),
            name: name.into(),
            image: display_text(image),
            status: display_text(status),
        });
        if containers.len() == MAX_CONTAINERS {
            break;
        }
    }
    containers
}

/// `text` trimmed, without control characters and cut to [`MAX_TEXT_BYTES`]
/// at a character boundary.
fn display_text(text: &str) -> String {
    let mut kept = String::new();
    for character in text
        .trim()
        .chars()
        .filter(|character| !character.is_control())
    {
        if kept.len() + character.len_utf8() > MAX_TEXT_BYTES {
            break;
        }
        kept.push(character);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(container: &str, user: Option<&str>) -> ContainerConnection {
        ContainerConnection {
            engine: ContainerEngine::Docker,
            container: container.into(),
            user: user.map(Into::into),
        }
    }

    #[test]
    fn connections_become_a_hop_after_trimming_and_refuse_malformed_fields() {
        let hop = connection(" api-1 ", Some(" 1000:1000 ")).hop().unwrap();
        assert_eq!(hop.container, "api-1");
        assert_eq!(hop.user.as_deref(), Some("1000:1000"));
        assert_eq!(hop.program, "docker");
        assert!(hop.working_directory.is_none() && hop.environment.is_empty());
        assert_eq!(connection("api", Some("  ")).hop().unwrap().user, None);
        let podman = ContainerConnection {
            engine: ContainerEngine::Podman,
            ..connection("api", None)
        };
        assert_eq!(podman.hop().unwrap().program, "podman");

        let long_container = "c".repeat(MAX_CONTAINER_NAME_BYTES + 1);
        let long_user = "u".repeat(MAX_CONTAINER_USER_BYTES + 1);
        for (refused, field, reason) in [
            (connection(" ", None), "container", Reason::Required),
            (
                connection(&long_container, None),
                "container",
                Reason::TooLong,
            ),
            (connection("-it", None), "container", Reason::Invalid),
            (connection("api web", None), "container", Reason::Invalid),
            (connection("api;id", None), "container", Reason::Invalid),
            (connection("api", Some(&long_user)), "user", Reason::TooLong),
            (connection("api", Some("-u")), "user", Reason::Invalid),
            (connection("api", Some("a b")), "user", Reason::Invalid),
        ] {
            assert_eq!(
                refused.hop(),
                Err(ConnectionError::at(field, reason)),
                "{refused:?}"
            );
        }
    }

    #[test]
    fn requests_name_only_the_engine_the_container_and_the_user() {
        let parsed: ContainerConnection =
            serde_json::from_value(serde_json::json!({"engine": "podman", "container": "api"}))
                .unwrap();
        assert_eq!(parsed.engine, ContainerEngine::Podman);
        assert_eq!(parsed.user, None);
        for refused in [
            serde_json::json!({"engine": "docker", "container": "api", "program": "/tmp/evil"}),
            serde_json::json!({"engine": "nerdctl", "container": "api"}),
            serde_json::json!({"container": "api"}),
        ] {
            assert!(
                serde_json::from_value::<ContainerConnection>(refused).is_err(),
                "a request with unknown or missing fields is refused"
            );
        }
    }

    #[test]
    fn the_listing_asks_the_engine_for_running_containers_with_full_ids() {
        assert_eq!(
            list_command(ContainerEngine::Podman),
            vec![
                "podman",
                "ps",
                "--no-trunc",
                "--format",
                "{{.ID}}\t{{.Names}}\t{{.Image}}\t{{.Status}}",
            ]
        );
        assert_eq!(list_command(ContainerEngine::Docker)[0], "docker");
    }

    #[test]
    fn listings_keep_valid_containers_in_order_and_once() {
        let id = "a".repeat(64);
        let other = "b".repeat(64);
        let listing = format!(
            "{id}\tapi,web/db\tghcr.io/acme/api:1\tUp 2 hours\r\n\
             malformed line\n\
             {other}\t-it\timage\tUp\n\
             {other}\tworker\tbusybox\tUp 3 seconds (healthy)\n\
             {id}\tagain\timage\tUp\n\
             -x\tname\timage\tUp\n"
        );
        assert_eq!(
            parse_containers(listing.as_bytes()),
            vec![
                ListedContainer {
                    id: id.clone(),
                    name: "api".into(),
                    image: "ghcr.io/acme/api:1".into(),
                    status: "Up 2 hours".into(),
                },
                ListedContainer {
                    id: other,
                    name: "worker".into(),
                    image: "busybox".into(),
                    status: "Up 3 seconds (healthy)".into(),
                },
            ]
        );
        assert!(parse_containers(b"").is_empty());
    }

    #[test]
    fn listings_bound_their_input_their_length_and_their_text() {
        let many: String = (0..MAX_CONTAINERS + 5)
            .map(|index| format!("id{index}\tname{index}\timage\tUp\n"))
            .collect();
        let listed = parse_containers(many.as_bytes());
        assert_eq!(listed.len(), MAX_CONTAINERS);
        assert_eq!(listed[0].name, "name0");

        let mut long = "a".repeat(MAX_LIST_BYTES).into_bytes();
        long.extend(b"\nid\tlate\timage\tUp\n");
        assert!(parse_containers(&long).is_empty());

        let image = format!("{}é", "i".repeat(MAX_TEXT_BYTES - 1));
        let listed = parse_containers(format!("id\tname\t{image}\tUp\u{1b}[31m\n").as_bytes());
        assert_eq!(listed[0].image, "i".repeat(MAX_TEXT_BYTES - 1));
        assert_eq!(listed[0].status, "Up[31m");
    }
}
