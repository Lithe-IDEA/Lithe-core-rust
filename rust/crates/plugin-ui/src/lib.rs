//! Plugin views: content a plugin draws in a tool window or a settings region
//! through one backend function, checked by Core before any host draws it.
//!
//! A view is an array of nodes. Host-control nodes (rows, sections, labels,
//! inputs, buttons, tables, trees) carry meaning only, and every host draws
//! them with its own native controls. Canvas nodes carry a bounded vector
//! scene the host draws natively, so a plugin can design a distinctive page
//! without a web view. The backend answers each event with a whole view or a
//! batch of patches; [`apply`] turns either into the next checked view, so
//! hosts never keep a second validator or patch engine.
//!
//! Settings regions never store values: a control there names a declared
//! setting and edits the host draft, which the host's Apply commits.
//!
//! Note: the decision is recorded in .agents/notes/proposed/architecture/2026-11-12-plugin-authored-settings-content.md

mod canvas;
mod fields;
mod patch;
mod tree;

use fields::{fail, object};
use lithe_plugin_model::settings::{SettingAction, SettingDeclaration};
use lithe_plugin_model::Validation;
use serde_json::Value;
use std::collections::BTreeSet;

pub use fields::INVALID_VIEW;

/// Largest serialized view, in bytes.
pub const MAX_VIEW_BYTES: usize = 3 * 1024 * 1024;
/// Most host-control nodes of one view; canvas shapes and tree items count separately.
pub const MAX_UI_NODES: usize = 512;
/// Deepest nesting of view nodes, tree items or canvas groups.
pub const MAX_TREE_DEPTH: usize = 16;
/// Most items of one tree.
pub const MAX_TREE_ITEMS: usize = 2000;
/// Most canvases of one view.
pub const MAX_CANVASES: usize = 8;
/// Most nodes of one canvas scene.
pub const MAX_CANVAS_NODES: usize = 2000;
/// Largest serialized canvas scene, in bytes.
pub const MAX_CANVAS_BYTES: usize = 256 * 1024;
/// Most patches of one result.
pub const MAX_PATCHES: usize = 512;
/// Largest absolute canvas coordinate or size.
pub const MAX_COORDINATE: f64 = 1e6;
/// Longest label, title or option text, in characters.
pub const MAX_LABEL_CHARS: usize = 120;
/// Longest body text, value or cell, in characters.
pub const MAX_TEXT_CHARS: usize = 1024;
/// Longest ⓘ explanation or tooltip, in characters.
pub const MAX_INFO_CHARS: usize = 480;

/// Where a view appears; some nodes belong to one surface only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A plugin tool window. Trees appear only here.
    ToolWindow,
    /// A plugin region of a settings page. Setting rows, tool rows, bound
    /// controls and setting-action buttons appear only here.
    Settings,
}

/// What the plugin's manifest lets a view refer to.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// Surface the view is drawn on.
    pub surface: Surface,
    /// Settings the plugin declares; bound controls and setting rows name them.
    pub settings: &'a [SettingDeclaration],
    /// Setting actions the plugin declares; buttons may run them.
    pub actions: &'a [SettingAction],
    /// Tools the plugin defines or requires; tool rows name them.
    pub tools: &'a BTreeSet<String>,
}

/// Checks a complete view against the vocabulary, the budgets and the
/// manifest references of `context`.
pub fn check(context: &Context<'_>, view: &Value) -> Validation<()> {
    check_at(context, view, "view")
}

/// Applies a backend result to the host's current view. A result holds a
/// whole `view`, a batch of `patches`, or neither when nothing changed; the
/// returned view is always complete and checked, with whether it differs
/// from `current`. A patch whose target is missing refuses the whole result,
/// so the host keeps drawing `current`.
pub fn apply(
    context: &Context<'_>,
    current: Option<&Value>,
    result: &Value,
) -> Validation<(Option<Value>, bool)> {
    let map = object(result, "result", &["view", "patches"])?;
    let next = match (map.get("view"), map.get("patches")) {
        (Some(_), Some(_)) => {
            return Err(fail(
                "result",
                "A result returns either a whole view or patches.",
            ));
        }
        (Some(view), None) => {
            check_at(context, view, "result.view")?;
            view.clone()
        }
        (None, Some(patches)) => {
            let Some(current) = current else {
                return Err(fail(
                    "result.patches",
                    "Patches apply to a drawn view; the first result returns a view.",
                ));
            };
            let mut view = current.clone();
            patch::apply(&mut view, patches, "result.patches")?;
            check_at(context, &view, "view")?;
            view
        }
        (None, None) => return Ok((current.cloned(), false)),
    };
    let changed = current != Some(&next);
    Ok((Some(next), changed))
}

fn check_at(context: &Context<'_>, view: &Value, path: &str) -> Validation<()> {
    if serde_json::to_vec(view).map_or(usize::MAX, |bytes| bytes.len()) > MAX_VIEW_BYTES {
        return Err(fail(path, "A view is at most 3 MiB of JSON."));
    }
    tree::Walk::new(context).nodes(view, path, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture {
        settings: Vec<SettingDeclaration>,
        actions: Vec<SettingAction>,
        tools: BTreeSet<String>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                settings: serde_json::from_value(json!([
                    {"id":"offline","type":"boolean","title":"Offline","default":false},
                    {"id":"mode","type":"enum","title":"Mode","default":"fast",
                     "options":[{"value":"fast","title":"Fast"},{"value":"safe","title":"Safe"}]},
                    {"id":"home","type":"directory","title":"Home","default":""}
                ]))
                .expect("settings"),
                actions: serde_json::from_value(json!([
                    {"id":"reindex","title":"Reindex","function":"index.rebuild"}
                ]))
                .expect("actions"),
                tools: BTreeSet::from(["maven".to_owned()]),
            }
        }

        fn context(&self, surface: Surface) -> Context<'_> {
            Context {
                surface,
                settings: &self.settings,
                actions: &self.actions,
                tools: &self.tools,
            }
        }
    }

    fn field(context: &Context<'_>, view: Value) -> String {
        let error = check(context, &view).expect_err("rejected");
        assert_eq!(error.code, INVALID_VIEW);
        error.field.expect("field")
    }

    fn scene(nodes: Value) -> Value {
        json!([{"type":"canvas","id":"map","label":"Dependency map","height":240,
                "viewBox":[0,0,400,240],"nodes":nodes}])
    }

    #[test]
    fn a_tool_window_view_with_every_tool_window_node_passes() {
        let fixture = Fixture::new();
        let view = json!([
            {"type":"section","title":"Summary","info":"What the plugin found.","children":[
                {"type":"row","children":[
                    {"type":"label","text":"3 modules","style":"strong","tone":"accent"},
                    {"type":"button","id":"refresh","text":"Refresh","icon":"refresh","tone":"primary"}
                ]},
                {"type":"keyValue","items":[{"label":"JDK","value":"21","tone":"success"}]},
                {"type":"table","columns":[{"title":"Module"},{"title":"Tests","align":"end"}],
                 "rows":[["core","120"],["ui","48"]]},
                {"type":"banner","text":"Two modules are stale.","tone":"warning"}
            ]},
            {"type":"textField","id":"filter","placeholder":"Filter","width":"fill","value":""},
            {"type":"select","id":"scope","value":"all",
             "options":[{"value":"all","label":"All"},{"value":"main","label":"Main"}]},
            {"type":"segmented","id":"layout","value":"tree",
             "options":[{"value":"tree","label":"Tree"},{"value":"flat","label":"Flat"}]},
            {"type":"toggle","id":"live","label":"Live","value":true},
            {"type":"tree","id":"modules","items":[
                {"id":"core","label":"core","icon":"plugin","expanded":true,"children":[
                    {"id":"core.api","label":"api","detail":"12 classes"}
                ]}
            ]},
            {"type":"canvas","id":"graph","label":"Module graph","height":200,"viewBox":[0,0,400,200],
             "pan":true,"zoom":[0.5,4],"on":["viewport"],"nodes":[
                {"type":"rect","x":0,"y":0,"w":400,"h":200,"radius":8,"fill":{"linear":[[0,"#1E1F22"],[1,"$surface"]],"angle":90}},
                {"type":"path","d":"M 10 10 L 390 190","stroke":{"light":"#3574F0","dark":"#548AF7"},"width":2,"dash":[4,2],"cap":"round"},
                {"type":"group","id":"node.core","x":40,"y":40,"w":120,"h":48,"radius":6,"on":["click"],
                 "label":"core module","tooltip":"core\n120 tests",
                 "states":{"hover":{"fill":"$hover"},"selected":{"stroke":"$accent","width":2}},"children":[
                    {"type":"text","x":60,"y":24,"runs":[{"text":"core "},{"text":"120","color":"$success"}],"style":"strong","align":"center"},
                    {"type":"image","x":4,"y":4,"w":16,"h":16,"src":"assets/module.png"}
                ]}
             ]}
        ]);
        check(&fixture.context(Surface::ToolWindow), &view).expect("valid view");
    }

    #[test]
    fn nodes_belong_to_their_surface() {
        let fixture = Fixture::new();
        let tool_window = fixture.context(Surface::ToolWindow);
        let settings = fixture.context(Surface::Settings);
        assert_eq!(
            field(
                &tool_window,
                json!([{"type":"setting","setting":"offline"}])
            ),
            "view[0]"
        );
        assert_eq!(
            field(
                &tool_window,
                json!([{"type":"toggle","id":"o","label":"Offline","setting":"offline"}])
            ),
            "view[0].setting"
        );
        assert_eq!(
            field(
                &tool_window,
                json!([{"type":"button","id":"r","text":"Reindex","action":"reindex"}])
            ),
            "view[0].action"
        );
        assert_eq!(
            field(&settings, json!([{"type":"tree","id":"t","items":[]}])),
            "view[0]"
        );
        check(
            &settings,
            &json!([
                {"type":"setting","setting":"home"},
                {"type":"tool","tool":"maven"},
                {"type":"toggle","id":"o","label":"Offline","setting":"offline"},
                {"type":"segmented","id":"m","setting":"mode"},
                {"type":"textField","id":"h","setting":"home","width":"fill"},
                {"type":"button","id":"r","text":"Reindex","action":"reindex"}
            ]),
        )
        .expect("settings region view");
    }

    #[test]
    fn bound_controls_match_their_declared_setting() {
        let fixture = Fixture::new();
        let settings = fixture.context(Surface::Settings);
        assert_eq!(
            field(
                &settings,
                json!([{"type":"toggle","id":"m","label":"Mode","setting":"mode"}])
            ),
            "view[0].setting"
        );
        assert_eq!(
            field(
                &settings,
                json!([{"type":"select","id":"m","setting":"mode",
                "options":[{"value":"fast","label":"Fast"}]}])
            ),
            "view[0].options"
        );
        assert_eq!(
            field(
                &settings,
                json!([{"type":"toggle","id":"o","label":"Offline","setting":"offline","value":true}])
            ),
            "view[0].value"
        );
        assert_eq!(
            field(&settings, json!([{"type":"setting","setting":"missing"}])),
            "view[0].setting"
        );
        assert_eq!(
            field(&settings, json!([{"type":"tool","tool":"gradle"}])),
            "view[0].tool"
        );
        assert_eq!(
            field(
                &settings,
                json!([{"type":"button","id":"x","text":"Run","action":"missing"}])
            ),
            "view[0].action"
        );
    }

    #[test]
    fn host_control_nodes_refuse_unknown_and_malformed_fields() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        assert_eq!(
            field(
                &context,
                json!([{"type":"label","text":"x","color":"#FF0000"}])
            ),
            "view[0].color"
        );
        assert_eq!(
            field(&context, json!([{"type":"label","text":"x","tone":null}])),
            "view[0].tone"
        );
        assert_eq!(field(&context, json!([{"type":"marquee"}])), "view[0].type");
        assert_eq!(
            field(
                &context,
                json!([{"type":"button","id":"a","text":"A"},{"type":"button","id":"a","text":"B"}])
            ),
            "view[1].id"
        );
        assert_eq!(
            field(
                &context,
                json!([{"type":"select","id":"s","value":"z",
                "options":[{"value":"a","label":"A"}]}])
            ),
            "view[0].value"
        );
        assert_eq!(
            field(
                &context,
                json!([{"type":"table","columns":[{"title":"A"},{"title":"B"}],"rows":[["only"]]}])
            ),
            "view[0].rows[0]"
        );
        assert_eq!(
            field(
                &context,
                json!([{"type":"button","id":"b","text":"Go","icon":"not a symbol!"}])
            ),
            "view[0].icon"
        );
    }

    #[test]
    fn canvas_scenes_use_the_bounded_vocabulary() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        let cases = [
            (
                json!([{"type":"rect","x":0,"y":0,"w":1,"h":1,"fill":"red"}]),
                "view[0].nodes[0].fill",
            ),
            (
                json!([{"type":"rect","x":0,"y":0,"w":1,"h":1,"fill":"$brand"}]),
                "view[0].nodes[0].fill",
            ),
            (
                json!([{"type":"rect","x":0,"y":0,"w":1,"h":1,"blur":4}]),
                "view[0].nodes[0].blur",
            ),
            (
                json!([{"type":"rect","x":2e6,"y":0,"w":1,"h":1}]),
                "view[0].nodes[0].x",
            ),
            (
                json!([{"type":"path","d":"M0 0 L<script>"}]),
                "view[0].nodes[0].d",
            ),
            (json!([{"type":"text","x":0,"y":0}]), "view[0].nodes[0]"),
            (
                json!([{"type":"text","x":0,"y":0,"text":"a","runs":[{"text":"b"}]}]),
                "view[0].nodes[0]",
            ),
            (
                json!([{"type":"image","x":0,"y":0,"w":1,"h":1,"src":"../secret.png"}]),
                "view[0].nodes[0].src",
            ),
            (
                json!([{"type":"image","x":0,"y":0,"w":1,"h":1,"src":"logo.svg"}]),
                "view[0].nodes[0].src",
            ),
            (
                json!([{"type":"group","id":"g","on":["click"],"label":"G","children":[]}]),
                "view[0].nodes[0]",
            ),
            (
                json!([{"type":"group","id":"g","w":1,"h":1,"on":["click"],"children":[]}]),
                "view[0].nodes[0].label",
            ),
            (
                json!([{"type":"group","w":1,"h":1,"on":["click"],"label":"G","children":[]}]),
                "view[0].nodes[0].id",
            ),
            (
                json!([{"type":"group","states":{"focus":{}},"children":[]}]),
                "view[0].nodes[0].states.focus",
            ),
            (
                json!([{"type":"rect","id":"a","x":0,"y":0,"w":1,"h":1},{"type":"rect","id":"a","x":0,"y":0,"w":1,"h":1}]),
                "view[0].nodes[1].id",
            ),
            (
                json!([{"type":"rect","x":0,"y":0,"w":1,"h":1,"fill":{"linear":[[0,"#000000"]]}}]),
                "view[0].nodes[0].fill.linear",
            ),
            (
                json!([{"type":"rect","x":0,"y":0,"w":1,"h":1,"fill":{"linear":[[0,"#000000"],[2,"#FFFFFF"]]}}]),
                "view[0].nodes[0].fill.linear[1]",
            ),
        ];
        for (nodes, expected) in cases {
            assert_eq!(field(&context, scene(nodes.clone())), expected, "{nodes}");
        }
        let bad_box = json!([{"type":"canvas","id":"c","label":"C","height":10,"viewBox":[0,0,0,10],"nodes":[]}]);
        assert_eq!(field(&context, bad_box), "view[0].viewBox");
    }

    #[test]
    fn canvas_ids_are_scoped_to_their_canvas() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        let canvas = |id: &str| {
            json!({"type":"canvas","id":id,"label":"C","height":10,"viewBox":[0,0,10,10],
                   "nodes":[{"type":"rect","id":"box","x":0,"y":0,"w":1,"h":1}]})
        };
        check(&context, &json!([canvas("a"), canvas("b")])).expect("same shape ID in two canvases");
        assert_eq!(
            field(&context, json!([canvas("a"), canvas("a")])),
            "view[1].id"
        );
    }

    #[test]
    fn budgets_bound_nodes_canvases_and_depth() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        let labels: Vec<_> = (0..=MAX_UI_NODES)
            .map(|_| json!({"type":"label","text":"x"}))
            .collect();
        assert_eq!(
            field(&context, Value::Array(labels)),
            format!("view[{MAX_UI_NODES}]")
        );
        let shapes: Vec<_> = (0..=MAX_CANVAS_NODES)
            .map(|_| json!({"type":"rect","x":0,"y":0,"w":1,"h":1}))
            .collect();
        assert_eq!(
            field(&context, scene(Value::Array(shapes))),
            format!("view[0].nodes[{MAX_CANVAS_NODES}]")
        );
        let canvases: Vec<_> = (0..=MAX_CANVASES)
            .map(|index| json!({"type":"canvas","id":format!("c{index}"),"label":"C","height":10,"viewBox":[0,0,10,10],"nodes":[]}))
            .collect();
        assert_eq!(
            field(&context, Value::Array(canvases)),
            format!("view[{MAX_CANVASES}]")
        );
        let mut deep = json!({"type":"label","text":"leaf"});
        for _ in 0..MAX_TREE_DEPTH {
            deep = json!({"type":"row","children":[deep]});
        }
        assert!(check(&context, &json!([deep])).is_err());
        let big = "M0 0".repeat(70_000);
        assert_eq!(
            field(&context, scene(json!([{"type":"path","d":big}]))),
            "view[0].nodes"
        );
    }

    fn drawn() -> Value {
        json!([
            {"type":"section","id":"summary","title":"Summary","children":[
                {"type":"label","id":"count","text":"3 modules"}
            ]},
            {"type":"tree","id":"modules","items":[{"id":"core","label":"core"}]},
            {"type":"canvas","id":"graph","label":"Graph","height":100,"viewBox":[0,0,100,100],"nodes":[
                {"type":"group","id":"node.core","children":[
                    {"type":"rect","id":"box","x":0,"y":0,"w":10,"h":10,"fill":"$surface"}
                ]}
            ]}
        ])
    }

    #[test]
    fn results_return_a_whole_view_patches_or_nothing() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        let current = drawn();
        let (view, changed) = apply(&context, None, &json!({"view": current})).expect("first view");
        assert_eq!(view.as_ref(), Some(&current));
        assert!(changed);
        let (view, changed) = apply(&context, Some(&current), &json!({})).expect("no change");
        assert_eq!(view.as_ref(), Some(&current));
        assert!(!changed);
        let (_, changed) =
            apply(&context, Some(&current), &json!({"view": current})).expect("same view");
        assert!(!changed);
        let both = apply(&context, Some(&current), &json!({"view":[],"patches":[]}));
        assert_eq!(both.expect_err("both").field.as_deref(), Some("result"));
        let first = apply(&context, None, &json!({"patches":[]}));
        assert_eq!(
            first.expect_err("no view").field.as_deref(),
            Some("result.patches")
        );
        let bad = apply(&context, None, &json!({"view":[{"type":"label"}]}));
        assert_eq!(
            bad.expect_err("bad").field.as_deref(),
            Some("result.view[0].text")
        );
    }

    #[test]
    fn patches_edit_view_tree_and_canvas_nodes() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        let current = drawn();
        let result = json!({"patches":[
            {"op":"set","id":"count","fields":{"text":"4 modules","tone":"accent"}},
            {"op":"insert","parent":"summary","node":{"type":"banner","id":"stale","text":"1 stale","tone":"warning"}},
            {"op":"insert","index":0,"node":{"type":"label","id":"top","text":"Top"}},
            {"op":"insert","tree":"modules","parent":"core","node":{"id":"core.api","label":"api"}},
            {"op":"set","tree":"modules","id":"core","fields":{"expanded":true}},
            {"op":"set","canvas":"graph","id":"box","fields":{"fill":"$accent"}},
            {"op":"insert","canvas":"graph","node":{"type":"rect","id":"edge","x":0,"y":0,"w":1,"h":1}},
            {"op":"set","id":"graph","fields":{"viewBox":[0,0,200,100]}},
            {"op":"remove","canvas":"graph","id":"edge"},
            {"op":"replace","id":"stale","node":{"type":"banner","id":"fresh","text":"All fresh","tone":"success"}}
        ]});
        let (view, changed) = apply(&context, Some(&current), &result).expect("patched");
        let view = view.expect("view");
        assert!(changed);
        assert_eq!(view[0]["id"], "top");
        assert_eq!(view[1]["children"][0]["text"], "4 modules");
        assert_eq!(view[1]["children"][0]["tone"], "accent");
        assert_eq!(view[1]["children"][1]["id"], "fresh");
        assert_eq!(view[2]["items"][0]["expanded"], true);
        assert_eq!(view[2]["items"][0]["children"][0]["id"], "core.api");
        assert_eq!(view[3]["viewBox"], json!([0, 0, 200, 100]));
        assert_eq!(view[3]["nodes"][0]["children"][0]["fill"], "$accent");
        assert_eq!(view[3]["nodes"].as_array().map(Vec::len), Some(1));
        let removed = apply(
            &context,
            Some(&current),
            &json!({"patches":[{"op":"set","id":"count","fields":{"tone":"accent"}},
                               {"op":"set","id":"count","fields":{"tone":null}}]}),
        )
        .expect("null removes a field");
        assert!(!removed.1);
    }

    #[test]
    fn a_patch_that_misses_refuses_the_whole_result() {
        let fixture = Fixture::new();
        let context = fixture.context(Surface::ToolWindow);
        let current = drawn();
        let cases = [
            (
                json!([{"op":"set","id":"count","fields":{"text":"ok"}},{"op":"remove","id":"gone"}]),
                "result.patches[1].id",
            ),
            (
                json!([{"op":"set","id":"box","fields":{"fill":"$accent"}}]),
                "result.patches[0].id",
            ),
            (
                json!([{"op":"set","canvas":"modules","id":"core","fields":{}}]),
                "result.patches[0].canvas",
            ),
            (
                json!([{"op":"set","id":"count","fields":{"type":"banner"}}]),
                "result.patches[0].fields.type",
            ),
            (
                json!([{"op":"insert","parent":"count","node":{"type":"label","text":"x"}}]),
                "result.patches[0].parent",
            ),
            (
                json!([{"op":"insert","index":9,"node":{"type":"label","text":"x"}}]),
                "result.patches[0].index",
            ),
            (json!([{"op":"move","id":"count"}]), "result.patches[0].op"),
            (
                json!([{"op":"set","id":"count","fields":{"text":"ok"},"node":{}}]),
                "result.patches[0].node",
            ),
            (
                json!([{"op":"set","id":"count","fields":{"style":"huge"}}]),
                "view[0].children[0].style",
            ),
        ];
        for (patches, expected) in cases {
            let error =
                apply(&context, Some(&current), &json!({"patches": patches})).expect_err("refused");
            assert_eq!(error.field.as_deref(), Some(expected), "{patches}");
        }
        let many: Vec<_> = (0..=MAX_PATCHES)
            .map(|_| json!({"op":"set","id":"count","fields":{"text":"x"}}))
            .collect();
        let error = apply(&context, Some(&current), &json!({"patches": many})).expect_err("many");
        assert_eq!(error.field.as_deref(), Some("result.patches"));
    }
}
