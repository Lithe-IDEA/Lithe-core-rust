use super::*;
use serde_json::json;

#[test]
fn document_paths_resolve_inside_lsp_requests_without_changing_virtual_uris() {
    let mut request = json!({"uri":{"path":"C:\\work\\A#B.txt","platform":"auto"},
        "virtualUri":"untitled://contents/A#B", "text":"unchanged",
        "diagnostics":[{"relatedInformation":[{"location":{"uri":{"path":"/work/A#B.txt","platform":"posix"}}},
            {"location":{"uri":"untitled://contents/A#B"}}]}]});
    resolve_request_paths("lsp.request", &mut request).unwrap();
    assert_eq!(request["uri"], "file:///C:/work/A%23B.txt");
    assert_eq!(request["virtualUri"], "untitled://contents/A#B");
    assert_eq!(
        request["diagnostics"][0]["relatedInformation"][0]["location"]["uri"],
        "file:///work/A%23B.txt"
    );
    assert_eq!(
        request["diagnostics"][0]["relatedInformation"][1]["location"]["uri"],
        "untitled://contents/A#B"
    );
    let mut changes = json!({"changes":[
        {"uri":{"path":"/work/A?.txt","platform":"posix"},"kind":"changed"},
        {"uri":"file:///already%20encoded","kind":"deleted"}
    ]});
    resolve_request_paths("lsp.workspaceFilesChanged", &mut changes).unwrap();
    assert_eq!(changes["changes"][0]["uri"], "file:///work/A%3F.txt");
    assert_eq!(changes["changes"][1]["uri"], "file:///already%20encoded");
    let mut invalid = json!({"uri":{"path":"relative","platform":"posix"}});
    assert!(resolve_request_paths("lsp.syncDocument", &mut invalid).is_err());
}
