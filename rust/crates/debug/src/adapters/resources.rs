//! Package resources an adapter declares and their link checks.

use super::*;

/// A file of the installed package, as the installer found it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct PackageResource {
    pub(crate) path: String,
    pub(crate) kind: PackageResourceKind,
    /// Link target of a symlink, as stored in the package.
    #[serde(default)]
    pub(crate) target: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum PackageResourceKind {
    File,
    Directory,
    Symlink,
}

/// Checks that every packaged file an adapter refers to (its packaged
/// executable and parameter schemas) is a regular file or a link that stays
/// inside the package. Only the installer can see the files, so this is
/// separate from the manifest checks.
#[cfg_attr(not(test), allow(dead_code))]
pub fn check_package_resources(
    adapters: &[DebugAdapterDeclaration],
    resources: &[PackageResource],
) -> Validation<()> {
    for (index, adapter) in adapters.iter().enumerate() {
        if let Some(DebugProcess::Server(process)) = &adapter.launch.process {
            for (key, path) in process.packaged_files() {
                check_resource(
                    resources,
                    path,
                    format!("debugAdapters[{index}].launch.process.launch.{key}"),
                    "The adapter file",
                )?;
            }
        }
        let Some(configurations) = &adapter.configurations else {
            continue;
        };
        for (name, configuration) in [
            ("launch", &configurations.launch),
            ("attach", &configurations.attach),
        ] {
            let Some(path) = configuration
                .as_ref()
                .and_then(|configuration| configuration.parameters_schema.as_ref())
            else {
                continue;
            };
            check_resource(
                resources,
                path,
                format!("debugAdapters[{index}].configurations.{name}.parametersSchema"),
                "The parameters schema",
            )?;
        }
    }
    Ok(())
}

/// Links followed before a chain counts as a loop.
const MAX_LINK_HOPS: usize = 8;

/// One packaged path: present, and a regular file once its links are
/// followed. Every link on the way must stay inside the package. `what`
/// starts the message.
#[cfg_attr(not(test), allow(dead_code))]
fn check_resource(
    resources: &[PackageResource],
    path: &str,
    field: String,
    what: &str,
) -> Validation<()> {
    let mut current = normalize(path);
    for _ in 0..=MAX_LINK_HOPS {
        let Some(resource) = resources.iter().find(|resource| resource.path == current) else {
            return Err(invalid(field, format!("{what} is not in the package.")));
        };
        match resource.kind {
            PackageResourceKind::File => return Ok(()),
            PackageResourceKind::Directory => {
                return Err(invalid(field, format!("{what} is a directory.")));
            }
            PackageResourceKind::Symlink => {
                let Some(next) = resolve_link(&resource.path, resource.target.as_deref()) else {
                    return Err(invalid(
                        field,
                        format!("{what} links outside the plugin package."),
                    ));
                };
                current = next;
            }
        }
    }
    Err(invalid(field, format!("{what} is behind too many links.")))
}

/// Drops empty and `.` components, so `./bin//x` names the file `bin/x`.
#[cfg_attr(not(test), allow(dead_code))]
fn normalize(path: &str) -> String {
    path.split('/')
        .filter(|component| !component.is_empty() && *component != ".")
        .collect::<Vec<_>>()
        .join("/")
}

/// Resolves a symlink target lexically from the link's directory to a
/// package-relative path. Only the final component is followed: a link in a
/// middle directory is the installer's to flatten into the resource list. A target that climbs above the package root, is
/// absolute, or resolves to the root itself has no such path.
#[cfg_attr(not(test), allow(dead_code))]
fn resolve_link(link: &str, target: Option<&str>) -> Option<String> {
    let target = target?;
    if target.is_empty() || target.starts_with('/') || target.contains(['\\', '\0']) {
        return None;
    }
    let mut parts: Vec<&str> = link.split('/').collect();
    parts.pop();
    for component in target.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}
