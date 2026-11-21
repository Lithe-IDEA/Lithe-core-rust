//! Transport chains.
//!
//! A chain is an ordered list of hops from the local machine to the place
//! where the workspace lives: `[]` is the local machine, `[ssh devbox]` a
//! remote host, `[ssh devbox, docker api]` a container on that host. The only
//! thing a chain does is build the local command line that runs a program at
//! its end with standard streams connected end to end.
//!
//! An SSH hop passes its remote command to the remote user's login shell as
//! one string. That shell may be sh, bash, zsh, fish or csh, which disagree on
//! escapes, so arguments that cross an SSH hop are single-quoted and the
//! characters any of those shells treat specially inside single quotes are
//! rejected instead of being escaped. The one exception is the command line
//! an inner SSH hop generates: its own single quotes are escaped as `'"'"'`,
//! which every supported shell reads back unchanged, so SSH hops can nest.
//!
//! A hop only sets SSH options from a fixed allowlist. Options that run local
//! commands, load libraries, relax host-key checking or forward local
//! resources may only come from the user's own SSH configuration, and every
//! hop turns agent, X11 and port forwarding off before its own options.

use std::collections::BTreeMap;
use std::fmt;

/// Seconds between SSH keep-alive probes. Together with
/// [`SSH_KEEPALIVE_MISSES`] it bounds how long a dead network path stays
/// undetected.
const SSH_KEEPALIVE_SECONDS: u32 = 15;
const SSH_KEEPALIVE_MISSES: u32 = 3;

/// Options every SSH hop passes before the hop's own. ssh keeps the first
/// value it sees, and command-line options win over the configuration files,
/// so neither the hop nor `~/.ssh/config` can turn these back on. Tunnels
/// travel inside the remote session, so ssh never needs to forward ports, and
/// the remote host never gets the local agent or display.
const FORCED_SSH_OPTIONS: [&str; 3] = [
    "ForwardAgent=no",
    "ForwardX11=no",
    "ClearAllForwardings=yes",
];

/// How the value of an allowed SSH option is checked.
#[derive(Clone, Copy, Debug)]
enum OptionSyntax {
    /// `yes` or `no`.
    YesNo,
    /// Decimal count of seconds or attempts.
    Count,
    /// TCP port from 1 to 65535.
    Port,
    /// Login name, as checked by [`is_ssh_user`].
    User,
    /// Local file path, already quoted by the caller where ssh_config needs it.
    Path,
    /// Comma-separated names from [`AUTHENTICATION_METHODS`].
    Methods,
    /// `none`, or comma-separated `[user@]host[:port]` jump hosts.
    Jumps,
}

/// SSH options a hop may set, in their canonical spelling. ssh matches option
/// names case-insensitively, and so does the check. Everything else is
/// rejected: `ProxyCommand`, `LocalCommand`, `KnownHostsCommand`,
/// `PKCS11Provider` and similar options run local commands or load libraries,
/// and `StrictHostKeyChecking` or `UserKnownHostsFile` would weaken the host
/// key check that OpenSSH alone performs.
const ALLOWED_SSH_OPTIONS: [(&str, OptionSyntax); 11] = [
    ("Port", OptionSyntax::Port),
    ("User", OptionSyntax::User),
    ("IdentityFile", OptionSyntax::Path),
    ("IdentitiesOnly", OptionSyntax::YesNo),
    ("PreferredAuthentications", OptionSyntax::Methods),
    ("PubkeyAuthentication", OptionSyntax::YesNo),
    ("BatchMode", OptionSyntax::YesNo),
    ("ConnectTimeout", OptionSyntax::Count),
    ("ServerAliveInterval", OptionSyntax::Count),
    ("ServerAliveCountMax", OptionSyntax::Count),
    ("ProxyJump", OptionSyntax::Jumps),
];

/// Authentication method names `PreferredAuthentications` accepts.
const AUTHENTICATION_METHODS: [&str; 5] = [
    "gssapi-with-mic",
    "hostbased",
    "publickey",
    "keyboard-interactive",
    "password",
];

/// Ordered hops from the local machine to the workspace host.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TransportChain {
    /// Hops in order, outermost first; empty for the local machine.
    pub hops: Vec<Hop>,
}

/// One hop of a transport chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Hop {
    /// An SSH connection to a host.
    Ssh(SshHop),
    /// An `exec` into a running container.
    Container(ContainerHop),
}

/// Connection through the system OpenSSH client. Host aliases, jump hosts,
/// keys and agents come from the user's own SSH configuration, so the hop
/// only carries what the user typed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SshHop {
    /// SSH executable, resolved by the platform adapter. Defaults to `ssh`.
    pub program: String,
    /// `host`, `user@host` or a `Host` alias from the SSH configuration.
    pub destination: String,
    /// Port passed as `-p`; the SSH configuration decides when absent.
    pub port: Option<u16>,
    /// Extra `-o key=value` options, in order. Only allowlisted options with
    /// well-formed values are accepted; [`TransportChain::command`] fails
    /// with [`TransportError::DisallowedOption`] or
    /// [`TransportError::InvalidHop`] otherwise.
    pub options: Vec<(String, String)>,
}

impl SshHop {
    /// A hop to `destination` with the default `ssh` executable and no options.
    pub fn new(destination: impl Into<String>) -> Self {
        Self {
            program: "ssh".into(),
            destination: destination.into(),
            port: None,
            options: Vec::new(),
        }
    }
}

/// Container engine whose CLI runs the `exec`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContainerEngine {
    /// Docker, or a Docker-compatible CLI.
    Docker,
    /// Podman.
    Podman,
}

impl ContainerEngine {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

/// `exec` into a running container. Docker-compatible CLIs such as OrbStack
/// and Colima use the Docker engine with their own `program`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContainerHop {
    /// Engine whose command-line syntax the hop uses.
    pub engine: ContainerEngine,
    /// Engine CLI executable. Defaults to the engine name.
    pub program: String,
    /// Container name or id.
    pub container: String,
    /// `user`, `uid` or `user:group`; the image default when absent.
    pub user: Option<String>,
    /// Working directory passed as `-w`; the image default when absent.
    pub working_directory: Option<String>,
    /// Variables passed as `-e`; names must be valid environment variable names.
    pub environment: BTreeMap<String, String>,
}

impl ContainerHop {
    /// A hop into `container` with the engine's default executable and the image
    /// defaults for user, directory and environment.
    pub fn new(engine: ContainerEngine, container: impl Into<String>) -> Self {
        Self {
            engine,
            program: engine.name().into(),
            container: container.into(),
            user: None,
            working_directory: None,
            environment: BTreeMap::new(),
        }
    }
}

/// Why a transport chain could not build a command line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransportError {
    /// An argument that must cross an SSH hop contains a character that
    /// cannot be passed safely to every supported login shell.
    UnsafeArgument {
        /// The rejected argument.
        argument: String,
    },
    /// A hop field is malformed, for example a destination starting with `-`
    /// that SSH would parse as an option, or an allowed SSH option with a
    /// malformed value.
    InvalidHop {
        /// What is wrong, for people.
        reason: String,
    },
    /// An SSH hop sets an option outside the allowlist. Such options may only
    /// come from the user's own SSH configuration.
    DisallowedOption {
        /// The option name as the hop gave it.
        name: String,
    },
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsafeArgument { argument } => write!(
                formatter,
                "argument {argument:?} contains a character that cannot cross an SSH hop"
            ),
            Self::InvalidHop { reason } => formatter.write_str(reason),
            Self::DisallowedOption { name } => write!(
                formatter,
                "SSH option {name:?} cannot be set by Lithe; set it in your SSH configuration"
            ),
        }
    }
}

impl std::error::Error for TransportError {}

impl TransportChain {
    /// The chain without hops, which runs programs on the local machine.
    pub fn local() -> Self {
        Self::default()
    }

    /// A chain through `hops`, outermost first.
    pub fn new(hops: Vec<Hop>) -> Self {
        Self { hops }
    }

    /// Local command line that runs `argv` at the end of the chain.
    pub fn command(&self, argv: &[String]) -> Result<Vec<String>, TransportError> {
        let mut current: Vec<Word> = argv.iter().cloned().map(Word::Raw).collect();
        for hop in self.hops.iter().rev() {
            current = match hop {
                Hop::Ssh(hop) => ssh_command(hop, &current)?,
                Hop::Container(hop) => container_command(hop, current)?,
            };
        }
        Ok(current.into_iter().map(Word::into_string).collect())
    }

    /// Stable identity of the chain, for example `ssh:devbox/docker:api`.
    /// Recent-project lists and per-host storage key on it, so it ignores
    /// settings such as keep-alive options that do not change the target.
    pub fn identity(&self) -> String {
        if self.hops.is_empty() {
            return "local".into();
        }
        self.hops
            .iter()
            .map(|hop| match hop {
                Hop::Ssh(hop) => match hop.port {
                    Some(port) => format!("ssh:{}:{port}", hop.destination),
                    None => format!("ssh:{}", hop.destination),
                },
                Hop::Container(hop) => format!("{}:{}", hop.engine.name(), hop.container),
            })
            .collect::<Vec<_>>()
            .join("/")
    }
}

/// One argument of a command line under construction.
#[derive(Clone, Debug)]
enum Word {
    /// Caller or hop input, validated before it crosses an SSH hop.
    Raw(String),
    /// Remote command string built by [`shell_join`] for an inner SSH hop.
    Generated(String),
}

impl Word {
    fn into_string(self) -> String {
        match self {
            Self::Raw(text) | Self::Generated(text) => text,
        }
    }
}

fn ssh_command(hop: &SshHop, remote: &[Word]) -> Result<Vec<Word>, TransportError> {
    validate_ssh_destination(&hop.destination)?;
    let mut command: Vec<String> = vec![
        hop.program.clone(),
        // No pseudo-terminal: the protocol stream must stay byte-exact.
        "-T".into(),
        // Never interpret `~.` style escapes inside the binary stream.
        "-e".into(),
        "none".into(),
    ];
    if let Some(port) = hop.port {
        command.push("-p".into());
        command.push(port.to_string());
    }
    // ssh keeps the first value it sees for each option: the forced options
    // come first so nothing can override them, and the hop's own options
    // precede the keep-alive defaults so they can override those.
    for option in FORCED_SSH_OPTIONS {
        command.push("-o".into());
        command.push(option.into());
    }
    for (key, value) in &hop.options {
        let name = allowed_option(key, value)?;
        command.push("-o".into());
        command.push(format!("{name}={value}"));
    }
    command.extend([
        "-o".into(),
        format!("ServerAliveInterval={SSH_KEEPALIVE_SECONDS}"),
        "-o".into(),
        format!("ServerAliveCountMax={SSH_KEEPALIVE_MISSES}"),
    ]);
    command.push("--".into());
    command.push(hop.destination.clone());
    let remote_command = shell_join(remote)?;
    let mut words: Vec<Word> = command.into_iter().map(Word::Raw).collect();
    words.push(Word::Generated(remote_command));
    Ok(words)
}

fn container_command(hop: &ContainerHop, inner: Vec<Word>) -> Result<Vec<Word>, TransportError> {
    if !is_plain_name(&hop.container) {
        return Err(TransportError::InvalidHop {
            reason: format!("invalid container name {:?}", hop.container),
        });
    }
    let mut command = vec![hop.program.clone(), "exec".into(), "-i".into()];
    if let Some(user) = &hop.user {
        if user.is_empty()
            || user.starts_with('-')
            || !user
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_.:-".contains(character))
        {
            return Err(TransportError::InvalidHop {
                reason: format!("invalid container user {user:?}"),
            });
        }
        command.push("-u".into());
        command.push(user.clone());
    }
    if let Some(directory) = &hop.working_directory {
        command.push("-w".into());
        command.push(directory.clone());
    }
    for (key, value) in &hop.environment {
        if !is_environment_name(key) {
            return Err(TransportError::InvalidHop {
                reason: format!("invalid environment variable name {key:?}"),
            });
        }
        command.push("-e".into());
        command.push(format!("{key}={value}"));
    }
    // The engine stops parsing its own options at the container name, so the
    // inner command needs no separator.
    command.push(hop.container.clone());
    let mut words: Vec<Word> = command.into_iter().map(Word::Raw).collect();
    words.extend(inner);
    Ok(words)
}

fn validate_ssh_destination(destination: &str) -> Result<(), TransportError> {
    if destination.is_empty()
        || destination.starts_with('-')
        || destination
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(TransportError::InvalidHop {
            reason: format!("invalid SSH destination {destination:?}"),
        });
    }
    Ok(())
}

/// Canonical name of the allowed SSH option `key`, after checking `value`
/// against the option's syntax.
fn allowed_option(key: &str, value: &str) -> Result<&'static str, TransportError> {
    let Some((name, syntax)) = ALLOWED_SSH_OPTIONS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
    else {
        return Err(TransportError::DisallowedOption { name: key.into() });
    };
    let well_formed = !value.chars().any(char::is_control)
        && match syntax {
            OptionSyntax::YesNo => matches!(value, "yes" | "no"),
            OptionSyntax::Count => is_decimal(value),
            OptionSyntax::Port => is_port(value),
            OptionSyntax::User => is_ssh_user(value),
            OptionSyntax::Path => !value.trim().is_empty(),
            OptionSyntax::Methods => value
                .split(',')
                .all(|method| AUTHENTICATION_METHODS.contains(&method)),
            OptionSyntax::Jumps => is_jumps(value),
        };
    if !well_formed {
        return Err(TransportError::InvalidHop {
            reason: format!("invalid value {value:?} for SSH option {name}"),
        });
    }
    Ok(name)
}

/// Up to nine decimal digits, so the value fits every counter ssh parses.
fn is_decimal(value: &str) -> bool {
    (1..=9).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_port(value: &str) -> bool {
    is_decimal(value) && value.parse::<u16>().is_ok_and(|port| port != 0)
}

/// A login name that ssh cannot read as an option or a host separator.
pub(crate) fn is_ssh_user(user: &str) -> bool {
    let mut characters = user.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphanumeric() || first == '_')
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
}

/// A `ProxyJump` value: `none`, or comma-separated jump hosts.
pub(crate) fn is_jumps(value: &str) -> bool {
    value == "none" || value.split(',').all(is_jump_host)
}

/// `[user@]host[:port]`, where `host` is a host name, an SSH alias or a
/// bracketed IPv6 address.
fn is_jump_host(jump: &str) -> bool {
    let (user, address) = match jump.rsplit_once('@') {
        Some((user, address)) => (Some(user), address),
        None => (None, jump),
    };
    let (host, port) = match address.strip_prefix('[') {
        Some(bracketed) => match bracketed.split_once(']') {
            Some((host, "")) => (host, None),
            Some((host, rest)) => match rest.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None => return false,
            },
            None => return false,
        },
        None => match address.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (address, None),
        },
    };
    let host_valid = if address.starts_with('[') {
        !host.is_empty()
            && host
                .chars()
                .all(|character| character.is_ascii_hexdigit() || ".:".contains(character))
    } else {
        is_plain_name(host)
    };
    user.is_none_or(is_ssh_user) && host_valid && port.is_none_or(is_port)
}

/// Letters, digits, `.`, `_` and `-`, starting with a letter or digit. Host
/// names, SSH `Host` aliases and container names or ids all fit, and none can
/// be read as an option.
fn is_plain_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphanumeric())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || "_.-".contains(character))
}

fn is_environment_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

/// Joins arguments into one string that every supported login shell splits
/// back into the same arguments.
fn shell_join(argv: &[Word]) -> Result<String, TransportError> {
    argv.iter()
        .map(|word| match word {
            Word::Raw(argument) => shell_quote(argument),
            Word::Generated(command) => Ok(quote_generated(command)),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|quoted| quoted.join(" "))
}

/// Quotes a command line produced by [`shell_join`]. It is built only from
/// validated arguments, so it holds no `\`, `!` or control characters, and
/// its own single quotes leave the quote, appear inside double quotes, and
/// reopen it.
fn quote_generated(command: &str) -> String {
    format!("'{}'", command.replace('\'', "'\"'\"'"))
}

fn shell_quote(argument: &str) -> Result<String, TransportError> {
    // `'` ends the quote everywhere, `\` escapes inside quotes in fish, `!`
    // triggers history expansion in csh, and line breaks end commands.
    if argument
        .chars()
        .any(|character| matches!(character, '\'' | '\\' | '!') || character.is_control())
    {
        return Err(TransportError::UnsafeArgument {
            argument: argument.into(),
        });
    }
    let plain = !argument.is_empty()
        && argument
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "_./:=@%+,-".contains(character));
    Ok(if plain {
        argument.into()
    } else {
        format!("'{argument}'")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn local_chain_runs_the_command_unchanged() {
        let argv = strings(&["lithe-remote-server", "--stdio"]);
        assert_eq!(TransportChain::local().command(&argv).unwrap(), argv);
        assert_eq!(TransportChain::local().identity(), "local");
    }

    #[test]
    fn ssh_hop_quotes_the_remote_command_and_guards_the_destination() {
        let mut hop = SshHop::new("dev@box");
        hop.port = Some(2222);
        hop.options.push(("connecttimeout".into(), "10".into()));
        let chain = TransportChain::new(vec![Hop::Ssh(hop)]);
        let command = chain
            .command(&strings(&["sh", "-c", "echo \"$HOME\" && uname -s"]))
            .unwrap();
        assert_eq!(
            command,
            strings(&[
                "ssh",
                "-T",
                "-e",
                "none",
                "-p",
                "2222",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "--",
                "dev@box",
                "sh -c 'echo \"$HOME\" && uname -s'",
            ])
        );
        assert_eq!(chain.identity(), "ssh:dev@box:2222");
    }

    // A destination such as `-oProxyCommand=...` would otherwise be parsed by
    // ssh as an option and run an arbitrary local command.
    #[test]
    fn option_like_destinations_are_rejected() {
        let chain = TransportChain::new(vec![Hop::Ssh(SshHop::new("-oProxyCommand=touch x"))]);
        assert!(matches!(
            chain.command(&strings(&["true"])),
            Err(TransportError::InvalidHop { .. })
        ));
    }

    fn ssh_with_option(key: &str, value: &str) -> Result<Vec<String>, TransportError> {
        let mut hop = SshHop::new("box");
        hop.options.push((key.into(), value.into()));
        TransportChain::new(vec![Hop::Ssh(hop)]).command(&strings(&["true"]))
    }

    // Options that run local commands, load libraries, weaken the host key
    // check or forward local resources may only come from ~/.ssh/config.
    #[test]
    fn options_outside_the_allowlist_are_rejected() {
        for name in [
            "ProxyCommand",
            "proxycommand",
            "LocalCommand",
            "PermitLocalCommand",
            "KnownHostsCommand",
            "PKCS11Provider",
            "SecurityKeyProvider",
            "StrictHostKeyChecking",
            "UserKnownHostsFile",
            "ForwardAgent",
            "LocalForward",
            "Include",
            "",
        ] {
            assert_eq!(
                ssh_with_option(name, "yes"),
                Err(TransportError::DisallowedOption { name: name.into() }),
                "{name}"
            );
        }
    }

    #[test]
    fn allowed_options_check_their_values() {
        for (key, value) in [
            ("Port", "22"),
            ("User", "dev_1"),
            ("IdentityFile", "\"~/My Keys/id_ed25519\""),
            ("IdentitiesOnly", "yes"),
            ("PreferredAuthentications", "keyboard-interactive,password"),
            ("PubkeyAuthentication", "no"),
            ("BatchMode", "no"),
            ("ServerAliveCountMax", "5"),
            ("ProxyJump", "none"),
            ("ProxyJump", "ops@bastion.example.com:2200,jump2"),
            ("ProxyJump", "[2001:db8::1]:22"),
        ] {
            let command = ssh_with_option(key, value).unwrap();
            assert!(command.contains(&format!("{key}={value}")), "{key}={value}");
        }
        for (key, value) in [
            ("Port", "0"),
            ("Port", "+22"),
            ("Port", "65536"),
            ("ConnectTimeout", ""),
            ("ConnectTimeout", "10 ProxyCommand=x"),
            ("IdentitiesOnly", "true"),
            ("User", "-oProxyCommand=x"),
            ("IdentityFile", " "),
            ("IdentityFile", "key\nProxyCommand x"),
            ("PreferredAuthentications", "publickey,none"),
            ("ProxyJump", "-oProxyCommand=x"),
            ("ProxyJump", "host:22:33"),
            ("ProxyJump", "ssh://host"),
            ("ProxyJump", "bastion,"),
            ("ProxyJump", "[::1"),
        ] {
            assert!(
                matches!(
                    ssh_with_option(key, value),
                    Err(TransportError::InvalidHop { .. })
                ),
                "{key}={value}"
            );
        }
    }

    #[test]
    fn arguments_that_shells_disagree_on_are_rejected() {
        let chain = TransportChain::new(vec![Hop::Ssh(SshHop::new("box"))]);
        for argument in ["it's", "a\\b", "hi!", "two\nlines"] {
            assert_eq!(
                chain.command(&strings(&["echo", argument])),
                Err(TransportError::UnsafeArgument {
                    argument: argument.into()
                })
            );
        }
        assert_eq!(
            chain
                .command(&strings(&["printf", ""]))
                .unwrap()
                .last()
                .unwrap(),
            "printf ''"
        );
    }

    // Each SSH hop hands its remote command to a login shell, so the command
    // an inner hop generates must survive one more round of parsing. A fake
    // `ssh` runs that string with a real shell, for every supported login
    // shell installed here.
    #[cfg(unix)]
    #[test]
    fn nested_ssh_hops_deliver_arguments_unchanged() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "lithe-remote-nested-ssh-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let _cleanup = RemoveOnDrop(directory.clone());
        let argv = strings(&[
            "sh",
            "-c",
            "printf %s/%s \"$0\" \"$1\"",
            "two words",
            "$HOME \"quoted\" * ; |",
        ]);
        let mut tested = 0;
        for shell in [
            "/bin/sh",
            "/bin/bash",
            "/bin/zsh",
            "/bin/csh",
            "/usr/bin/fish",
        ] {
            if !std::path::Path::new(shell).exists() {
                continue;
            }
            let fake_ssh = directory.join(format!("ssh-{}", shell.rsplit('/').next().unwrap()));
            std::fs::write(
                &fake_ssh,
                format!("#!/bin/sh\nfor last; do :; done\nexec {shell} -c \"$last\"\n"),
            )
            .unwrap();
            std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
            let hop = || {
                let mut hop = SshHop::new("box");
                hop.program = fake_ssh.to_str().unwrap().into();
                Hop::Ssh(hop)
            };
            let chain = TransportChain::new(vec![hop(), hop(), hop()]);
            let output = crate::client::command::run(
                &chain.command(&argv).unwrap(),
                std::time::Duration::from_secs(10),
            )
            .unwrap();
            assert_eq!(
                (output.status, String::from_utf8_lossy(&output.stdout)),
                (Some(0), "two words/$HOME \"quoted\" * ; |".into()),
                "through {shell}: {}",
                output.stderr_text()
            );
            tested += 1;
        }
        assert!(tested > 0, "no supported shell found");
    }

    #[cfg(unix)]
    struct RemoveOnDrop(std::path::PathBuf);

    #[cfg(unix)]
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn local_container_hop_passes_arguments_without_a_shell() {
        let mut hop = ContainerHop::new(ContainerEngine::Podman, "api-1");
        hop.user = Some("1000:1000".into());
        hop.working_directory = Some("/workspace".into());
        hop.environment.insert("LANG".into(), "C.UTF-8".into());
        let chain = TransportChain::new(vec![Hop::Container(hop)]);
        // No SSH hop, so characters that are unsafe for shells are allowed.
        assert_eq!(
            chain.command(&strings(&["echo", "it's"])).unwrap(),
            strings(&[
                "podman",
                "exec",
                "-i",
                "-u",
                "1000:1000",
                "-w",
                "/workspace",
                "-e",
                "LANG=C.UTF-8",
                "api-1",
                "echo",
                "it's",
            ])
        );
        assert_eq!(chain.identity(), "podman:api-1");
    }

    #[test]
    fn container_behind_ssh_nests_the_exec_inside_the_remote_command() {
        let chain = TransportChain::new(vec![
            Hop::Ssh(SshHop::new("devbox")),
            Hop::Container(ContainerHop::new(ContainerEngine::Docker, "api")),
        ]);
        let command = chain
            .command(&strings(&[
                "/home/dev/.lithe-server/bin/1/lithe-remote-server",
                "--stdio",
            ]))
            .unwrap();
        assert_eq!(
            command.last().unwrap(),
            "docker exec -i api /home/dev/.lithe-server/bin/1/lithe-remote-server --stdio"
        );
        assert_eq!(&command[command.len() - 2], "devbox");
        assert_eq!(chain.identity(), "ssh:devbox/docker:api");
    }

    #[test]
    fn malformed_container_fields_are_rejected() {
        for hop in [
            ContainerHop::new(ContainerEngine::Docker, "-it"),
            ContainerHop {
                user: Some("--privileged".into()),
                ..ContainerHop::new(ContainerEngine::Docker, "api")
            },
        ] {
            assert!(matches!(
                TransportChain::new(vec![Hop::Container(hop)]).command(&strings(&["true"])),
                Err(TransportError::InvalidHop { .. })
            ));
        }
    }
}
