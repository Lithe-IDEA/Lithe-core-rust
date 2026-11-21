//! What an OpenSSH question asks for, so the connection window can show a
//! fitting form: a host key to confirm, a password, a key passphrase or a
//! one-time code.
//!
//! OpenSSH hands its questions to AskPass only as text. The categories match
//! its stable English wording; anything else stays `other`, and the window
//! then shows the question as asked.

use serde::Serialize;

/// The kind of answer a question expects.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptKind {
    /// An unknown host key to accept (`yes`) or decline (`no`).
    HostKey,
    /// The account's password.
    Password,
    /// The passphrase of a private key.
    Passphrase,
    /// A one-time or verification code.
    Code,
    /// Any other question.
    Other,
}

/// A classified question.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Prompt {
    /// What the answer is.
    pub kind: PromptKind,
    /// The host key's algorithm as OpenSSH names it, such as `ED25519`.
    /// Only set for [`PromptKind::HostKey`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_type: Option<String>,
    /// The host key's fingerprint, such as `SHA256:…`. Only set for
    /// [`PromptKind::HostKey`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Longest algorithm and fingerprint kept; OpenSSH's are far shorter.
const MAX_KEY_TEXT: usize = 128;

/// Classifies one question as OpenSSH asked it.
pub fn classify(prompt: &str) -> Prompt {
    let lower = prompt.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));
    let kind = if has(&[
        "authenticity of host",
        "continue connecting",
        "key fingerprint is",
    ]) {
        PromptKind::HostKey
    } else if has(&["passphrase"]) {
        PromptKind::Passphrase
    } else if has(&["password"]) && !has(&["one-time", "one time"]) {
        PromptKind::Password
    } else if has(&[
        "verification code",
        "one-time",
        "one time",
        "otp",
        "token",
        "authenticator",
        "two-factor",
        "passcode",
    ]) {
        PromptKind::Code
    } else {
        PromptKind::Other
    };
    let (key_type, fingerprint) = if kind == PromptKind::HostKey {
        host_key(prompt)
    } else {
        (None, None)
    };
    Prompt {
        kind,
        key_type,
        fingerprint,
    }
}

/// Reads `ED25519 key fingerprint is SHA256:….` from a host key question.
fn host_key(prompt: &str) -> (Option<String>, Option<String>) {
    const MARKER: &str = " key fingerprint is ";
    for line in prompt.lines() {
        let Some(at) = line.find(MARKER) else {
            continue;
        };
        let key_type = line[..at].split_whitespace().last().map(str::to_owned);
        let fingerprint = line[at + MARKER.len()..]
            .split_whitespace()
            .next()
            .map(|word| word.trim_end_matches('.').to_owned());
        let bounded = |text: Option<String>| {
            text.filter(|text| {
                !text.is_empty()
                    && text.len() <= MAX_KEY_TEXT
                    && text.chars().all(|c| c.is_ascii_graphic())
            })
        };
        return (bounded(key_type), bounded(fingerprint));
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_host_keys_carry_the_algorithm_and_fingerprint() {
        let prompt = classify(
            "The authenticity of host 'build-box (192.0.2.10)' can't be established.\n\
             ED25519 key fingerprint is SHA256:Q2xhdWRlRXhhbXBsZUZpbmdlcnByaW50.\n\
             This key is not known by any other names.\n\
             Are you sure you want to continue connecting (yes/no/[fingerprint])? ",
        );
        assert_eq!(prompt.kind, PromptKind::HostKey);
        assert_eq!(prompt.key_type.as_deref(), Some("ED25519"));
        assert_eq!(
            prompt.fingerprint.as_deref(),
            Some("SHA256:Q2xhdWRlRXhhbXBsZUZpbmdlcnByaW50")
        );
    }

    #[test]
    fn a_host_key_question_without_a_fingerprint_line_has_none() {
        let prompt = classify("Are you sure you want to continue connecting (yes/no)? ");
        assert_eq!(prompt.kind, PromptKind::HostKey);
        assert_eq!(prompt.key_type, None);
        assert_eq!(prompt.fingerprint, None);
    }

    #[test]
    fn secrets_are_told_apart_by_their_wording() {
        let cases = [
            ("dev@build-box's password: ", PromptKind::Password),
            ("(dev@build-box) Password: ", PromptKind::Password),
            (
                "Enter passphrase for key '/home/dev/.ssh/id_ed25519': ",
                PromptKind::Passphrase,
            ),
            (
                "Enter passphrase for /home/dev/.ssh/id_ed25519: ",
                PromptKind::Passphrase,
            ),
            ("Verification code: ", PromptKind::Code),
            ("One-time password (OATH) for `dev': ", PromptKind::Code),
            ("Enter PIN for ED25519-SK key: ", PromptKind::Other),
        ];
        for (text, kind) in cases {
            let prompt = classify(text);
            assert_eq!(prompt.kind, kind, "{text}");
            assert_eq!(prompt.fingerprint, None, "{text}");
        }
    }
}
