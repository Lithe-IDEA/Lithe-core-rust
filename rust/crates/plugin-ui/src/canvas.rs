//! Canvas nodes: a bounded vector scene the host draws natively. A scene is
//! rects, paths, text, package images and groups in view-box coordinates.
//! Groups may react to hover, press and selection without a round trip and
//! report clicks; pan, zoom, hover and tooltips stay local to the host.
//!
//! Colors are `#RRGGBB`, `#RRGGBBAA`, a `$token` the host maps to its theme,
//! a `{light, dark}` pair, or a `{linear, angle}` gradient. Blur, filters,
//! blend modes, radial gradients, custom fonts and animation are not part of
//! the vocabulary.

use crate::fields::{
    array, at, boolean, choice, events, fail, identifier, integer, line, number, numbers, object,
    string, text, Checked, Object,
};
use crate::tree::Walk;
use crate::{
    MAX_CANVASES, MAX_CANVAS_BYTES, MAX_CANVAS_NODES, MAX_COORDINATE, MAX_INFO_CHARS,
    MAX_LABEL_CHARS, MAX_TEXT_CHARS, MAX_TREE_DEPTH,
};
use lithe_plugin_model::is_safe_relative_path;
use serde_json::Value;
use std::collections::BTreeSet;

/// Theme colors a canvas may name with `$`.
pub(crate) const COLOR_TOKENS: &[&str] = &[
    "text",
    "textSecondary",
    "textMuted",
    "accent",
    "success",
    "warning",
    "error",
    "surface",
    "background",
    "sidebar",
    "line",
    "hover",
    "selection",
    "badge",
];
/// Text styles of canvas text, mapped to the host type scale.
const TEXT_STYLES: &[&str] = &[
    "title",
    "strong",
    "body",
    "caption",
    "display",
    "mono",
    "monoSmall",
];
/// Any coordinate.
const COORDINATE: (f64, f64) = (-MAX_COORDINATE, MAX_COORDINATE);
/// Any size, stroke width or radius.
const EXTENT: (f64, f64) = (0.0, MAX_COORDINATE);
/// Opacity.
const UNIT: (f64, f64) = (0.0, 1.0);
/// Group scale.
const SCALE: (f64, f64) = (0.1, 10.0);
/// Shadow elevation steps.
const SHADOW: (u64, u64) = (1, 3);
/// Longest path data, in bytes.
const MAX_PATH_DATA: usize = 8192;
/// Most runs of one text node and most stops of one gradient.
const MAX_RUNS: usize = 64;
const GRADIENT_STOPS: (usize, usize) = (2, 8);
/// Tallest canvas, in points.
const MAX_HEIGHT: u64 = 2000;

/// Checks the canvas node at `path` and its scene.
pub(crate) fn check(walk: &mut Walk<'_>, value: &Value, path: &str) -> Checked<()> {
    let map = object(
        value,
        path,
        &[
            "type", "id", "label", "height", "viewBox", "pan", "zoom", "on", "nodes",
        ],
    )?;
    walk.canvases += 1;
    if walk.canvases > MAX_CANVASES {
        return Err(fail(path, "A view has at most 8 canvases."));
    }
    walk.id(map, path, true)?;
    line(map, path, "label", MAX_LABEL_CHARS, true)?;
    integer(map, path, "height", (1, MAX_HEIGHT), true)?;
    let view_box = numbers(map, path, "viewBox", (4, 4), COORDINATE, true)?.unwrap_or_default();
    if view_box[2] <= 0.0 || view_box[3] <= 0.0 {
        return Err(fail(
            at(path, "viewBox"),
            "A view box is [x, y, width, height] with a positive size.",
        ));
    }
    boolean(map, path, "pan")?;
    if let Some(zoom) = numbers(map, path, "zoom", (2, 2), (0.05, 20.0), false)? {
        if !(zoom[0] <= 1.0 && 1.0 <= zoom[1]) {
            return Err(fail(
                at(path, "zoom"),
                "Zoom is [minimum, maximum] around 1, from 0.05 to 20.",
            ));
        }
    }
    events(map, path, &["viewport", "resize"])?;
    let nodes = map
        .get("nodes")
        .ok_or_else(|| fail(at(path, "nodes"), "Required."))?;
    if serde_json::to_vec(nodes).map_or(usize::MAX, |bytes| bytes.len()) > MAX_CANVAS_BYTES {
        return Err(fail(
            at(path, "nodes"),
            "A canvas scene is at most 256 KiB of JSON.",
        ));
    }
    Scene::default().shapes(nodes, &at(path, "nodes"), 1)
}

/// Identifiers and node count of one canvas scene.
#[derive(Default)]
struct Scene {
    ids: BTreeSet<String>,
    count: usize,
}

impl Scene {
    fn shapes(&mut self, value: &Value, path: &str, depth: usize) -> Checked<()> {
        if depth > MAX_TREE_DEPTH {
            return Err(fail(path, "Canvas groups nest at most 16 levels deep."));
        }
        let shapes = value
            .as_array()
            .ok_or_else(|| fail(path, "Expected an array of canvas nodes."))?;
        for (index, shape) in shapes.iter().enumerate() {
            self.shape(shape, &format!("{path}[{index}]"), depth)?;
        }
        Ok(())
    }

    fn id(&mut self, map: &Object, path: &str, required: bool) -> Checked<Option<String>> {
        let id = identifier(map, path, "id", required)?.map(str::to_owned);
        if let Some(id) = &id {
            if !self.ids.insert(id.clone()) {
                return Err(fail(
                    at(path, "id"),
                    "Canvas node identifiers are unique within the canvas.",
                ));
            }
        }
        Ok(id)
    }

    fn shape(&mut self, value: &Value, path: &str, depth: usize) -> Checked<()> {
        self.count += 1;
        if self.count > MAX_CANVAS_NODES {
            return Err(fail(path, "A canvas has at most 2000 nodes."));
        }
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| fail(at(path, "type"), "A canvas node names its type."))?;
        match kind {
            "rect" => {
                let map = object(
                    value,
                    path,
                    &[
                        "type", "id", "x", "y", "w", "h", "radius", "fill", "stroke", "width",
                        "dash", "shadow", "opacity",
                    ],
                )?;
                self.id(map, path, false)?;
                frame(map, path, true)?;
                number(map, path, "radius", EXTENT, false)?;
                paint(map, path)?;
                integer(map, path, "shadow", SHADOW, false)?;
                Ok(())
            }
            "path" => {
                let map = object(
                    value,
                    path,
                    &[
                        "type", "id", "d", "bounds", "fill", "stroke", "width", "dash", "cap",
                        "rule", "opacity",
                    ],
                )?;
                self.id(map, path, false)?;
                let data = string(map, path, "d", true)?.unwrap_or_default();
                if data.trim().is_empty()
                    || data.len() > MAX_PATH_DATA
                    || !data.chars().all(|character| {
                        "MmLlHhVvCcSsQqTtAaZz0123456789.,eE+- ".contains(character)
                    })
                {
                    return Err(fail(
                        at(path, "d"),
                        "Path data is at most 8192 bytes of SVG path commands and numbers.",
                    ));
                }
                if let Some(bounds) = numbers(map, path, "bounds", (4, 4), COORDINATE, false)? {
                    if bounds[2] <= 0.0 || bounds[3] <= 0.0 {
                        return Err(fail(
                            at(path, "bounds"),
                            "Bounds are [x, y, width, height] with a positive size.",
                        ));
                    }
                }
                paint(map, path)?;
                choice(map, path, "cap", &["butt", "round", "square"], false)?;
                choice(map, path, "rule", &["nonzero", "evenodd"], false)?;
                Ok(())
            }
            "text" => {
                let map = object(
                    value,
                    path,
                    &[
                        "type", "id", "x", "y", "text", "runs", "style", "align", "color",
                        "maxWidth", "opacity",
                    ],
                )?;
                self.id(map, path, false)?;
                number(map, path, "x", COORDINATE, true)?;
                number(map, path, "y", COORDINATE, true)?;
                match (map.contains_key("text"), map.contains_key("runs")) {
                    (true, false) => {
                        text(map, path, "text", MAX_TEXT_CHARS, true)?;
                    }
                    (false, true) => {
                        let runs = array(map, path, "runs", (1, MAX_RUNS), true)?;
                        for (index, run) in runs.into_iter().flatten().enumerate() {
                            let run_path = format!("{path}.runs[{index}]");
                            let run = object(run, &run_path, &["text", "color"])?;
                            text(run, &run_path, "text", MAX_TEXT_CHARS, true)?;
                            color(run, &run_path, "color")?;
                        }
                    }
                    _ => {
                        return Err(fail(path, "A text node has either text or runs."));
                    }
                }
                choice(map, path, "style", TEXT_STYLES, false)?;
                choice(map, path, "align", &["start", "center", "end"], false)?;
                color(map, path, "color")?;
                number(map, path, "maxWidth", (1.0, MAX_COORDINATE), false)?;
                number(map, path, "opacity", UNIT, false)?;
                Ok(())
            }
            "image" => {
                let map = object(
                    value,
                    path,
                    &["type", "id", "x", "y", "w", "h", "src", "opacity"],
                )?;
                self.id(map, path, false)?;
                frame(map, path, true)?;
                let source = string(map, path, "src", true)?.unwrap_or_default();
                if !is_safe_relative_path(source)
                    || source.len() > 256
                    || !source.ends_with(".png")
                    || source.contains(':')
                    || source.chars().any(char::is_control)
                    || source.split('/').any(|part| part.is_empty() || part == ".")
                {
                    return Err(fail(
                        at(path, "src"),
                        "An image is a package-relative PNG path of at most 256 bytes.",
                    ));
                }
                number(map, path, "opacity", UNIT, false)?;
                Ok(())
            }
            "group" => self.group(value, path, depth),
            other => Err(fail(
                at(path, "type"),
                format!("Unknown canvas node type {other}."),
            )),
        }
    }

    fn group(&mut self, value: &Value, path: &str, depth: usize) -> Checked<()> {
        let map = object(
            value,
            path,
            &[
                "type", "id", "x", "y", "w", "h", "radius", "clip", "scale", "children", "on",
                "label", "tooltip", "states", "selected", "opacity", "shadow", "hidden",
            ],
        )?;
        let interactive = events(map, path, &["click"])?.contains(&"click".to_owned());
        let id = self.id(map, path, interactive)?;
        frame(map, path, false)?;
        if interactive {
            if !map.contains_key("w") || !map.contains_key("h") {
                return Err(fail(
                    path,
                    "A clickable group declares its hit area with w and h.",
                ));
            }
            if map.get("label").is_none() {
                return Err(fail(
                    at(path, "label"),
                    "A clickable group has an accessible label.",
                ));
            }
        }
        debug_assert!(!interactive || id.is_some());
        number(map, path, "radius", EXTENT, false)?;
        boolean(map, path, "clip")?;
        number(map, path, "scale", SCALE, false)?;
        line(map, path, "label", MAX_LABEL_CHARS, false)?;
        text(map, path, "tooltip", MAX_INFO_CHARS, false)?;
        boolean(map, path, "selected")?;
        boolean(map, path, "hidden")?;
        number(map, path, "opacity", UNIT, false)?;
        integer(map, path, "shadow", SHADOW, false)?;
        if let Some(states) = map.get("states") {
            let states_path = at(path, "states");
            let states = object(states, &states_path, &["hover", "pressed", "selected"])?;
            for (name, state) in states {
                let state_path = at(&states_path, name);
                let state = object(
                    state,
                    &state_path,
                    &["fill", "stroke", "width", "shadow", "scale", "opacity"],
                )?;
                color(state, &state_path, "fill")?;
                color(state, &state_path, "stroke")?;
                number(state, &state_path, "width", EXTENT, false)?;
                integer(state, &state_path, "shadow", SHADOW, false)?;
                number(state, &state_path, "scale", SCALE, false)?;
                number(state, &state_path, "opacity", UNIT, false)?;
            }
        }
        let children = map
            .get("children")
            .ok_or_else(|| fail(at(path, "children"), "Required."))?;
        self.shapes(children, &at(path, "children"), depth + 1)
    }
}

/// `x`, `y`, `w`, `h`: required for rects and images, optional for groups.
fn frame(map: &Object, path: &str, required: bool) -> Checked<()> {
    number(map, path, "x", COORDINATE, required)?;
    number(map, path, "y", COORDINATE, required)?;
    number(map, path, "w", EXTENT, required)?;
    number(map, path, "h", EXTENT, required)?;
    Ok(())
}

/// Fill, stroke, stroke width, dash pattern and opacity of a rect or path.
fn paint(map: &Object, path: &str) -> Checked<()> {
    color(map, path, "fill")?;
    color(map, path, "stroke")?;
    number(map, path, "width", EXTENT, false)?;
    numbers(map, path, "dash", (1, 8), EXTENT, false)?;
    number(map, path, "opacity", UNIT, false)?;
    Ok(())
}

/// A color: plain, a light/dark pair, or a linear gradient of plain colors.
fn color(map: &Object, path: &str, key: &str) -> Checked<()> {
    let Some(value) = map.get(key) else {
        return Ok(());
    };
    let field = at(path, key);
    match value {
        Value::String(color) => plain(color, &field),
        Value::Object(pair) if pair.contains_key("light") || pair.contains_key("dark") => {
            let pair = object(value, &field, &["light", "dark"])?;
            for side in ["light", "dark"] {
                let color = string(pair, &field, side, true)?.unwrap_or_default();
                plain(color, &at(&field, side))?;
            }
            Ok(())
        }
        Value::Object(_) => {
            let gradient = object(value, &field, &["linear", "angle"])?;
            number(gradient, &field, "angle", (-360.0, 360.0), false)?;
            let stops = array(gradient, &field, "linear", GRADIENT_STOPS, true)?;
            for (index, stop) in stops.into_iter().flatten().enumerate() {
                let stop_path = format!("{field}.linear[{index}]");
                let (Some(offset), Some(color)) = (
                    stop.get(0).and_then(Value::as_f64),
                    stop.get(1).and_then(Value::as_str),
                ) else {
                    return Err(fail(&stop_path, "A stop is [offset, color]."));
                };
                if stop.as_array().map_or(0, Vec::len) != 2 || !(0.0..=1.0).contains(&offset) {
                    return Err(fail(&stop_path, "A stop is [offset from 0 to 1, color]."));
                }
                plain(color, &format!("{stop_path}[1]"))?;
            }
            Ok(())
        }
        _ => Err(fail(field, "Expected a color.")),
    }
}

fn plain(color: &str, path: &str) -> Checked<()> {
    let valid = match color.strip_prefix('#') {
        Some(hex) => {
            (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => color
            .strip_prefix('$')
            .is_some_and(|token| COLOR_TOKENS.contains(&token)),
    };
    if !valid {
        return Err(fail(
            path,
            "A color is #RRGGBB, #RRGGBBAA or a $token such as $accent.",
        ));
    }
    Ok(())
}
