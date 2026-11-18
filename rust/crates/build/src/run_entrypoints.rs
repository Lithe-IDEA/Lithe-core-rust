//! Entrypoints a provider finds in one source file (`buildProviders` 1).
//!
//! A run declared with `entrypoints: true` starts an entrypoint the provider
//! names, such as a class with a `main` method, in one of its projects. Core
//! never reads the source: the provider answers `lithe/build/runEntrypoints`
//! for a file and Core checks that every answer is something the run can
//! start, so the host can save and start it as an ordinary provider run.

use super::decode::{invalid_output, MAX_OUTPUT_BYTES};
use super::manifest::{BuildSystemDeclaration, TaskSource};
use super::plans::{self, OutputPlanRequest, PlanBase, SelectedEntity};
use lithe_plugin_model::Validation;
use serde::Deserialize;
use serde_json::{json, Value};

/// Version of the `litheRunEntrypoints` document a provider returns.
const ENTRYPOINTS_VERSION: u64 = 0;

/// Most entrypoints one file may name.
const MAX_ENTRYPOINTS: usize = 64;

/// Longest entrypoint name, in bytes.
const MAX_NAME: usize = 256;

/// Longest opaque entrypoint, in bytes, as `launchOptions.entrypoint` allows.
const MAX_ENTRYPOINT: usize = 4096;

/// The source file whose entrypoints the host asks for.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunEntrypointsPlanRequest {
    #[serde(flatten)]
    pub(crate) base: OutputPlanRequest,
    /// Workspace-relative file inside the instance root.
    pub(crate) file: String,
}

/// The plan's configuration and the provider's answer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DecodeRunEntrypointsRequest {
    #[serde(flatten)]
    pub(crate) base: PlanBase,
    pub(crate) result: Value,
}

/// One entrypoint as the provider names it.
#[derive(Deserialize)]
struct Entrypoint {
    run: String,
    name: String,
    entrypoint: String,
    project: SelectedEntity,
}

fn has_entrypoint_run(build_system: &BuildSystemDeclaration) -> bool {
    build_system
        .run
        .iter()
        .any(|run| run.entrypoints && run.source == TaskSource::Provider)
}

/// `build.runEntrypointsPlan`: asks the provider which entrypoints `file` holds.
pub(crate) fn plan(
    build_system: &BuildSystemDeclaration,
    request: &RunEntrypointsPlanRequest,
) -> Validation<Value> {
    if !has_entrypoint_run(build_system) {
        return Err(plans::unavailable(build_system, "run entrypoints"));
    }
    if request.base.project.is_some() || request.base.output_file.is_some() {
        return Err(plans::invalid_selection(
            "Run entrypoints are found by file, not by project.",
        ));
    }
    let context = plans::PlanContext::new(build_system, &request.base.base)?;
    let file = super::decode::request_path(&request.file, "file")?;
    if super::decode::relative_to(&context.root, &file).is_none_or(|relative| relative == ".") {
        return Err(plans::invalid_selection(
            "The file is outside the build-system root.",
        ));
    }
    let mut params = plans::provider_hints(&request.base)?;
    params.insert("file".to_string(), json!(file));
    let provider = build_system
        .provider
        .as_ref()
        .ok_or_else(|| plans::unavailable(build_system, "a provider"))?;
    Ok(context.provider_plan(
        provider,
        "lithe/build/runEntrypoints",
        "litheRunEntrypoints",
        params,
    ))
}

/// `build.decodeRunEntrypoints`: the entrypoints the host may save and start, in
/// the provider's order without duplicates. A malformed answer is refused whole.
pub(crate) fn decode(
    build_system: &BuildSystemDeclaration,
    request: &DecodeRunEntrypointsRequest,
) -> Validation<Value> {
    if !has_entrypoint_run(build_system) {
        return Err(plans::unavailable(build_system, "run entrypoints"));
    }
    let context = plans::PlanContext::new(build_system, &request.base)?;
    let result = &request.result;
    if serde_json::to_vec(result).map_or(usize::MAX, |bytes| bytes.len()) > MAX_OUTPUT_BYTES
        || result["format"] != "litheRunEntrypoints"
        || result["version"] != ENTRYPOINTS_VERSION
    {
        return Err(invalid_output("Invalid run entrypoints document."));
    }
    let items = result["entrypoints"]
        .as_array()
        .filter(|items| items.len() <= MAX_ENTRYPOINTS)
        .ok_or_else(|| invalid_output("Run entrypoints hold at most 64 entries."))?;
    let mut entrypoints: Vec<Value> = Vec::new();
    for item in items {
        let entry: Entrypoint = serde_json::from_value(item.clone())
            .map_err(|_| invalid_output("Invalid run entrypoint."))?;
        let runnable = build_system.run.iter().any(|run| {
            run.id == entry.run && run.entrypoints && run.source == TaskSource::Provider
        });
        let name = entry.name.trim();
        if !runnable
            || name.is_empty()
            || name.len() > MAX_NAME
            || name.chars().any(char::is_control)
            || entry.entrypoint.is_empty()
            || entry.entrypoint.len() > MAX_ENTRYPOINT
            || entry.entrypoint.contains('\0')
        {
            return Err(invalid_output(
                "A run entrypoint names a run, a name or an entrypoint this build system cannot start.",
            ));
        }
        let project =
            plans::selection(&entry.project, &context.root, None, "project").map_err(|_| {
                invalid_output("A run entrypoint's project is outside the build-system root.")
            })?;
        let decoded = json!({
            "run": entry.run,
            "name": name,
            "entrypoint": entry.entrypoint,
            "project": {"name": project.name, "path": project.path},
        });
        if !entrypoints.contains(&decoded) {
            entrypoints.push(decoded);
        }
    }
    Ok(json!({ "entrypoints": entrypoints }))
}
