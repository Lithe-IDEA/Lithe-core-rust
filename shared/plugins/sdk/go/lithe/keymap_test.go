package lithe_test

// Shortcut providers negotiate independently of plugin functions.

import (
	"encoding/json"
	"fmt"
	"testing"
	"time"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

func initializeKeymap(t *testing.T, host *testHost, experimental string) any {
	t.Helper()
	host.request(0, "initialize", j(t, `{"capabilities": {"experimental": `+experimental+`}}`))
	answer, _ := host.answer(0)
	return answer["result"]
}

func TestNegotiatesOnlyOfferedProvidersAndInvokesTheTypedDomainRequest(t *testing.T) {
	requests := make(chan [2]string, 1)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.KeymapImportProvider("first", func(request lithe.KeymapImportRequest, _ *lithe.Context) (any, error) {
		requests <- [2]string{request.ProviderID(), request.SnapshotID()}
		return map[string]any{"bindings": []any{}}, nil
	}))
	must(t, backend.KeymapImportProvider("not-offered", func(lithe.KeymapImportRequest, *lithe.Context) (any, error) {
		return map[string]any{"bindings": []any{}}, nil
	}))
	host := startHost(t, backend)
	result := initializeKeymap(t, host, `{
		"litheLogging": {"versions": [0]},
		"litheKeymapImport": {"versions": [0], "providers": ["first", "unknown"]}
	}`)
	eq(t, at(result, "capabilities", "experimental", "litheKeymapImport"), `{"version": 0, "providers": ["first"]}`)

	host.request(1, "lithe/keymap/import", map[string]any{"providerID": "first", "snapshotID": "snapshot-1"})
	answer, before := host.answer(1)
	eq(t, answer["result"], `{"bindings": []}`)
	if got := within(t, requests); got != [2]string{"first", "snapshot-1"} {
		t.Errorf("the provider saw %v", got)
	}
	// The fact names the wire method Core accepts, never the provider-scoped call ID.
	var facts []map[string]any
	for _, frame := range before {
		if frame["method"] == "lithe/log" {
			facts = append(facts, frame)
		}
	}
	if len(facts) != 1 {
		t.Fatalf("%d facts before the answer: %s", len(facts), encode(t, before))
	}
	eq(t, at(facts[0], "params", "kind"), `"operationFinished"`)
	eq(t, at(facts[0], "params", "operation"), `"lithe/keymap/import"`)

	host.request(2, "lithe/keymap/import", map[string]any{"providerID": "not-offered", "snapshotID": "snapshot-1"})
	if answer, _ := host.answer(2); at(answer, "error") == nil {
		t.Errorf("a provider the host did not offer answered: %s", encode(t, answer))
	}
	host.request(3, "lithe/keymap/import", map[string]any{"providerID": "first", "snapshotID": "../private"})
	if answer, _ := host.answer(3); at(answer, "error") == nil {
		t.Errorf("an invalid snapshot ID answered: %s", encode(t, answer))
	}
	host.request(4, "shutdown", nil)
	answer, _ = host.answer(4)
	if result, present := answer["result"]; !present || result != nil {
		t.Errorf("shutdown answered %s", encode(t, answer))
	}
	host.notify("exit", nil)
	if exit := host.exitCode(); exit != 0 {
		t.Errorf("exit code = %d", exit)
	}
}

func TestCancellationReachesARunningProvider(t *testing.T) {
	entered := make(chan struct{}, 1)
	cancelled := make(chan struct{}, 1)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.KeymapImportProvider("first", func(_ lithe.KeymapImportRequest, call *lithe.Context) (any, error) {
		release := make(chan struct{}, 1)
		call.Cancellation().OnCancel(func() {
			cancelled <- struct{}{}
			release <- struct{}{}
		})
		entered <- struct{}{}
		select {
		case <-release:
		case <-time.After(wait):
			return nil, fmt.Errorf("the provider was not cancelled")
		}
		if err := call.CheckCancelled(); err != nil {
			return nil, err
		}
		return map[string]any{"bindings": []any{}}, nil
	}))
	host := startHost(t, backend)
	initializeKeymap(t, host, `{
		"litheLogging": {"versions": [0]},
		"litheKeymapImport": {"versions": [0], "providers": ["first"]}
	}`)
	host.request(1, "lithe/keymap/import", map[string]any{"providerID": "first", "snapshotID": "snapshot-1"})
	within(t, entered)
	host.notify("$/cancelRequest", map[string]any{"id": 1})
	within(t, cancelled)
	answer, _ := host.answer(1)
	eq(t, at(answer, "error", "code"), `-32800`)
	host.close()
}

func TestAProviderListsAndReadsSelectedFilesThroughTheScopedHost(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.KeymapImportProvider("first", func(request lithe.KeymapImportRequest, call *lithe.Context) (any, error) {
		files, err := request.ListInputs(call.Host())
		if err != nil {
			return nil, err
		}
		data, err := request.ReadChunk(call.Host(), files[0].ID, 0, 4)
		if err != nil {
			return nil, err
		}
		if string(data) != "data" || files[0].Path != "keys.json" || files[0].ByteCount != 4 {
			return nil, fmt.Errorf("read %q from %+v", data, files[0])
		}
		return json.RawMessage(`{"bindings": [{"commandID": "find-next", "keys": [{"key": "f3", "modifiers": []}]}]}`), nil
	}))
	host := startHost(t, backend)
	initializeKeymap(t, host, `{
		"litheLogging": {"versions": [0]}, "litheFunctions": {"versions": [0]},
		"litheKeymapImport": {"versions": [0], "providers": ["first"]}
	}`)
	host.request(1, "lithe/keymap/import", map[string]any{"providerID": "first", "snapshotID": "snapshot-1"})
	list := host.next()
	eq(t, list["method"], `"lithe/host/keymapImport/listInputs"`)
	host.reply(list, j(t, `{"files": [{"id": "file-0", "path": "keys.json", "byteCount": 4}]}`))
	read := host.next()
	eq(t, read["method"], `"lithe/host/keymapImport/readChunk"`)
	eq(t, at(read, "params", "fileID"), `"file-0"`)
	host.reply(read, map[string]any{"data": "ZGF0YQ=="})
	answer, _ := host.answer(1)
	eq(t, answer["result"], `{"bindings": [{"commandID": "find-next", "keys": [{"key": "f3", "modifiers": []}]}]}`)
	host.close()
}
