//! A view function is a plugin function whose params are a view event.
mod common;

use common::Host;
use lithe_plugin_sdk::view::{Patch, ViewEventKind, ViewResult, ViewSurface};
use lithe_plugin_sdk::Backend;
use serde_json::json;

fn params(event: &str) -> serde_json::Value {
    json!({
        "event": event,
        "instance": "i1",
        "surface": { "toolWindowID": "dev.example.explorer" },
        "language": "zh-Hans",
        "settings": { "effective": {}, "draft": {} },
        "node": "refresh",
    })
}

#[test]
fn a_view_function_answers_views_patches_and_nothing() {
    let mut backend = Backend::new("test-backend");
    backend
        .view("explorer.draw", |event, _context| {
            assert_eq!(
                event.surface,
                ViewSurface::ToolWindow("dev.example.explorer".into())
            );
            Ok(match event.kind {
                ViewEventKind::Open => {
                    ViewResult::View(vec![json!({ "type": "label", "text": event.language })])
                }
                ViewEventKind::Click => ViewResult::Patches(vec![Patch::set(
                    event.node.as_deref().unwrap_or_default(),
                    json!({ "enabled": false }),
                )]),
                _ => ViewResult::Unchanged,
            })
        })
        .unwrap();
    let mut host = Host::start(backend);
    let answer = host.initialize(json!([0]));
    assert_eq!(
        answer["capabilities"]["experimental"]["litheFunctions"]["functions"],
        json!([{ "id": "explorer.draw" }])
    );

    host.invoke(1, "explorer.draw", params("open"));
    assert_eq!(
        host.answer(1).0["result"],
        json!({ "view": [{ "type": "label", "text": "zh-Hans" }] })
    );
    host.invoke(2, "explorer.draw", params("click"));
    assert_eq!(
        host.answer(2).0["result"],
        json!({ "patches": [{ "op": "set", "id": "refresh", "fields": { "enabled": false } }] })
    );
    host.invoke(3, "explorer.draw", params("close"));
    assert_eq!(host.answer(3).0["result"], json!({}));
    // Params that are not a view event never reach the handler.
    host.invoke(4, "explorer.draw", json!({ "event": "open" }));
    assert_eq!(host.answer(4).0["error"]["code"], -32603);
    assert_eq!(host.close(), 1);
}
