//! Native build workflows run against the host contract of this checkout.
//! `shared/plugins/scripts/sdk-e2e.mjs` always runs them.
mod plugin_sdk;

#[test]
fn node_sdk_serves_native_build_test_and_run_workflows() {
    let node = plugin_sdk::node_from_environment("native Node build workflows")
        .expect("native workflow checks require Node");
    // Package the SDK next to the entry point, as a distributed plugin does.
    let package = plugin_sdk::TemporaryPackage::new(
        "node-native",
        &[(
            "plugin.json",
            plugin_sdk::repository_path("sdk/node/examples/sample-native-build/plugin.json"),
        )],
    );
    let backend = std::fs::read_to_string(plugin_sdk::repository_path(
        "sdk/node/examples/sample-native-build/backend/main.mjs",
    ))
    .unwrap();
    let packaged = backend.replace("../../../src/index.mjs", "./vendor/index.mjs");
    assert_ne!(
        packaged, backend,
        "the packaged entry must use its vendored SDK"
    );
    let vendor = package.root.join("backend/vendor");
    std::fs::create_dir_all(&vendor).unwrap();
    for entry in std::fs::read_dir(plugin_sdk::repository_path("sdk/node/src")).unwrap() {
        let entry = entry.unwrap();
        assert!(
            entry.file_type().unwrap().is_file(),
            "update packaging for SDK subdirectories"
        );
        std::fs::copy(entry.path(), vendor.join(entry.file_name())).unwrap();
    }
    std::fs::write(package.root.join("backend/main.mjs"), packaged).unwrap();
    plugin_sdk::native_build::run(&plugin_sdk::Sample {
        tool: "node",
        runtime: node.clone(),
        node,
        package: package.root.clone(),
    });
}

#[test]
fn java_sdk_serves_native_build_test_and_run_workflows() {
    let node = plugin_sdk::node_from_environment("native Java build workflows")
        .expect("native workflow checks require Node");
    let java = std::env::var("LITHE_PLUGIN_SDK_JAVA").expect("native workflow checks require Java");
    let jar = std::env::var_os("LITHE_PLUGIN_SDK_JAVA_NATIVE_SAMPLE")
        .expect("build the native Java sample first");
    let package = plugin_sdk::TemporaryPackage::new(
        "java-native",
        &[
            (
                "plugin.json",
                plugin_sdk::repository_path("sdk/java/examples/sample-native-build/plugin.json"),
            ),
            ("backend/sample-native-build.jar", jar.into()),
        ],
    );
    plugin_sdk::native_build::run(&plugin_sdk::Sample {
        tool: "java",
        runtime: java,
        node,
        package: package.root.clone(),
    });
}
