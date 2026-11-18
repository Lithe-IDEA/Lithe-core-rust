//! Builds a portable reference for the caret from language-server document symbols.
//!
//! The host owns the document path and actual caret, while Core owns the common
//! formatting and symbol-range selection rule. No language-specific source parsing is
//! performed: unknown or incomplete symbols fall back to an explicit file location.

use lithe_protocol::{CoreError, ErrorCode};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    path: String,
    line: u32,
    utf16_column: u32,
    #[serde(default)]
    symbols: Vec<Symbol>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Position {
    line: u32,
    utf16_column: u32,
}

#[derive(Deserialize)]
struct Range {
    start: Position,
    end: Position,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Symbol {
    name: String,
    kind: u32,
    range: Range,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    container_name: Option<String>,
    #[serde(default)]
    children: Option<Vec<Symbol>>,
}

/// Formats the server's innermost containing symbol, with a one-based source line.
/// A namespace, type or method is used only when the server reports it: the
/// caller never guesses a package or method signature from the file name.
pub fn reference_command(payload: Value) -> Result<Value, CoreError> {
    let request: Request = serde_json::from_value(payload).map_err(|_| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "Invalid reference location request",
        )
    })?;
    if request.path.is_empty() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "Reference location requires a file path",
        ));
    }
    let line = u64::from(request.line) + 1;
    let position = Position {
        line: request.line,
        utf16_column: request.utf16_column,
    };
    let symbol = reference_symbol(&request.symbols, position);
    let (reference, source) = match symbol {
        Some(symbol) => (format!("{symbol}#L{line}"), "symbol"),
        None => (format!("{}#L{line}", request.path), "file"),
    };
    Ok(json!({"reference":reference, "source":source}))
}

fn contains(range: &Range, position: Position) -> bool {
    range.start <= position && position < range.end
}

fn extent(range: &Range) -> (u32, u32) {
    (
        range.end.line.saturating_sub(range.start.line),
        range
            .end
            .utf16_column
            .saturating_sub(range.start.utf16_column),
    )
}

// Prefer a nested symbol chain over a flat entry, and a smaller enclosing range
// when a server returns overlapping flat SymbolInformation entries.
fn enclosing(symbols: &[Symbol], position: Position) -> Vec<&Symbol> {
    let mut best = Vec::new();
    let mut best_extent = (u32::MAX, u32::MAX);
    for symbol in symbols {
        if !contains(&symbol.range, position) {
            continue;
        }
        let mut candidate = vec![symbol];
        candidate.extend(enclosing(
            symbol.children.as_deref().unwrap_or(&[]),
            position,
        ));
        let span = extent(&symbol.range);
        if candidate.len() > best.len() || (candidate.len() == best.len() && span < best_extent) {
            best = candidate;
            best_extent = span;
        }
    }
    best
}

fn clean(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty() && !value.chars().any(char::is_control)).then_some(value)
}

fn callable_name(symbol: &Symbol) -> Option<String> {
    let name = clean(&symbol.name)?;
    // LSP implementations may put a method's exact signature in its name,
    // or put only the parameter list in detail. Never invent the types.
    if name.contains('(') {
        return Some(name.to_owned());
    }
    if let Some(detail) = symbol.detail.as_deref().and_then(clean) {
        if detail.starts_with('(') {
            if let Some(end) = detail.find(')') {
                return Some(format!("{}{}", name, &detail[..=end]));
            }
        }
    }
    Some(name.to_owned())
}

fn reference_symbol(symbols: &[Symbol], position: Position) -> Option<String> {
    let path = enclosing(symbols, position);
    let mut namespaces = Vec::new();
    let mut types = Vec::new();
    let mut callable = None;
    for symbol in &path {
        let Some(name) = clean(&symbol.name) else {
            continue;
        };
        match symbol.kind {
            // LSP SymbolKind: Module, Namespace, Package
            2..=4 => namespaces.push(name.to_owned()),
            // Class, Enum, Interface, Struct
            5 | 10 | 11 | 23 => types.push(name.to_owned()),
            // Method, Constructor, Function
            6 | 9 | 12 => callable = callable_name(symbol),
            _ => {}
        }
    }
    // Flat SymbolInformation includes containerName instead of nested children.
    // Use the server's name as-is; it may already be fully qualified.
    if types.is_empty() {
        if let Some(container) = path
            .last()
            .and_then(|s| s.container_name.as_deref())
            .and_then(clean)
        {
            types.push(container.to_owned());
        }
    }
    let mut owner = namespaces.join(".");
    if !types.is_empty() {
        if !owner.is_empty() {
            owner.push('.');
        }
        owner.push_str(&types.join("."));
    }
    match (owner.is_empty(), callable) {
        (true, None) => None,
        (true, Some(method)) => Some(method),
        (false, Some(method)) => Some(format!("{owner}#{method}")),
        (false, None) => Some(owner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn server_symbol_tree_and_detail_are_preserved() {
        let symbols = json!([{
            "name":"com.example", "kind":4,
            "range":{"start":{"line":0,"utf16Column":0},"end":{"line":200,"utf16Column":0}},
            "children":[{
                "name":"DemoClass","kind":5,
                "range":{"start":{"line":1,"utf16Column":0},"end":{"line":190,"utf16Column":0}},
                "children":[{
                    "name":"goHome","kind":6,"detail":"(String,Integer) : void",
                    "range":{"start":{"line":130,"utf16Column":0},"end":{"line":144,"utf16Column":0}}
                }]
            }]
        }]);
        let data = reference_command(
            json!({"path":"/tmp/DemoClass.java","line":136,"utf16Column":4,"symbols":symbols}),
        )
        .unwrap();
        assert_eq!(
            data["reference"],
            "com.example.DemoClass#goHome(String,Integer)#L137"
        );
        assert_eq!(data["source"], "symbol");
    }

    #[test]
    fn missing_or_nonmatching_symbols_fall_back_to_path() {
        for symbols in [
            json!([]),
            json!([{
                "name":"DemoClass","kind":5,
                "range":{"start":{"line":10,"utf16Column":0},"end":{"line":12,"utf16Column":0}}
            }]),
        ] {
            let data = reference_command(json!({"path":"C:/src/DemoClass.java","line":136,"utf16Column":4,"symbols":symbols})).unwrap();
            assert_eq!(data["reference"], "C:/src/DemoClass.java#L137");
            assert_eq!(data["source"], "file");
        }
    }

    #[test]
    fn flat_method_uses_its_server_container_and_exact_name() {
        let symbols = json!([{
            "name":"goHome(String,Integer)","kind":6,"containerName":"DemoClass",
            "range":{"start":{"line":6,"utf16Column":0},"end":{"line":9,"utf16Column":0}}
        }]);
        let data = reference_command(
            json!({"path":"file.java","line":7,"utf16Column":0,"symbols":symbols}),
        )
        .unwrap();
        assert_eq!(data["reference"], "DemoClass#goHome(String,Integer)#L8");
    }

    #[test]
    fn rejects_empty_path_and_invalid_positions() {
        assert!(reference_command(json!({"path":"","line":0,"utf16Column":0})).is_err());
        assert!(reference_command(json!({"path":"a.java","line":-1,"utf16Column":0})).is_err());
    }
}
