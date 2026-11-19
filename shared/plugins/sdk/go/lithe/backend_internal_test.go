package lithe

import (
	"errors"
	"runtime"
	"slices"
	"sync"
	"testing"
	"time"
)

func TestFileURIsBecomePaths(t *testing.T) {
	if runtime.GOOS == "windows" {
		if path, ok := fileURIPath("file:///C:/Users/me/"); !ok || path != "C:/Users/me" {
			t.Errorf("drive path = %q, %t", path, ok)
		}
	}
	for uri, want := range map[string]string{
		"file:///var/state/":            "/var/state",
		"file://localhost/a%20b/%C3%A9": "/a b/é",
		"file:///":                      "/",
	} {
		if path, ok := fileURIPath(uri); !ok || path != want {
			t.Errorf("%s = %q, %t", uri, path, ok)
		}
	}
	for _, invalid := range []string{"https://example.com/", "file:///bad%zz", "file://host/share", "file:///%FF"} {
		if path, ok := fileURIPath(invalid); ok {
			t.Errorf("%s = %q", invalid, path)
		}
	}
}

func TestCancellationWakesWaitersAndRunsCallbacksOnce(t *testing.T) {
	cancellation := NewCancellation()
	if cancellation.WaitTimeout(0) {
		t.Fatal("a new token is cancelled")
	}
	var (
		mu       sync.Mutex
		received []string
	)
	record := func(name string) func() {
		return func() {
			mu.Lock()
			received = append(received, name)
			mu.Unlock()
		}
	}
	cancellation.OnCancel(record("registered"))
	context := cancellation.Context()
	if context.Err() != nil {
		t.Fatal("the context of a new token is done")
	}
	woke := make(chan bool, 1)
	go func() { woke <- cancellation.WaitTimeout(10 * time.Second) }()
	cancellation.Cancel()
	cancellation.Cancel()
	select {
	case cancelled := <-woke:
		if !cancelled {
			t.Error("the waiter saw no cancellation")
		}
	case <-time.After(10 * time.Second):
		t.Fatal("the waiter did not wake")
	}
	cancellation.OnCancel(record("late"))
	mu.Lock()
	got := slices.Clone(received)
	mu.Unlock()
	if !slices.Equal(got, []string{"registered", "late"}) {
		t.Errorf("callbacks = %v", got)
	}
	if !errors.Is(cancellation.Check(), ErrCancelled) {
		t.Error("Check does not report the cancellation")
	}
	select {
	case <-context.Done():
	case <-time.After(10 * time.Second):
		t.Fatal("the token's context was not cancelled")
	}
	if cancellation.Context() != context {
		t.Error("Context returns a new context")
	}
	select {
	case <-cancellation.Done():
	default:
		t.Error("Done is open after Cancel")
	}
}

func TestAContextOfACancelledTokenIsDone(t *testing.T) {
	cancellation := NewCancellation()
	cancellation.Cancel()
	if cancellation.Context().Err() == nil {
		t.Error("the context of a cancelled token is not done")
	}
}

func TestAWatchCanBeWithdrawn(t *testing.T) {
	cancellation := NewCancellation()
	ran := false
	id, ok := cancellation.watch(func() { ran = true })
	if !ok {
		t.Fatal("a new token refused a watch")
	}
	cancellation.unwatch(id)
	cancellation.Cancel()
	if ran {
		t.Error("a withdrawn callback ran")
	}
	if _, ok := cancellation.watch(func() {}); ok {
		t.Error("a cancelled token accepted a watch")
	}
}
