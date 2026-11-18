//! Steps run before a configuration starts (IDEA's "Before launch").
//!
//! A configuration may name, in order, built-in process runs to start first:
//! a shell script that compiles a module, a program that generates sources.
//! Each step must exit with 0 before the next one, and before the
//! configuration itself, starts. A step reuses everything a process run
//! already has (interpreter, arguments, working directory, environment and
//! launch plan), so a step is a reference to such a run, not a second command
//! format. A referenced run that names steps of its own runs them first, as
//! starting it on its own would.
//!
//! Core checks the references, expands them into one ordered list, and says
//! whether the list needs the user's confirmation: steps a project file names
//! run code from the repository the user did not choose on this machine, so
//! the host asks before running a list it has not seen. The host runs each
//! step through `runConfig.createLaunchPlan`.

use super::process::{self, ProcessOptions};
use super::*;
use sha2::{Digest, Sha256};

/// Most steps one configuration names.
pub(crate) const MAXIMUM_STEPS: usize = 16;
/// Most steps one start runs once referenced runs are expanded.
const MAXIMUM_EXPANDED_STEPS: usize = 32;
/// Longest configuration ID a step names.
const MAXIMUM_ID_BYTES: usize = 256;

/// One step of `beforeLaunch`: the run configuration to start first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeforeLaunchStep {
    /// ID of a built-in process run of the same project.
    pub configuration: String,
}

/// Checks the shape of a configuration's own steps: their number, and IDs
/// that are not empty, not too long and not the configuration itself. Whether
/// a step names a runnable process run is checked when it starts, so a
/// removed step target never hides the configuration from the list.
pub(crate) fn validate_shape(configuration: &RunConfiguration) -> Result<(), CoreError> {
    let steps = &configuration.before_launch;
    if steps.len() > MAXIMUM_STEPS {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "A run configuration names at most 16 steps before launch",
        ));
    }
    for step in steps {
        let id = step.configuration.as_str();
        if id.trim().is_empty() || id.len() > MAXIMUM_ID_BYTES || id == configuration.id {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                "A step before launch names another run configuration",
            )
            .with_details(id.to_string()));
        }
    }
    Ok(())
}

/// Why a list of steps cannot run, for the field or the step it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StepRefusal {
    /// `notFound`, `notAllowed`, `unavailable`, `cycle` or `tooLong`.
    pub(crate) code: &'static str,
    /// The configuration the refusal is about.
    pub(crate) configuration: String,
}

/// The resolved configurations by ID.
pub(crate) struct Resolved {
    by_id: BTreeMap<String, RunConfiguration>,
}

impl Resolved {
    pub(crate) fn read(root: &Path, layers: &Layers) -> Result<Self, CoreError> {
        let resolved = resolve_layers(root, layers, None)?;
        let configurations: Vec<RunConfiguration> =
            serde_json::from_value(resolved["configurations"].clone()).map_err(|error| {
                CoreError::new(ErrorCode::ParseFailed, "Invalid resolved run configuration")
                    .with_details(error.to_string())
            })?;
        Ok(Self {
            by_id: configurations
                .into_iter()
                .map(|configuration| (configuration.id.clone(), configuration))
                .collect(),
        })
    }

    /// The configurations a step of `owner` may name: every runnable built-in
    /// process run but `owner`, in name order.
    pub(crate) fn candidates(&self, owner: Option<&str>) -> Vec<Value> {
        let mut candidates = self
            .by_id
            .values()
            .filter(|configuration| {
                Some(configuration.id.as_str()) != owner
                    && process::is_process_provider(&configuration.provider)
                    && configuration.availability.is_none()
            })
            .map(|configuration| json!({ "id": configuration.id, "name": configuration.name }))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| {
            (left["name"].as_str(), left["id"].as_str())
                .cmp(&(right["name"].as_str(), right["id"].as_str()))
        });
        candidates
    }

    /// Expands `steps`, the steps `owner` would have, into the runs they
    /// start, nested steps first. `owner` may be a configuration being
    /// created, which nothing names yet.
    pub(crate) fn expand(
        &self,
        owner: &str,
        steps: &[BeforeLaunchStep],
    ) -> Result<Expansion, StepRefusal> {
        let mut expansion = Expansion::default();
        let mut stack = vec![owner.to_string()];
        self.expand_into(steps, &mut stack, &mut expansion)?;
        expansion.owners.insert(0, owner.to_string());
        Ok(expansion)
    }

    fn expand_into(
        &self,
        steps: &[BeforeLaunchStep],
        stack: &mut Vec<String>,
        expansion: &mut Expansion,
    ) -> Result<(), StepRefusal> {
        if steps.len() > MAXIMUM_STEPS {
            return Err(StepRefusal {
                code: "tooLong",
                configuration: stack.last().cloned().unwrap_or_default(),
            });
        }
        for step in steps {
            let id = &step.configuration;
            let refuse = |code| StepRefusal {
                code,
                configuration: id.clone(),
            };
            if stack.contains(id) {
                return Err(refuse("cycle"));
            }
            let target = self.by_id.get(id).ok_or_else(|| refuse("notFound"))?;
            if !process::is_process_provider(&target.provider) {
                return Err(refuse("notAllowed"));
            }
            if target.availability.is_some() {
                return Err(refuse("unavailable"));
            }
            if !target.before_launch.is_empty() {
                stack.push(id.clone());
                expansion.owners.push(id.clone());
                self.expand_into(&target.before_launch, stack, expansion)?;
                stack.pop();
            }
            if expansion.steps.len() >= MAXIMUM_EXPANDED_STEPS {
                return Err(refuse("tooLong"));
            }
            expansion.steps.push(target.clone());
        }
        Ok(())
    }
}

/// Steps expanded for one start.
#[derive(Default)]
pub(crate) struct Expansion {
    /// The runs to start, in order.
    pub(crate) steps: Vec<RunConfiguration>,
    /// The configurations whose step lists were read: the one that starts,
    /// then each referenced run that names steps of its own.
    pub(crate) owners: Vec<String>,
}

/// A step that refused to expand, in the shape the host shows.
fn refusal_value(refusal: &StepRefusal) -> Value {
    json!({ "code": refusal.code, "configurationId": refusal.configuration })
}

/// The steps a list of configuration IDs names, read from an editor.
pub(crate) fn steps_of(ids: &[String]) -> Vec<BeforeLaunchStep> {
    ids.iter()
        .map(|id| BeforeLaunchStep {
            configuration: id.trim().to_string(),
        })
        .collect()
}

/// The layer a configuration's own step list was read from: the local layer
/// when it names `beforeLaunch` there, else the project layer.
fn list_from_project(layers: &Layers, id: &str) -> bool {
    let names_steps = |document: &Value| {
        document["configurations"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|item| item["id"] == id && item.get("beforeLaunch").is_some())
    };
    if names_steps(&layers.local.document) {
        return false;
    }
    names_steps(&layers.project.document)
}

/// A one-line description of what a process run starts, for the
/// confirmation that lists the steps.
fn summary(configuration: &RunConfiguration) -> String {
    let options = ProcessOptions::of(configuration).unwrap_or_default();
    let mut words = Vec::new();
    if let Some(program) = options.program() {
        words.push(program.to_string());
    } else if let Some(script) = options.script() {
        if let Some(interpreter) = options.interpreter() {
            words.push(interpreter.to_string());
        }
        words.push(script.to_string());
    } else if let Some(text) = options.text() {
        let first = text
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let more = text.lines().filter(|line| !line.trim().is_empty()).count() > 1;
        words.push(if more {
            format!("{} …", first.trim())
        } else {
            first.trim().to_string()
        });
    }
    if !configuration.args.is_empty() {
        words.push(
            crate::format_argument_text(crate::ArgumentTextRequest {
                argument_lists: vec![configuration.args.clone()],
            })
            .remove(0),
        );
    }
    words.join(" ")
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Request for the steps a configuration runs before it starts.
pub struct BeforeLaunchRequest {
    /// Canonical workspace root.
    pub root: String,
    /// The configuration that starts.
    pub configuration_id: String,
    /// Host-owned replacement for the machine-local layer.
    #[serde(default)]
    pub local_document: Option<Value>,
}

/// `runConfig.beforeLaunch`: the runs to start, in order, before
/// configuration `configurationId` starts.
///
/// Replies `{status: "ok", steps, confirmation}` with each step's
/// `{configurationId, name, summary}` and, when a project file names any of
/// the step lists, `confirmation: {fingerprint}`. The fingerprint covers the
/// lists and the runs they start; the host asks once per fingerprint before
/// running them. A list that cannot run replies `{status: "rejected", refusal:
/// {code, configurationId}}` with `notFound`, `notAllowed` (not a built-in
/// process run), `unavailable`, `cycle` or `tooLong`.
pub fn before_launch(request: BeforeLaunchRequest) -> Result<Value, CoreError> {
    let root = existing_root(&request.root)?;
    let layers = read_layers(&root, None, request.local_document)?;
    let resolved = Resolved::read(&root, &layers)?;
    let configuration = resolved
        .by_id
        .get(&request.configuration_id)
        .ok_or_else(|| {
            CoreError::new(ErrorCode::InvalidRequest, "Run configuration was not found")
        })?;
    let expansion = match resolved.expand(&configuration.id, &configuration.before_launch) {
        Ok(expansion) => expansion,
        Err(refusal) => {
            return Ok(json!({ "status": "rejected", "refusal": refusal_value(&refusal) }))
        }
    };
    let steps = expansion
        .steps
        .iter()
        .map(|step| {
            json!({ "configurationId": step.id, "name": step.name, "summary": summary(step) })
        })
        .collect::<Vec<_>>();
    let from_project = expansion
        .owners
        .iter()
        .any(|owner| list_from_project(&layers, owner));
    let confirmation = (from_project && !steps.is_empty())
        .then(|| json!({ "fingerprint": fingerprint(&resolved, &expansion) }));
    Ok(json!({ "status": "ok", "steps": steps, "confirmation": confirmation }))
}

/// Identity of what the steps start: each list and each run's definition.
/// Changing a list or a run the steps start asks the user again.
fn fingerprint(resolved: &Resolved, expansion: &Expansion) -> String {
    let lists = expansion
        .owners
        .iter()
        .map(|owner| {
            let steps = resolved
                .by_id
                .get(owner)
                .map(|configuration| configuration.before_launch.clone())
                .unwrap_or_default();
            json!({ "owner": owner, "steps": steps })
        })
        .collect::<Vec<_>>();
    let runs = expansion
        .steps
        .iter()
        .map(|step| {
            json!({
                "id": step.id,
                "provider": step.provider,
                "args": step.args,
                "cwd": step.cwd,
                "env": step.env,
                "process": step.extensions.get("process"),
            })
        })
        .collect::<Vec<_>>();
    let bytes = serde_json::to_vec(&json!({ "lists": lists, "runs": runs }))
        .expect("fingerprint input encodes");
    format!("{:x}", Sha256::digest(bytes))
}

/// Checks the steps an editor names for configuration `owner` against the
/// saved configurations; the code of the first that is wrong.
pub(crate) fn check_edit(
    root: &Path,
    layers: &Layers,
    owner: &str,
    ids: &[String],
) -> Result<Option<&'static str>, CoreError> {
    if ids.len() > MAXIMUM_STEPS {
        return Ok(Some("tooLong"));
    }
    if ids
        .iter()
        .any(|id| id.trim().is_empty() || id.trim() == owner)
    {
        return Ok(Some("notAllowed"));
    }
    let resolved = Resolved::read(root, layers)?;
    Ok(resolved
        .expand(owner, &steps_of(ids))
        .err()
        .map(|refusal| match refusal.code {
            "cycle" => "conflict",
            code => code,
        }))
}
