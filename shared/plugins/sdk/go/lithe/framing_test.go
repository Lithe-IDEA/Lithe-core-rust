package lithe

import (
	"bufio"
	"bytes"
	"fmt"
	"reflect"
	"strings"
	"testing"
)

type readResult struct {
	frame *frame
	err   error
}

// readAll reads frames until the input ends or a frame fails, through a
// small buffer so headers cross buffer boundaries.
func readAll(input []byte) []readResult {
	reader := bufio.NewReaderSize(bytes.NewReader(input), 16)
	var results []readResult
	for {
		message, err := readFrame(reader)
		results = append(results, readResult{message, err})
		if err != nil || message == nil {
			return results
		}
	}
}

func appendFrame(t *testing.T, input *bytes.Buffer, value any) {
	t.Helper()
	body, err := encodeJSON(value)
	if err != nil {
		t.Fatal(err)
	}
	writer := bufio.NewWriter(input)
	if err := writeFrame(writer, body); err != nil {
		t.Fatal(err)
	}
}

func TestReadFrameDecodesConsecutiveFramesAndReportsMalformedBodiesInPlace(t *testing.T) {
	var input bytes.Buffer
	appendFrame(t, &input, map[string]any{"id": 1})
	input.WriteString("content-length: 3\r\nContent-Type: x\r\n\r\nnot")
	appendFrame(t, &input, map[string]any{"id": "é"})
	results := readAll(input.Bytes())
	if len(results) != 4 {
		t.Fatalf("got %d results, want 4", len(results))
	}
	for index, result := range results {
		if result.err != nil {
			t.Fatalf("frame %d failed: %v", index, result.err)
		}
	}
	if got := canonical(t, results[0].frame.value); got != `{"id":1}` {
		t.Errorf("first frame = %s", got)
	}
	if !results[1].frame.malformed {
		t.Errorf("second frame is not malformed")
	}
	if got := canonical(t, results[2].frame.value); got != `{"id":"é"}` {
		t.Errorf("third frame = %s", got)
	}
	if results[3].frame != nil {
		t.Errorf("the input end is a frame")
	}
}

func TestReadFrameRefusesFramesItCannotDelimit(t *testing.T) {
	cases := []struct{ input, want string }{
		{"Content-Type: x\r\n\r\n{}", "no valid Content-Length"},
		{"Content-Length: -1\r\n\r\n", "no valid Content-Length"},
		{fmt.Sprintf("Content-Length: %d\r\n\r\n", maxMessageBytes+1), "exceeds"},
		{"X: " + strings.Repeat("a", maxHeaderBytes) + "\r\n\r\n", "too long"},
		{"Content-Length: 2\r\n", "inside a message header"},
		{"Content-Length: 4\r\n\r\n{}", "input failed"},
	}
	for _, c := range cases {
		results := readAll([]byte(c.input))
		last := results[len(results)-1]
		if last.err == nil {
			t.Errorf("%q: no error", c.input)
			continue
		}
		if !strings.Contains(last.err.Error(), c.want) {
			t.Errorf("%q: %v", c.input, last.err)
		}
	}
}

// canonical is value as compact JSON with sorted keys.
func canonical(t *testing.T, value any) string {
	t.Helper()
	data, err := encodeJSON(value)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := decodeJSON(data)
	if err != nil {
		t.Fatal(err)
	}
	data, err = encodeJSON(decoded)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

// mustValue decodes JSON text for a test.
func mustValue(t *testing.T, text string) any {
	t.Helper()
	value, err := decodeJSON([]byte(text))
	if err != nil {
		t.Fatalf("%s: %v", text, err)
	}
	return value
}

func assertJSON(t *testing.T, got any, want string) {
	t.Helper()
	if !reflect.DeepEqual(mustValue(t, canonical(t, got)), mustValue(t, want)) {
		t.Errorf("got %s, want %s", canonical(t, got), canonical(t, mustValue(t, want)))
	}
}
