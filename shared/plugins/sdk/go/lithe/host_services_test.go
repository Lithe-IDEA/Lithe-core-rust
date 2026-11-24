package lithe_test

// Host services of the plugin-function protocol, driven by a scripted host
// and the contract fixture; mirrors the Rust SDK's tests/host_services.rs.

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

const quiet = 200 * time.Millisecond

func fixture(t *testing.T) map[string]any {
	t.Helper()
	// Read at run time: the published module has no contract directory.
	path := filepath.Join("..", "..", "..", "contract", "v1", "fixtures", "backend", "function-protocol.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("%s: %v", path, err)
	}
	var value map[string]any
	if err := json.Unmarshal(data, &value); err != nil {
		t.Fatalf("%s: %v", path, err)
	}
	return value
}

func cases(t *testing.T, fixture map[string]any, key string) []map[string]any {
	t.Helper()
	list, ok := fixture[key].([]any)
	if !ok {
		t.Fatalf("the fixture has no %s cases", key)
	}
	result := make([]map[string]any, 0, len(list))
	for _, entry := range list {
		result = append(result, entry.(map[string]any))
	}
	return result
}

func testContext() lithe.WorkspaceContext {
	context, err := lithe.NewWorkspaceContext("w1", 2)
	if err != nil {
		panic(err)
	}
	return context
}

// outcome is what blocking SDK work returned.
type outcome[T any] struct {
	value T
	err   error
}

// background runs blocking SDK work off the test goroutine.
func background[T any](work func() (T, error)) <-chan outcome[T] {
	result := make(chan outcome[T], 1)
	go func() {
		value, err := work()
		result <- outcome[T]{value, err}
	}()
	return result
}

// done is the outcome of background work, failing after the bounded wait.
func done[T any](t *testing.T, pending <-chan outcome[T]) outcome[T] {
	t.Helper()
	return within(t, pending)
}

// refusal is the SDK error background work failed with.
func refusal[T any](t *testing.T, pending <-chan outcome[T]) *lithe.SdkError {
	t.Helper()
	result := done(t, pending)
	var sdk *lithe.SdkError
	if !errors.As(result.err, &sdk) {
		t.Fatalf("the host request did not fail with an SDK error: %v, %v", result.value, result.err)
	}
	return sdk
}

func rpcCode(t *testing.T, err *lithe.SdkError) any {
	t.Helper()
	if number, ok := err.RPCCode(); ok {
		return number
	}
	return nil
}

func number(value any) int64 {
	n, _ := value.(float64)
	return int64(n)
}

// session is a host whose session negotiated plugin functions (or did not),
// and every frame the backend sent.
type session struct {
	*testHost
	log    []map[string]any
	unread []map[string]any
}

// startSession initializes with litheFunctions versions, or none for nil.
func startSession(t *testing.T, backend *lithe.Backend, versions any) *session {
	t.Helper()
	host := startHost(t, backend)
	host.initialize(versions)
	return &session{testHost: host}
}

func (s *session) pull(timeout time.Duration) (map[string]any, bool) {
	frame, ok := s.nextWithin(timeout)
	if ok {
		s.log = append(s.log, frame)
	}
	return frame, ok
}

// takeWhere is the next frame the test has not taken that matches.
func (s *session) takeWhere(matches func(map[string]any) bool) map[string]any {
	s.t.Helper()
	for index, frame := range s.unread {
		if matches(frame) {
			s.unread = slices.Delete(s.unread, index, index+1)
			return frame
		}
	}
	for {
		frame, ok := s.pull(wait)
		if !ok {
			s.t.Fatalf("no matching frame; unread: %s", encode(s.t, s.unread))
		}
		if matches(frame) {
			return frame
		}
		s.unread = append(s.unread, frame)
	}
}

// take is the next host request with method.
func (s *session) take(method string) map[string]any {
	s.t.Helper()
	return s.takeWhere(func(frame map[string]any) bool {
		_, isRequest := frame["id"]
		return frame["method"] == method && isRequest
	})
}

// response is the backend's answer to request id.
func (s *session) response(id any) map[string]any {
	s.t.Helper()
	want := normalize(s.t, id)
	return s.takeWhere(func(frame map[string]any) bool {
		_, isRequest := frame["method"]
		return !isRequest && reflect.DeepEqual(frame["id"], want)
	})
}

// sent is every frame with method the backend sent, once it went quiet.
func (s *session) sent(method string) []map[string]any {
	for {
		frame, ok := s.pull(quiet)
		if !ok {
			break
		}
		s.unread = append(s.unread, frame)
	}
	var frames []map[string]any
	for _, frame := range s.log {
		if frame["method"] == method {
			frames = append(frames, frame)
		}
	}
	return frames
}

func TestTheSDKAnswersPluginFunctionsOnlyWhenTheHostOffersVersionZero(t *testing.T) {
	for _, c := range []struct {
		offered    []any
		negotiated bool
	}{
		{[]any{0}, true},
		{[]any{0, 99}, true},
		{[]any{1}, false},
		{[]any{99}, false},
		{[]any{}, false},
	} {
		backend := lithe.NewBackend("test-backend")
		must(t, backend.Function("query.run", func(json.RawMessage, *lithe.Context) (any, error) { return nil, nil }))
		handle := backend.Handle()
		host := startHost(t, backend)
		answer := at(host.initialize(c.offered), "capabilities", "experimental", "litheFunctions")
		version, ok := handle.FunctionsVersion()
		if c.negotiated {
			eq(t, answer, `{"version": 0, "functions": [{"id": "query.run"}]}`)
			if !ok || version != 0 {
				t.Errorf("%v: functions version = %d, %t", c.offered, version, ok)
			}
		} else {
			if answer != nil {
				t.Errorf("%v: litheFunctions = %s", c.offered, encode(t, answer))
			}
			if ok {
				t.Errorf("%v: functions version = %d", c.offered, version)
			}
		}
		if handle.Host().Negotiated() != c.negotiated {
			t.Errorf("%v: negotiated = %t", c.offered, !c.negotiated)
		}
		host.close()
	}
}

func TestCredentialsRoundTripThroughTheHostWithTheContractShapes(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	type secret struct {
		value string
		found bool
	}
	read := func() <-chan outcome[secret] {
		return background(func() (secret, error) {
			value, found, err := host.Credentials().Get("connection.prod")
			return secret{value, found}, err
		})
	}

	reading := read()
	get := s.take("lithe/host/credentials/get")
	eq(t, get["params"], `{"key": "connection.prod"}`)
	s.reply(get, map[string]any{"secret": "s3cret"})
	if result := done(t, reading); result.err != nil || result.value != (secret{"s3cret", true}) {
		t.Errorf("get = %+v, %v", result.value, result.err)
	}

	missing := read()
	s.reply(s.take("lithe/host/credentials/get"), nil)
	if result := done(t, missing); result.err != nil || result.value.found {
		t.Errorf("missing = %+v, %v", result.value, result.err)
	}

	malformed := read()
	s.reply(s.take("lithe/host/credentials/get"), map[string]any{})
	if got := code(done(t, malformed).err); got != "sdk.host.invalidResult" {
		t.Errorf("malformed = %q", got)
	}

	writing := background(func() (struct{}, error) {
		return struct{}{}, host.Credentials().Set("connection.prod:password", "s3cret")
	})
	set := s.take("lithe/host/credentials/set")
	eq(t, set["params"], `{"key": "connection.prod:password", "secret": "s3cret"}`)
	s.reply(set, nil)
	must(t, done(t, writing).err)

	deleting := background(func() (struct{}, error) {
		return struct{}{}, host.Credentials().Delete("connection.prod")
	})
	remove := s.take("lithe/host/credentials/delete")
	eq(t, remove["params"], `{"key": "connection.prod"}`)
	s.reply(remove, nil)
	must(t, done(t, deleting).err)

	// Malformed arguments fail locally and never reach the host.
	for _, key := range []string{"", "has space", strings.Repeat("k", 129)} {
		if _, _, err := host.Credentials().Get(key); code(err) != "sdk.credentials.invalidKey" {
			t.Errorf("%q: %v", key, err)
		}
	}
	if err := host.Credentials().Set("key", strings.Repeat("x", 16*1024+1)); code(err) != "sdk.credentials.invalidSecret" {
		t.Errorf("an oversized secret: %v", err)
	}
	s.assertQuiet()
	s.close()
}

func TestLocalKeyAndContextChecksAgreeWithTheFixtureCasesCoreRefusesOrForwards(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	keys, contexts := 0, 0
	for _, c := range cases(t, fixture(t), "hostRequests") {
		name, _ := c["name"].(string)
		method, _ := c["method"].(string)
		expected, _ := c["expected"].(map[string]any)
		if strings.HasPrefix(method, "lithe/host/credentials/") {
			key, _ := at(c, "params", "key").(string)
			if _, forward := expected["forward"]; forward {
				if !lithe.IsCredentialKey(key) {
					t.Errorf("%s: the key is refused", name)
				}
			} else if strings.Contains(name, "key") {
				if _, _, err := host.Credentials().Get(key); code(err) != "sdk.credentials.invalidKey" {
					t.Errorf("%s: %v", name, err)
				}
				keys++
			}
		}
		for _, pattern := range []string{"context without", "context ID outside", "generation is positive"} {
			if strings.Contains(name, pattern) {
				if _, err := lithe.ParseWorkspaceContext(at(c, "params", "context")); code(err) != "sdk.host.invalidContext" {
					t.Errorf("%s: %v", name, err)
				}
				contexts++
				break
			}
		}
	}
	if keys != 2 || contexts != 3 {
		t.Errorf("checked %d keys and %d contexts, want 2 and 3", keys, contexts)
	}
	s.assertQuiet()

	// A full context from listContexts is passed on as its reference.
	workspace, err := lithe.ParseWorkspaceContext(j(t, `{"contextID": "w1", "generation": 2, "name": "orders", "folders": []}`))
	must(t, err)
	asking := background(func() (json.RawMessage, error) { return host.Workspace().GetFolders(workspace) })
	request := s.take("lithe/host/workspace/getFolders")
	eq(t, request["params"], `{"context": {"contextID": "w1", "generation": 2}}`)
	s.reply(request, map[string]any{"folders": []any{}})
	result := done(t, asking)
	must(t, result.err)
	eq(t, result.value, `{"folders": []}`)
	s.close()
}

func TestHostRefusalsKeepTheCodeNumberAndDetailsCoreSent(t *testing.T) {
	f := fixture(t)
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	answer := func(wire any) *lithe.SdkError {
		t.Helper()
		pending := background(func() (json.RawMessage, error) {
			return host.Request("lithe/host/workspace/listContexts", nil)
		})
		s.refuse(s.take("lithe/host/workspace/listContexts"), wire)
		return refusal(t, pending)
	}
	refusals := 0
	for _, c := range cases(t, f, "hostRequests") {
		expected, _ := c["expected"].(map[string]any)
		expectedCode, ok := expected["code"].(string)
		if !ok {
			continue
		}
		data := map[string]any{"code": expectedCode}
		if details := expected["details"]; details != nil {
			data["details"] = details
		}
		err := answer(map[string]any{"code": expected["rpcCode"], "message": "Refused.", "data": data})
		if err.Code() != expectedCode {
			t.Errorf("%s: code = %q", c["name"], err.Code())
		}
		eq(t, rpcCode(t, err), encode(t, expected["rpcCode"]))
		if details := expected["details"]; details == nil {
			if err.Details() != nil {
				t.Errorf("%s: details = %v", c["name"], err.Details())
			}
		} else {
			eq(t, err.Details(), encode(t, details))
		}
		if err.Message() != "Refused." {
			t.Errorf("%s: message = %q", c["name"], err.Message())
		}
		refusals++
	}
	if refusals == 0 {
		t.Error("the fixture has no refusals")
	}
	failures := cases(t, f, "hostFailures")
	if len(failures) == 0 {
		t.Error("the fixture has no host failures")
	}
	for _, c := range failures {
		expected := c["expected"].(map[string]any)
		wireNumber := number(expected["rpcCode"])
		index := slices.IndexFunc(lithe.HostErrorCodes, func(entry lithe.ErrorCode) bool { return entry.Code == expected["code"] })
		if index < 0 || lithe.HostErrorCodes[index].Number != wireNumber {
			t.Errorf("%s: %v is not listed with %d", c["name"], expected["code"], wireNumber)
		}
		// The number alone names the code.
		if err := answer(map[string]any{"code": wireNumber, "message": "Failed."}); err.Code() != expected["code"] {
			t.Errorf("%s: code = %q", c["name"], err.Code())
		}
	}
	later := answer(j(t, `{"code": -32050, "message": "Later code.", "data": {"details": ["not", "an", "object"]}}`))
	if later.Code() != "plugin.host.failed" || later.Details() != nil {
		t.Errorf("a later code = %q, %v", later.Code(), later.Details())
	}
	s.close()
}

func TestWithoutPluginFunctionsTheHostOffersNoServiceAndHostRequestsStayWithinLitheHost(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, nil)
	unavailable := func(label string, err error) {
		t.Helper()
		var sdk *lithe.SdkError
		if !errors.As(err, &sdk) || sdk.Code() != "plugin.host.unavailable" {
			t.Errorf("%s: %v", label, err)
			return
		}
		if number, ok := sdk.RPCCode(); !ok || number != -32601 {
			t.Errorf("%s: rpc code = %d, %t", label, number, ok)
		}
	}
	_, _, err := host.Credentials().Get("connection.prod")
	unavailable("credentials", err)
	_, err = host.Capabilities()
	unavailable("capabilities", err)
	if supported, err := host.Supports("lithe/host/credentials/get"); supported || err != nil {
		t.Errorf("supports = %t, %v", supported, err)
	}
	// The relay is a host service too, and so is the generic request.
	call := map[string]any{"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"}
	_, err = host.Functions().Providers("languageImport.settings")
	unavailable("providers", err)
	_, err = host.Functions().Invoke(testContext(), call)
	unavailable("invoke", err)
	_, err = host.Request("lithe/host/functions/invoke", map[string]any{})
	unavailable("request", err)
	if _, err := host.Request("lithe/function/invoke", map[string]any{}); code(err) != "sdk.host.method" {
		t.Errorf("a backend method: %v", err)
	}
	s.assertQuiet()
	s.close()

	idle := lithe.NewBackend("idle").Handle().Host()
	if _, _, err := idle.Credentials().Get("key"); code(err) != "sdk.session.notReady" {
		t.Errorf("an idle backend: %v", err)
	}
}

func TestCapabilitiesAreAskedForOnceAndSupportsReadsThem(t *testing.T) {
	var answer any
	for _, c := range cases(t, fixture(t), "hostRequests") {
		if value, ok := c["expected"].(map[string]any)["answer"]; ok {
			answer = value
			break
		}
	}
	if answer == nil {
		t.Fatal("the fixture has no capabilities answer")
	}
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	first := background(func() (bool, error) { return host.Supports("lithe/host/workspace/getFolders") })
	second := background(func() (bool, error) { return host.Supports("lithe/host/workspace/readFile") })
	request := s.take("lithe/host/getCapabilities")
	if _, present := request["params"]; present {
		t.Errorf("the capabilities request has params: %s", encode(t, request))
	}
	s.reply(request, answer)
	if result := done(t, first); !result.value || result.err != nil {
		t.Errorf("getFolders = %t, %v", result.value, result.err)
	}
	if result := done(t, second); result.value || result.err != nil {
		t.Errorf("readFile = %t, %v", result.value, result.err)
	}
	capabilities, err := host.Capabilities()
	must(t, err)
	eq(t, at(j(t, string(capabilities)), "limits", "pendingHostRequests"), `16`)
	if asked := len(s.sent("lithe/host/getCapabilities")); asked != 1 {
		t.Errorf("capabilities were asked for %d times", asked)
	}
	s.close()
}

func TestACallCarriesItsWorkspaceContextToTheHandler(t *testing.T) {
	checked := 0
	for _, c := range cases(t, fixture(t), "invocationContexts") {
		params, _ := at(c, "expected", "params").(map[string]any)
		if params == nil {
			continue
		}
		seen := make(chan *lithe.WorkspaceContext, 1)
		backend := lithe.NewBackend("test-backend")
		must(t, backend.Function(params["functionID"].(string), func(_ json.RawMessage, call *lithe.Context) (any, error) {
			seen <- call.WorkspaceContext()
			return nil, nil
		}))
		s := startSession(t, backend, []any{0})
		s.request(9, "lithe/function/invoke", params)
		if response := s.response(9); response["error"] != nil {
			t.Errorf("%s: %s", c["name"], encode(t, response))
		}
		eq(t, within(t, seen), encode(t, params["context"]))
		s.close()
		checked++
	}
	if checked == 0 {
		t.Error("the fixture has no invocation contexts")
	}
}

func TestCancellingACallCancelsItsHostRequestsAtCore(t *testing.T) {
	failure := make(chan string, 1)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("secret.read", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		secret, _, err := call.Host().Credentials().Get("connection.prod")
		failure <- code(err)
		if err != nil {
			return nil, err
		}
		return secret, nil
	}))
	s := startSession(t, backend, []any{0})
	s.request(41, "lithe/function/invoke", map[string]any{"functionID": "secret.read", "operationID": "c"})
	request := s.take("lithe/host/credentials/get")
	s.notify("$/cancelRequest", map[string]any{"id": 41})
	eq(t, at(s.response(41), "error", "code"), `-32800`)
	// The host request fails with the call's cancellation.
	if got := within(t, failure); got != "sdk.call.cancelled" {
		t.Errorf("the host request failed with %q", got)
	}
	var cancels []any
	for _, frame := range s.sent("$/cancelRequest") {
		cancels = append(cancels, frame["params"])
	}
	eq(t, cancels, encode(t, []any{map[string]any{"id": request["id"]}}))
	// Core's late answer is dropped; the session continues.
	s.refuse(request, j(t, `{"code": -32800, "message": "Cancelled.", "data": {"code": "plugin.host.cancelled"}}`))
	if exit := s.close(); exit != 1 {
		t.Errorf("exit code = %d", exit)
	}
}

func TestACancelledTokenEndsItsRequestWhichIsNeverSentAgain(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	stop := lithe.NewCancellation()
	cancellable := host.WithCancellation(stop)
	edit := j(t, `{"documentID": "d1", "expectedVersion": 4, "edits": []}`)
	pending := background(func() (json.RawMessage, error) {
		return cancellable.Editor().ApplyEdits(testContext(), edit)
	})
	request := s.take("lithe/host/editor/applyEdits")
	eq(t, request["params"], `{
		"documentID": "d1", "expectedVersion": 4, "edits": [],
		"context": {"contextID": "w1", "generation": 2}
	}`)
	stop.Cancel()
	if got := code(done(t, pending).err); got != "sdk.call.cancelled" {
		t.Errorf("the cancelled request failed with %q", got)
	}
	cancel := s.takeWhere(func(frame map[string]any) bool { return frame["method"] == "$/cancelRequest" })
	eq(t, cancel["params"], encode(t, map[string]any{"id": request["id"]}))
	s.reply(request, j(t, `{"documentID": "d1", "version": 5, "applied": true}`))
	if _, err := cancellable.Request("lithe/host/editor/applyEdits", map[string]any{}); code(err) != "sdk.call.cancelled" {
		t.Errorf("a later request: %v", err)
	}
	if sent := len(s.sent("lithe/host/editor/applyEdits")); sent != 1 {
		t.Errorf("applyEdits was sent %d times", sent)
	}
	s.close()
}

func TestAtMostSixteenHostRequestsAreUnansweredLaterOnesWaitForAFreeSlot(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	var calls []<-chan outcome[json.RawMessage]
	for range 18 {
		calls = append(calls, background(func() (json.RawMessage, error) { return host.Workspace().ListContexts() }))
	}
	first := s.sent("lithe/host/workspace/listContexts")
	if len(first) != 16 {
		t.Fatalf("%d requests were sent, want 16", len(first))
	}
	s.reply(first[0], j(t, `{"revision": 1, "contexts": []}`))
	if sent := len(s.sent("lithe/host/workspace/listContexts")); sent != 17 {
		t.Errorf("%d requests were sent after one answer, want 17", sent)
	}
	// Ending the session fails the queued request too.
	s.close()
	var outcomes []string
	for _, call := range calls {
		result := done(t, call)
		if result.err != nil {
			outcomes = append(outcomes, code(result.err))
		} else {
			outcomes = append(outcomes, encode(t, normalize(t, result.value)))
		}
	}
	slices.Sort(outcomes)
	expected := []string{encode(t, j(t, `{"revision": 1, "contexts": []}`))}
	for range 17 {
		expected = append(expected, "sdk.session.closed")
	}
	slices.Sort(expected)
	if !slices.Equal(outcomes, expected) {
		t.Errorf("outcomes = %v", outcomes)
	}
}

func TestSubscriptionEventsArriveInOrderAlsoWhenSentBeforeTheAnswer(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	watched := j(t, `{"resource": {"rootID": "r1", "path": ""}, "recursive": true}`)
	watching := background(func() (*lithe.Subscription, error) {
		return host.Workspace().Watch(testContext(), watched)
	})
	request := s.take("lithe/host/workspace/watch")
	eq(t, request["params"], `{
		"resource": {"rootID": "r1", "path": ""}, "recursive": true,
		"context": {"contextID": "w1", "generation": 2}
	}`)
	notify := func(sequence int) {
		s.notify("lithe/host/subscriptions/notify", map[string]any{
			"subscriptionID": "s1", "sequence": sequence,
			"event": map[string]any{"rootID": "r1", "changedPaths": []any{fmt.Sprintf("f%d", sequence)}, "requiresRescan": false},
		})
	}
	notify(1)
	s.reply(request, map[string]any{"subscriptionID": "s1"})
	answered := done(t, watching)
	must(t, answered.err)
	subscription := answered.value
	notify(2)
	notify(4)
	var seen []any
	for range 3 {
		event, err := subscription.RecvTimeout(wait)
		must(t, err)
		changed := at(j(t, string(event.Event())), "changedPaths")
		seen = append(seen, []any{event.SubscriptionID(), event.Sequence(), event.Gap(), changed})
	}
	eq(t, seen, `[["s1", 1, false, ["f1"]], ["s1", 2, false, ["f2"]], ["s1", 4, true, ["f4"]]]`)
	if subscription.ID() != "s1" || subscription.IsDisposed() {
		t.Errorf("subscription %q, disposed %t", subscription.ID(), subscription.IsDisposed())
	}

	disposing := background(func() (struct{}, error) { return struct{}{}, subscription.Dispose() })
	dispose := s.take("lithe/host/subscriptions/dispose")
	if !subscription.IsDisposed() {
		t.Error("the subscription is live while it is disposed")
	}
	eq(t, dispose["params"], `{"subscriptionID": "s1"}`)
	notify(5)
	s.reply(dispose, nil)
	must(t, done(t, disposing).err)
	// Event 5 came after the disposal; it was dropped.
	if _, err := subscription.RecvTimeout(quiet); !errors.Is(err, lithe.ErrSubscriptionDisposed) {
		t.Errorf("RecvTimeout after the disposal = %v", err)
	}
	if _, ok := subscription.Recv(); ok {
		t.Error("Recv after the disposal received an event")
	}
	s.close()
}

func TestCommandsContextChangesAndLaterHostNotificationsReachTheirHandlers(t *testing.T) {
	var delivered []map[string]any
	for _, c := range cases(t, fixture(t), "hostNotifications") {
		if at(c, "expected", "delivered") == true && c["method"] != "lithe/host/subscriptions/notify" {
			delivered = append(delivered, c)
		}
	}
	key := func(entry []any) string { return encode(t, normalize(t, entry)) }
	expected := map[string]bool{}
	for _, c := range delivered {
		params := c["params"]
		switch method := c["method"].(string); method {
		case "lithe/host/commands/didRun":
			expected[key([]any{"command", at(params, "commandID"), at(params, "context")})] = true
		case "lithe/host/workspace/didChangeContexts":
			expected[key([]any{"contexts", params})] = true
		default:
			expected[key([]any{"notification", method, params})] = true
		}
	}
	expected[key([]any{"later", map[string]any{"value": 1}})] = true
	// Every delivered case is told apart by its method and params.
	if len(expected) != len(delivered)+1 {
		t.Fatalf("%d distinct notifications for %d cases", len(expected), len(delivered))
	}

	seen := make(chan []any, 64)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.OnCommand("db.refresh", func(commandID string, context *lithe.WorkspaceContext) error {
		var reference any
		if context != nil {
			reference = *context
		}
		seen <- []any{"command", commandID, reference}
		return nil
	}))
	backend.OnContextsChanged(func(params json.RawMessage) error {
		seen <- []any{"contexts", params}
		return nil
	})
	must(t, backend.OnHostNotification("lithe/host/later/didSomething", func(params json.RawMessage) error {
		seen <- []any{"later", params}
		return nil
	}))
	// The other delivered methods (run exits, language server changes and
	// forwarded notifications) use the generic registration, once each.
	generic := map[string]bool{}
	for _, c := range delivered {
		method := c["method"].(string)
		if method != "lithe/host/commands/didRun" && method != "lithe/host/workspace/didChangeContexts" {
			generic[method] = true
		}
	}
	if len(generic) != 3 {
		t.Fatalf("%d generic methods, want 3", len(generic))
	}
	for method := range generic {
		must(t, backend.OnHostNotification(method, func(params json.RawMessage) error {
			seen <- []any{"notification", method, params}
			return nil
		}))
	}
	if err := backend.OnHostNotification("lithe/host/commands/didRun", func(json.RawMessage) error { return nil }); code(err) != "sdk.method.reserved" {
		t.Errorf("a routed method: %v", err)
	}
	if err := backend.OnHostNotification("workspace/didChange", func(json.RawMessage) error { return nil }); code(err) != "sdk.method.invalid" {
		t.Errorf("a method outside lithe/host: %v", err)
	}
	if err := backend.OnCommand("DB_Refresh", func(string, *lithe.WorkspaceContext) error { return nil }); code(err) != "sdk.command.invalidID" {
		t.Errorf("an invalid command ID: %v", err)
	}

	s := startSession(t, backend, []any{0})
	for _, c := range delivered {
		s.notify(c["method"].(string), c["params"])
	}
	// A notification this SDK does not know is ignored, as the contract asks.
	s.notify("lithe/host/unknown/ignored", map[string]any{"anything": true})
	s.notify("lithe/host/later/didSomething", map[string]any{"value": 1})
	// Handlers run on their own goroutines, so only the set is fixed.
	received := map[string]bool{}
	for range len(expected) {
		received[key(within(t, seen))] = true
	}
	if !reflect.DeepEqual(received, expected) {
		t.Errorf("received %v\nexpected %v", received, expected)
	}
	select {
	case extra := <-seen:
		t.Errorf("an extra notification: %s", encode(t, extra))
	case <-time.After(quiet):
	}
	s.assertQuiet()
	s.close()
}

func TestNativeTerminalMethodsKeepContextAndLiteralArguments(t *testing.T) {
	backend := lithe.NewBackend("terminal-test")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	creating := background(func() (json.RawMessage, error) {
		return host.Terminal().Create(testContext(), map[string]any{
			"executable": "example-client", "arguments": []string{"literal;$(value)", "space value"},
		})
	})
	create := s.take("lithe/host/terminal/create")
	eq(t, at(create, "params", "context"), `{"contextID": "w1", "generation": 2}`)
	eq(t, at(create, "params", "arguments"), `["literal;$(value)", "space value"]`)
	s.reply(create, map[string]any{"terminalID": "owned"})
	created := done(t, creating)
	must(t, created.err)
	eq(t, created.value, `{"terminalID": "owned"}`)
	for _, method := range []string{"show", "close", "reopen", "dispose"} {
		operation := background(func() (json.RawMessage, error) {
			terminal := host.Terminal()
			switch method {
			case "show":
				return terminal.Show(testContext(), "owned")
			case "close":
				return terminal.Close(testContext(), "owned", 4096)
			case "reopen":
				return terminal.Reopen(testContext(), "owned")
			}
			return terminal.Dispose(testContext(), "owned")
		})
		request := s.take("lithe/host/terminal/" + method)
		eq(t, at(request, "params", "terminalID"), `"owned"`)
		if method == "close" {
			eq(t, at(request, "params", "retainedContextBytes"), `4096`)
		}
		s.reply(request, nil)
		result := done(t, operation)
		must(t, result.err)
		eq(t, result.value, `null`)
	}
	s.close()
}

func TestSaveAndFileChangeMethodsCarryTheirParamsWithTheContext(t *testing.T) {
	backend := lithe.NewBackend("file-changes-test")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	resource := map[string]any{"rootID": "r1", "path": "src/a.txt"}
	target := map[string]any{"rootID": "r1", "path": "src/b.txt"}
	for _, call := range []struct {
		method string
		params map[string]any
		run    func(map[string]any) (json.RawMessage, error)
	}{
		{"documents/save", map[string]any{"documentID": "d1", "expectedVersion": 3},
			func(p map[string]any) (json.RawMessage, error) { return host.Documents().Save(testContext(), p) }},
		{"documents/saveAs", map[string]any{"documentID": "d1", "target": target},
			func(p map[string]any) (json.RawMessage, error) { return host.Documents().SaveAs(testContext(), p) }},
		{"documents/close", map[string]any{"documentID": "d1"},
			func(p map[string]any) (json.RawMessage, error) { return host.Documents().Close(testContext(), p) }},
		{"workspace/create", map[string]any{"resource": resource, "kind": "file", "text": "x"},
			func(p map[string]any) (json.RawMessage, error) { return host.Workspace().Create(testContext(), p) }},
		{"workspace/rename", map[string]any{"resource": resource, "target": target},
			func(p map[string]any) (json.RawMessage, error) { return host.Workspace().Rename(testContext(), p) }},
		{"workspace/copy", map[string]any{"resource": resource, "target": target},
			func(p map[string]any) (json.RawMessage, error) { return host.Workspace().Copy(testContext(), p) }},
		{"workspace/trash", map[string]any{"resource": resource},
			func(p map[string]any) (json.RawMessage, error) { return host.Workspace().Trash(testContext(), p) }},
	} {
		operation := background(func() (json.RawMessage, error) { return call.run(call.params) })
		request := s.take("lithe/host/" + call.method)
		eq(t, at(request, "params", "context"), `{"contextID": "w1", "generation": 2}`)
		for key, value := range call.params {
			want, _ := json.Marshal(value)
			eq(t, at(request, "params", key), string(want))
		}
		s.reply(request, nil)
		result := done(t, operation)
		must(t, result.err)
		eq(t, result.value, `null`)
	}
	s.close()
}

func TestHostPathRootIDsCoreRefusesFailLocallyAndTheAnswerHoldsHostPaths(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	workspace := backend.Handle().Host().Workspace()
	s := startSession(t, backend, []any{0})
	checked := 0
	for _, c := range cases(t, fixture(t), "hostRequests") {
		params, _ := c["params"].(map[string]any)
		if c["method"] != "lithe/host/workspace/getHostPaths" || params["context"] == nil || params["path"] != nil {
			continue
		}
		name, _ := c["name"].(string)
		context, err := lithe.ParseWorkspaceContext(params["context"])
		must(t, err)
		var rootIDs []string
		list, _ := params["rootIDs"].([]any)
		for _, id := range list {
			rootIDs = append(rootIDs, id.(string))
		}
		expected, _ := c["expected"].(map[string]any)
		switch {
		case expected["forward"] != nil:
			asking := background(func() ([]lithe.HostFolder, error) { return workspace.GetHostPaths(context, rootIDs...) })
			request := s.take("lithe/host/workspace/getHostPaths")
			eq(t, request["params"], encode(t, params))
			roots := rootIDs
			if len(roots) == 0 {
				roots = []string{"r0c1d2e3f4a5b6c7d"}
			}
			folders := make([]any, len(roots))
			for i, id := range roots {
				folders[i] = map[string]any{"rootID": id, "hostPath": fmt.Sprintf("/srv/root%d", i)}
			}
			s.reply(request, map[string]any{"folders": folders})
			result := done(t, asking)
			must(t, result.err)
			if len(result.value) != len(roots) || result.value[0].RootID != roots[0] || result.value[0].HostPath != "/srv/root0" {
				t.Errorf("%s: %v", name, result.value)
			}
			checked++
		case expected["code"] == "plugin.host.invalidParams" && len(rootIDs) > 0:
			// An empty list cannot be written: no root IDs asks for every root.
			if _, err := workspace.GetHostPaths(context, rootIDs...); code(err) != "sdk.host.invalidParams" {
				t.Errorf("%s: %v", name, err)
			}
			checked++
		}
	}
	if checked != 4 {
		t.Errorf("checked %d cases, want 4", checked)
	}
	tooMany := make([]string, 257)
	for i := range tooMany {
		tooMany[i] = fmt.Sprintf("r%d", i)
	}
	if _, err := workspace.GetHostPaths(testContext(), tooMany...); code(err) != "sdk.host.invalidParams" {
		t.Errorf("257 root IDs: %v", err)
	}
	s.assertQuiet()

	// An answer that is not root IDs with absolute paths is refused.
	for _, answer := range []string{`{}`, `{"folders": [{"rootID": "r0", "hostPath": "srv/root0"}]}`, `{"folders": [{"hostPath": "/srv/root0"}]}`} {
		asking := background(func() ([]lithe.HostFolder, error) { return workspace.GetHostPaths(testContext()) })
		s.reply(s.take("lithe/host/workspace/getHostPaths"), j(t, answer))
		if err := done(t, asking).err; code(err) != "sdk.host.invalidResult" {
			t.Errorf("%s: %v", answer, err)
		}
	}
	s.close()
}

func TestProcessRootsReadsALocalContextAndAsksTheHostOnlyForARemoteOne(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	workspace := backend.Handle().Host().Workspace()
	s := startSession(t, backend, []any{0})
	local, err := workspace.ProcessRoots(json.RawMessage(`{
		"contextID": "w1", "generation": 2, "name": "orders", "authority": {"kind": "local"},
		"folders": [{"rootID": "r0", "name": "orders", "path": "/Users/me/orders"}]
	}`))
	must(t, err)
	eq(t, local, `[{"rootID": "r0", "path": "/Users/me/orders"}]`)
	s.assertQuiet()

	remote := j(t, `{
		"contextID": "w1", "generation": 2, "name": "orders",
		"authority": {"kind": "remote", "id": "c-7d41", "platform": {"os": "linux", "arch": "x86_64"}},
		"folders": [{"rootID": "r0", "name": "orders"}]
	}`)
	asking := background(func() ([]lithe.ProcessRoot, error) { return workspace.ProcessRoots(remote) })
	request := s.take("lithe/host/workspace/getHostPaths")
	eq(t, request["params"], `{"context": {"contextID": "w1", "generation": 2}}`)
	s.reply(request, j(t, `{"folders": [{"rootID": "r0", "hostPath": "/home/me/orders"}]}`))
	result := done(t, asking)
	must(t, result.err)
	eq(t, result.value, `[{"rootID": "r0", "hostPath": "/home/me/orders"}]`)
	if result.value[0].Path != "" || result.value[0].HostPath.String() != "/home/me/orders" {
		t.Errorf("a remote root: %+v", result.value[0])
	}

	// A reference alone does not say the authority, and a local folder names its path.
	for _, context := range []any{testContext(), j(t, `{"contextID": "w1", "generation": 2, "authority": {"kind": "local"}, "folders": [{"rootID": "r0"}]}`)} {
		if _, err := workspace.ProcessRoots(context); code(err) != "sdk.host.invalidContext" {
			t.Errorf("%s: %v", encode(t, context), err)
		}
	}
	s.assertQuiet()
	s.close()
}

func TestARelayedCallNamesThePluginThatCalledItAndAHostCallNamesNone(t *testing.T) {
	checked := 0
	for _, c := range cases(t, fixture(t), "invocationCallers") {
		params := at(c, "expected", "params").(map[string]any)
		seen := make(chan *lithe.Caller, 1)
		backend := lithe.NewBackend("test-backend")
		must(t, backend.Function(params["functionID"].(string), func(_ json.RawMessage, call *lithe.Context) (any, error) {
			seen <- call.Caller()
			return nil, nil
		}))
		s := startSession(t, backend, []any{0})
		s.request(9, "lithe/function/invoke", params)
		if response := s.response(9); response["error"] != nil {
			t.Errorf("%s: %s", c["name"], encode(t, response))
		}
		caller := within(t, seen)
		want, named := at(params, "caller", "pluginID").(string)
		if named != (caller != nil) || caller != nil && caller.PluginID() != want {
			t.Errorf("%s: caller = %v, want %q", c["name"], caller, want)
		}
		s.close()
		checked++
	}
	if checked == 0 {
		t.Error("the fixture has no invocation callers")
	}
}

func TestACallerThatIsNotExactlyAPluginIDIsNotReported(t *testing.T) {
	seen := make(chan *lithe.Caller, 1)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("languageImport.settings", func(_ json.RawMessage, call *lithe.Context) (any, error) {
		seen <- call.Caller()
		return nil, nil
	}))
	s := startSession(t, backend, []any{0})
	python := "dev.lithe.fixture.python"
	attempts := []struct {
		caller any
		want   string
	}{
		{map[string]any{"pluginID": python}, python},
		{map[string]any{"pluginID": ""}, ""},
		{map[string]any{"pluginID": 7}, ""},
		{map[string]any{}, ""},
		{map[string]any{"pluginID": python, "extra": true}, ""},
		{python, ""},
		{nil, ""},
	}
	for index, attempt := range attempts {
		id := 10 + index
		s.invokeWith(id, "languageImport.settings", map[string]any{}, map[string]any{"caller": attempt.caller})
		response := s.response(id)
		if response["error"] != nil || response["result"] != nil {
			t.Errorf("%s: %s", encode(t, attempt.caller), encode(t, response))
		}
		caller := within(t, seen)
		reported := ""
		if caller != nil {
			reported = caller.PluginID()
		}
		if reported != attempt.want {
			t.Errorf("%s: reported %q", encode(t, attempt.caller), reported)
		}
	}
	s.close()
}

func TestRelayRequestsCarryTheFixtureParamsAndOnlyTheFieldsTheMethodsDefine(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	forwarded := 0
	for _, c := range cases(t, fixture(t), "hostRequests") {
		if at(c, "expected", "relay") != true {
			continue
		}
		method := c["method"].(string)
		params := c["params"].(map[string]any)
		providers := strings.HasSuffix(method, "/providers")
		answer := j(t, `{"value": {"settings": []}}`)
		if providers {
			answer = j(t, `{"providers": [{"pluginID": "dev.lithe.fixture.go", "version": 1}]}`)
		}
		sending := background(func() (json.RawMessage, error) {
			if providers {
				return host.Functions().Providers(params["functionID"].(string))
			}
			context, err := lithe.ParseWorkspaceContext(params["context"])
			if err != nil {
				return nil, err
			}
			return host.Functions().Invoke(context, params)
		})
		request := s.take(method)
		eq(t, request["params"], encode(t, params))
		s.reply(request, answer)
		result := done(t, sending)
		if result.err != nil {
			t.Errorf("%s: %v", c["name"], result.err)
		}
		eq(t, result.value, encode(t, answer))
		forwarded++
	}
	if forwarded != 3 {
		t.Errorf("%d relay cases, want 3", forwarded)
	}

	// Fields the method does not define are not sent: Core would refuse a
	// top-level caller, and the call's context is the argument.
	call := j(t, `{
		"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings",
		"caller": {"pluginID": "dev.lithe.fixture.other"},
		"context": {"contextID": "other", "generation": 9}, "extra": true
	}`)
	sending := background(func() (json.RawMessage, error) { return host.Functions().Invoke(testContext(), call) })
	request := s.take("lithe/host/functions/invoke")
	eq(t, request["params"], `{
		"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings",
		"context": {"contextID": "w1", "generation": 2}
	}`)
	s.reply(request, map[string]any{"value": nil})
	must(t, done(t, sending).err)
	s.close()
}

func TestARelayedCallFromAHandlerBelongsToTheCallItServesUnlessItNamesAnother(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("project.collect", func(params json.RawMessage, served *lithe.Context) (any, error) {
		value, err := served.Host().Functions().Invoke(testContext(), params)
		if err != nil {
			return nil, err
		}
		return value, nil
	}))
	s := startSession(t, backend, []any{0})
	call := func(extra string) any {
		return j(t, `{"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"`+extra+`}`)
	}

	s.invoke(5, "project.collect", call(""))
	request := s.take("lithe/host/functions/invoke")
	eq(t, at(request, "params", "parentOperationID"), `"op-5"`)
	s.reply(request, j(t, `{"value": {"files": 3}}`))
	eq(t, s.response(5)["result"], `{"value": {"files": 3}}`)

	s.invoke(6, "project.collect", call(`, "parentOperationID": "op-elsewhere"`))
	request = s.take("lithe/host/functions/invoke")
	eq(t, at(request, "params", "parentOperationID"), `"op-elsewhere"`)
	s.reply(request, map[string]any{"value": nil})
	eq(t, s.response(6)["result"], `{"value": null}`)

	// A parent the contract would refuse never leaves the SDK.
	s.invoke(7, "project.collect", call(`, "parentOperationID": ""`))
	if response := s.response(7); response["error"] == nil {
		t.Errorf("an invalid parent: %s", encode(t, response))
	}
	if sent := len(s.sent("lithe/host/functions/invoke")); sent != 2 {
		t.Errorf("%d relayed calls were sent, want 2", sent)
	}
	s.close()
}

func TestRelayRefusalsKeepTheCodeNumberAndTheProvidersOwnCode(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	call := map[string]any{"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"}
	refused := func(wire any) *lithe.SdkError {
		t.Helper()
		pending := background(func() (json.RawMessage, error) { return host.Functions().Invoke(testContext(), call) })
		s.refuse(s.take("lithe/host/functions/invoke"), wire)
		return refusal(t, pending)
	}
	relayErrors := cases(t, fixture(t), "relayErrors")
	if len(relayErrors) != 14 {
		t.Errorf("%d relay errors, want 14", len(relayErrors))
	}
	for _, c := range relayErrors {
		expected := c["code"].(string)
		wireNumber := number(c["rpcCode"])
		named := refused(map[string]any{"code": wireNumber, "message": "Refused.", "data": map[string]any{"code": expected}})
		// The number alone names the code.
		numbered := refused(map[string]any{"code": wireNumber, "message": "Refused."})
		for _, err := range []*lithe.SdkError{named, numbered} {
			if got, ok := err.RPCCode(); err.Code() != expected || !ok || got != wireNumber {
				t.Errorf("%s: %q, %d", c["name"], err.Code(), got)
			}
		}
	}
	failed := refused(j(t, `{
		"code": -32070, "message": "The provider failed.",
		"data": {"code": "plugin.function.failed", "details": {"pluginErrorCode": "db.locked"}}
	}`))
	if failed.Code() != "plugin.function.failed" {
		t.Errorf("code = %q", failed.Code())
	}
	eq(t, failed.Details(), `{"pluginErrorCode": "db.locked"}`)
	s.close()
}

func TestAnInvalidRelayedCallFailsLocallyAndNothingIsSent(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	functions := host.Functions()
	invalid := func(label string, err error) {
		t.Helper()
		if code(err) != "sdk.functions.invalidCall" {
			t.Errorf("%s: %v", label, err)
		}
	}
	for _, attempt := range []struct {
		field string
		bad   any
	}{
		{"pluginID", ""},
		{"pluginID", 7},
		{"functionID", "Not-Valid"},
		{"params", []any{1}},
		{"timeoutMilliseconds", 0},
		{"timeoutMilliseconds", -5},
		{"timeoutMilliseconds", 1.5},
		{"timeoutMilliseconds", "5"},
		{"parentOperationID", 17},
		{"parentOperationID", ""},
		{"parentOperationID", strings.Repeat("p", 129)},
	} {
		call := map[string]any{"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"}
		call[attempt.field] = attempt.bad
		_, err := functions.Invoke(testContext(), call)
		invalid(fmt.Sprintf("%s: %v", attempt.field, attempt.bad), err)
	}
	for _, call := range []string{
		`[]`,
		`{"functionID": "languageImport.settings"}`,
		`{"pluginID": "dev.lithe.fixture.go"}`,
	} {
		_, err := functions.Invoke(testContext(), j(t, call))
		invalid(call, err)
	}
	for _, bad := range []string{"", "Not-Valid", "UPPER", "a..b"} {
		_, err := functions.Providers(bad)
		invalid(fmt.Sprintf("providers %q", bad), err)
	}

	// The fixture's cases Core refuses as invalid params agree with these
	// checks, whenever the call names a context.
	refused := 0
	for _, c := range cases(t, fixture(t), "hostRequests") {
		params, _ := c["params"].(map[string]any)
		if _, named := params["context"]; c["method"] != "lithe/host/functions/invoke" ||
			at(c, "expected", "code") != "plugin.host.invalidParams" || !named {
			continue
		}
		workspace, err := lithe.ParseWorkspaceContext(params["context"])
		must(t, err)
		_, err = functions.Invoke(workspace, params)
		invalid(c["name"].(string), err)
		refused++
	}
	if refused != 4 {
		t.Errorf("%d invalid relay cases, want 4", refused)
	}
	s.assertQuiet()
	s.close()
}

func TestRelayAnswersAreTheHostsObjectsAndANullValueStaysInsideValue(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	providers := func() <-chan outcome[json.RawMessage] {
		return background(func() (json.RawMessage, error) { return host.Functions().Providers("languageImport.settings") })
	}
	listing := providers()
	answer := `{"providers": [{"pluginID": "dev.lithe.fixture.go", "version": 1}]}`
	s.reply(s.take("lithe/host/functions/providers"), j(t, answer))
	listed := done(t, listing)
	must(t, listed.err)
	eq(t, listed.value, answer)

	call := map[string]any{"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"}
	calling := background(func() (json.RawMessage, error) { return host.Functions().Invoke(testContext(), call) })
	s.reply(s.take("lithe/host/functions/invoke"), map[string]any{"value": nil})
	called := done(t, calling)
	must(t, called.err)
	eq(t, called.value, `{"value": null}`)

	// An answer that is not an object is the host's mistake, not a value.
	broken := providers()
	s.reply(s.take("lithe/host/functions/providers"), []any{})
	if got := code(done(t, broken).err); got != "sdk.host.invalidResult" {
		t.Errorf("a non-object answer: %q", got)
	}
	s.close()
}

func TestCancellingACallCancelsTheRelayedCallItWaitsOn(t *testing.T) {
	failure := make(chan string, 1)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.Function("project.collect", func(params json.RawMessage, served *lithe.Context) (any, error) {
		value, err := served.Host().Functions().Invoke(testContext(), params)
		failure <- code(err)
		if err != nil {
			return nil, err
		}
		return value, nil
	}))
	s := startSession(t, backend, []any{0})
	s.invoke(41, "project.collect", map[string]any{"pluginID": "dev.lithe.fixture.go", "functionID": "languageImport.settings"})
	request := s.take("lithe/host/functions/invoke")
	s.notify("$/cancelRequest", map[string]any{"id": 41})
	eq(t, at(s.response(41), "error", "code"), `-32800`)
	// The relayed request fails with the call's cancellation, and Core is told.
	if got := within(t, failure); got != "sdk.call.cancelled" {
		t.Errorf("the relayed call failed with %q", got)
	}
	var cancels []any
	for _, frame := range s.sent("$/cancelRequest") {
		cancels = append(cancels, frame["params"])
	}
	eq(t, cancels, encode(t, []any{map[string]any{"id": request["id"]}}))
	s.close()
}

func TestProgressCancellationAndEndReleaseSubscription(t *testing.T) {
	backend := lithe.NewBackend("test-backend")
	host := backend.Handle().Host()
	s := startSession(t, backend, []any{0})
	creating := background(func() (*lithe.Subscription, error) {
		return host.Progress().Create(testContext(), map[string]any{"title": "Index", "cancellable": true})
	})
	create := s.take("lithe/host/progress/create")
	eq(t, create["params"], `{"context":{"contextID":"w1","generation":2},"title":"Index","cancellable":true}`)
	s.reply(create, map[string]any{"progressID": "p1", "subscriptionID": "p1"})
	result := done(t, creating)
	must(t, result.err)
	subscription := result.value
	s.notify("lithe/host/subscriptions/notify", map[string]any{"subscriptionID": "p1", "sequence": 1, "event": map[string]bool{"cancelled": true}})
	event, err := subscription.RecvTimeout(wait)
	must(t, err)
	eq(t, j(t, string(event.Event())), `{"cancelled":true}`)
	ending := background(func() (json.RawMessage, error) {
		return host.Progress().End(testContext(), map[string]any{"progressID": "p1"})
	})
	end := s.take("lithe/host/progress/end")
	eq(t, end["params"], `{"context":{"contextID":"w1","generation":2},"progressID":"p1"}`)
	s.reply(end, nil)
	dispose := s.take("lithe/host/subscriptions/dispose")
	eq(t, dispose["params"], `{"subscriptionID":"p1"}`)
	if !subscription.IsDisposed() {
		t.Fatal("ending progress left subscription live")
	}
	s.reply(dispose, nil)
	must(t, done(t, ending).err)
	if _, err := subscription.RecvTimeout(wait); !errors.Is(err, lithe.ErrSubscriptionDisposed) {
		t.Fatalf("ended subscription: %v", err)
	}
	s.close()
}
