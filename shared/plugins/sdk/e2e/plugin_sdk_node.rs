//! End-to-end check of the official Node.js plugin SDK (`sdk/node`) against
//! Rust Core; the scenario is shared with the Java SDK in `plugin_sdk/mod.rs`.
//! The VCS scenario is in `plugin_sdk/vcs.rs`. Both run only when
//! `LITHE_PLUGIN_SDK_NODE` names a Node.js 20.3 or newer executable;
//! `shared/plugins/scripts/sdk-e2e.mjs` sets it.

mod plugin_sdk;

#[test]
fn the_node_sdk_import_provider_reads_a_selected_source_through_core() {
    let Some(node) = plugin_sdk::node_from_environment("the Node keymap provider") else {
        return;
    };
    let script = plugin_sdk::repository_path("sdk/node/examples/sample-build/backend/main.mjs");
    plugin_sdk::keymap::run(
        "node-keymap",
        &node,
        &[script.to_string_lossy().into_owned()],
    );
}

#[test]
fn the_node_sdk_sample_backend_serves_build_requests_and_plugin_functions() {
    let Some(node) = plugin_sdk::node_from_environment("the Node.js SDK end-to-end check") else {
        return;
    };
    plugin_sdk::run_sample_scenario(&plugin_sdk::Sample {
        tool: "node",
        runtime: node.clone(),
        package: plugin_sdk::repository_path("sdk/node/examples/sample-build"),
        node,
    });
}

#[test]
fn the_node_sdk_sample_vcs_provider_answers_every_operation() {
    let Some(node) = plugin_sdk::node_from_environment("the Node.js SDK VCS end-to-end check")
    else {
        return;
    };
    plugin_sdk::vcs::run_vcs_scenario(&node);
}
