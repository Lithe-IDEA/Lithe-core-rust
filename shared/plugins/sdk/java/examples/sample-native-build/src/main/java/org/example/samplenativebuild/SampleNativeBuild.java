package org.example.samplenativebuild;

import io.github.litheidea.plugin.sdk.Backend;
import io.github.litheidea.plugin.sdk.CallContext;
import io.github.litheidea.plugin.sdk.Json;
import io.github.litheidea.plugin.sdk.PluginError;
import io.github.litheidea.plugin.sdk.Processes;
import java.io.IOException;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.ArrayList;
import java.util.Collections;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/** Fictional build engine demonstrating native workflows without a plugin page. */
public final class SampleNativeBuild {
    private final Backend backend = Backend.create("sample-native-build", null);
    private volatile Path workspace;

    public static void main(String[] arguments) { new SampleNativeBuild().serve(); }

    private void serve() {
        backend.onInitialize((params, negotiated) -> workspace = Paths.get(URI.create(Json.string(params, "rootUri"))).toRealPath());
        backend.onModel((params, context) -> {
            List<Object> model = new ArrayList<>();
            List<Object> targets = new ArrayList<>();
            for (Map<String, Object> project : projects(Json.string(params, "root"))) {
                if (project.get("run") instanceof Map) targets.add(object("name", "Main", "path", project.get("modelPath"),
                        "project", project.get("modelPath"), "tags", Collections.singletonList("application")));
                Map<String, Object> row = object("name", project.get("name"), "path", project.get("modelPath"));
                if (params.get("modelInputs") instanceof Map) {
                    Map<String, Object> inputs = Json.object(params.get("modelInputs"));
                    int count = 0;
                    for (Object slot : (List<?>) inputs.get("inputs")) {
                        for (Object input : (List<?>) Json.object(slot).get("instances")) {
                            Map<String, Object> instance = Json.object(input);
                            if ("ready".equals(instance.get("state"))) count += ((List<?>) Json.object(instance.get("model")).get("projects")).size();
                        }
                    }
                    row.put("properties", object("inputRevision", String.valueOf(inputs.get("revision")), "inputProjects", String.valueOf(count)));
                }
                model.add(row);
            }
            return object("projects", model, "targets", targets);
        });
        backend.onDependencies((params, context) -> object("dependencies", Collections.emptyList()));
        backend.onExecute((params, context) -> {
            if (!"build".equals(params.get("task"))) throw new PluginError("sample.task", "Unknown build task.");
            return build(select(params), strings(params.get("userArguments")), context);
        });
        backend.onDiscoverTests((params, context) -> {
            List<Object> discovered = new ArrayList<>();
            for (Map<String, Object> project : select(params)) {
                for (Map<String, Object> test : tests(project)) discovered.add(object("suite", test.get("suite"), "case", test.get("case"),
                        "project", object("name", project.get("name"), "path", project.get("path"))));
            }
            return object("tests", discovered);
        });
        backend.onTest(this::test);
        backend.onPrepareRun((params, context) -> prepare(params, context, false));
        backend.onPrepareDebug((params, context) -> prepare(params, context, true));
        backend.listen();
    }

    private Object prepare(Map<String, Object> params, CallContext context, boolean debug) throws IOException {
            List<Map<String, Object>> selected = select(params);
            Map<String, Object> target = Json.object(params.get("target"));
            if (selected.size() != 1 || !"application".equals(params.get("run"))) throw new PluginError("sample.target", "Select one application.");
            Map<String, Object> project = selected.get(0);
            if (!(project.get("run") instanceof Map) || !"Main".equals(target.get("name")) || !project.get("path").equals(target.get("path"))) {
                throw new PluginError("sample.target", "The selected application no longer exists.");
            }
            Map<String, Object> built = build(selected, Collections.emptyList(), context);
            if (((Number) built.get("exitCode")).intValue() != 0) throw new PluginError("sample.buildFailed", "Build failed before launch.");
            Map<String, Object> run = Json.object(project.get("run"));
            if (debug) {
                Object program = run.get("debugProgram");
                if (!(program instanceof String)) throw new PluginError("sample.target", "This application has no native debug program.");
                String path = project.get("path").equals(".") ? (String) program : project.get("path") + "/" + program;
                return object("program", path, "arguments", strings(params.get("userArguments")),
                        "workingDirectory", project.get("path"), "environment", object());
            }
            List<String> arguments = strings(run.get("arguments"));
            arguments.addAll(strings(params.get("userArguments")));
            return object("executable", object("kind", "command", "value", run.get("command")), "arguments", arguments,
                    "workingDirectory", project.get("path"), "environment", object());
    }

    private Map<String, Object> build(List<Map<String, Object>> projects, List<String> arguments, CallContext context) throws IOException {
        StringBuilder output = new StringBuilder();
        for (Map<String, Object> project : projects) {
            context.progress("Building " + project.get("name"), null);
            Processes.Result result = run(project.get("build"), project, arguments, context);
            output.append(result.stdout()).append(result.stderr());
            if (result.exitCode() != 0) return object("exitCode", result.exitCode(), "output", output.toString());
        }
        return object("exitCode", 0, "output", output.toString());
    }

    private Object test(Map<String, Object> params, CallContext context) throws IOException {
        int matched = 0;
        StringBuilder output = new StringBuilder();
        for (Map<String, Object> project : select(params)) {
            for (Map<String, Object> test : tests(project)) {
                if (params.containsKey("suite") && !params.get("suite").equals(test.get("suite"))) continue;
                if (params.containsKey("case") && !params.get("case").equals(test.get("case"))) continue;
                matched++;
                Processes.Result result = run(test.get("command"), project, strings(params.get("userArguments")), context);
                output.append(result.stdout()).append(result.stderr());
                if (result.exitCode() != 0) return object("exitCode", result.exitCode(), "output", output.toString());
            }
        }
        if (matched == 0 && (params.containsKey("suite") || params.containsKey("case"))) throw new PluginError("sample.test", "The selected test no longer exists.");
        return object("exitCode", 0, "output", output.toString());
    }

    private Processes.Result run(Object declared, Map<String, Object> project, List<String> arguments, CallContext context) throws IOException {
        context.throwIfCancelled();
        List<String> command = strings(declared);
        if (command.isEmpty()) throw new PluginError("sample.command", "A tool command must be a nonempty argument array.");
        command.addAll(arguments);
        Processes.Result result = context.run(command, new Processes.Options().directory(inside(Json.string(project, "path")))
                .onStdout(output -> context.progress(null, null, output))
                .onStderr(output -> context.progress(null, null, output)));
        context.throwIfCancelled();
        return result;
    }

    private List<Map<String, Object>> select(Map<String, Object> params) throws IOException {
        List<Map<String, Object>> all = projects(Json.string(params, "root"));
        if (!(params.get("project") instanceof Map)) return all;
        Map<String, Object> selected = Json.object(params.get("project"));
        List<Map<String, Object>> matches = new ArrayList<>();
        for (Map<String, Object> project : all) {
            if (project.get("name").equals(selected.get("name")) && project.get("path").equals(selected.get("path"))) matches.add(project);
        }
        if (matches.size() != 1) throw new PluginError("sample.project", "The selected project no longer exists.");
        return matches;
    }

    private List<Map<String, Object>> projects(String root) throws IOException {
        Path base = inside(root);
        String contents = new String(Files.readAllBytes(base.resolve("lithe-sample.json")), StandardCharsets.UTF_8);
        List<Map<String, Object>> result = new ArrayList<>();
        for (Map<String, Object> raw : objects(Json.object(Json.parse(contents)).get("projects"))) {
            String declared = Json.string(raw, "path");
            if (declared == null || Paths.get(declared).isAbsolute()) throw new PluginError("sample.path", "A project path must be relative to its instance.");
            Path directory = base.resolve(declared).toRealPath();
            if (!directory.startsWith(base)) throw new PluginError("sample.path", "A project is outside its instance.");
            // The model is instance-relative; later request selectors are workspace-relative.
            String modelPath = base.relativize(directory).toString().replace(java.io.File.separatorChar, '/');
            if (modelPath.isEmpty()) modelPath = ".";
            Map<String, Object> project = new LinkedHashMap<>(raw);
            project.put("modelPath", modelPath);
            String workspacePath = Paths.get(root).resolve(modelPath).normalize().toString().replace(java.io.File.separatorChar, '/');
            project.put("path", workspacePath.isEmpty() ? "." : workspacePath);
            result.add(project);
        }
        return result;
    }

    private List<Map<String, Object>> tests(Map<String, Object> project) {
        return project.containsKey("tests") ? objects(project.get("tests")) : Collections.emptyList();
    }

    private Path inside(String relative) throws IOException {
        if (relative == null || Paths.get(relative).isAbsolute()) throw new PluginError("sample.path", "A workspace-relative directory is required.");
        Path path = workspace.resolve(relative).toRealPath();
        if (!path.startsWith(workspace)) throw new PluginError("sample.path", "The directory is outside the workspace.");
        return path;
    }

    private static List<String> strings(Object value) {
        if (!(value instanceof List)) throw new PluginError("sample.config", "A string array is required.");
        List<String> result = new ArrayList<>();
        for (Object part : (List<?>) value) {
            if (!(part instanceof String)) throw new PluginError("sample.config", "A string array is required.");
            result.add((String) part);
        }
        return result;
    }

    private static List<Map<String, Object>> objects(Object value) {
        if (!(value instanceof List)) throw new PluginError("sample.config", "A list of objects is required.");
        List<Map<String, Object>> result = new ArrayList<>();
        for (Object item : (List<?>) value) result.add(Json.object(item));
        return result;
    }

    private static Map<String, Object> object(Object... entries) {
        Map<String, Object> result = new LinkedHashMap<>();
        for (int index = 0; index < entries.length; index += 2) result.put((String) entries[index], entries[index + 1]);
        return result;
    }
}
