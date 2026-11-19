package lithe_test

// The backend session driven by a scripted host over in-process pipes.

import (
	"encoding/json"
	"errors"
	"slices"
	"sort"
	"strings"
	"testing"
	"time"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

func echoBackend(t *testing.T) *lithe.Backend {
	t.Helper()
	backend := lithe.NewBackend("test-backend").Version("1.2.3")
	must(t, backend.Function("echo.value", func(params json.RawMessage, _ *lithe.Context) (any, error) {
		return params, nil
	}))
	return backend
}

func TestNegotiatesVersionZeroAndListsTheFunctions(t *testing.T) {
	host := startHost(t, echoBackend(t))
	eq(t, host.initialize([]any{0}), `{
		"capabilities": {"experimental": {
			"litheLogging": {"version": 0},
			"litheFunctions": {"version": 0, "functions": [{"id": "echo.value"}]}
		}},
		"serverInfo": {"name": "test-backend", "version": "1.2.3"}
	}`)
	host.invoke(1, "echo.value", j(t, `{"a": 1}`))
	answer, _ := host.answer(1)
	eq(t, answer["result"], `{"a": 1}`)
	if code := host.close(); code != 1 {
		t.Errorf("exit code = %d", code)
	}
}

func TestAHostWithoutPluginFunctionsGetsNoFunctions(t *testing.T) {
	host := startHost(t, echoBackend(t))
	eq(t, at(host.initialize(nil), "capabilities"), `{"experimental": {"litheLogging": {"version": 0}}}`)
	host.invoke(1, "echo.value", map[string]any{})
	answer, _ := host.answer(1)
	eq(t, at(answer, "error", "code"), `-32602`)
	host.close()
}

func TestFollowsTheLSPLifecycle(t *testing.T) {
	host := startHost(t, echoBackend(t))
	host.invoke(1, "echo.value", map[string]any{})
	answer, _ := host.answer(1)
	eq(t, at(answer, "error", "code"), `-32002`)
	host.sendRaw("Content-Length: 3\r\n\r\nnot")
	malformed := host.next()
	if id, present := malformed["id"]; !present || id != nil {
		t.Errorf("malformed answer id = %v, %t", id, present)
	}
	eq(t, at(malformed, "error", "code"), `-32700`)
	host.send([]any{1, 2})
	eq(t, at(host.next(), "error", "code"), `-32600`)

	host.initialize([]any{0})
	host.request(2, "initialize", map[string]any{})
	answer, _ = host.answer(2)
	eq(t, at(answer, "error", "code"), `-32600`)
	host.request(3, "textDocument/hover", map[string]any{})
	answer, _ = host.answer(3)
	eq(t, at(answer, "error", "code"), `-32601`)
	host.request(4, "shutdown", nil)
	answer, _ = host.answer(4)
	eq(t, answer, `{"jsonrpc": "2.0", "id": 4, "result": null}`)
	host.invoke(5, "echo.value", map[string]any{})
	answer, _ = host.answer(5)
	eq(t, at(answer, "error", "code"), `-32600`)
	host.notify("exit", nil)
	if code := host.exitCode(); code != 0 {
		t.Errorf("exit code = %d", code)
	}
}

func TestExitWithoutShutdownAndABrokenFrameEndWithAFailure(t *testing.T) {
	host := startHost(t, echoBackend(t))
	host.notify("exit", nil)
	if code := host.exitCode(); code != 1 {
		t.Errorf("exit without shutdown = %d", code)
	}
	host = startHost(t, echoBackend(t))
	host.sendRaw("Content-Length: nope\r\n\r\n")
	if code := host.exitCode(); code != 1 {
		t.Errorf("broken frame = %d", code)
	}
}

func TestMapsResultsFailuresAndPanics(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("work.run", func(params json.RawMessage, call *lithe.Context) (any, error) {
		call.ProgressPercent("Halfway", 150)
		switch decode(params)["mode"] {
		case "plugin":
			return nil, lithe.NewPluginError("work.failed", "The work failed.")
		case "internal":
			return nil, errors.New("/private/path: denied")
		case "panic":
			panic("secret detail")
		case "large":
			return strings.Repeat("x", 1<<20), nil
		}
		return map[string]any{"done": true}, nil
	}))
	host := startHost(t, backend)
	host.initialize([]any{0})

	host.invoke(1, "work.run", map[string]any{})
	answer, before := host.answer(1)
	eq(t, answer["result"], `{"done": true}`)
	if len(before) != 2 {
		t.Fatalf("frames before the answer: %s", encode(t, before))
	}
	eq(t, before[0], `{
		"jsonrpc": "2.0", "method": "lithe/function/progress",
		"params": {"operationID": "op-1", "message": "Halfway", "percentage": 100}
	}`)
	eq(t, before[1]["method"], `"lithe/log"`)
	eq(t, at(before[1], "params", "kind"), `"operationFinished"`)
	eq(t, at(before[1], "params", "operationId"), `"op-1"`)

	host.invoke(2, "work.run", map[string]any{"mode": "plugin"})
	answer, _ = host.answer(2)
	eq(t, answer["error"], `{"code": -32000, "message": "The work failed.", "data": {"code": "work.failed"}}`)
	for id, mode := range map[int]string{3: "internal", 4: "panic", 5: "large"} {
		host.invoke(id, "work.run", map[string]any{"mode": mode})
		answer, _ := host.answer(id)
		if !strings.Contains(encode(t, answer["error"]), "The plugin backend failed unexpectedly.") {
			t.Errorf("%s: %s", mode, encode(t, answer))
		}
		eq(t, answer["error"], `{"code": -32603, "message": "The plugin backend failed unexpectedly."}`)
	}
	// The session survives a panicking call.
	host.invoke(6, "work.run", map[string]any{})
	answer, _ = host.answer(6)
	eq(t, answer["result"], `{"done": true}`)
	host.close()
}

func TestCancelRequestCancelsTheRunningCall(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("work.wait", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		call.Progress("Waiting")
		if call.Cancellation().WaitTimeout(30 * time.Second) {
			if err := call.CheckCancelled(); err != nil {
				return nil, err
			}
		}
		return "not cancelled", nil
	}))
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.invoke(7, "work.wait", map[string]any{})
	eq(t, host.next()["method"], `"lithe/function/progress"`)
	host.notify("$/cancelRequest", map[string]any{"id": 7})
	answer, _ := host.answer(7)
	eq(t, answer["error"], `{"code": -32800, "message": "The request was cancelled."}`)
	host.close()
}

func TestAHandlersContextEndsWithTheCall(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("work.wait", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		call.Progress("Waiting")
		select {
		case <-call.Cancellation().Context().Done():
			return nil, lithe.ErrCancelled
		case <-time.After(30 * time.Second):
			return "not cancelled", nil
		}
	}))
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.invoke(1, "work.wait", map[string]any{})
	eq(t, host.next()["method"], `"lithe/function/progress"`)
	host.notify("$/cancelRequest", map[string]any{"id": 1})
	answer, _ := host.answer(1)
	eq(t, at(answer, "error", "code"), `-32800`)
	host.close()
}

func TestShutdownCancelsRunningCalls(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	hook := make(chan string, 1)
	must(t, backend.Function("work.wait", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		call.Progress("Waiting")
		call.Cancellation().WaitTimeout(30 * time.Second)
		return nil, call.CheckCancelled()
	}))
	backend.OnShutdown(func() { hook <- "shutdown hook" })
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.invoke(1, "work.wait", map[string]any{})
	eq(t, host.next()["method"], `"lithe/function/progress"`)
	host.request(2, "shutdown", nil)
	answers := []map[string]any{host.next(), host.next()}
	sort.Slice(answers, func(a, b int) bool {
		return answers[a]["id"].(float64) < answers[b]["id"].(float64)
	})
	eq(t, at(answers[0], "error", "code"), `-32800`)
	if result, present := answers[1]["result"]; !present || result != nil {
		t.Errorf("shutdown answer = %s", encode(t, answers[1]))
	}
	if got := within(t, hook); got != "shutdown hook" {
		t.Errorf("hook = %s", got)
	}
	host.close()
}

// credentialsBackend has a function that stores, reads, and deletes a
// credential.
func credentialsBackend(t *testing.T) *lithe.Backend {
	t.Helper()
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("secret.roundTrip", func(params json.RawMessage, call *lithe.Context) (any, error) {
		credentials := call.Credentials()
		key, _ := decode(params)["key"].(string)
		if err := credentials.Set(key, "s3cr3t"); err != nil {
			return nil, lithe.NewPluginError("test.credentials", code(err))
		}
		stored, _, err := credentials.Get(key)
		if err != nil {
			return nil, err
		}
		if err := credentials.Delete(key); err != nil {
			return nil, err
		}
		return map[string]any{"stored": stored}, nil
	}))
	return backend
}

func TestCredentialsAreHostRequests(t *testing.T) {
	host := startHost(t, credentialsBackend(t))
	host.initialize([]any{0})
	host.invoke(1, "secret.roundTrip", map[string]any{"key": "db:main"})
	set := host.next()
	eq(t, set["method"], `"lithe/host/credentials/set"`)
	eq(t, set["params"], `{"key": "db:main", "secret": "s3cr3t"}`)
	host.reply(set, nil)
	get := host.next()
	eq(t, get["method"], `"lithe/host/credentials/get"`)
	eq(t, get["params"], `{"key": "db:main"}`)
	host.reply(get, map[string]any{"secret": "s3cr3t"})
	remove := host.next()
	eq(t, remove["method"], `"lithe/host/credentials/delete"`)
	host.reply(remove, nil)
	answer, _ := host.answer(1)
	eq(t, answer["result"], `{"stored": "s3cr3t"}`)

	// Core's refusal reaches the handler with its code.
	host.invoke(2, "secret.roundTrip", map[string]any{"key": "db:main"})
	set = host.next()
	host.refuse(set, j(t, `{"code": -32601, "message": "Not granted.", "data": {"code": "plugin.host.permissionDenied"}}`))
	answer, _ = host.answer(2)
	eq(t, at(answer, "error", "message"), `"plugin.host.permissionDenied"`)

	// A malformed key never reaches the host.
	host.invoke(3, "secret.roundTrip", map[string]any{"key": "has space"})
	answer, _ = host.answer(3)
	eq(t, at(answer, "error", "message"), `"sdk.credentials.invalidKey"`)
	host.close()
}

func TestCancellingACallEndsItsWaitingHostRequest(t *testing.T) {
	host := startHost(t, credentialsBackend(t))
	host.initialize([]any{0})
	host.invoke(1, "secret.roundTrip", map[string]any{"key": "db:main"})
	eq(t, host.next()["method"], `"lithe/host/credentials/set"`)
	host.notify("$/cancelRequest", map[string]any{"id": 1})
	answer, _ := host.answer(1)
	eq(t, at(answer, "error", "code"), `-32800`)
	host.close()
}

func TestFunctionsAreRegisteredAndWithdrawnAtRuntime(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("extra.add", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		err := call.Backend().RegisterFunction("extra.value", func(json.RawMessage, *lithe.Context) (any, error) {
			return 42, nil
		})
		if err != nil {
			return nil, err
		}
		return call.Backend().FunctionIDs(), nil
	}))
	must(t, backend.Function("extra.remove", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		return nil, call.Backend().UnregisterFunction("extra.value")
	}))
	handle := backend.Handle()
	host := startHost(t, backend)
	host.initialize([]any{0})

	host.invoke(1, "extra.add", map[string]any{})
	register := host.next()
	eq(t, register["method"], `"lithe/function/register"`)
	eq(t, register["params"], `{"functions": [{"id": "extra.value"}]}`)
	host.reply(register, nil)
	answer, _ := host.answer(1)
	eq(t, answer["result"], `["extra.add", "extra.remove", "extra.value"]`)
	host.invoke(2, "extra.value", map[string]any{})
	answer, _ = host.answer(2)
	eq(t, answer["result"], `42`)

	host.invoke(3, "extra.remove", map[string]any{})
	unregister := host.next()
	eq(t, unregister["method"], `"lithe/function/unregister"`)
	eq(t, unregister["params"], `{"ids": ["extra.value"]}`)
	host.reply(unregister, nil)
	answer, _ = host.answer(3)
	if result, present := answer["result"]; !present || result != nil {
		t.Errorf("unregister answer = %s", encode(t, answer))
	}
	host.invoke(4, "extra.value", map[string]any{})
	answer, _ = host.answer(4)
	eq(t, at(answer, "error", "code"), `-32602`)

	// A refused registration leaves the function out.
	refused := make(chan error, 1)
	go func() {
		refused <- handle.RegisterFunction("extra.refused", func(json.RawMessage, *lithe.Context) (any, error) {
			return nil, nil
		})
	}()
	register = host.next()
	host.refuse(register, j(t, `{"code": -32602, "message": "No.", "data": {"code": "plugin.function.invalidRegistration"}}`))
	if err := within(t, refused); code(err) != "plugin.function.invalidRegistration" {
		t.Errorf("refused registration = %v", err)
	}
	if slices.Contains(handle.FunctionIDs(), "extra.refused") {
		t.Error("a refused function is served")
	}
	host.close()
	// After the session, requests fail instead of waiting.
	late := handle.RegisterFunction("extra.late", func(json.RawMessage, *lithe.Context) (any, error) { return nil, nil })
	if code(late) != lithe.CodeSessionClosed {
		t.Errorf("late registration = %v", late)
	}
}

func TestFunctionDeclarationsAreChecked(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	nothing := func(json.RawMessage, *lithe.Context) (any, error) { return nil, nil }
	if err := backend.Function("Bad", nothing); code(err) != "sdk.function.invalidID" {
		t.Errorf("invalid ID = %v", err)
	}
	must(t, backend.Function("a.b", nothing))
	if err := backend.Function("a.b", nothing); code(err) != "sdk.function.duplicate" {
		t.Errorf("duplicate = %v", err)
	}
	if err := backend.Handle().RegisterFunction("c.d", nothing); code(err) != "sdk.functions.unavailable" {
		t.Errorf("register before the session = %v", err)
	}
}

func TestInitializeHooksSeeTheRootAndAddFunctions(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	roots := make(chan string, 1)
	backend.OnInitialize(func(initialize *lithe.Initialize) error {
		root, _ := initialize.RootPath()
		roots <- root
		return initialize.Function("late.added", func(json.RawMessage, *lithe.Context) (any, error) {
			return "added", nil
		})
	})
	host := startHost(t, backend)
	result := host.initialize([]any{0})
	eq(t, at(result, "capabilities", "experimental", "litheFunctions", "functions"), `[{"id": "late.added"}]`)
	if root := within(t, roots); root != "/tmp/state" {
		t.Errorf("root = %q", root)
	}

	refusing := lithe.NewBackend("test-backend")
	refusing.OnInitialize(func(*lithe.Initialize) error {
		return lithe.NewPluginError("test.notReady", "Not ready.")
	})
	host = startHost(t, refusing)
	host.request(0, "initialize", j(t, `{"capabilities": {"experimental": {"litheLogging": {"versions": [0]}}}}`))
	answer, _ := host.answer(0)
	eq(t, at(answer, "error", "data", "code"), `"test.notReady"`)
	host.close()
}

func TestTypedDiagnosticsPreserveOperationAndSuppressOnlySuccess(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("work.inspect", func(params json.RawMessage, call *lithe.Context) (any, error) {
		if err := call.Diagnostics().Degraded("work.inspect", "cache_unavailable", 2); err != nil {
			return nil, err
		}
		if err := call.Diagnostics().Recovered("work.inspect", "cache_restored"); err != nil {
			return nil, err
		}
		if decode(params)["fail"] == true {
			return nil, lithe.NewPluginError("work.failed", "private explanation")
		}
		return nil, nil
	}))
	must(t, backend.SuppressSuccessDiagnostics("work.inspect"))
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.invoke(1, "work.inspect", map[string]any{"fail": false})
	_, success := host.answer(1)
	if len(success) != 2 {
		t.Fatalf("success facts: %s", encode(t, success))
	}
	eq(t, at(success[0], "params", "operationId"), `"op-1"`)
	eq(t, at(success[0], "params", "kind"), `"degraded"`)
	eq(t, at(success[0], "params", "count"), `2`)
	eq(t, at(success[1], "params", "kind"), `"recovered"`)
	host.invoke(2, "work.inspect", map[string]any{"fail": true})
	_, failure := host.answer(2)
	if len(failure) != 3 {
		t.Fatalf("failure facts: %s", encode(t, failure))
	}
	eq(t, at(failure[2], "params", "kind"), `"operationFailed"`)
	eq(t, at(failure[2], "params", "code"), `"work.failed"`)
	if _, ok := at(failure[2], "params", "durationMs").(float64); !ok {
		t.Errorf("no duration: %s", encode(t, failure[2]))
	}
	if strings.Contains(encode(t, failure[2]), "private explanation") {
		t.Errorf("the failure fact holds the message: %s", encode(t, failure[2]))
	}
	host.close()
}

func TestPerCallSuppressionKeepsUserSuccessAndFailures(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("database.call", func(params json.RawMessage, call *lithe.Context) (any, error) {
		object := decode(params)
		if object["readOnly"] == true {
			call.SuppressSuccessDiagnostics()
		}
		if object["fail"] == true {
			return nil, errors.New("private failure")
		}
		return nil, nil
	}))
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.invoke(1, "database.call", map[string]any{"readOnly": true})
	if _, before := host.answer(1); len(before) != 0 {
		t.Errorf("a suppressed success logged: %s", encode(t, before))
	}
	host.invoke(2, "database.call", map[string]any{})
	_, before := host.answer(2)
	eq(t, at(before, 0, "params", "kind"), `"operationFinished"`)
	host.invoke(3, "database.call", map[string]any{"readOnly": true, "fail": true})
	_, before = host.answer(3)
	eq(t, at(before, 0, "params", "kind"), `"operationFailed"`)
	eq(t, at(before, 0, "params", "code"), `"internal_error"`)
	host.close()
}

func TestInitializeDiagnosticsWaitForTheCapabilityAnswerAndStayBounded(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	diagnostics := backend.Handle().Diagnostics()
	backend.OnInitialize(func(*lithe.Initialize) error {
		for count := range uint64(10) {
			if err := diagnostics.Degraded("initialize", "snapshot_unavailable", count); err != nil {
				return err
			}
		}
		return nil
	})
	host := startHost(t, backend)
	host.request(0, "initialize", j(t, `{"capabilities": {"experimental": {"litheLogging": {"versions": [0]}}}}`))
	answer := host.next()
	eq(t, answer["id"], `0`)
	eq(t, at(answer, "result", "capabilities", "experimental", "litheLogging", "version"), `0`)
	for count := range 8 {
		fact := host.next()
		eq(t, fact["method"], `"lithe/log"`)
		eq(t, at(fact, "params", "count"), encode(t, count))
	}
	host.request(1, "shutdown", nil)
	// The initialization buffer is bounded to eight facts.
	eq(t, host.next()["id"], `1`)
	host.close()
}

func TestDiagnosticsUseTheOriginalRPCIDWhenTheOperationIDIsAbsent(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("work.inspect", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		return nil, call.Diagnostics().Degraded("work.inspect", "cache_unavailable")
	}))
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.request(42, "lithe/function/invoke", j(t, `{"functionID": "work.inspect"}`))
	_, facts := host.answer(42)
	if len(facts) != 2 {
		t.Fatalf("facts: %s", encode(t, facts))
	}
	for _, fact := range facts {
		eq(t, at(fact, "params", "operationId"), `"42"`)
	}
	host.request("rpc-second", "lithe/function/invoke", j(t, `{"functionID": "work.inspect", "operationID": "wire-original"}`))
	_, facts = host.answer("rpc-second")
	for _, fact := range facts {
		eq(t, at(fact, "params", "operationId"), `"wire-original"`)
	}
	host.close()
}

func TestADiagnosticNeedsARegisteredOperationAndAControlledCode(t *testing.T) {
	diagnostics := lithe.NewBackend("test-backend").Handle().Diagnostics()
	if err := diagnostics.Degraded("not.registered", "code"); code(err) != "sdk.logging.invalidOperation" {
		t.Errorf("unregistered operation = %v", err)
	}
	if err := diagnostics.Degraded("initialize", "has space"); code(err) != "sdk.logging.invalidCode" {
		t.Errorf("free-form code = %v", err)
	}
	if err := diagnostics.Degraded("initialize", "ok", 1<<60); code(err) != "sdk.logging.invalidCount" {
		t.Errorf("unsafe count = %v", err)
	}
}
