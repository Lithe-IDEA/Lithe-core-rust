package lithe

import (
	"errors"
	"fmt"
	"strings"
	"testing"
)

func TestOnlyPluginErrorsReachTheHostWithTheirMessageAndCode(t *testing.T) {
	plugin := responseError(NewPluginError("db.locked", strings.Repeat("é", 600)))
	if plugin["code"] != rpcRequestFailed {
		t.Errorf("code = %v", plugin["code"])
	}
	if code := plugin["data"].(map[string]any)["code"]; code != "db.locked" {
		t.Errorf("data.code = %v", code)
	}
	if message := plugin["message"].(string); len(message) != 1024 {
		t.Errorf("message is %d bytes", len(message))
	}
	// A wrapped plugin error still reaches the host.
	wrapped := responseError(fmt.Errorf("context: %w", NewPluginError("db.locked", "Locked.")))
	if wrapped["message"] != "Locked." {
		t.Errorf("wrapped = %v", wrapped)
	}
	assertJSON(t, responseError(errors.New("/home/me/secret: No such file")), canonical(t, internalResponse()))
	if responseError(ErrCancelled)["code"] != rpcRequestCancelled {
		t.Error("a cancelled call is not answered as cancelled")
	}
	// A host request cut short by the call's cancellation ends the call as
	// cancelled.
	if !errors.Is(cancelledError(), ErrCancelled) {
		t.Error("a cancelled host request is not ErrCancelled")
	}
	if responseError(cancelledError())["code"] != rpcRequestCancelled {
		t.Error("a cancelled host request is not answered as cancelled")
	}
}

func TestHostFailuresKeepTheirCodeOrTakeItFromTheNumber(t *testing.T) {
	coded := sdkErrorFromResponse(methodCredentialsGet, mustValue(t,
		`{"code":-32010,"message":"No.","data":{"code":"plugin.host.permissionDenied","details":{"a":1}}}`))
	if coded.Code() != "plugin.host.permissionDenied" {
		t.Errorf("code = %s", coded.Code())
	}
	if number, ok := coded.RPCCode(); !ok || number != -32010 {
		t.Errorf("rpc code = %d, %t", number, ok)
	}
	assertJSON(t, coded.Details(), `{"a":1}`)
	for _, known := range HostErrorCodes {
		failure := sdkErrorFromResponse(methodCredentialsGet, mustValue(t, fmt.Sprintf(`{"code":%d}`, known.Number)))
		if failure.Code() != known.Code {
			t.Errorf("%d: code = %s, want %s", known.Number, failure.Code(), known.Code)
		}
		if failure.Message() != "lithe/host/credentials/get failed." {
			t.Errorf("%d: message = %s", known.Number, failure.Message())
		}
	}
	later := sdkErrorFromResponse(methodCredentialsGet, mustValue(t,
		`{"code":-32050,"message":"x","data":{"details":["not","an","object"]}}`))
	if later.Code() != "plugin.host.failed" || later.Details() != nil {
		t.Errorf("later = %s, %v", later.Code(), later.Details())
	}
	other := sdkErrorFromResponse(methodRegister, mustValue(t, `{"code":-32602}`))
	if other.Code() != "sdk.host.error" {
		t.Errorf("other = %s", other.Code())
	}
}

func TestTheHostAndRelayTablesHoldEveryDocumentedCodeOnce(t *testing.T) {
	if len(HostErrorCodes) != 23 || len(RelayErrorCodes) != 11 {
		t.Fatalf("tables hold %d and %d codes", len(HostErrorCodes), len(RelayErrorCodes))
	}
	codes := map[string]bool{}
	numbers := map[int64]bool{}
	for _, table := range []struct {
		prefix string
		codes  []ErrorCode
	}{{"plugin.host.", HostErrorCodes}, {"plugin.function.", RelayErrorCodes}} {
		for _, known := range table.codes {
			if !strings.HasPrefix(known.Code, table.prefix) {
				t.Errorf("%s does not start with %s", known.Code, table.prefix)
			}
			if codes[known.Code] {
				t.Errorf("%s is listed twice", known.Code)
			}
			if numbers[known.Number] {
				t.Errorf("%d names two codes", known.Number)
			}
			codes[known.Code], numbers[known.Number] = true, true
		}
	}
}

func TestRelayFailuresKeepTheirCodeNumberAndTheProvidersOwnCode(t *testing.T) {
	for _, known := range RelayErrorCodes {
		failure := sdkErrorFromResponse(methodRelayInvoke, mustValue(t, fmt.Sprintf(`{"code":%d}`, known.Number)))
		number, _ := failure.RPCCode()
		if failure.Code() != known.Code || number != known.Number {
			t.Errorf("%d: %s, %d", known.Number, failure.Code(), number)
		}
		if failure.Message() != "lithe/host/functions/invoke failed." {
			t.Errorf("%d: message = %s", known.Number, failure.Message())
		}
	}
	failed := sdkErrorFromResponse(methodRelayInvoke, mustValue(t, `{
		"code": -32070,
		"message": "The plugin function failed.",
		"data": {"code": "plugin.function.failed", "details": {"pluginErrorCode": "go.noModule"}}
	}`))
	if failed.Code() != "plugin.function.failed" || failed.Message() != "The plugin function failed." {
		t.Errorf("failed = %s, %s", failed.Code(), failed.Message())
	}
	assertJSON(t, failed.Details(), `{"pluginErrorCode":"go.noModule"}`)
	// Outside a host method a relay number stays an unnamed answer.
	outside := sdkErrorFromResponse(methodRegister, mustValue(t, `{"code":-32070}`))
	if outside.Code() != "sdk.host.error" {
		t.Errorf("outside = %s", outside.Code())
	}
}

func TestAnInvalidPluginErrorCodeIsAProgrammingError(t *testing.T) {
	defer func() {
		message, _ := recover().(string)
		if !strings.Contains(message, "must start with a letter") {
			t.Errorf("panic = %q", message)
		}
	}()
	NewPluginError("1bad", "message")
}
