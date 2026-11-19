package lithe_test

// A scripted host for driving a backend session over in-process pipes.
// Every wait is bounded, so a broken session fails with the frames it sent
// instead of hanging.

import (
	"bufio"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

const wait = 10 * time.Second

// testHost is the host side of one session.
type testHost struct {
	t      *testing.T
	input  *io.PipeWriter
	frames chan map[string]any
	exit   chan int
	closed bool
}

func startHost(t *testing.T, backend *lithe.Backend) *testHost {
	t.Helper()
	backendInput, input := io.Pipe()
	output, backendOutput := io.Pipe()
	host := &testHost{t: t, input: input, frames: make(chan map[string]any, 4096), exit: make(chan int, 1)}
	go func() {
		code := backend.Serve(backendInput, backendOutput)
		// Writes after the session ended fail instead of blocking.
		backendInput.CloseWithError(errors.New("the backend ended"))
		backendOutput.Close()
		host.exit <- code
	}()
	go func() {
		reader := bufio.NewReader(output)
		for {
			frame, ok := readTestFrame(reader)
			if !ok {
				close(host.frames)
				return
			}
			host.frames <- frame
		}
	}()
	t.Cleanup(func() {
		if !host.closed {
			host.input.Close()
			select {
			case <-host.exit:
			case <-time.After(wait):
				t.Errorf("the backend did not end after its input closed")
			}
		}
		output.Close()
	})
	return host
}

func readTestFrame(reader *bufio.Reader) (map[string]any, bool) {
	length := -1
	for {
		line, err := reader.ReadString('\n')
		if err != nil {
			return nil, false
		}
		line = strings.TrimRight(line, "\r\n")
		if line == "" {
			break
		}
		if value, ok := strings.CutPrefix(line, "Content-Length: "); ok {
			if parsed, err := strconv.Atoi(value); err == nil {
				length = parsed
			}
		}
	}
	if length < 0 {
		return nil, false
	}
	body := make([]byte, length)
	if _, err := io.ReadFull(reader, body); err != nil {
		return nil, false
	}
	var frame map[string]any
	if err := json.Unmarshal(body, &frame); err != nil {
		return nil, false
	}
	return frame, true
}

func (h *testHost) sendRaw(data string) {
	h.t.Helper()
	if _, err := io.WriteString(h.input, data); err != nil {
		h.t.Fatalf("write to the backend: %v", err)
	}
}

func (h *testHost) send(message any) {
	h.t.Helper()
	body, err := json.Marshal(message)
	if err != nil {
		h.t.Fatal(err)
	}
	h.sendRaw(fmt.Sprintf("Content-Length: %d\r\n\r\n%s", len(body), body))
}

func (h *testHost) request(id any, method string, params any) {
	h.t.Helper()
	h.send(map[string]any{"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

func (h *testHost) notify(method string, params any) {
	h.t.Helper()
	h.send(map[string]any{"jsonrpc": "2.0", "method": method, "params": params})
}

// reply answers the host request frame with result.
func (h *testHost) reply(frame map[string]any, result any) {
	h.t.Helper()
	h.send(map[string]any{"jsonrpc": "2.0", "id": frame["id"], "result": result})
}

// refuse answers the host request frame with a JSON-RPC error.
func (h *testHost) refuse(frame map[string]any, failure any) {
	h.t.Helper()
	h.send(map[string]any{"jsonrpc": "2.0", "id": frame["id"], "error": failure})
}

func (h *testHost) next() map[string]any {
	h.t.Helper()
	select {
	case frame, ok := <-h.frames:
		if !ok {
			h.t.Fatalf("the backend closed its output")
		}
		return frame
	case <-time.After(wait):
		h.t.Fatalf("no frame from the backend in time")
	}
	return nil
}

// nextWithin is the next frame, if the backend sends one within timeout.
func (h *testHost) nextWithin(timeout time.Duration) (map[string]any, bool) {
	select {
	case frame, ok := <-h.frames:
		return frame, ok
	case <-time.After(timeout):
		return nil, false
	}
}

// assertQuiet fails when the backend sends a frame within a short wait.
func (h *testHost) assertQuiet() {
	h.t.Helper()
	if frame, ok := h.nextWithin(200 * time.Millisecond); ok {
		h.t.Fatalf("unexpected frame: %s", encode(h.t, frame))
	}
}

// answer is the answer to request id, after any frames before it.
func (h *testHost) answer(id any) (map[string]any, []map[string]any) {
	h.t.Helper()
	want := normalize(h.t, id)
	var before []map[string]any
	for {
		frame := h.next()
		if _, isRequest := frame["method"]; !isRequest && reflect.DeepEqual(frame["id"], want) {
			return frame, before
		}
		before = append(before, frame)
	}
}

func (h *testHost) initialize(versions any) any {
	h.t.Helper()
	experimental := map[string]any{"litheLogging": map[string]any{"versions": []any{0}}}
	if versions != nil {
		experimental["litheFunctions"] = map[string]any{"versions": versions}
	}
	h.request(0, "initialize", map[string]any{
		"rootUri":      "file:///tmp/state/",
		"capabilities": map[string]any{"experimental": experimental},
	})
	answer, _ := h.answer(0)
	return answer["result"]
}

func (h *testHost) invoke(id int, function string, params any) {
	h.t.Helper()
	h.invokeWith(id, function, params, nil)
}

// invokeWith is an invocation with extra top-level fields (context, caller,
// or a different operationID) merged over the ones invoke sends.
func (h *testHost) invokeWith(id int, function string, params any, extra map[string]any) {
	h.t.Helper()
	call := map[string]any{"functionID": function, "operationID": fmt.Sprintf("op-%d", id), "params": params}
	for key, value := range extra {
		call[key] = value
	}
	h.request(id, "lithe/function/invoke", call)
}

// close ends the input stream and returns the backend's exit code.
func (h *testHost) close() int {
	h.t.Helper()
	h.closed = true
	h.input.Close()
	return h.exitCode()
}

func (h *testHost) exitCode() int {
	h.t.Helper()
	select {
	case code := <-h.exit:
		h.closed = true
		return code
	case <-time.After(wait):
		h.t.Fatalf("the backend did not end in time")
	}
	return 0
}

// j parses JSON text written in a test.
func j(t *testing.T, text string) any {
	t.Helper()
	var value any
	if err := json.Unmarshal([]byte(text), &value); err != nil {
		t.Fatalf("%s: %v", text, err)
	}
	return value
}

// normalize is value as the harness reads frames: encoded and decoded.
func normalize(t *testing.T, value any) any {
	t.Helper()
	return j(t, encode(t, value))
}

func encode(t *testing.T, value any) string {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

// eq fails unless got is the JSON text want.
func eq(t *testing.T, got any, want string) {
	t.Helper()
	if !reflect.DeepEqual(normalize(t, got), j(t, want)) {
		t.Errorf("got  %s\nwant %s", encode(t, got), encode(t, j(t, want)))
	}
}

// at follows object keys and array indexes through a decoded frame.
func at(value any, path ...any) any {
	for _, step := range path {
		switch key := step.(type) {
		case string:
			object, _ := value.(map[string]any)
			value = object[key]
		case int:
			array, _ := value.([]any)
			if frames, ok := value.([]map[string]any); ok {
				array = make([]any, len(frames))
				for i, frame := range frames {
					array[i] = frame
				}
			}
			if key < 0 || key >= len(array) {
				return nil
			}
			value = array[key]
		}
	}
	return value
}

// code is the error code of an SDK error, "" for any other error.
func code(err error) string {
	var sdk *lithe.SdkError
	if errors.As(err, &sdk) {
		return sdk.Code()
	}
	return ""
}

func must(t *testing.T, err error) {
	t.Helper()
	if err != nil {
		t.Fatal(err)
	}
}

// within receives one value from channel, failing after the bounded wait.
func within[T any](t *testing.T, channel <-chan T) T {
	t.Helper()
	select {
	case value := <-channel:
		return value
	case <-time.After(wait):
		t.Fatalf("no value in time")
	}
	var zero T
	return zero
}

// decode unmarshals a handler's params.
func decode(params json.RawMessage) map[string]any {
	var object map[string]any
	_ = json.Unmarshal(params, &object)
	return object
}
