package lithe

import (
	"fmt"
	"strings"
	"testing"
)

func TestEveryProtocolThisSDKSpeaksIsVersionZero(t *testing.T) {
	if FunctionsVersion != 0 || LoggingVersion != 0 || KeymapImportVersion != 0 {
		t.Fatalf("versions = %d, %d, %d", FunctionsVersion, LoggingVersion, KeymapImportVersion)
	}
}

func TestAnOfferListsTheVersionExactly(t *testing.T) {
	for _, valid := range []string{`{"versions":[0]}`, `{"versions":[99,0]}`} {
		if !offers(mustValue(t, valid), 0) {
			t.Errorf("%s is not an offer of 0", valid)
		}
	}
	for _, invalid := range []string{
		`{"versions":[1]}`,
		`{"versions":[]}`,
		`{"versions":["0",true,null,0.5]}`,
		`{"versions":0}`,
		`{"version":0}`,
		`0`,
		`null`,
	} {
		if offers(mustValue(t, invalid), 0) {
			t.Errorf("%s is an offer of 0", invalid)
		}
	}
	if offers(nil, 0) {
		t.Error("an absent offer offers 0")
	}
}

func TestFunctionIDsAreDotSeparatedLowerCamelCase(t *testing.T) {
	for _, valid := range []string{"snapshot.refresh", "query.runAll2"} {
		if !IsFunctionID(valid) {
			t.Errorf("%q is refused", valid)
		}
	}
	for _, invalid := range []string{"", "Snapshot", "snapshot.", ".a", "a..b", "a-b", "a.B", "1a", strings.Repeat("a", 129)} {
		if IsFunctionID(invalid) {
			t.Errorf("%q is accepted", invalid)
		}
	}
}

func TestPluginErrorCodesAndCredentialKeysUseTheirAlphabets(t *testing.T) {
	if !IsPluginErrorCode("db.connection-failed_2") {
		t.Error("a valid plugin error code is refused")
	}
	if IsPluginErrorCode("2db") || IsPluginErrorCode(strings.Repeat("a", 65)) {
		t.Error("an invalid plugin error code is accepted")
	}
	if !IsCredentialKey("connection:prod.password") {
		t.Error("a valid credential key is refused")
	}
	for _, invalid := range []string{"", "has space", strings.Repeat("k", 129)} {
		if IsCredentialKey(invalid) {
			t.Errorf("credential key %q is accepted", invalid)
		}
	}
	if !IsCommandID("db.refresh-all2") {
		t.Error("a valid command ID is refused")
	}
	for _, invalid := range []string{"", "DB.refresh", "db_refresh", "db refresh", strings.Repeat("a", 129)} {
		if IsCommandID(invalid) {
			t.Errorf("command ID %q is accepted", invalid)
		}
	}
}

func TestParentOperationIDsAreOneToOneHundredTwentyEightBytes(t *testing.T) {
	for _, valid := range []string{"17", strings.Repeat("o", 128), strings.Repeat("é", 64)} {
		if !isParentOperationID(valid) {
			t.Errorf("%q is refused", valid)
		}
	}
	for _, invalid := range []string{"", strings.Repeat("o", 129), strings.Repeat("é", 65)} {
		if isParentOperationID(invalid) {
			t.Errorf("%q is accepted", invalid)
		}
	}
}

func TestGenerationsArePositiveIntegersJSONCarriesExactly(t *testing.T) {
	for text, want := range map[string]uint64{
		"2":                               2,
		"2.0":                             2,
		fmt.Sprint(maxSafeInteger):        maxSafeInteger,
		fmt.Sprintf("%de0", 3):            3,
		fmt.Sprint(maxSafeInteger) + ".0": maxSafeInteger,
	} {
		got, ok := generation(mustValue(t, text))
		if !ok || got != want {
			t.Errorf("generation(%s) = %d, %t", text, got, ok)
		}
	}
	for _, invalid := range []string{"0", "-1", "1.5", `"2"`, "null", fmt.Sprint(maxSafeInteger + 1)} {
		if got, ok := generation(mustValue(t, invalid)); ok {
			t.Errorf("generation(%s) = %d", invalid, got)
		}
	}
}

func TestBoundedTextNeverSplitsACharacter(t *testing.T) {
	for _, c := range []struct {
		text  string
		limit int
		want  string
	}{
		{"abc", 8, "abc"},
		{"aé", 2, "a"},
		{"日本語", 7, "日本"},
	} {
		if got := boundedText(c.text, c.limit); got != c.want {
			t.Errorf("boundedText(%q, %d) = %q", c.text, c.limit, got)
		}
	}
}

func TestDecodeJSONKeepsNumbersExactAndRefusesTrailingData(t *testing.T) {
	assertJSON(t, mustValue(t, `{"big":9007199254740993}`), `{"big":9007199254740993}`)
	if _, err := decodeJSON([]byte(`{} {}`)); err == nil {
		t.Error("trailing data is accepted")
	}
	data, err := encodeJSON(map[string]any{"html": "<a&b>"})
	if err != nil || string(data) != `{"html":"<a&b>"}` {
		t.Errorf("encodeJSON escapes HTML: %s, %v", data, err)
	}
}
