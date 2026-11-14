//! The static site an HTML file is previewed as.
//!
//! A preview loads an HTML document with everything it references, the way a static web server
//! would: stylesheets, scripts, images, fonts, media and the pages beside it. This module decides
//! what the site is and what each request of it serves, so every frontend serves the same files.
//! The site's root is the folder the document may reach, as for a Markdown document; the document
//! itself is the editor's text, so unsaved changes show; any other file must be an existing
//! regular file inside the root that is neither hidden nor a key or certificate store. The page
//! runs the document's own scripts, so nothing it requests is trusted: the frontend reads only
//! what this answers.

use crate::markdown_reference::{document_scope, join_lexically, within};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// The file a folder of the site serves.
const INDEX: &str = "index.html";

/// Lowercase extensions of files that hold keys or certificates, refused even inside the root.
const SECRET_EXTENSIONS: &[&str] = &["jks", "key", "keystore", "p12", "pem", "pfx"];

/// Content types by lowercase extension. Engines refuse a module script or a stylesheet of the
/// wrong type, so the types a page depends on are named here rather than sniffed. Text types are
/// UTF-8, the encoding of the editor's documents and of web files in practice; a file in another
/// encoding is read right only with a byte order mark, which outranks the header.
const CONTENT_TYPES: &[(&str, &str)] = &[
    ("apng", "image/apng"),
    ("avif", "image/avif"),
    ("bmp", "image/bmp"),
    ("cjs", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("csv", "text/csv; charset=utf-8"),
    ("flac", "audio/flac"),
    ("gif", "image/gif"),
    ("htm", "text/html; charset=utf-8"),
    ("html", "text/html; charset=utf-8"),
    ("ico", "image/x-icon"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("js", "text/javascript; charset=utf-8"),
    ("json", "application/json"),
    ("m4a", "audio/mp4"),
    ("map", "application/json"),
    ("md", "text/markdown; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("mp3", "audio/mpeg"),
    ("mp4", "video/mp4"),
    ("oga", "audio/ogg"),
    ("ogg", "audio/ogg"),
    ("ogv", "video/ogg"),
    ("otf", "font/otf"),
    ("pdf", "application/pdf"),
    ("png", "image/png"),
    ("svg", "image/svg+xml"),
    ("ttf", "font/ttf"),
    ("txt", "text/plain; charset=utf-8"),
    ("vtt", "text/vtt; charset=utf-8"),
    ("wasm", "application/wasm"),
    ("wav", "audio/wav"),
    ("webm", "video/webm"),
    ("webmanifest", "application/manifest+json"),
    ("webp", "image/webp"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
    ("xhtml", "application/xhtml+xml; charset=utf-8"),
    ("xml", "application/xml"),
];

/// The type of a file whose extension is not in [`CONTENT_TYPES`].
const UNKNOWN_TYPE: &str = "application/octet-stream";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// The HTML document a preview shows.
pub struct HtmlPreviewSiteRequest {
    /// Absolute path of the document.
    pub document_path: String,
    /// Absolute path of the open workspace's root. A document inside it has the whole workspace
    /// as its site; without one, or for a document outside it, the site is the document's folder.
    #[serde(default)]
    pub workspace_root: Option<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
/// Where a document's site is, and where the document is in it.
pub struct HtmlPreviewSite {
    /// Absolute path of the site's root folder.
    pub root: String,
    /// The document's path from the root as a relative URL: its segments percent-encoded and
    /// joined by `/`. The frontend appends it to the address its site is served from.
    pub address: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// One request of a document's site.
pub struct HtmlPreviewRequest {
    /// Absolute path of the document the site is for.
    pub document_path: String,
    /// The workspace's root, as in [`HtmlPreviewSiteRequest`].
    #[serde(default)]
    pub workspace_root: Option<String>,
    /// The requested path from the site's root as the web engine reports it: already
    /// percent-decoded, separated by `/`, without a query or a fragment, and empty for the root.
    /// It is not decoded again, so a file named `a%20b.html` is reachable.
    pub path: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase")]
/// What one request of a site serves.
pub enum HtmlPreviewResource {
    /// The previewed document, which the frontend serves from the editor's text as UTF-8 HTML.
    Document,
    /// A file of the site. The path is canonical: an existing regular file inside the root, which
    /// the frontend reads without following another link, and serves as `mime`.
    File {
        /// Absolute, canonical path of the file.
        path: String,
        /// Content type to serve the file as.
        mime: String,
    },
    /// A folder of the site that holds an `index.html`; the frontend sends the page on to that
    /// file, relative to the folder.
    Directory,
    /// A hidden path or a key or certificate store inside the root, refused: HTTP 403.
    Denied,
    /// Nothing is served: a path out of the root, a malformed path, or a file or index that does
    /// not exist. HTTP 404.
    None,
}

/// The site of a document, or `None` for a document that is not an absolute file path.
pub fn html_preview_site(request: HtmlPreviewSiteRequest) -> Option<HtmlPreviewSite> {
    let document = Path::new(&request.document_path);
    let (_, root) = document_scope(document, request.workspace_root.as_deref())?;
    let relative = names(document.strip_prefix(root).ok()?)?;
    if relative.is_empty() {
        return None;
    }
    let address = relative
        .iter()
        .map(|name| percent_encode(name))
        .collect::<Vec<_>>()
        .join("/");
    Some(HtmlPreviewSite {
        root: root.to_string_lossy().into_owned(),
        address,
    })
}

/// Resolves one request of a document's site.
///
/// The request path is checked as written before the file system is touched, and again once
/// symbolic links are resolved, so neither `..` nor a link inside the root reaches a file outside
/// it or a hidden one.
pub fn resolve_html_preview_request(request: HtmlPreviewRequest) -> HtmlPreviewResource {
    let document = Path::new(&request.document_path);
    let Some((folder, root)) = document_scope(document, request.workspace_root.as_deref()) else {
        return HtmlPreviewResource::None;
    };
    // A segment the engine would not send as one name, such as a Windows drive or a backslash
    // path, makes no file of the site.
    if request
        .path
        .split('/')
        .any(|segment| segment.contains(['\\', ':', '\0']))
    {
        return HtmlPreviewResource::None;
    }
    let lexical = join_lexically(root, &request.path);
    if !within(&lexical, root) {
        return HtmlPreviewResource::None;
    }
    if lexical == document {
        return HtmlPreviewResource::Document;
    }
    // Hidden folders the document itself lies in are its own; any other is not served.
    let home = folder.strip_prefix(root).unwrap_or(Path::new(""));
    if refused(lexical.strip_prefix(root).unwrap_or(&lexical), home) {
        return HtmlPreviewResource::Denied;
    }
    let (Ok(canonical_root), Ok(path)) =
        (std::fs::canonicalize(root), std::fs::canonicalize(&lexical))
    else {
        return HtmlPreviewResource::None;
    };
    let Ok(inside) = path.strip_prefix(&canonical_root) else {
        return HtmlPreviewResource::None;
    };
    let canonical_home = std::fs::canonicalize(folder)
        .ok()
        .and_then(|folder| {
            folder
                .strip_prefix(&canonical_root)
                .ok()
                .map(Path::to_path_buf)
        })
        .unwrap_or_default();
    if refused(inside, &canonical_home) {
        return HtmlPreviewResource::Denied;
    }
    if path.is_dir() {
        return match resolve_index(&path, &canonical_root) {
            true => HtmlPreviewResource::Directory,
            false => HtmlPreviewResource::None,
        };
    }
    if !path.is_file() {
        return HtmlPreviewResource::None;
    }
    // The document reached through another name, such as a link to it, is still the editor's.
    if std::fs::canonicalize(document).is_ok_and(|document| document == path) {
        return HtmlPreviewResource::Document;
    }
    HtmlPreviewResource::File {
        mime: content_type(&path).to_string(),
        path: path.to_string_lossy().into_owned(),
    }
}

/// Whether a folder of the site has an index file that stays inside the root.
fn resolve_index(folder: &Path, root: &Path) -> bool {
    std::fs::canonicalize(folder.join(INDEX))
        .is_ok_and(|index| index.starts_with(root) && index.is_file())
}

/// Whether a path inside the root, given from the root, is refused: it names a hidden file or
/// folder below `home`, the document's folder from the root, or a key or certificate store.
fn refused(relative: &Path, home: &Path) -> bool {
    let mut home = home.components();
    let mut own = true;
    let mut last = None;
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        last = Some(name);
        // The leading folders shared with the document's own folder were chosen by the user.
        own = own && home.next() == Some(component);
        if !own && name.to_string_lossy().starts_with('.') {
            return true;
        }
    }
    last.and_then(|name| Path::new(name).extension())
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .is_some_and(|extension| SECRET_EXTENSIONS.contains(&extension.as_str()))
}

/// The content type a file is served as, by its extension.
fn content_type(path: &Path) -> &'static str {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    CONTENT_TYPES
        .iter()
        .find(|(name, _)| *name == extension)
        .map_or(UNKNOWN_TYPE, |(_, mime)| mime)
}

/// The names of a relative path, or `None` when it has any other component.
fn names(relative: &Path) -> Option<Vec<String>> {
    relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str().map(str::to_string),
            _ => None,
        })
        .collect()
}

/// Percent-encodes one path segment, keeping only RFC 3986's unreserved characters, so that no
/// name reads as a query, a fragment or another segment.
fn percent_encode(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::{
        html_preview_site, resolve_html_preview_request, HtmlPreviewRequest, HtmlPreviewResource,
        HtmlPreviewSite, HtmlPreviewSiteRequest,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    /// The path of slash-separated `relative` under `base`, in the platform's own separators.
    fn under(base: &Path, relative: &str) -> PathBuf {
        relative
            .split('/')
            .fold(base.to_path_buf(), |path, name| path.join(name))
    }

    /// A temporary workspace with a site in `site/`, a page in a hidden folder, secrets and,
    /// outside the workspace, `outside.css`.
    struct Workspace {
        top: PathBuf,
        root: PathBuf,
    }

    impl Workspace {
        fn new(name: &str) -> Self {
            let top = crate::test_support::temporary_root(&format!("html-preview-{name}"));
            let root = top.join("workspace");
            for folder in [
                "site/css",
                "site/docs",
                "site/empty",
                ".github/pages",
                ".git",
            ] {
                fs::create_dir_all(under(&root, folder)).expect("workspace folder");
            }
            for file in [
                "site/index.html",
                "site/page one.html",
                "site/a%20b.html",
                "site/css/app.css",
                "site/app.mjs",
                "site/data.bin",
                "site/docs/index.html",
                "site/server.pem",
                "site/.env",
                ".git/config",
                ".github/pages/page.html",
                ".github/pages/style.css",
                ".github/secret.txt",
            ] {
                fs::write(under(&root, file), b"x").expect("workspace file");
            }
            fs::write(top.join("outside.css"), b"x").expect("outside file");
            Self { top, root }
        }

        fn document(&self, relative: &str) -> String {
            under(&self.root, relative).to_string_lossy().into_owned()
        }

        fn resolve_for(&self, document: &str, path: &str) -> HtmlPreviewResource {
            resolve_html_preview_request(HtmlPreviewRequest {
                document_path: self.document(document),
                workspace_root: Some(self.root.to_string_lossy().into_owned()),
                path: path.to_string(),
            })
        }

        fn resolve(&self, path: &str) -> HtmlPreviewResource {
            self.resolve_for("site/index.html", path)
        }

        fn file(&self, relative: &str, mime: &str) -> HtmlPreviewResource {
            HtmlPreviewResource::File {
                path: fs::canonicalize(under(&self.root, relative))
                    .expect("canonical workspace file")
                    .to_string_lossy()
                    .into_owned(),
                mime: mime.to_string(),
            }
        }
    }

    impl Drop for Workspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.top);
        }
    }

    #[test]
    fn the_site_is_the_workspace_with_the_document_addressed_from_its_root() {
        let workspace = Workspace::new("site");
        let site = html_preview_site(HtmlPreviewSiteRequest {
            document_path: workspace.document("site/page one.html"),
            workspace_root: Some(workspace.root.to_string_lossy().into_owned()),
        });
        assert_eq!(
            site,
            Some(HtmlPreviewSite {
                root: workspace.root.to_string_lossy().into_owned(),
                address: "site/page%20one.html".to_string(),
            })
        );
        // Without a workspace, the document's folder is the site.
        let alone = html_preview_site(HtmlPreviewSiteRequest {
            document_path: workspace.document("site/a%20b.html"),
            workspace_root: None,
        });
        assert_eq!(
            alone.map(|site| site.address),
            Some("a%2520b.html".to_string())
        );
        assert_eq!(
            html_preview_site(HtmlPreviewSiteRequest {
                document_path: "page.html".to_string(),
                workspace_root: None,
            }),
            None
        );
    }

    #[test]
    fn requests_serve_the_document_files_and_folders_of_the_site() {
        let workspace = Workspace::new("files");
        assert_eq!(
            workspace.resolve("site/index.html"),
            HtmlPreviewResource::Document
        );
        assert_eq!(
            workspace.resolve("site/css/app.css"),
            workspace.file("site/css/app.css", "text/css; charset=utf-8")
        );
        assert_eq!(
            workspace.resolve("site/app.mjs"),
            workspace.file("site/app.mjs", "text/javascript; charset=utf-8")
        );
        assert_eq!(
            workspace.resolve("site/data.bin"),
            workspace.file("site/data.bin", "application/octet-stream")
        );
        // The engine has decoded the path already; it is not decoded again.
        assert_eq!(
            workspace.resolve("site/page one.html"),
            workspace.file("site/page one.html", "text/html; charset=utf-8")
        );
        assert_eq!(
            workspace.resolve("site/a%20b.html"),
            workspace.file("site/a%20b.html", "text/html; charset=utf-8")
        );
        assert_eq!(
            workspace.resolve("site/docs"),
            HtmlPreviewResource::Directory
        );
        assert_eq!(
            workspace.resolve("site/docs/"),
            HtmlPreviewResource::Directory
        );
        assert_eq!(workspace.resolve("site/empty"), HtmlPreviewResource::None);
        assert_eq!(
            workspace.resolve("site/missing.css"),
            HtmlPreviewResource::None
        );
    }

    #[test]
    fn requests_stay_inside_the_root_and_out_of_hidden_files_and_keys() {
        let workspace = Workspace::new("refused");
        for path in [
            "../outside.css",
            "site/../../outside.css",
            "C:/Windows/win.ini",
            "site\\..\\..\\outside.css",
            "site/css/app.css\0",
        ] {
            assert_eq!(workspace.resolve(path), HtmlPreviewResource::None, "{path}");
        }
        for path in [
            ".git/config",
            "site/.env",
            "site/server.pem",
            ".github/pages/page.html",
        ] {
            assert_eq!(
                workspace.resolve(path),
                HtmlPreviewResource::Denied,
                "{path}"
            );
        }
        // A page inside a hidden folder reaches its own folder, but no other hidden one.
        let page = ".github/pages/page.html";
        assert_eq!(
            workspace.resolve_for(page, ".github/pages/style.css"),
            workspace.file(".github/pages/style.css", "text/css; charset=utf-8")
        );
        assert_eq!(
            workspace.resolve_for(page, ".github/secret.txt"),
            workspace.file(".github/secret.txt", "text/plain; charset=utf-8")
        );
        assert_eq!(
            workspace.resolve_for(page, ".git/config"),
            HtmlPreviewResource::Denied
        );
        assert_eq!(
            workspace.resolve_for(page, page),
            HtmlPreviewResource::Document
        );
    }

    #[cfg(unix)]
    #[test]
    fn links_are_resolved_before_the_root_and_hidden_names_are_checked() {
        use std::os::unix::fs::symlink;
        let workspace = Workspace::new("links");
        symlink(
            workspace.top.join("outside.css"),
            under(&workspace.root, "site/out.css"),
        )
        .expect("link out of the workspace");
        symlink(
            under(&workspace.root, "site/.env"),
            under(&workspace.root, "site/env.txt"),
        )
        .expect("link to a hidden file");
        symlink(
            under(&workspace.root, "site/index.html"),
            under(&workspace.root, "site/home.html"),
        )
        .expect("link to the document");
        assert_eq!(workspace.resolve("site/out.css"), HtmlPreviewResource::None);
        assert_eq!(
            workspace.resolve("site/env.txt"),
            HtmlPreviewResource::Denied
        );
        assert_eq!(
            workspace.resolve("site/home.html"),
            HtmlPreviewResource::Document
        );
    }
}
