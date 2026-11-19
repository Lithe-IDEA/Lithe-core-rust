package lithe_test

// A view function is a plugin function whose params are a view event.

import (
	"testing"

	"github.com/Lithe-IDEA/Lithe-core-rust/shared/plugins/sdk/go/lithe"
)

func viewParams(event string) map[string]any {
	return map[string]any{
		"event":    event,
		"instance": "i1",
		"surface":  map[string]any{"toolWindowID": "dev.example.explorer"},
		"language": "zh-Hans",
		"settings": map[string]any{"effective": map[string]any{}, "draft": map[string]any{}},
		"node":     "refresh",
	}
}

func TestAViewFunctionAnswersViewsPatchesAndNothing(t *testing.T) {
	surfaces := make(chan lithe.ViewSurface, 4)
	backend := lithe.NewBackend("test-backend")
	must(t, backend.View("explorer.draw", func(event *lithe.ViewEvent, _ *lithe.Context) (lithe.ViewResult, error) {
		surfaces <- event.Surface
		switch event.Kind {
		case lithe.ViewOpen:
			return lithe.View(map[string]any{"type": "label", "text": event.Language}), nil
		case lithe.ViewClick:
			return lithe.Patches(lithe.PatchSet(event.Node, map[string]any{"enabled": false})), nil
		}
		return lithe.Unchanged(), nil
	}))
	host := startHost(t, backend)
	eq(t, at(host.initialize([]any{0}), "capabilities", "experimental", "litheFunctions", "functions"), `[{"id": "explorer.draw"}]`)

	host.invoke(1, "explorer.draw", viewParams("open"))
	answer, _ := host.answer(1)
	eq(t, answer["result"], `{"view": [{"type": "label", "text": "zh-Hans"}]}`)
	host.invoke(2, "explorer.draw", viewParams("click"))
	answer, _ = host.answer(2)
	eq(t, answer["result"], `{"patches": [{"op": "set", "id": "refresh", "fields": {"enabled": false}}]}`)
	host.invoke(3, "explorer.draw", viewParams("close"))
	answer, _ = host.answer(3)
	eq(t, answer["result"], `{}`)
	for range 3 {
		if surface := within(t, surfaces); surface != (lithe.ViewSurface{Kind: lithe.SurfaceToolWindow, ID: "dev.example.explorer"}) {
			t.Errorf("surface = %+v", surface)
		}
	}
	// Params that are not a view event never reach the handler.
	host.invoke(4, "explorer.draw", map[string]any{"event": "open"})
	answer, _ = host.answer(4)
	eq(t, at(answer, "error", "code"), `-32603`)
	if exit := host.close(); exit != 1 {
		t.Errorf("exit code = %d", exit)
	}
	select {
	case surface := <-surfaces:
		t.Errorf("an invalid event reached the handler: %+v", surface)
	default:
	}
}
