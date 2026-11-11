//! Real SDK processes answer native Core requests without a plugin page or a
//! private function. The fictional engine owns all build/test/run facts.
use super::{core, listing, Fixture, Sample, Session, BUILD_SYSTEM, WAIT};
use serde_json::{json, Value};
use std::fs;
use std::time::Instant;

pub fn run(sample: &Sample) {
    let manifest: Value =
        serde_json::from_slice(&fs::read(sample.package.join("plugin.json")).unwrap()).unwrap();
    let before = listing(&sample.package);
    let validation = core("plugins.declarative.validate", json!({"manifest":manifest}));
    assert_eq!(validation["status"], "ok", "{validation}");
    let fixture = Fixture::new(&format!("native-{}", sample.tool));
    let workspace = fixture.workspace();
    let root = "nested";
    let instance = workspace.join(root);
    fs::create_dir_all(instance.join("app")).unwrap();
    fs::create_dir_all(instance.join("broken")).unwrap();
    fs::create_dir_all(instance.join("slow")).unwrap();
    let config = json!({"projects":[
        {"name":"app","path":"app","build":[sample.node,"-e","const fs=require('fs'); const n=fs.existsSync('build-count')?Number(fs.readFileSync('build-count','utf8')):0; fs.writeFileSync('build-count',String(n+1)); console.log('native build')"],
         "tests":[
            {"suite":"Suite","case":"exact # [case]","command":[sample.node,"-e","console.log(JSON.stringify(process.argv.slice(1)))"]},
            {"suite":"Suite","case":"other","command":[sample.node,"-e","process.exit(7)"]}],
         "run":{"command":"node","arguments":["-e","console.log(JSON.stringify(process.argv.slice(1)))"],"debugProgram":"native-program"}},
        {"name":"broken","path":"broken","build":[sample.node,"-e","process.exit(2)"],
         "run":{"command":"node","arguments":["-e","process.exit(99)"]}},
        {"name":"slow","path":"slow","build":[sample.node,"-e","require('fs').writeFileSync('started', String(process.pid)); setInterval(()=>{}, 1000)"],"run":{"command":"node","arguments":[],"debugProgram":"native-program"}}
    ]});
    fs::write(instance.join("lithe-sample.json"), config.to_string()).unwrap();
    fs::write(
        instance.join("app/native-program"),
        "native debugger target fixture",
    )
    .unwrap();
    let launch = core(
        "plugins.declarative.languageServerLaunchPlan",
        json!({
        "manifest":manifest,"buildSystemID":BUILD_SYSTEM,"request":{
            "pluginDirectory":sample.package,"stateDirectory":fixture.root.join("state"),
            "workspaceStateDirectory":fixture.root.join("workspace-state"),
            "architecture":if cfg!(target_arch="aarch64") {"arm64"} else {"x86_64"}
        }}),
    );
    assert_eq!(launch["status"], "ok", "{launch}");
    let arguments = super::host_launch_arguments(&launch["plan"], &sample.runtime);
    let mut session = Session::start(
        manifest["id"].as_str().unwrap(),
        &sample.runtime,
        &arguments,
        &workspace,
    );
    assert!(
        session.functions().is_empty(),
        "native workflows must not need private functions"
    );
    let build = |command: &str, request: Value| {
        core(
            command,
            json!({"manifest":manifest,"buildSystemID":BUILD_SYSTEM,"request":request}),
        )
    };
    let app = json!({"name":"app","path":"nested/app"});
    let plan = build("build.modelPlan", json!({"root":root}))["plan"].clone();
    let result = session.build_request("model", &plan);
    let model = build(
        "build.decodeModel",
        json!({"workspaceRoot":workspace,"root":root,"result":result}),
    );
    assert_eq!(model["model"]["targets"][0]["name"], "Main", "{model}");
    let target = &model["model"]["targets"][0];
    assert_eq!(target["path"], "nested/app");
    assert_eq!(target["projectID"], model["model"]["projects"][0]["id"]);

    // A second real backend owns the producer model. Host/Core project its facts
    // before the consumer receives them; neither process calls the other.
    let mut producer = manifest.clone();
    producer["id"] = json!(format!("{}.source", manifest["id"].as_str().unwrap()));
    producer["requires"]["buildModelInputs"] = json!(0);
    producer["buildSystems"][0]["id"] = json!("shared-source");
    producer["activationEvents"] = json!(["onBuildSystem:shared-source", "onDebug:native"]);
    producer["buildSystems"][0]["modelSharing"] = json!({});
    let mut source_session = Session::start(
        producer["id"].as_str().unwrap(),
        &sample.runtime,
        &arguments,
        &workspace,
    );
    let source_plan = core(
        "build.modelPlan",
        json!({"manifest":producer,"buildSystemID":"shared-source","request":{"root":root}}),
    );
    assert_eq!(source_plan["status"], "ok", "{source_plan}");
    let source_reply = source_session.build_request("source-model", &source_plan["plan"]);
    let source_model = core(
        "build.decodeModel",
        json!({"manifest":producer,"buildSystemID":"shared-source",
        "request":{"workspaceRoot":workspace,"root":root,"result":source_reply}}),
    );
    assert_eq!(source_model["status"], "ok", "{source_model}");
    let mut consumer = manifest.clone();
    consumer["requires"]["buildModelInputs"] = json!(0);
    consumer["buildSystems"][0]["modelInputs"] =
        json!([{"pluginID":producer["id"],"buildSystemID":"shared-source"}]);
    let resolved = core(
        "build.resolveModelInputs",
        json!({"manifest":consumer,"buildSystemID":BUILD_SYSTEM,"request":{
        "revision":19,"sources":[{"manifest":producer,"instances":[{
            "buildSystemID":"shared-source","root":root,"state":"ready","model":source_model["model"]}]}]}}),
    );
    assert_eq!(resolved["status"], "ok", "{resolved}");
    let snapshot = &resolved["modelInputs"];
    assert!(snapshot["inputs"][0]["instances"][0]["model"]
        .get("tasks")
        .is_none());
    assert!(snapshot["inputs"][0]["instances"][0]["model"]
        .get("run")
        .is_none());
    let input_plan = core(
        "build.modelPlan",
        json!({"manifest":consumer,"buildSystemID":BUILD_SYSTEM,
        "request":{"root":root,"modelInputs":snapshot}}),
    );
    assert_eq!(input_plan["status"], "ok", "{input_plan}");
    let input_reply = session.build_request("consumer-model", &input_plan["plan"]);
    let input_model = core(
        "build.decodeModel",
        json!({"manifest":consumer,"buildSystemID":BUILD_SYSTEM,
        "request":{"workspaceRoot":workspace,"root":root,"result":input_reply}}),
    );
    assert_eq!(
        input_model["model"]["projects"][0]["properties"]["inputRevision"], "19",
        "{input_model}"
    );
    assert_eq!(
        input_model["model"]["projects"][0]["properties"]["inputProjects"], "3",
        "{input_model}"
    );
    drop(source_session);

    for (project, exit) in [
        (app.clone(), 0),
        (json!({"name":"broken","path":"nested/broken"}), 2),
    ] {
        let plan = build(
            "build.taskPlan",
            json!({"root":root,"task":"build","project":project}),
        )["plan"]
            .clone();
        let result = session.build_request(&format!("build-{exit}"), &plan);
        assert_eq!(result["exitCode"], exit, "{result}");
    }
    assert_eq!(
        fs::read_to_string(instance.join("app/build-count")).unwrap(),
        "1"
    );
    let discovery_plan = build("build.testDiscoveryPlan", json!({"root":root}))["plan"].clone();
    let discovery = session.build_request("discover", &discovery_plan);
    let decoded = build(
        "build.decodeTestDiscovery",
        json!({"root":root,"result":discovery}),
    );
    let tests = decoded["discovery"]["tests"]
        .as_array()
        .expect("decoded native test selectors");
    assert_eq!(tests.len(), 2);
    let selected = tests
        .iter()
        .find(|test| test["case"] == "exact # [case]")
        .unwrap();
    let request = json!({"root":root,"project":selected["project"],"suite":selected["suite"],
        "case":selected["case"],"userArguments":["a b","x#y"]});
    let test_plan = build("build.testPlan", request)["plan"].clone();
    let tested = session.build_request("test-exact", &test_plan);
    assert_eq!(tested["exitCode"], 0, "{tested}");
    assert_eq!(tested["output"], "[\"a b\",\"x#y\"]\n");

    // An externally removed selector cannot silently widen to all tests.
    let mut changed = config.clone();
    changed["projects"][0]["tests"] = json!([]);
    fs::write(instance.join("lithe-sample.json"), changed.to_string()).unwrap();
    session.send_build_request("stale-test", &test_plan);
    assert_eq!(
        session.completion("stale-test")["error"]["code"],
        "plugin.build.failed"
    );
    fs::write(instance.join("lithe-sample.json"), config.to_string()).unwrap();

    let prepare = build("build.prepareRunPlan", json!({"root":root,"run":"application","project":app,
        "target":{"name":target["name"],"path":target["path"],"tags":target["tags"]},"userArguments":["a b","x#y"]}))["plan"].clone();
    let result = session.build_request("prepare", &prepare);
    let decoded = build(
        "build.decodeRunPreparation",
        json!({"root":root,"workspaceRoot":workspace,"result":result}),
    );
    assert_eq!(decoded["status"], "ok", "{decoded}");
    assert_eq!(decoded["launch"]["workingDirectory"], "nested/app");
    assert_eq!(
        fs::read_to_string(instance.join("app/build-count")).unwrap(),
        "2",
        "preparation skipped the upstream build"
    );
    assert_eq!(
        decoded["launch"]["executable"],
        json!({"kind":"command","value":"node"})
    );
    assert_eq!(
        decoded["launch"]["arguments"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .take(2)
            .cloned()
            .collect::<Vec<_>>(),
        json!(["x#y", "a b"]).as_array().unwrap().clone()
    );
    assert!(
        session
            .events
            .iter()
            .any(|event| event["operationId"] == "prepare" && event["type"] == "requestProgress"),
        "preparation must report its own progress"
    );
    let debug = build("build.prepareDebugPlan", json!({"root":root,"run":"application","project":app,
        "target":{"name":target["name"],"path":target["path"],"tags":target["tags"]},"userArguments":["a b","x#y"]}))["plan"].clone();
    assert_eq!(debug["provider"]["method"], "lithe/build/prepareDebug");
    let result = session.build_request("prepare-debug", &debug);
    let decoded_debug = build(
        "build.decodeDebugPreparation",
        json!({"root":root,"workspaceRoot":workspace,"run":"application","result":result}),
    );
    assert_eq!(decoded_debug["status"], "ok", "{decoded_debug}");
    assert_eq!(decoded_debug["preparation"]["adapter"], "native");
    assert_eq!(
        decoded_debug["preparation"]["program"],
        "nested/app/native-program"
    );
    assert_eq!(
        decoded_debug["preparation"]["arguments"],
        json!(["a b", "x#y"])
    );
    assert_eq!(
        fs::read_to_string(instance.join("app/build-count")).unwrap(),
        "3",
        "debug preparation skipped the upstream build"
    );
    assert!(
        session
            .events
            .iter()
            .any(|event| event["operationId"] == "prepare-debug"
                && event["type"] == "requestProgress")
    );
    let failed = build(
        "build.prepareRunPlan",
        json!({"root":root,"run":"application",
        "project":{"name":"broken","path":"nested/broken"},
        "target":{"name":"Main","path":"nested/broken","tags":["application"]}}),
    )["plan"]
        .clone();
    session.send_build_request("failed-preparation", &failed);
    let failed = session.completion("failed-preparation");
    // Preparation retains the common LSP error category, rather than an
    // execution exit-code result or a private-function pluginErrorCode.
    assert_eq!(failed["error"]["code"], "serverError");
    assert!(
        failed.get("result").is_none(),
        "a failed build returned a launch plan"
    );

    let failed_debug = build("build.prepareDebugPlan", json!({"root":root,"run":"application",
        "project":{"name":"broken","path":"nested/broken"},"target":{"name":"Main","path":"nested/broken","tags":["application"]}}))["plan"].clone();
    session.send_build_request("failed-debug-preparation", &failed_debug);
    let failed_debug = session.completion("failed-debug-preparation");
    assert_eq!(failed_debug["error"]["code"], "serverError");
    assert!(failed_debug.get("result").is_none());

    // The second workspace uses the same plugin with different current facts.
    // Cancelling one instance must leave the other process and model usable.
    let other_workspace = fixture.root.join("other");
    fs::create_dir_all(&other_workspace).unwrap();
    fs::write(other_workspace.join("lithe-sample.json"), json!({"projects":[{"name":"other","path":".","build":[sample.node,"-e","console.log('other workspace')"]}]}).to_string()).unwrap();
    let mut other = Session::start(
        manifest["id"].as_str().unwrap(),
        &sample.runtime,
        &arguments,
        &other_workspace,
    );
    for (operation, command, request) in [
        (
            "slow-build",
            "build.taskPlan",
            json!({"root":root,"task":"build","project":{"name":"slow","path":"nested/slow"}}),
        ),
        (
            "slow-debug",
            "build.prepareDebugPlan",
            json!({"root":root,"run":"application","project":{"name":"slow","path":"nested/slow"},
            "target":{"name":"Main","path":"nested/slow","tags":["application"]}}),
        ),
    ] {
        let _ = fs::remove_file(instance.join("slow/started"));
        let slow = build(command, request)["plan"].clone();
        session.send_build_request(operation, &slow);
        let deadline = Instant::now() + WAIT;
        let pid = loop {
            if let Some(pid) = fs::read_to_string(instance.join("slow/started"))
                .ok()
                .and_then(|text| text.trim().parse::<u32>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "native tool never started");
            let events = core(
                "lsp.waitEvents",
                json!({"sessionId":session.id,"timeoutMilliseconds":50}),
            );
            session
                .events
                .extend(events["events"].as_array().cloned().unwrap_or_default());
        };
        core(
            "lsp.cancelOperation",
            json!({"sessionId":session.id,"operationId":operation}),
        );
        assert_eq!(
            session.completion(operation)["error"]["code"],
            "requestCancelled"
        );
        #[cfg(unix)]
        {
            let deadline = Instant::now() + WAIT;
            while super::is_alive(pid) {
                assert!(
                    Instant::now() < deadline,
                    "cancelled native tool {pid} survived"
                );
                let events = core(
                    "lsp.waitEvents",
                    json!({"sessionId":session.id,"timeoutMilliseconds":50}),
                );
                session
                    .events
                    .extend(events["events"].as_array().cloned().unwrap_or_default());
            }
        }
        #[cfg(not(unix))]
        let _ = pid;
    }
    let plan = build(
        "build.taskPlan",
        json!({"root":".","task":"build","project":{"name":"other","path":"."}}),
    )["plan"]
        .clone();
    assert_eq!(
        other.build_request("other-build", &plan)["output"],
        "other workspace\n"
    );
    other.stop();
    session.stop();
    drop(other);
    drop(session);
    assert_eq!(
        listing(&sample.package),
        before,
        "native workflows changed the plugin package"
    );
}
