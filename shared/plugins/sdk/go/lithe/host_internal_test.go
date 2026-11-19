package lithe

import (
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

func errorCode(err error) string {
	var sdk *SdkError
	if errors.As(err, &sdk) {
		return sdk.Code()
	}
	return ""
}

func TestContextsAreReferencesOrFullContexts(t *testing.T) {
	full := mustValue(t, `{"contextID": "w1", "generation": 2, "name": "orders"}`)
	context, err := ParseWorkspaceContext(full)
	if err != nil {
		t.Fatal(err)
	}
	assertJSON(t, context, `{"contextID": "w1", "generation": 2}`)
	made, err := NewWorkspaceContext("w1", 2)
	if err != nil || made != context {
		t.Errorf("NewWorkspaceContext = %+v, %v", made, err)
	}
	// Raw JSON from a host answer is read too.
	fromRaw, err := ParseWorkspaceContext(json.RawMessage(`{"contextID":"w1","generation":2.0,"roots":[]}`))
	if err != nil || fromRaw != context {
		t.Errorf("from raw = %+v, %v", fromRaw, err)
	}
	var unmarshalled WorkspaceContext
	if err := json.Unmarshal([]byte(`{"contextID":"w1","generation":2}`), &unmarshalled); err != nil || unmarshalled != context {
		t.Errorf("unmarshalled = %+v, %v", unmarshalled, err)
	}
	// An invocation's context is exactly a reference.
	if workspaceContextFromReference(full) != nil {
		t.Error("a full context is accepted as a reference")
	}
	if reference := workspaceContextFromReference(mustValue(t, canonical(t, context))); reference == nil || *reference != context {
		t.Errorf("reference = %v", reference)
	}
	for _, invalid := range []string{
		`{"contextID": "w1"}`,
		`{"contextID": "w1", "generation": 0}`,
		`{"contextID": "../w", "generation": 1}`,
		`null`,
	} {
		if _, err := ParseWorkspaceContext(json.RawMessage(invalid)); errorCode(err) != "sdk.host.invalidContext" {
			t.Errorf("%s: %v", invalid, err)
		}
	}
	if _, err := NewWorkspaceContext("", 1); err == nil {
		t.Error("an empty context ID is accepted")
	}
	if err := json.Unmarshal([]byte(`{"contextID":"w1"}`), &unmarshalled); errorCode(err) != "sdk.host.invalidContext" {
		t.Errorf("unmarshal invalid = %v", err)
	}
}

func TestARelayedCallKeepsOnlyTheFieldsTheMethodDefines(t *testing.T) {
	call := mustValue(t, `{
		"pluginID": "dev.lithe.fixture.java", "functionID": "project.describe",
		"params": {"caller": "data"}, "timeoutMilliseconds": 900000,
		"parentOperationID": "17", "caller": {"pluginID": "forged"},
		"context": {"contextID": "x"}, "extra": true
	}`)
	relayed, err := relayParams(call, "served")
	if err != nil {
		t.Fatal(err)
	}
	// Core passes a caller inside params unchanged; only a top-level one is
	// dropped.
	assertJSON(t, relayed, `{
		"pluginID": "dev.lithe.fixture.java", "functionID": "project.describe",
		"params": {"caller": "data"}, "timeoutMilliseconds": 900000,
		"parentOperationID": "17"
	}`)
}

func TestOptionalFieldsStayOutAndTheParentDefaultsToTheCallBeingServed(t *testing.T) {
	call := `{"pluginID": "a.b", "functionID": "x.y"}`
	relayed, err := relayParams(mustValue(t, call), "")
	if err != nil {
		t.Fatal(err)
	}
	assertJSON(t, relayed, call)
	served, err := relayParams(mustValue(t, call), "op-7")
	if err != nil {
		t.Fatal(err)
	}
	assertJSON(t, served, `{"pluginID": "a.b", "functionID": "x.y", "parentOperationID": "op-7"}`)
	named, err := relayParams(mustValue(t, `{"pluginID": "a.b", "functionID": "x.y", "parentOperationID": "own"}`), "op-7")
	if err != nil || named["parentOperationID"] != "own" {
		t.Errorf("named = %v, %v", named, err)
	}
	// The host caps the timeout; the SDK sends what the plugin chose.
	slow, err := relayParams(mustValue(t, `{"pluginID": "a.b", "functionID": "x.y", "timeoutMilliseconds": 700000}`), "")
	if err != nil {
		t.Fatal(err)
	}
	assertJSON(t, slow["timeoutMilliseconds"], `700000`)
}

func TestAnInvalidRelayedCallIsRefusedBeforeAnythingIsSent(t *testing.T) {
	with := func(key, value string) string {
		call := mustValue(t, `{"pluginID": "a.b", "functionID": "x.y"}`).(map[string]any)
		call[key] = mustValue(t, value)
		return canonical(t, call)
	}
	without := func(key string) string {
		call := mustValue(t, `{"pluginID": "a.b", "functionID": "x.y"}`).(map[string]any)
		delete(call, key)
		return canonical(t, call)
	}
	for _, invalid := range []string{
		`"x.y"`,
		`null`,
		without("pluginID"),
		with("pluginID", `""`),
		with("pluginID", `7`),
		without("functionID"),
		with("functionID", `"Not-Valid"`),
		with("params", `[]`),
		with("params", `null`),
		with("timeoutMilliseconds", `0`),
		with("timeoutMilliseconds", `-1`),
		with("timeoutMilliseconds", `1.5`),
		with("timeoutMilliseconds", `"5000"`),
		with("parentOperationID", `""`),
		with("parentOperationID", `"`+strings.Repeat("p", 129)+`"`),
		with("parentOperationID", `12`),
	} {
		if _, err := relayParams(mustValue(t, invalid), "op-1"); errorCode(err) != "sdk.functions.invalidCall" {
			t.Errorf("%s: %v", invalid, err)
		}
	}
}
