//! What a press on the title bar's Run or Debug button does
//! (`runConfig.decideTitleAction`).
//!
//! The title bar runs or debugs the run configuration it shows, or the current
//! file. A press can come before the project's configurations are read, and a
//! provider configuration starts only once its build instance's model is
//! loaded. Core decides from the facts the host reports whether the press acts
//! now, waits for the configurations, loads the model first or ends with a
//! notice. The host owns the loads, keeps the waiting press and shows the
//! notices; once what it waited for has arrived it asks again with trigger
//! `settled`. A model that failed to load is loaded again only on a press, so
//! an answer that fails again ends instead of looping.

use serde::{Deserialize, Serialize};

/// The title bar button pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TitleAction {
    Run,
    Debug,
}

/// Why the host asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TitleTrigger {
    /// The user pressed the button.
    Press,
    /// What a waiting press needed has arrived.
    Settled,
}

/// The run configuration the title bar shows, as far as starting it depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub(crate) enum TitleConfiguration {
    /// The configuration does not start through a build provider.
    Unsupported,
    /// The build instance the configuration's run belongs to was not detected.
    InstanceMissing,
    /// The instance's model is not loaded; `failed` says its last load failed.
    ModelNotLoaded { failed: bool },
    /// The instance's model is loaded; `debuggable` says the run declares a debug adapter.
    ModelLoaded { debuggable: bool },
}

/// The facts one decision depends on.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TitleActionRequest {
    pub(crate) action: TitleAction,
    pub(crate) trigger: TitleTrigger,
    /// The user chose Current File over the project's configurations.
    pub(crate) current_file_chosen: bool,
    /// The configurations are read: detection answered, no model is loading and the
    /// configurations were resolved since.
    pub(crate) settled: bool,
    /// The configuration the title bar shows; absent when the project has none.
    #[serde(default)]
    pub(crate) configuration: Option<TitleConfiguration>,
}

/// What the host does with the press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TitleDecision {
    /// Act on the current file.
    CurrentFile,
    /// Keep the press until the configurations are read.
    Wait,
    /// Load the instance's model and keep the press until it arrives.
    LoadModel,
    /// The configuration cannot start from the title bar.
    Unsupported,
    /// The configuration's build instance was not detected.
    TargetMissing,
    /// The model failed to load again; show its error.
    ModelFailed,
    /// Run the configuration.
    Run,
    /// Debug the configuration.
    Debug,
    /// The configuration's run declares no debug adapter.
    NotDebuggable,
}

/// Decides what one press does.
pub fn decide_title_action(request: TitleActionRequest) -> TitleDecision {
    if request.current_file_chosen {
        return TitleDecision::CurrentFile;
    }
    if !request.settled {
        return TitleDecision::Wait;
    }
    let Some(configuration) = request.configuration else {
        return TitleDecision::CurrentFile;
    };
    match configuration {
        TitleConfiguration::Unsupported => TitleDecision::Unsupported,
        TitleConfiguration::InstanceMissing => TitleDecision::TargetMissing,
        TitleConfiguration::ModelNotLoaded { failed: true }
            if request.trigger == TitleTrigger::Settled =>
        {
            TitleDecision::ModelFailed
        }
        TitleConfiguration::ModelNotLoaded { .. } => TitleDecision::LoadModel,
        TitleConfiguration::ModelLoaded { debuggable } => match request.action {
            TitleAction::Run => TitleDecision::Run,
            TitleAction::Debug if debuggable => TitleDecision::Debug,
            TitleAction::Debug => TitleDecision::NotDebuggable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    // Every case is one request and the decision Core must return for it.
    #[test]
    fn title_actions_match_the_shared_fixture() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../fixtures/run-configuration/title-action-v1.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let request: TitleActionRequest =
                serde_json::from_value(case["request"].clone()).unwrap();
            assert_eq!(
                serde_json::to_value(decide_title_action(request)).unwrap(),
                case["expected"],
                "{name}"
            );
        }
    }

    #[test]
    fn unknown_facts_are_rejected() {
        let request = serde_json::json!({
            "action": "run",
            "trigger": "press",
            "currentFileChosen": false,
            "settled": true,
            "retry": true
        });
        assert!(serde_json::from_value::<TitleActionRequest>(request).is_err());
    }
}
