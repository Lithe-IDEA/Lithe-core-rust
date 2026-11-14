//! The resolution snapshot, its fixed selections and traces.

use super::*;

/// Provenance of homes found by expanding definition-owned `discovery` scans.
pub(crate) const SYSTEM_PROVIDER: &str = "host:system";

/// One canonical home reported by several routes stays one installation. A provider
/// answer outranks the host scan, then qualified provider IDs give a stable label;
/// the label never decides between different homes.
pub(crate) fn attribution(selection: &Value) -> (bool, &str) {
    let provider = selection["provider"].as_str().unwrap_or("");
    (provider == SYSTEM_PROVIDER, provider)
}

pub(crate) fn applicable(value: &Value, platform: &str) -> bool {
    value.get("platforms").is_none_or(|values| {
        values
            .as_array()
            .is_some_and(|values| values.iter().any(|value| value == platform))
    })
}

/// A host-side value snapshot is not a provider reply or a persisted launch permit.
pub(crate) struct Snapshot<'a> {
    pub(crate) input: &'a Value,
    pub(crate) definition: &'a Value,
    pub(crate) platform: &'a str,
    pub(crate) case_sensitive: bool,
    pub(crate) roots: Vec<String>,
    pub(crate) facts: &'a [Value],
    /// Resolved targets implied by link facts; canonical selections are looked up here.
    pub(crate) targets: Vec<Value>,
    pub(crate) probes: BTreeMap<String, Probe>,
    pub(crate) minimum: Option<&'a str>,
}

impl Snapshot<'_> {
    /// An explicit consumer choice precedes directory pins, but has exactly the
    /// same native version and project-code boundary as other checked candidates.
    pub(crate) fn fixed_selection(
        &self,
        fixed: &FixedToolchainSelection,
    ) -> Result<Value, &'static str> {
        match fixed {
            FixedToolchainSelection::Installation { home } => {
                self.selection(home, "host:manual", "pinned")
            }
            FixedToolchainSelection::Executable { executable } => {
                let fact = self.fact(executable, "file").map_err(|_| "notFound")?;
                if fact["executable"] != true {
                    return Err("notExecutable");
                }
                let canonical = fact["canonicalPath"].as_str().ok_or("unsafePath")?;
                let project = self.roots.iter().any(|root| {
                    paths::within(executable, root, self.platform, self.case_sensitive)
                        || paths::within(canonical, root, self.platform, self.case_sensitive)
                });
                if project && self.input["projectCodeAllowed"] != true {
                    return Err("projectCodeRequiresAction");
                }
                if self.definition["launch"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|token| token == "${home}")
                {
                    return Err("homeUnavailable");
                }
                let version = self.version(executable)?;
                self.environment_selection(
                    None,
                    executable,
                    Some(&version),
                    "host:manual",
                    "pinned",
                    project,
                )
            }
        }
    }

    pub(crate) fn key(&self, path: &str) -> Validation<String> {
        paths::normalize(path, self.platform, self.case_sensitive).ok_or_else(|| reject("path"))
    }

    pub(crate) fn fact(&self, path: &str, kind: &str) -> Validation<&Value> {
        let key = self.key(path)?;
        self.facts
            .iter()
            .chain(&self.targets)
            .find(|fact| {
                fact["path"]
                    .as_str()
                    .and_then(|path| self.key(path).ok())
                    .as_ref()
                    == Some(&key)
                    && fact["kind"] == kind
            })
            .ok_or_else(|| reject("filesystem"))
    }

    pub(crate) fn validate_facts(&self) -> Validation<()> {
        let mut identities = BTreeSet::new();
        for fact in self.facts {
            fields(fact, &["path", "canonicalPath", "kind", "executable"])?;
            let path = fact["path"]
                .as_str()
                .ok_or_else(|| reject("filesystem.path"))?;
            let canonical = fact["canonicalPath"]
                .as_str()
                .ok_or_else(|| reject("filesystem.canonicalPath"))?;
            self.key(canonical)?;
            if !identities.insert(self.key(path)?)
                || !matches!(
                    fact["kind"].as_str(),
                    Some("file" | "directory" | "missing")
                )
                || fact
                    .get("executable")
                    .is_some_and(|value| !value.is_boolean())
            {
                return Err(reject("filesystem"));
            }
        }
        for root in &self.roots {
            self.fact(root, "directory")?;
        }
        Ok(())
    }

    pub(crate) fn checked(&self, home: &str, layout: Option<&str>) -> Validation<Value> {
        query::validate_installation(
            home,
            self.definition,
            self.facts,
            &self.roots,
            self.platform,
            self.case_sensitive,
            layout,
        )
    }

    pub(crate) fn version(&self, executable: &str) -> Result<String, &'static str> {
        let key = self.key(executable).map_err(|_| "unsafePath")?;
        let probe = self.probes.get(&key).ok_or("probeFailed")?;
        if probe.exit_code != 0 {
            return Err("probeFailed");
        }
        let checked =
            capture(self.definition, &probe.output, self.minimum).map_err(|_| "probeFailed")?;
        if checked["satisfied"] != true {
            return Err("versionUnsatisfied");
        }
        checked["version"]
            .as_str()
            .map(str::to_owned)
            .ok_or("probeFailed")
    }

    pub(crate) fn selection(
        &self,
        home: &str,
        provider: &str,
        layer: &str,
    ) -> Result<Value, &'static str> {
        self.layout_selection(home, None, provider, layer)
    }

    /// Selects `home` through one declared executable layout, or the first present one.
    pub(crate) fn layout_selection(
        &self,
        home: &str,
        layout: Option<&str>,
        provider: &str,
        layer: &str,
    ) -> Result<Value, &'static str> {
        let checked = self.checked(home, layout).map_err(|_| "unsafePath")?;
        let executable = checked["executable"].as_str().ok_or("unsafePath")?;
        let version = self.version(executable)?;
        self.environment_selection(
            checked["home"].as_str(),
            executable,
            Some(&version),
            provider,
            layer,
            false,
        )
    }

    pub(crate) fn environment_selection(
        &self,
        home: Option<&str>,
        executable: &str,
        version: Option<&str>,
        provider: &str,
        layer: &str,
        project: bool,
    ) -> Result<Value, &'static str> {
        let mut environment = serde_json::Map::new();
        for export in self.definition["exports"].as_array().into_iter().flatten() {
            let value = match export["value"].as_str() {
                Some("${home}") => home,
                Some("${executable}") => Some(executable),
                _ => None,
            };
            if let Some(value) = value {
                environment.insert(
                    export["name"].as_str().ok_or("unsafePath")?.to_owned(),
                    json!(value),
                );
            }
        }
        let mut prepend = Vec::new();
        if let Some(home) = home {
            for template in self.definition["pathPrepend"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let suffix = template
                    .as_str()
                    .and_then(|template| template.strip_prefix("${home}"))
                    .ok_or("unsafePath")?;
                let path = format!("{home}{suffix}");
                let fact = self.fact(&path, "directory").map_err(|_| "unsafePath")?;
                let canonical = fact["canonicalPath"].as_str().ok_or("unsafePath")?;
                if !paths::within(canonical, home, self.platform, self.case_sensitive) {
                    return Err("unsafePath");
                }
                prepend.push(canonical.to_owned());
            }
        }
        Ok(
            json!({"home":home,"executable":executable,"version":version,"provider":provider,
            "layer":layer,"projectCode":project,"environment":environment,"pathPrepend":prepend}),
        )
    }
}

pub(crate) fn reason(code: &str, tool: &str) -> Value {
    json!({"code":code,"tool":tool})
}

pub(crate) fn trace(
    trace: &mut Vec<Value>,
    id: &str,
    layer: &str,
    state: &str,
    reason: Option<Value>,
) {
    let mut row = json!({"candidateID":id,"layer":layer,"state":state});
    if let Some(reason) = reason {
        row["reason"] = reason;
    }
    trace.push(row);
}

pub(crate) fn output(
    input: &Value,
    outcome: Value,
    trace: Vec<Value>,
    unmet: Vec<Value>,
    conflicts: Vec<Value>,
) -> Value {
    json!({"operationID":input["operationID"],"tool":input["tool"],"directory":input["directory"],
        "outcome":outcome,"trace":trace,"unmet":unmet,"conflicts":conflicts})
}
