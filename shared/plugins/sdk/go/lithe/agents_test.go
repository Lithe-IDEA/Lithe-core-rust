package lithe_test

// Native agent sessions and the agents host service (plan API-45), driven by
// the scripted host and fixtures/agents/native-session.json.

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"sync"
	"testing"
	"time"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

func agentsFixture(t *testing.T) map[string]any {
	t.Helper()
	path := filepath.Join("..", "..", "..", "contract", "v1", "fixtures", "agents", "native-session.json")
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var fixture map[string]any
	must(t, json.Unmarshal(data, &fixture))
	return fixture
}

// gate opens once agent-2 exited. agent-1's first output waits for it, so
// sessions sharing one worker never get past it.
type gate struct {
	once   sync.Once
	opened chan struct{}
}

func (g *gate) open() { g.once.Do(func() { close(g.opened) }) }

// recorder records every call as [session, call, args...], as the fixture
// lists them; it fails prompts that say fail.
type recorder struct {
	session     string
	calls       chan<- []any
	otherExited *gate
}

func (r *recorder) record(call ...any) {
	r.calls <- append([]any{r.session}, call...)
}

func (r *recorder) Output(lines []string) error {
	if r.session == "agent-1" && slices.Contains(lines, "one") {
		select {
		case <-r.otherExited.opened:
		case <-time.After(wait):
			return errors.New("agent-2 did not exit in time")
		}
	}
	r.record("output", lines)
	return nil
}

func (r *recorder) Prompt(turn uint64, text string) error {
	r.record("prompt", turn, text)
	if text == "fail" {
		return errors.New("prompt_failed")
	}
	return nil
}

func (r *recorder) PromptImages(turn uint64, text string, images []lithe.AgentImage) error {
	r.record("prompt", turn, text, images)
	return nil
}

func (r *recorder) Cancel(turn uint64) error {
	r.record("cancel", turn)
	return nil
}

func (r *recorder) DecidePermission(turn uint64, permission, option string) error {
	r.record("decidePermission", turn, permission, option)
	return nil
}

func (r *recorder) SetConfigOption(option, value string) error {
	r.record("setConfigOption", option, value)
	return nil
}

func (r *recorder) Exited(exitCode *int64) {
	r.record("exited", exitCode)
	if r.session == "agent-2" {
		r.otherExited.open()
	}
}

// recordingBackend fails to start sessions in /work/broken.
func recordingBackend(t *testing.T, calls chan<- []any) *lithe.Backend {
	backend := lithe.NewBackend("agents-test")
	otherExited := &gate{opened: make(chan struct{})}
	backend.OnAgentSessions(func(start lithe.AgentSessionStart, session *lithe.AgentSessionHandle) (lithe.AgentSession, error) {
		if session.ID() != start.Session {
			t.Errorf("handle %s for session %s", session.ID(), start.Session)
		}
		if start.Cwd == "/work/broken" {
			return nil, errors.New("start_failed")
		}
		r := &recorder{session: start.Session, calls: calls, otherExited: otherExited}
		r.record("start", map[string]any{
			"session": start.Session, "agentID": start.AgentID, "cwd": start.Cwd, "resume": start.Resume,
		})
		return r, nil
	})
	return backend
}

func sendAll(host *testHost, notifications any) {
	host.t.Helper()
	for _, notification := range notifications.([]any) {
		host.notify(at(notification, "method").(string), at(notification, "params"))
	}
}

// callsBySession is the calls of every session, in the order each session
// received them. Every session's last call is its exit, so nothing follows
// the count.
func callsBySession(t *testing.T, calls <-chan []any, count int) map[string][]any {
	t.Helper()
	sessions := map[string][]any{}
	for range count {
		call := within(t, calls)
		session := call[0].(string)
		sessions[session] = append(sessions[session], call[1:])
	}
	return sessions
}

func callCount(calls any) int {
	count := 0
	for _, session := range calls.(map[string]any) {
		count += len(session.([]any))
	}
	return count
}

// failureCodes is every operationFailed code among the frames not read yet,
// once the session closed and the backend closed its output.
func failureCodes(t *testing.T, host *testHost) []string {
	t.Helper()
	var codes []string
	for {
		select {
		case frame, ok := <-host.frames:
			if !ok {
				return codes
			}
			if frame["method"] == "lithe/log" && at(frame, "params", "kind") == "operationFailed" {
				codes = append(codes, at(frame, "params", "code").(string))
			}
		case <-time.After(wait):
			t.Fatalf("the backend did not close its output in time")
			return nil
		}
	}
}

func TestEachAgentSessionGetsItsNotificationsInOrderFromStartToExit(t *testing.T) {
	fixture := agentsFixture(t)
	expected := at(fixture, "sessions", "calls")
	calls := make(chan []any, 64)
	host := startHost(t, recordingBackend(t, calls))
	host.initialize([]any{0})
	sendAll(host, at(fixture, "sessions", "notifications"))
	eq(t, callsBySession(t, calls, callCount(expected)), encode(t, expected))
	if code := host.close(); code != 1 {
		t.Errorf("exit code %d", code)
	}
	// agent-9 never started, so its output reaches no handler.
	eq(t, failureCodes(t, host), `["agent_session_unknown"]`)
}

func TestAFailedAgentStartOrPromptEndsTheTurnAsFailed(t *testing.T) {
	fixture := agentsFixture(t)
	failures := at(fixture, "failures")
	expected := at(failures, "calls")
	calls := make(chan []any, 64)
	host := startHost(t, recordingBackend(t, calls))
	host.initialize([]any{0})
	sendAll(host, at(failures, "notifications"))

	wanted := at(failures, "reports").([]any)
	var reports []string
	for len(reports) < len(wanted) {
		frame := host.next()
		if frame["method"] == "lithe/host/agents/report" {
			reports = append(reports, encode(t, frame["params"]))
			host.reply(frame, map[string]any{})
		}
	}
	var expectedReports []string
	for _, report := range wanted {
		expectedReports = append(expectedReports, encode(t, report))
	}
	sort.Strings(reports)
	sort.Strings(expectedReports)
	eq(t, reports, encode(t, expectedReports))
	eq(t, callsBySession(t, calls, callCount(expected)), encode(t, expected))
	if code := host.close(); code != 1 {
		t.Errorf("exit code %d", code)
	}
}

func TestAgentHostMethodsSendTheFixtureRequests(t *testing.T) {
	fixture := agentsFixture(t)
	backend := lithe.NewBackend("agents-test")
	handle := backend.Handle()
	host := startHost(t, backend)
	host.initialize([]any{0})
	for _, test := range at(fixture, "methods").([]any) {
		agents := handle.Host().Agents()
		session, _ := at(test, "session").(string)
		done := make(chan error, 1)
		result := make(chan json.RawMessage, 1)
		go func() {
			switch at(test, "call") {
			case "write":
				var lines []string
				for _, line := range at(test, "lines").([]any) {
					lines = append(lines, line.(string))
				}
				done <- agents.Write(session, lines)
			case "report":
				done <- agents.Report(session, at(test, "events"))
			default:
				answer, err := agents.Usage(at(test, "params"))
				result <- answer
				done <- err
			}
		}()
		request := host.next()
		eq(t, request["method"], encode(t, at(test, "request", "method")))
		eq(t, request["params"], encode(t, at(test, "request", "params")))
		answer := map[string]any{}
		if at(test, "call") == "usage" {
			answer = map[string]any{"sinceMs": 1, "untilMs": 2, "summaries": []any{}, "rateLimits": []any{}}
		}
		host.reply(request, answer)
		must(t, within(t, done))
		if at(test, "call") == "usage" {
			eq(t, within(t, result), encode(t, answer))
		}
	}
	if code := host.close(); code != 1 {
		t.Errorf("exit code %d", code)
	}
}

func TestAgentNotificationsAreRoutedOnlyThroughAgentSessions(t *testing.T) {
	backend := lithe.NewBackend("agents-test")
	for _, method := range []string{"lithe/host/agents/didStart", "lithe/host/agents/didOutput", "lithe/host/agents/didExit"} {
		err := backend.OnHostNotification(method, func(json.RawMessage) error { return nil })
		if code(err) != "sdk.method.reserved" {
			t.Errorf("%s: %v", method, err)
		}
	}
	// Without a registration a started session is only logged.
	host := startHost(t, backend)
	host.initialize([]any{0})
	host.notify("lithe/host/agents/didStart", map[string]any{"session": "agent-1", "agentID": "example", "cwd": "/work/orders"})
	host.notify("lithe/host/agents/didOutput", map[string]any{"session": "agent-1", "lines": []any{"one"}})
	// The reader logs both before it reads the end of the input.
	if code := host.close(); code != 1 {
		t.Errorf("exit code %d", code)
	}
	eq(t, failureCodes(t, host), `["agent_unhandled"]`)
}
