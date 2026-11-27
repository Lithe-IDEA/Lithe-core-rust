package lithe

// Native agent sessions and the agents host service (plan API-45).
//
// Core starts an agent's CLI and relays it through the plugin backend: the
// CLI's standard output arrives as lithe/host/agents/didOutput, the user's
// prompts, cancellations and permission decisions as didPrompt, didCancel and
// didDecidePermission, and the backend answers with lithe/host/agents/write
// (the CLI's standard input) and lithe/host/agents/report (session events). A
// translation is only right when it sees the CLI's lines in order, so the SDK
// gives every session one goroutine that runs its handler's methods one at a
// time, in the order Core sent the notifications.

import (
	"encoding/json"
	"errors"
	"sync"
)

const (
	methodAgentsWrite     = "lithe/host/agents/write"
	methodAgentsReport    = "lithe/host/agents/report"
	methodAgentsUsage     = "lithe/host/agents/usage"
	methodAgentsDidStart  = "lithe/host/agents/didStart"
	methodAgentsDidOutput = "lithe/host/agents/didOutput"
	methodAgentsDidPrompt = "lithe/host/agents/didPrompt"
	methodAgentsDidCancel = "lithe/host/agents/didCancel"
	methodAgentsDidDecide = "lithe/host/agents/didDecidePermission"
	methodAgentsDidExit   = "lithe/host/agents/didExit"
)

func isAgentNotification(method string) bool {
	switch method {
	case methodAgentsDidStart, methodAgentsDidOutput, methodAgentsDidPrompt,
		methodAgentsDidCancel, methodAgentsDidDecide, methodAgentsDidExit:
		return true
	}
	return false
}

// AgentSessionStart is what lithe/host/agents/didStart says about a session
// Core started.
type AgentSessionStart struct {
	// Session is Core's ID of the session, such as agent-1.
	Session string
	// AgentID is the agents[].id from the manifest.
	AgentID string
	// Cwd is the working directory the CLI runs in.
	Cwd string
	// Resume is the CLI's own session ID to resume, when the user resumes one.
	Resume *string
}

// AgentSession is the plugin's translation of one native session. Every
// method runs on the session's own goroutine, one at a time and in the order
// Core sent the notifications, so a method may block on host requests.
//
// A failing Prompt ends its turn: the SDK reports turnEnded with reason
// failed. Other failures are only logged. Embed [AgentSessionBase] for the
// methods a translation does not need.
type AgentSession interface {
	// Output gets lines the CLI wrote to standard output, without line breaks.
	Output(lines []string) error
	// Prompt gets the user's prompt for turn; write what the CLI expects.
	Prompt(turn uint64, text string) error
	// Cancel says the user cancelled turn.
	Cancel(turn uint64) error
	// DecidePermission says the user answered permission request permission
	// with option.
	DecidePermission(turn uint64, permission, option string) error
	// Exited says the CLI ended; the session gets no further calls.
	Exited(exitCode *int64)
}

// AgentSessionBase answers Cancel and Exited without doing anything.
type AgentSessionBase struct{}

// Cancel does nothing.
func (AgentSessionBase) Cancel(uint64) error { return nil }

// Exited does nothing.
func (AgentSessionBase) Exited(*int64) {}

// AgentSessionStarter makes the translation of a session Core started. An
// error leaves the session without a handler: its turns end as failed.
type AgentSessionStarter func(start AgentSessionStart, session *AgentSessionHandle) (AgentSession, error)

// AgentSessionHandle is the host methods of one session.
type AgentSessionHandle struct {
	id   string
	host *Host
}

// ID is Core's ID of the session.
func (s *AgentSessionHandle) ID() string { return s.id }

// Host is the host services outside the session, such as other plugins'
// functions.
func (s *AgentSessionHandle) Host() *Host { return s.host }

// Write sends lithe/host/agents/write: 1-256 lines for the CLI's standard
// input, without line breaks; Core adds the newline.
func (s *AgentSessionHandle) Write(lines []string) error {
	return s.host.Agents().Write(s.id, lines)
}

// Report sends lithe/host/agents/report: 1-256 session events, applied
// together.
func (s *AgentSessionHandle) Report(events []any) error {
	return s.host.Agents().Report(s.id, events)
}

// Agents is the host's agents service. Core answers it; each method needs
// its own grant: Write and Report agents.native, Usage agents.usage.read.
type Agents struct{ host *Host }

// Agents returns the agents service of native agent sessions and usage
// totals (plan API-45).
func (h *Host) Agents() *Agents { return &Agents{host: h} }

// Write writes lines to the standard input of session's CLI.
func (a *Agents) Write(session string, lines []string) error {
	_, err := a.host.Request(methodAgentsWrite, map[string]any{"session": session, "lines": lines})
	return err
}

// Report reports events of session; one refused event refuses them all.
// events is a JSON array of session events.
func (a *Agents) Report(session string, events any) error {
	_, err := a.host.Request(methodAgentsReport, map[string]any{"session": session, "events": events})
	return err
}

// Usage answers { sinceMs, untilMs, summaries, rateLimits } for
// { groupBy, sinceMs?, untilMs?, utcOffsetSeconds? }.
func (a *Agents) Usage(params any) (json.RawMessage, error) {
	value, err := toValue(params)
	if err != nil {
		return nil, invalidHostParams()
	}
	return a.host.requestObject(methodAgentsUsage, value)
}

// OnAgentSessions translates the native sessions of the plugin's agents
// (plan API-45). For each lithe/host/agents/didStart, start makes the
// session's [AgentSession] on a goroutine of the session's own, which then
// gets the session's didOutput, didPrompt, didCancel, didDecidePermission and
// didExit one at a time, in the order Core sent them. When start fails, or a
// prompt fails, the SDK reports turnEnded with reason failed for the turn. A
// later registration replaces the earlier one.
func (b *Backend) OnAgentSessions(start AgentSessionStarter) {
	b.agentSessions = start
}

// agentNotice is one notification for a session's worker.
type agentNotice struct {
	method     string
	lines      []string
	turn       uint64
	text       string
	permission string
	option     string
	exitCode   *int64
}

// agentQueue is an unbounded queue of one session's notifications, so the
// reading goroutine never waits for a handler.
type agentQueue struct {
	mu     sync.Mutex
	ready  *sync.Cond
	items  []agentNotice
	closed bool
}

func newAgentQueue() *agentQueue {
	q := &agentQueue{}
	q.ready = sync.NewCond(&q.mu)
	return q
}

func (q *agentQueue) push(notice agentNotice) {
	q.mu.Lock()
	defer q.mu.Unlock()
	if !q.closed {
		q.items = append(q.items, notice)
		q.ready.Signal()
	}
}

func (q *agentQueue) close() {
	q.mu.Lock()
	defer q.mu.Unlock()
	q.closed = true
	q.ready.Signal()
}

// pop waits for the next notification; false once the queue is closed and
// empty.
func (q *agentQueue) pop() (agentNotice, bool) {
	q.mu.Lock()
	defer q.mu.Unlock()
	for len(q.items) == 0 && !q.closed {
		q.ready.Wait()
	}
	if len(q.items) == 0 {
		return agentNotice{}, false
	}
	notice := q.items[0]
	q.items = q.items[1:]
	return notice, true
}

// agentWorkers is the queues of the live sessions, by session ID.
type agentWorkers struct {
	mu       sync.Mutex
	sessions map[string]*agentQueue
}

// closeAll ends every worker after its queued notifications; used when the
// backend stops serving.
func (w *agentWorkers) closeAll() {
	w.mu.Lock()
	sessions := w.sessions
	w.sessions = nil
	w.mu.Unlock()
	for _, queue := range sessions {
		queue.close()
	}
}

// route hands one lithe/host/agents/did* notification to its session. Core
// checked its shape; a notification of a session the SDK did not start is
// dropped.
func (b *Backend) routeAgentNotification(method string, params map[string]any) {
	s := b.shared
	w := &b.agentWorkers
	session, _ := params["session"].(string)
	if method == methodAgentsDidStart {
		if b.agentSessions == nil {
			s.failed("notification", "agent_unhandled")
			return
		}
		start := AgentSessionStart{Session: session}
		start.AgentID, _ = params["agentID"].(string)
		start.Cwd, _ = params["cwd"].(string)
		if resume, ok := params["resume"].(string); ok {
			start.Resume = &resume
		}
		queue := newAgentQueue()
		w.mu.Lock()
		if w.sessions == nil {
			w.sessions = map[string]*agentQueue{}
		}
		w.sessions[session] = queue
		w.mu.Unlock()
		go runAgentSession(s, b.agentSessions, start, queue)
		return
	}
	notice := agentNotice{method: method}
	notice.turn = unsigned(params["turn"])
	notice.text, _ = params["text"].(string)
	notice.permission, _ = params["permission"].(string)
	notice.option, _ = params["option"].(string)
	if lines, ok := params["lines"].([]any); ok {
		for _, line := range lines {
			text, _ := line.(string)
			notice.lines = append(notice.lines, text)
		}
	}
	if number, ok := params["exitCode"].(json.Number); ok {
		if code, err := number.Int64(); err == nil {
			notice.exitCode = &code
		}
	}
	w.mu.Lock()
	queue := w.sessions[session]
	if method == methodAgentsDidExit {
		delete(w.sessions, session)
	}
	w.mu.Unlock()
	if queue == nil {
		if b.agentSessions != nil {
			s.failed("notification", "agent_session_unknown")
		}
		return
	}
	queue.push(notice)
	if method == methodAgentsDidExit {
		queue.close()
	}
}

func unsigned(value any) uint64 {
	number, _ := value.(json.Number)
	turn, err := number.Int64()
	if err != nil || turn < 0 {
		return 0
	}
	return uint64(turn)
}

// errNoHandler marks a notification of a session whose start failed.
var errNoHandler = errors.New("agent session has no handler")

// runAgentSession starts the session's handler, then runs its notifications
// in order.
func runAgentSession(s *shared, starter AgentSessionStarter, start AgentSessionStart, queue *agentQueue) {
	handle := &AgentSessionHandle{id: start.Session, host: &Host{shared: s}}
	handler, err := startAgentSession(starter, start, handle)
	if err != nil {
		logAgentFailure(s, err)
	}
	for {
		notice, ok := queue.pop()
		if !ok {
			return
		}
		err := errNoHandler
		if handler != nil {
			err = deliverAgentNotice(handler, notice)
			if err != nil {
				logAgentFailure(s, err)
			}
		}
		if err != nil && notice.method == methodAgentsDidPrompt {
			// No handler runs the turn, so it ends here. The handler may have
			// ended it already; Core then refuses the second end.
			_ = handle.Report([]any{map[string]any{"kind": "turnEnded", "turn": notice.turn, "reason": "failed"}})
		}
	}
}

func startAgentSession(starter AgentSessionStarter, start AgentSessionStart, handle *AgentSessionHandle) (handler AgentSession, err error) {
	defer func() {
		if recover() != nil {
			handler, err = nil, errPanicked
		}
	}()
	handler, err = starter(start, handle)
	if err == nil && handler == nil {
		err = errNoHandler
	}
	return handler, err
}

var errPanicked = errors.New("panic")

func deliverAgentNotice(handler AgentSession, notice agentNotice) (err error) {
	defer func() {
		if recover() != nil {
			err = errPanicked
		}
	}()
	switch notice.method {
	case methodAgentsDidOutput:
		return handler.Output(notice.lines)
	case methodAgentsDidPrompt:
		return handler.Prompt(notice.turn, notice.text)
	case methodAgentsDidCancel:
		return handler.Cancel(notice.turn)
	case methodAgentsDidDecide:
		return handler.DecidePermission(notice.turn, notice.permission, notice.option)
	default:
		handler.Exited(notice.exitCode)
		return nil
	}
}

func logAgentFailure(s *shared, err error) {
	switch {
	case errors.Is(err, errPanicked):
		s.failed("notification", "panic")
	case errors.Is(err, errNoHandler):
		s.failed("notification", "internal_error")
	default:
		if kind, _ := classify(err); kind != kindCancelled {
			s.failed("notification", failureKind(err))
		}
	}
}
