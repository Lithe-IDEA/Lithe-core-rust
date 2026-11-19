//! End-to-end check of the official Go plugin SDK (`sdk/go`) against Rust
//! Core, with the plugin backend scenario in `plugin_sdk/backend.rs` that the
//! Rust SDK sample also runs.
//!
//! The sample is installed the way a plugin ships: `plugin.json`, the
//! backend executable at its packaged path, and the web page that calls it.
//! It runs only when `LITHE_PLUGIN_SDK_GO_SAMPLE` names the built
//! `sample-backend` executable; `shared/plugins/scripts/sdk-e2e.mjs` sets it.

mod plugin_sdk;

#[test]
fn the_go_sdk_import_provider_reads_a_selected_source_through_core() {
    let Some(executable) = std::env::var_os("LITHE_PLUGIN_SDK_GO_SAMPLE") else {
        return;
    };
    plugin_sdk::keymap::run("go-keymap", &executable.to_string_lossy(), &[]);
}

#[test]
fn the_go_sdk_sample_backend_serves_functions_and_host_services() {
    let Some(executable) = std::env::var_os("LITHE_PLUGIN_SDK_GO_SAMPLE") else {
        eprintln!("skipped: set LITHE_PLUGIN_SDK_GO_SAMPLE to run the Go SDK end-to-end check");
        return;
    };
    let sample = |file: &str| {
        (
            file.to_string(),
            plugin_sdk::repository_path(&format!("sdk/go/examples/sample-backend/{file}")),
        )
    };
    let files = [
        sample("plugin.json"),
        sample("ui/index.html"),
        sample("ui/main.js"),
        sample("ui/style.css"),
        ("bin/sample-backend".to_string(), executable.into()),
    ];
    let files: Vec<_> = files
        .iter()
        .map(|(destination, source)| (destination.as_str(), source.clone()))
        .collect();
    let package = plugin_sdk::TemporaryPackage::new("go", &files);
    plugin_sdk::backend::run_backend_scenario(&package.root);
}
