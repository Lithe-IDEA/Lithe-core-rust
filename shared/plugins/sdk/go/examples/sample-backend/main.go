// Sample plugin backend for the Go SDK: the `backend` process of the sample
// plugin in this directory. Lithe starts it when the plugin's web page first
// calls a function or the user runs its command, with `--state ${stateDir}`.
// It:
//
//   - keeps notes in its state directory, with progress (note.save,
//     note.read), and registers note.clear once there is a note to clear;
//   - keeps a secret in the host's credential store, never in its state
//     (secret.remember, secret.check, secret.forget);
//   - waits until it is cancelled (work.wait), and fails unexpectedly without
//     leaking details to the page (fail.unexpectedly);
//   - watches the files of the page's workspace through the host
//     (files.nextChange), answers its command with a message in the window it
//     ran in, and lists the open workspaces when they change.
//
// Build it with `go build -o bin/sample-backend ./examples/sample-backend`
// from the SDK directory.
package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

const (
	// version is the version plugin.json declares.
	version = "0.1.0"
	// greet is the command plugin.json declares.
	greet = "org.example.sample-backend.greet"
	watch = "lithe/host/workspace/watch"
)

func main() {
	state, err := stateDirectory(os.Args[1:])
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(2)
	}
	backend := lithe.NewBackend("sample-backend").Version(version)

	// Exercised only when the Core end-to-end harness offers this provider.
	must(backend.KeymapImportProvider("fixture", func(request lithe.KeymapImportRequest, call *lithe.Context) (any, error) {
		files, err := request.ListInputs(call.Host())
		if err != nil {
			return nil, err
		}
		if len(files) == 0 {
			return nil, errors.New("no fixture source")
		}
		data, err := request.ReadChunk(call.Host(), files[0].ID, 0, 4)
		if err != nil {
			return nil, err
		}
		if string(data) != "data" {
			return nil, errors.New("unexpected fixture source")
		}
		return map[string]any{"bindings": []any{map[string]any{
			"commandID": "find-next",
			"keys":      []any{map[string]any{"key": "f3", "modifiers": []any{}}},
		}}}, nil
	}))

	must(backend.Function("note.save", func(params json.RawMessage, call *lithe.Context) (any, error) {
		var input struct {
			Text *string `json:"text"`
		}
		_ = json.Unmarshal(params, &input)
		name, err := noteName(params)
		if err != nil {
			return nil, err
		}
		if input.Text == nil {
			return nil, lithe.NewPluginError("sample.invalidParams", `"text" must be a string.`)
		}
		call.ProgressPercent("Saving", 0)
		directory := filepath.Join(state, "notes")
		if err := os.MkdirAll(directory, 0o755); err != nil {
			return nil, err
		}
		// Written beside the note and renamed, so a reader never sees half a note.
		temporary := filepath.Join(directory, "."+name+".tmp")
		if err := os.WriteFile(temporary, []byte(*input.Text), 0o644); err != nil {
			return nil, err
		}
		if err := os.Rename(temporary, filepath.Join(directory, name+".txt")); err != nil {
			return nil, err
		}
		call.ProgressPercent("Saved", 100)
		offerClear(call.Backend(), state)
		return map[string]any{"name": name, "bytes": len(*input.Text)}, nil
	}))

	must(backend.Function("note.read", func(params json.RawMessage, _ *lithe.Context) (any, error) {
		name, err := noteName(params)
		if err != nil {
			return nil, err
		}
		text, err := os.ReadFile(filepath.Join(state, "notes", name+".txt"))
		if errors.Is(err, fs.ErrNotExist) {
			return nil, lithe.NewPluginError("sample.noteMissing", fmt.Sprintf("There is no note named %s.", name))
		}
		if err != nil {
			return nil, err
		}
		return map[string]any{"text": string(text)}, nil
	}))

	must(backend.Function("secret.remember", func(params json.RawMessage, call *lithe.Context) (any, error) {
		key, secret, err := credential(params)
		if err != nil {
			return nil, err
		}
		if err := call.Credentials().Set(key, secret); err != nil {
			return nil, credentialsFailure(err)
		}
		return map[string]any{}, nil
	}))

	must(backend.Function("secret.check", func(params json.RawMessage, call *lithe.Context) (any, error) {
		key, secret, err := credential(params)
		if err != nil {
			return nil, err
		}
		stored, present, err := call.Credentials().Get(key)
		if err != nil {
			return nil, credentialsFailure(err)
		}
		// The page learns whether its input matches, never the secret.
		return map[string]any{"stored": present, "matches": present && stored == secret}, nil
	}))

	must(backend.Function("secret.forget", func(params json.RawMessage, call *lithe.Context) (any, error) {
		key, err := credentialKey(params)
		if err != nil {
			return nil, err
		}
		if err := call.Credentials().Delete(key); err != nil {
			return nil, credentialsFailure(err)
		}
		return nil, nil
	}))

	must(backend.Function("work.wait", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		if err := os.WriteFile(filepath.Join(state, "waiting"), nil, 0o644); err != nil {
			return nil, err
		}
		// Blocks without spinning until the host cancels the call.
		if call.Cancellation().WaitTimeout(600 * time.Second) {
			if err := os.WriteFile(filepath.Join(state, "cancelled"), nil, 0o644); err != nil {
				return nil, err
			}
			return nil, lithe.ErrCancelled
		}
		return "finished", nil
	}))

	must(backend.Function("fail.unexpectedly", func(json.RawMessage, *lithe.Context) (any, error) {
		return nil, fmt.Errorf("could not open %s", filepath.Join(state, "missing"))
	}))

	must(backend.Function("files.nextChange", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		return nextChange(call)
	}))

	must(backend.Function("workspace.processRoots", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		context := call.WorkspaceContext()
		if context == nil {
			return nil, lithe.NewPluginError("sample.noWorkspace", "Open a workspace first.")
		}
		workspace := call.Host().Workspace()
		full, err := workspace.GetContext(*context)
		if err != nil {
			return nil, err
		}
		return workspace.ProcessRoots(full)
	}))

	// The terminal API is a host service. The lifecycle sample deliberately
	// uses a harmless interactive client and display-only history; it never
	// executes retained text as input.
	must(backend.Function("terminal.cycle", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		return terminalCycle(call)
	}))

	// Notifications are events: each handler runs on a goroutine of its own
	// and may ask the host for more, outside any call.
	handle := backend.Handle()
	must(backend.OnCommand(greet, func(_ string, context *lithe.WorkspaceContext) error {
		if context == nil {
			return nil
		}
		host := handle.Host()
		answer, err := host.Workspace().GetFolders(*context)
		if err != nil {
			return err
		}
		var folders struct {
			Folders []struct {
				Name string `json:"name"`
			} `json:"folders"`
		}
		_ = json.Unmarshal(answer, &folders)
		var names []string
		for _, folder := range folders.Folders {
			if folder.Name != "" {
				names = append(names, folder.Name)
			}
		}
		text := "Sample Backend sees no folders."
		if len(names) > 0 {
			text = fmt.Sprintf("Sample Backend sees %s.", strings.Join(names, ", "))
		}
		_, err = host.Window().ShowInformation(*context, map[string]any{"text": text})
		return err
	}))

	// A backend that keeps state per workspace would refresh it here.
	backend.OnContextsChanged(func(json.RawMessage) error {
		_, err := handle.Host().Workspace().ListContexts()
		return err
	})

	backend.OnInitialize(func(initialize *lithe.Initialize) error {
		if hasNotes(state) {
			return initialize.Function("note.clear", func(_ json.RawMessage, call *lithe.Context) (any, error) {
				return clearNotes(state, call)
			})
		}
		return nil
	})

	os.Exit(backend.Listen())
}

func stateDirectory(arguments []string) (string, error) {
	if len(arguments) == 2 && arguments[0] == "--state" {
		return arguments[1], nil
	}
	return "", errors.New("usage: sample-backend --state <directory>")
}

// must stops the sample on a registration its own code got wrong.
func must(err error) {
	if err != nil {
		panic(err)
	}
}

// offerClear registers note.clear the first time there is a note to clear.
func offerClear(backend *lithe.BackendHandle, state string) {
	if slices.Contains(backend.FunctionIDs(), "note.clear") {
		return
	}
	err := backend.RegisterFunction("note.clear", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		return clearNotes(state, call)
	})
	if err != nil {
		degraded(backend.Diagnostics(), "initialize", "registration_failed")
	}
}

func clearNotes(state string, call *lithe.Context) (any, error) {
	if err := call.CheckCancelled(); err != nil {
		return nil, err
	}
	// RemoveAll succeeds when there is nothing to remove.
	if err := os.RemoveAll(filepath.Join(state, "notes")); err != nil {
		return nil, err
	}
	return nil, nil
}

// nextChange waits for the next change under the first folder of the page's
// workspace, through a host file watch.
func nextChange(call *lithe.Context) (any, error) {
	workspace := call.WorkspaceContext()
	if workspace == nil {
		return nil, lithe.NewPluginError("sample.noWorkspace", "Open a workspace first.")
	}
	host := call.Host()
	// Core answers capability discovery itself, from the plugin's permissions.
	supported, err := host.Supports(watch)
	if err != nil {
		return nil, err
	}
	if !supported {
		return nil, lithe.NewPluginError("sample.watchUnavailable", "Lithe does not let this plugin watch files.")
	}
	answer, err := host.Workspace().GetFolders(*workspace)
	if err != nil {
		return nil, workspaceFailure(err)
	}
	var folders struct {
		Folders []struct {
			RootID string `json:"rootID"`
		} `json:"folders"`
	}
	_ = json.Unmarshal(answer, &folders)
	if len(folders.Folders) == 0 || folders.Folders[0].RootID == "" {
		return nil, lithe.NewPluginError("sample.noWorkspace", "The workspace has no folders.")
	}
	subscription, err := host.Workspace().Watch(*workspace, map[string]any{
		"resource":  map[string]any{"rootID": folders.Folders[0].RootID, "path": ""},
		"recursive": true,
	})
	if err != nil {
		return nil, workspaceFailure(err)
	}
	result, err := firstChange(call, subscription)
	// A subscription outlives the call until it is disposed.
	if subscription.Dispose() != nil {
		degraded(call.Diagnostics(), "files.nextChange", "subscription_dispose_failed")
	}
	return result, err
}

func firstChange(call *lithe.Context, subscription *lithe.Subscription) (any, error) {
	for {
		if call.Cancellation().IsCancelled() {
			return nil, lithe.ErrCancelled
		}
		event, err := subscription.RecvTimeout(200 * time.Millisecond)
		if errors.Is(err, lithe.ErrRecvTimeout) {
			continue
		}
		if err != nil {
			return nil, errors.New("the watch ended")
		}
		var change struct {
			ChangedPaths json.RawMessage `json:"changedPaths"`
		}
		_ = json.Unmarshal(event.Event(), &change)
		return map[string]any{"changedPaths": change.ChangedPaths}, nil
	}
}

func terminalCycle(call *lithe.Context) (any, error) {
	workspace := call.WorkspaceContext()
	if workspace == nil {
		return nil, lithe.NewPluginError("sample.noWorkspace", "Open a workspace first.")
	}
	host := call.Host()
	supported, err := host.Supports("lithe/host/terminal/reopen")
	if err != nil {
		return nil, err
	}
	if !supported {
		return nil, lithe.NewPluginError("sample.terminalUnavailable", "The host has no native terminal API.")
	}
	terminal := host.Terminal()
	answer, err := terminal.Create(*workspace, map[string]any{
		"executable": "example-client", "arguments": []string{"--interactive"},
		"retainedContextBytes": 4096, "show": true,
	})
	if err != nil {
		return nil, err
	}
	var created struct {
		TerminalID string `json:"terminalID"`
	}
	_ = json.Unmarshal(answer, &created)
	if created.TerminalID == "" {
		return nil, errors.New("missing terminalID")
	}
	subscription, err := terminal.OnExit(*workspace, created.TerminalID)
	if err != nil {
		return nil, err
	}
	closed, err := terminal.Close(*workspace, created.TerminalID, 4096)
	if err != nil {
		return nil, err
	}
	event, err := subscription.RecvTimeout(5 * time.Second)
	if err != nil {
		return nil, errors.New("terminal exit did not arrive")
	}
	reopened, err := terminal.Reopen(*workspace, created.TerminalID)
	if err != nil {
		return nil, err
	}
	if _, err := terminal.Dispose(*workspace, created.TerminalID); err != nil {
		return nil, err
	}
	if err := subscription.Dispose(); err != nil {
		return nil, err
	}
	return map[string]any{"closed": closed, "reopened": reopened, "event": event.Event()}, nil
}

// workspaceFailure turns a stale context (plugin.host.contextExpired), left by
// a window that switched folders, into an error the page can recover from.
func workspaceFailure(err error) error {
	var sdk *lithe.SdkError
	if errors.As(err, &sdk) && sdk.Code() == "plugin.host.contextExpired" {
		return lithe.NewPluginError("sample.workspaceChanged", "The workspace changed; try again.")
	}
	return err
}

func hasNotes(state string) bool {
	entries, err := os.ReadDir(filepath.Join(state, "notes"))
	return err == nil && len(entries) > 0
}

// noteName is a file name the sample chooses the alphabet of, so a page can
// never name a path outside the notes directory.
func noteName(params json.RawMessage) (string, error) {
	var input struct {
		Name string `json:"name"`
	}
	_ = json.Unmarshal(params, &input)
	valid := len(input.Name) >= 1 && len(input.Name) <= 64
	for _, b := range []byte(input.Name) {
		if !(b >= 'a' && b <= 'z' || b >= '0' && b <= '9' || b == '-') {
			valid = false
		}
	}
	if !valid {
		return "", lithe.NewPluginError("sample.invalidParams", `"name" must be 1-64 lowercase letters, digits, or "-".`)
	}
	return input.Name, nil
}

func credentialKey(params json.RawMessage) (string, error) {
	var input struct {
		Key string `json:"key"`
	}
	_ = json.Unmarshal(params, &input)
	if !lithe.IsCredentialKey(input.Key) {
		return "", lithe.NewPluginError("sample.invalidParams", `"key" is not a credential key.`)
	}
	return input.Key, nil
}

func credential(params json.RawMessage) (key, secret string, err error) {
	if key, err = credentialKey(params); err != nil {
		return "", "", err
	}
	var input struct {
		Secret *string `json:"secret"`
	}
	_ = json.Unmarshal(params, &input)
	if input.Secret == nil {
		return "", "", lithe.NewPluginError("sample.invalidParams", `"secret" must be a string.`)
	}
	return key, *input.Secret, nil
}

// credentialsFailure explains a host that offers no credential store
// (plugin.host.unavailable), or a plugin installed without the
// credentials.store permission, to the page; other failures stay internal.
func credentialsFailure(err error) error {
	var sdk *lithe.SdkError
	if errors.As(err, &sdk) && strings.HasPrefix(sdk.Code(), "plugin.host.") {
		return lithe.NewPluginError("sample.credentialsUnavailable", "Lithe cannot store credentials for this plugin.")
	}
	return err
}

// degraded reports a fixed degradation; the values are valid, so a failure
// only means the session already ended.
func degraded(diagnostics *lithe.Diagnostics, operation, code string) {
	if err := diagnostics.Degraded(operation, code); err != nil {
		fmt.Fprintln(os.Stderr, err)
	}
}
