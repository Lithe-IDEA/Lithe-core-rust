package lithe

import (
	"encoding/json"
	"errors"
	"testing"
)

// viewEvent is a change event with extra fields merged over it.
func viewEvent(t *testing.T, extra string) json.RawMessage {
	t.Helper()
	params := mustValue(t, `{
		"event": "change",
		"instance": "i1",
		"surface": {"settingRegionID": "otp"},
		"language": "zh-Hans",
		"settings": {"effective": {"otpRelease": "26"}, "draft": {"otpRelease": "27"}}
	}`).(map[string]any)
	for key, value := range mustValue(t, extra).(map[string]any) {
		params[key] = value
	}
	return raw(params)
}

func TestParseViewEventReadsEveryFieldTheHostSends(t *testing.T) {
	parsed, err := ParseViewEvent(viewEvent(t, `{
		"node": "otp", "setting": "otpRelease", "value": "27",
		"viewport": {"x": 0, "y": 10.5, "width": 1200, "height": 640},
		"width": 612
	}`))
	if err != nil {
		t.Fatal(err)
	}
	if parsed.Kind != ViewChange || parsed.Instance != "i1" || parsed.Language != "zh-Hans" {
		t.Errorf("kind, instance, language = %s, %s, %s", parsed.Kind, parsed.Instance, parsed.Language)
	}
	if parsed.Surface != (ViewSurface{SurfaceSettingRegion, "otp"}) {
		t.Errorf("surface = %+v", parsed.Surface)
	}
	if parsed.Draft["otpRelease"] != "27" || parsed.Effective["otpRelease"] != "26" {
		t.Errorf("settings = %v, %v", parsed.Effective, parsed.Draft)
	}
	if parsed.Node != "otp" || parsed.Setting != "otpRelease" || parsed.Value != "27" || parsed.Item != "" {
		t.Errorf("node, setting, value, item = %q, %q, %v, %q", parsed.Node, parsed.Setting, parsed.Value, parsed.Item)
	}
	if parsed.Viewport == nil || *parsed.Viewport != (Viewport{0, 10.5, 1200, 640}) {
		t.Errorf("viewport = %+v", parsed.Viewport)
	}
	if parsed.Width == nil || *parsed.Width != 612 {
		t.Errorf("width = %v", parsed.Width)
	}

	// An optional field sent as null counts as absent.
	absent, err := ParseViewEvent(viewEvent(t, `{"node": null, "value": null, "viewport": null, "width": null}`))
	if err != nil {
		t.Fatal(err)
	}
	if absent.Node != "" || absent.Value != nil || absent.Viewport != nil || absent.Width != nil {
		t.Errorf("absent = %+v", absent)
	}

	resize, err := ParseViewEvent(viewEvent(t, `{"event": "resize", "width": 340}`))
	if err != nil {
		t.Fatal(err)
	}
	if resize.Kind != ViewResize || resize.Width == nil || *resize.Width != 340 {
		t.Errorf("resize = %s, %v", resize.Kind, resize.Width)
	}

	toggle, err := ParseViewEvent(viewEvent(t, `{"surface": {"toolWindowID": "deps"}, "value": true}`))
	if err != nil {
		t.Fatal(err)
	}
	if toggle.Surface != (ViewSurface{SurfaceToolWindow, "deps"}) || toggle.Value != true {
		t.Errorf("toggle = %+v, %v", toggle.Surface, toggle.Value)
	}
}

func TestParseViewEventRefusesEventsWithoutTheirRequiredShape(t *testing.T) {
	invalid := []json.RawMessage{
		json.RawMessage(`null`),
		json.RawMessage(`not json`),
		viewEvent(t, `{"event": "hover"}`),
		viewEvent(t, `{"surface": {"toolWindowID": "a", "settingRegionID": "b"}}`),
		viewEvent(t, `{"surface": {"toolWindowID": "a", "settingRegionID": 3}}`),
		viewEvent(t, `{"surface": {}}`),
		viewEvent(t, `{"settings": {"effective": {}}}`),
		viewEvent(t, `{"node": 3}`),
		viewEvent(t, `{"value": ["a"]}`),
		viewEvent(t, `{"viewport": {"x": 0}}`),
		viewEvent(t, `{"width": "wide"}`),
	}
	for _, params := range invalid {
		_, err := ParseViewEvent(params)
		var sdk *SdkError
		if !errors.As(err, &sdk) || sdk.Code() != codeInvalidViewEvent {
			t.Errorf("%s: %v", params, err)
		}
	}
}

func TestViewResultsAndPatchesSerializeAsTheSchemaNamesThem(t *testing.T) {
	assertJSON(t, Unchanged(), `{}`)
	assertJSON(t, ViewResult{}, `{}`)
	assertJSON(t, View(map[string]any{"type": "label", "text": "a"}), `{"view": [{"type": "label", "text": "a"}]}`)
	assertJSON(t, View(), `{"view": []}`)
	assertJSON(t, Patches(), `{"patches": []}`)
	patches := Patches(
		PatchSet("filter", map[string]any{"enabled": false}),
		PatchRemove("guava").InCanvas("graph"),
		PatchInsert(map[string]any{"id": "web", "label": "web"}).InTree("modules").Under("app").At(0),
		PatchReplace("refresh", map[string]any{"type": "button", "id": "refresh", "text": "Stop"}),
	)
	assertJSON(t, patches, `{"patches": [
		{"op": "set", "id": "filter", "fields": {"enabled": false}},
		{"op": "remove", "id": "guava", "canvas": "graph"},
		{"op": "insert", "node": {"id": "web", "label": "web"}, "tree": "modules", "parent": "app", "index": 0},
		{"op": "replace", "id": "refresh", "node": {"type": "button", "id": "refresh", "text": "Stop"}}
	]}`)
	// Each method returns a new patch.
	base := PatchRemove("a")
	_ = base.InCanvas("graph")
	assertJSON(t, base, `{"op": "remove", "id": "a"}`)
}
