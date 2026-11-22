//! Selects a different platform's archive for an installed release and checks
//! that the downloaded manifest stays within that release's approval.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Selection {
    index: Value,
    source_kind: SourceKind,
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
    installed_platform: String,
    installed_sha256: String,
    #[serde(default)]
    repository: Option<String>,
    target: Target,
    host_capabilities: HostCapabilities,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Target {
    os: String,
    arch: String,
}

impl Target {
    fn marketplace_platform(&self) -> Option<&'static str> {
        match (self.os.as_str(), self.arch.as_str()) {
            ("linux", "x86_64") => Some("linux-x64"),
            ("linux", "arm64") => Some("linux-arm64"),
            _ => None,
        }
    }
}

fn unavailable() -> PluginRejection {
    PluginRejection::new(
        "plugin.remote.variantUnavailable",
        "The approved plugin release has no compatible archive for this remote host.",
    )
}

fn select(request: Selection) -> Validation<Value> {
    let platform = request
        .target
        .marketplace_platform()
        .ok_or_else(unavailable)?;
    if !is_plugin_id(&request.plugin_id)
        || parse_version(&request.version).is_none()
        || !is_platform(&request.installed_platform)
        || !is_sha256(&request.installed_sha256)
    {
        return Err(unavailable());
    }
    let repository = request
        .repository
        .as_deref()
        .map(|url| parse_repository(url).ok_or_else(unavailable))
        .transpose()?;
    let approved = ApprovedRelease {
        version: &request.version,
        platform: &request.installed_platform,
        sha256: &request.installed_sha256,
    };
    let index = validate_index_for_release(
        request.index,
        request.source_kind,
        platform,
        Some(&request.host_capabilities),
        Some(&approved),
    )?;
    let Some(entries) = index["plugins"].as_array() else {
        return Err(unavailable());
    };
    entries
        .iter()
        .find(|entry| {
            entry["id"].as_str() == Some(request.plugin_id.as_str())
                && repository.as_ref().is_none_or(|approved| {
                    entry["repository"]
                        .as_str()
                        .and_then(parse_repository)
                        .is_some_and(|repository| repository.same_as(approved))
                })
                && entry["availability"]["status"].as_str() == Some("available")
        })
        .map(|entry| entry["availability"].clone())
        .ok_or_else(unavailable)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Verification {
    installed_manifest: Value,
    variant_manifest: Value,
    #[serde(rename = "pluginID")]
    plugin_id: String,
    version: String,
    approved_programs: Vec<Value>,
    host_capabilities: HostCapabilities,
}

fn verify(request: Verification) -> Validation<Value> {
    let installed = validated_manifest_json(request.installed_manifest)?;
    let variant = validated_manifest_json(request.variant_manifest)?;
    let review = install_review(
        variant.clone(),
        Some(&request.plugin_id),
        Some(&request.version),
        Some(&request.approved_programs),
        Some(&request.host_capabilities),
    )?;
    // Keep version, permissions and launch inputs identical across platforms.
    // Each archive can ship different native bytes at those declared paths.
    if variant != installed
        || review["newPrograms"]
            .as_array()
            .is_none_or(|v| !v.is_empty())
    {
        return Err(PluginRejection::new(
            "plugin.remote.approvalRequired",
            "The remote plugin archive changes the approved manifest or programs.",
        ));
    }
    Ok(variant)
}

/// Returns the archive selection for the remote target and the exact approved
/// release. The caller fetches the recorded index and verifies the archive hash.
pub fn select_remote_variant_command(payload: Value) -> Result<Value, CoreError> {
    let request = decode_payload(payload, "plugins.marketplace.selectRemoteVariant")?;
    Ok(outcome("selection", select(request)))
}

/// Validates the installed and target manifests and checks the stored
/// executable approvals before allowing deployment.
pub fn verify_remote_variant_command(payload: Value) -> Result<Value, CoreError> {
    let request = decode_payload(payload, "plugins.marketplace.verifyRemoteVariant")?;
    Ok(outcome("manifest", verify(request)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Deployment {
    archive_sha256: Option<String>,
    installed_tree_sha256: Option<String>,
    manifest: Value,
    target: Target,
    required_paths: Vec<String>,
    #[serde(default)]
    archive_files: Option<Vec<String>>,
}

fn invalid_deployment() -> PluginRejection {
    PluginRejection::new(
        "plugin.remote.invalidDeployment",
        "The remote plugin deployment has an invalid input path or package digest.",
    )
}

/// A directory excludes its descendants, but never a similarly named sibling.
fn is_local_only(path: &str, local_only: &BTreeSet<String>) -> bool {
    local_only.contains(path)
        || path
            .match_indices('/')
            .any(|(separator, _)| local_only.contains(&path[..separator]))
}

fn deployment(request: Deployment) -> Validation<Value> {
    use lithe_plugin_model::deployment_package_path;
    use sha2::{Digest, Sha256};

    let platform = request
        .target
        .marketplace_platform()
        .ok_or_else(unavailable)?;
    let (source, digest) = match (request.archive_sha256, request.installed_tree_sha256) {
        (Some(digest), None) => ("archive", digest),
        (None, Some(digest)) => ("installed", digest),
        _ => return Err(invalid_deployment()),
    };
    let paths: BTreeSet<_> = request.required_paths.into_iter().collect();
    if !is_sha256(&digest)
        || paths.is_empty()
        || paths.iter().any(|file| !deployment_package_path(file))
    {
        return Err(invalid_deployment());
    }
    let manifest = validate_manifest_value(request.manifest)?;
    let local_only = manifest.local_only_package_paths(&request.target.os, &request.target.arch);
    if paths.iter().any(|path| is_local_only(path, &local_only)) {
        return Err(invalid_deployment());
    }

    // The verified package digest and target determine the inventory and
    // exclusions; the declared paths select a subset.
    let mut hash = Sha256::new();
    hash.update(b"lithe-plugin-deployment-v3\0");
    hash.update(source);
    hash.update(b"\0");
    hash.update(digest.to_ascii_lowercase());
    hash.update(b"\0");
    hash.update(platform);
    for file in &paths {
        hash.update(b"\0");
        hash.update(file);
    }
    let selected = request
        .archive_files
        .map(|files| select_deployment_files(&paths, &local_only, files))
        .transpose()?;
    Ok(json!({"sha256": format!("{:x}", hash.finalize()), "paths": selected}))
}

/// Expands approved input trees against the verified ZIP's regular-file inventory.
fn select_deployment_files(
    required: &BTreeSet<String>,
    local_only: &BTreeSet<String>,
    files: Vec<String>,
) -> Validation<BTreeSet<String>> {
    use lithe_plugin_model::deployment_package_path;

    if files.iter().any(|file| !deployment_package_path(file)) {
        return Err(invalid_deployment());
    }
    let files: BTreeSet<_> = files
        .into_iter()
        .filter(|file| !is_local_only(file, local_only))
        .collect();
    let mut selected = BTreeSet::new();
    for path in required {
        if let Some(file) = files.get(path) {
            selected.insert(file.clone());
            continue;
        }
        let prefix = format!("{path}/");
        let mut descendants = files
            .range(prefix.clone()..)
            .take_while(|file| file.starts_with(&prefix))
            .peekable();
        if descendants.peek().is_none() {
            return Err(PluginRejection::new(
                "plugin.remote.variantUnavailable",
                "The remote plugin archive is missing a declared process input.",
            ));
        }
        selected.extend(descendants.cloned());
    }
    Ok(selected)
}

/// Validates process input paths and derives the deployment identity before
/// download. With an archive inventory, it returns only the files to upload.
pub fn plan_remote_deployment_command(payload: Value) -> Result<Value, CoreError> {
    let request = decode_payload(payload, "plugins.marketplace.planRemoteDeployment")?;
    Ok(outcome("deployment", deployment(request)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> Value {
        serde_json::from_str(include_str!(
            "../../../../shared/plugins/contract/v1/fixtures/marketplace/remote-variant.json"
        ))
        .unwrap()
    }

    #[test]
    fn selects_the_installed_release_even_with_a_newer_compatible_release() {
        let answer = select_remote_variant_command(selection()).unwrap();

        assert_eq!(answer["status"], "ok");
        assert_eq!(answer["selection"]["version"], "1.0.0");
        assert_eq!(answer["selection"]["sha256"], "2".repeat(64));
        assert_eq!(
            answer["selection"]["downloadURL"],
            "https://github.com/example/remote-tools/releases/download/1.0.0/linux.zip"
        );
    }

    #[test]
    fn refuses_a_release_without_the_approved_origin_or_target() {
        for (field, value) in [
            ("version", json!("3.0.0")),
            ("installedSha256", json!("3".repeat(64))),
            ("repository", json!("https://github.com/example/other")),
            ("target", json!({"os": "linux", "arch": "arm64"})),
            ("hostCapabilities", json!({})),
        ] {
            let mut request = selection();
            request[field] = value;
            let answer = select_remote_variant_command(request).unwrap();

            assert_eq!(
                answer["error"]["code"], "plugin.remote.variantUnavailable",
                "{field}"
            );
        }
    }

    fn variant_manifest() -> Value {
        json!({
            "schemaVersion": 0, "pluginAPIVersion": 0,
            "id": "dev.lithe.test.remote-variant", "version": "1.0.0", "displayName": "Remote Variant",
            "vendor": {"id": "org.example", "displayName": "Example", "signatureRequirement": "publisherPackage"},
            "requires": {"languageServers": 0, "languages": 0},
            "permissions": ["languageServer.launch"],
            "activationEvents": ["onLanguage:example"],
            "languages": [{"id": "example", "displayName": "Example", "fileExtensions": ["sample"]}],
            "languageServers": [{"languageID": "example", "languageIdentifier": "example", "launch": {"kind": "packaged", "path": "server/bin/run"}}],
        })
    }

    #[test]
    fn deployment_selects_declared_files_and_trees_without_local_assets() {
        let mut manifest = variant_manifest();
        manifest["localOnlyPaths"] = json!(["resources/local", "resources/private.js"]);
        manifest["requires"]["ui"] = json!(0);
        manifest["requires"]["backend"] = json!(0);
        manifest["permissions"] = json!([
            "languageServer.launch",
            "backend.launch",
            "ui.contribute",
            "ui.web",
        ]);
        manifest["activationEvents"] = json!([
            "onLanguage:example",
            "onView:dev.lithe.test.remote-variant.window",
            "onCommand:dev.lithe.test.remote-variant.open",
        ]);
        manifest["ui"] = json!({
            "toolWindows": [{
                "id": "dev.lithe.test.remote-variant.window", "title": "Example",
                "placement": "bottom", "web": {"entry": "resources/page.html"},
                "icon": {"path": "resources/window.svg"},
            }],
            "commands": [{
                "id": "dev.lithe.test.remote-variant.open", "title": "Open",
                "toolWindowID": "dev.lithe.test.remote-variant.window",
                "placements": ["toolbar"],
                "icon": {"path": "resources/command.svg", "darkPath": "resources/command-dark.svg"},
            }],
        });
        manifest["backend"] = json!({
            "launch": {"kind": "packaged", "path": "resources/bin/backend"},
            "arguments": ["--assets=${pluginDir}/resources/backend-assets"],
            "initializationOptions": {"script": {"path": "${pluginDir}/resources/backend.js"}},
        });
        let mut request = json!({
            "archiveSha256": "3".repeat(64),
            "manifest": manifest,
            "target": {"os": "linux", "arch": "arm64"},
            "requiredPaths": ["resources", "resources/bin/run", "server/bin/run"],
        });
        let preflight = plan_remote_deployment_command(request.clone()).unwrap();
        assert_eq!(preflight["status"], "ok", "{preflight}");
        request["archiveFiles"] = json!([
            "l10n/en.json",
            "plugin.json",
            "lithe-plugin-signature.json",
            "resources/backend-assets/data.json",
            "resources/backend.js",
            "resources/bin/backend",
            "resources/bin/run",
            "resources/command-dark.svg",
            "resources/command.svg",
            "resources/lib/a.so",
            "resources/lib/sub/b.so",
            "resources/local/entry.html",
            "resources/local-tool",
            "resources/page.html",
            "resources/private.js",
            "resources/window.svg",
            "resources-old/not-required",
            "server/bin/run",
        ]);
        let selected = plan_remote_deployment_command(request.clone()).unwrap();
        assert_eq!(
            selected["deployment"]["sha256"],
            preflight["deployment"]["sha256"]
        );
        let mut installed = request.clone();
        installed["installedTreeSha256"] = request["archiveSha256"].clone();
        let ambiguous = plan_remote_deployment_command(installed.clone()).unwrap();
        assert_eq!(
            ambiguous["error"]["code"],
            "plugin.remote.invalidDeployment"
        );
        installed.as_object_mut().unwrap().remove("archiveSha256");
        let installed = plan_remote_deployment_command(installed).unwrap();
        assert_ne!(
            installed["deployment"]["sha256"],
            selected["deployment"]["sha256"]
        );
        assert_eq!(
            installed["deployment"]["paths"],
            selected["deployment"]["paths"]
        );
        assert_eq!(
            selected["deployment"]["paths"],
            json!([
                "resources/bin/run",
                "resources/lib/a.so",
                "resources/lib/sub/b.so",
                "resources/local-tool",
                "server/bin/run",
            ])
        );
        for path in [
            "resources/local",
            "plugin.json",
            "l10n/en.json",
            "../resources",
        ] {
            request["requiredPaths"] = json!([path]);
            let answer = plan_remote_deployment_command(request.clone()).unwrap();
            assert_eq!(
                answer["error"]["code"], "plugin.remote.invalidDeployment",
                "{path}"
            );
        }
        request["requiredPaths"] = json!(["resource"]);
        let answer = plan_remote_deployment_command(request).unwrap();
        assert_eq!(answer["error"]["code"], "plugin.remote.variantUnavailable");
    }

    #[test]
    fn variant_cannot_change_the_manifest_or_add_unapproved_programs() {
        let manifest = variant_manifest();
        let programs = validate_manifest_value(manifest.clone())
            .unwrap()
            .programs();
        let mut request = json!({
            "installedManifest": manifest,
            "variantManifest": manifest,
            "pluginID": "dev.lithe.test.remote-variant", "version": "1.0.0",
            "hostCapabilities": {"languageServers": [0], "languages": [0]},
            "approvedPrograms": programs,
        });
        let answer = verify_remote_variant_command(request.clone()).unwrap();
        assert_eq!(answer["status"], "ok", "{answer}");

        let mut changed = request.clone();
        changed["variantManifest"]["displayName"] = json!("Other");
        let answer = verify_remote_variant_command(changed).unwrap();
        assert_eq!(answer["error"]["code"], "plugin.remote.approvalRequired");

        request["approvedPrograms"] = json!([]);
        let answer = verify_remote_variant_command(request).unwrap();
        assert_eq!(answer["error"]["code"], "plugin.remote.approvalRequired");
    }
}
