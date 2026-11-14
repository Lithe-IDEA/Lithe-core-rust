//! What a link or a resource of a rendered Markdown document points at.
//!
//! A preview follows the links a document holds and loads the images it shows. This module
//! decides what each reference means, so every frontend opens and loads the same things: a web
//! or mail address, an anchor of the document, or a file. A resource is confined to the folder
//! the document may reach and to image types, so a document cannot make its preview read other
//! files of the machine; the frontend only reads or downloads what this answers.

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// Image types a preview may load from a file, by lowercase extension. Each is a format the
/// system web engines decode inside an image element, which runs no script.
const IMAGE_TYPES: &[(&str, &str)] = &[
    ("apng", "image/apng"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("gif", "image/gif"),
    ("ico", "image/x-icon"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
];

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// What the frontend does with the reference.
pub enum MarkdownReferencePurpose {
    /// The user clicked a link: web and mail addresses, anchors and files anywhere are followed.
    Link,
    /// The page loads an image: web addresses and image files inside the document's scope.
    Resource,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// One reference of a Markdown document.
pub struct MarkdownReferenceRequest {
    /// Absolute path of the document that holds the reference.
    pub document_path: String,
    /// Absolute path of the open workspace's root. A document inside it reaches the whole
    /// workspace and resolves `/path` from its root; without one, or for a document outside it,
    /// the document's own folder takes both roles.
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// The link destination or `src` exactly as the rendered HTML holds it.
    pub reference: String,
    /// Whether the reference is followed as a link or loaded as a resource.
    pub purpose: MarkdownReferencePurpose,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
/// What a reference points at. Each kind answers only the purpose named in its variant.
pub enum MarkdownReference {
    /// Link: an http, https or mailto address for the system's browser or mail client.
    External {
        /// The address normalized as a browser reads it; a scheme-relative `//host` one gets
        /// `https:`.
        url: String,
    },
    /// Resource: an http or https address the frontend downloads through its proxy policy.
    Remote {
        /// The address normalized as a browser reads it; a scheme-relative `//host` one gets
        /// `https:`.
        url: String,
    },
    /// Link: an anchor of the document itself.
    Anchor {
        /// The fragment without its `#`, percent-decoded.
        anchor: String,
    },
    /// A file. For a resource the path is canonical, an existing regular file inside the
    /// document's scope with an image type, and `mime` names that type; the frontend reads it
    /// without following another link. For a link it is the lexical path, which need not exist,
    /// and `mime` is absent.
    File {
        /// Absolute path of the file.
        path: String,
        /// Content type of a resource file.
        #[serde(skip_serializing_if = "Option::is_none")]
        mime: Option<String>,
    },
    /// Nothing is followed or loaded: another scheme, a path out of the scope, a type that is not
    /// an image, or a file that does not exist.
    None,
}

/// Resolves one reference of a Markdown document for a link click or a resource load.
///
/// Only a resource touches the file system, to confirm its file and to resolve symbolic links
/// before the scope is checked; a link is resolved from the text alone.
pub fn resolve_reference(request: MarkdownReferenceRequest) -> MarkdownReference {
    let purpose = request.purpose;
    let reference = request.reference.trim();
    if reference.is_empty() || reference.contains('\0') {
        return MarkdownReference::None;
    }
    if let Some(anchor) = reference.strip_prefix('#') {
        return match (purpose, percent_decode(anchor)) {
            (MarkdownReferencePurpose::Link, Some(anchor)) if !anchor.is_empty() => {
                MarkdownReference::Anchor { anchor }
            }
            _ => MarkdownReference::None,
        };
    }
    if reference.starts_with("//") {
        // A scheme-relative address takes the scheme of a page served over https.
        return web_address(purpose, &format!("https:{reference}"));
    }
    if let Some(scheme) = scheme_of(reference) {
        // One letter is a Windows drive, which a document cannot name: neither a link nor a
        // resource of a shared file can point at an absolute path of one machine.
        if scheme.len() == 1 {
            return MarkdownReference::None;
        }
        return web_address(purpose, reference);
    }
    let written = reference.split(['?', '#']).next().unwrap_or_default();
    let Some(decoded) = percent_decode(written) else {
        return MarkdownReference::None;
    };
    if decoded.is_empty() || decoded.contains('\0') {
        return MarkdownReference::None;
    }
    let document = Path::new(&request.document_path);
    let Some(folder) = document.parent().filter(|_| document.is_absolute()) else {
        return MarkdownReference::None;
    };
    let scope = request
        .workspace_root
        .as_deref()
        .map(Path::new)
        .filter(|root| root.is_absolute() && within(document, root))
        .unwrap_or(folder);
    // `/path` is the workspace's, as on a repository host; any other path is the document's.
    let (base, relative) = match decoded.strip_prefix('/') {
        Some(rest) => (scope, rest),
        None => (folder, decoded.as_str()),
    };
    let lexical = join_lexically(base, relative);
    match purpose {
        MarkdownReferencePurpose::Link => MarkdownReference::File {
            path: lexical.to_string_lossy().into_owned(),
            mime: None,
        },
        MarkdownReferencePurpose::Resource => resource_file(&lexical, scope),
    }
}

/// The answer for an address with a scheme: links follow web and mail addresses, resources
/// download web addresses that name a host.
fn web_address(purpose: MarkdownReferencePurpose, address: &str) -> MarkdownReference {
    let Ok(parsed) = url::Url::parse(address) else {
        return MarkdownReference::None;
    };
    let web = matches!(parsed.scheme(), "http" | "https") && parsed.host().is_some();
    match purpose {
        MarkdownReferencePurpose::Link if web || parsed.scheme() == "mailto" => {
            MarkdownReference::External { url: parsed.into() }
        }
        MarkdownReferencePurpose::Resource if web => {
            MarkdownReference::Remote { url: parsed.into() }
        }
        _ => MarkdownReference::None,
    }
}

/// The resource file at a lexical path, when it stays in the scope once its links are resolved
/// and has an image type.
fn resource_file(lexical: &Path, scope: &Path) -> MarkdownReference {
    if !within(lexical, scope) {
        return MarkdownReference::None;
    }
    // Both sides are canonical, so a link inside the workspace that leads out of it is refused
    // and a workspace reached through a link still contains its own files.
    let (Ok(path), Ok(scope)) = (std::fs::canonicalize(lexical), std::fs::canonicalize(scope))
    else {
        return MarkdownReference::None;
    };
    if !path.starts_with(&scope) || !path.is_file() {
        return MarkdownReference::None;
    }
    let Some(mime) = image_type(&path) else {
        return MarkdownReference::None;
    };
    MarkdownReference::File {
        path: path.to_string_lossy().into_owned(),
        mime: Some(mime.to_string()),
    }
}

/// The content type of an image file by its extension.
fn image_type(path: &Path) -> Option<&'static str> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_TYPES
        .iter()
        .find(|(name, _)| *name == extension)
        .map(|(_, mime)| *mime)
}

/// The scheme of an absolute URI (RFC 3986: a letter, then letters, digits, `+`, `-` or `.`,
/// before the first `:`), when the reference has one before any `/`, `?` or `#`.
fn scheme_of(reference: &str) -> Option<&str> {
    let end = reference.find(':')?;
    let scheme = &reference[..end];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    let valid = first.is_ascii_alphabetic()
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then_some(scheme)
}

/// Decodes `%XX` escapes; a `%` that does not start one stays. Bytes that are not UTF-8 make no
/// path or anchor.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// Joins slash-separated segments to a base, resolving `.` and `..` without the file system. A
/// `..` at the root of the file system stays there.
fn join_lexically(base: &Path, relative: &str) -> PathBuf {
    let mut path = base.to_path_buf();
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            name => path.push(name),
        }
    }
    path
}

/// Whether a path lies inside a folder, comparing their normal components. Neither needs to
/// exist; `..` components do not count as leaving and are resolved by the callers first.
fn within(path: &Path, folder: &Path) -> bool {
    fn normal(path: &Path) -> Vec<Component<'_>> {
        path.components()
            .filter(|component| !matches!(component, Component::CurDir))
            .collect()
    }
    let (path, folder) = (normal(path), normal(folder));
    path.len() >= folder.len() && path[..folder.len()] == folder[..]
}

#[cfg(test)]
mod tests {
    use super::{
        resolve_reference, MarkdownReference, MarkdownReferencePurpose, MarkdownReferenceRequest,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    /// A temporary workspace with `docs/guide.md`, `docs/logo.png`, `assets/badge.svg`, a text
    /// file and, outside it, `secret.png`.
    struct Workspace {
        top: PathBuf,
        root: PathBuf,
    }

    impl Workspace {
        fn new(name: &str) -> Self {
            let top = crate::test_support::temporary_root(&format!("markdown-reference-{name}"));
            let root = top.join("workspace");
            fs::create_dir_all(root.join("docs")).expect("docs folder");
            fs::create_dir_all(root.join("assets")).expect("assets folder");
            for file in [
                "docs/guide.md",
                "docs/logo.png",
                "assets/badge.svg",
                "notes.txt",
            ] {
                fs::write(root.join(file), b"x").expect("workspace file");
            }
            fs::write(top.join("secret.png"), b"x").expect("outside file");
            Self { top, root }
        }

        fn resolve(&self, reference: &str, purpose: MarkdownReferencePurpose) -> MarkdownReference {
            resolve_reference(MarkdownReferenceRequest {
                document_path: self
                    .root
                    .join("docs/guide.md")
                    .to_string_lossy()
                    .into_owned(),
                workspace_root: Some(self.root.to_string_lossy().into_owned()),
                reference: reference.to_string(),
                purpose,
            })
        }

        fn canonical(&self, relative: &str) -> String {
            fs::canonicalize(self.root.join(relative))
                .expect("canonical workspace file")
                .to_string_lossy()
                .into_owned()
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.top);
        }
    }

    fn file(path: String, mime: Option<&str>) -> MarkdownReference {
        MarkdownReference::File {
            path,
            mime: mime.map(str::to_string),
        }
    }

    #[test]
    fn links_follow_addresses_anchors_and_files() {
        let workspace = Workspace::new("links");
        let link = MarkdownReferencePurpose::Link;
        let external = |url: &str| MarkdownReference::External {
            url: url.to_string(),
        };
        assert_eq!(
            workspace.resolve("https://example.com/a", link),
            external("https://example.com/a")
        );
        assert_eq!(
            workspace.resolve("mailto:me@example.com", link),
            external("mailto:me@example.com")
        );
        assert_eq!(
            workspace.resolve("//example.com/a", link),
            external("https://example.com/a")
        );
        assert_eq!(
            workspace.resolve("#install-%E4%B8%AD", link),
            MarkdownReference::Anchor {
                anchor: "install-中".to_string()
            }
        );
        let lexical = |relative: &str| {
            let path = relative
                .split('/')
                .fold(workspace.root.clone(), |path, name| path.join(name));
            path.to_string_lossy().into_owned()
        };
        // A file link need not exist and may leave the workspace; the query and fragment go.
        assert_eq!(
            workspace.resolve("../README%20old.md?plain=1#usage", link),
            file(lexical("README old.md"), None)
        );
        assert_eq!(
            workspace.resolve("/assets/badge.svg", link),
            file(lexical("assets/badge.svg"), None)
        );
        for refused in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "C:/Windows",
            "#",
            "",
            "  ",
        ] {
            assert_eq!(
                workspace.resolve(refused, link),
                MarkdownReference::None,
                "{refused}"
            );
        }
    }

    #[test]
    fn resources_are_images_inside_the_workspace() {
        let workspace = Workspace::new("resources");
        let resource = MarkdownReferencePurpose::Resource;
        assert_eq!(
            workspace.resolve("logo.png", resource),
            file(workspace.canonical("docs/logo.png"), Some("image/png"))
        );
        assert_eq!(
            workspace.resolve("../assets/badge.svg?v=2", resource),
            file(
                workspace.canonical("assets/badge.svg"),
                Some("image/svg+xml")
            )
        );
        assert_eq!(
            workspace.resolve("/assets/badge.svg", resource),
            file(
                workspace.canonical("assets/badge.svg"),
                Some("image/svg+xml")
            )
        );
        assert_eq!(
            workspace.resolve("https://example.com/a.png", resource),
            MarkdownReference::Remote {
                url: "https://example.com/a.png".to_string()
            }
        );
        // Out of the workspace, not an image, missing, an anchor or another scheme: nothing loads.
        for refused in [
            "../../secret.png",
            "/../secret.png",
            "../notes.txt",
            "missing.png",
            "#logo",
            "data:image/png;base64,AA==",
            "file:///etc/passwd",
            "mailto:me@example.com",
            "https://",
        ] {
            assert_eq!(
                workspace.resolve(refused, resource),
                MarkdownReference::None,
                "{refused}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn resources_do_not_leave_the_workspace_through_a_link() {
        let workspace = Workspace::new("symlink");
        std::os::unix::fs::symlink(
            workspace.top.join("secret.png"),
            workspace.root.join("docs/inside.png"),
        )
        .expect("link to the outside file");
        assert_eq!(
            workspace.resolve("inside.png", MarkdownReferencePurpose::Resource),
            MarkdownReference::None
        );
    }

    #[test]
    fn a_document_outside_the_workspace_reaches_only_its_folder() {
        let workspace = Workspace::new("outside");
        let request = |reference: &str| MarkdownReferenceRequest {
            document_path: workspace
                .top
                .join("notes.md")
                .to_string_lossy()
                .into_owned(),
            workspace_root: Some(workspace.root.to_string_lossy().into_owned()),
            reference: reference.to_string(),
            purpose: MarkdownReferencePurpose::Resource,
        };
        let secret = fs::canonicalize(workspace.top.join("secret.png"))
            .expect("canonical outside file")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            resolve_reference(request("/secret.png")),
            file(secret, Some("image/png"))
        );
        assert_eq!(
            resolve_reference(request("workspace/docs/logo.png")),
            file(workspace.canonical("docs/logo.png"), Some("image/png"))
        );
        assert_eq!(
            resolve_reference(MarkdownReferenceRequest {
                document_path: "relative/notes.md".to_string(),
                ..request("logo.png")
            }),
            MarkdownReference::None
        );
        assert!(Path::new(&workspace.top).exists());
    }
}
