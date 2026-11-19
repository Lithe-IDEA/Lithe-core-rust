//! Projects the toolchain settings list: status classes, filters, groups, ordering and
//! counts for every enabled tool. Hosts render only the rows they can see; they never
//! re-sort, re-count or classify resolver reasons themselves.

use lithe_plugin_model::{invalid, Validation};
use serde::Deserialize;
use serde_json::{json, Value};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Display groups a definition may declare; the order is the on-screen group order.
pub const CATEGORIES: [&str; 6] = ["runtime", "build", "package", "debug", "lint", "other"];
/// Who a tool is for: a user-facing tool, or a runtime a plugin runs for itself.
pub const AUDIENCES: [&str; 2] = ["user", "plugin"];
/// Resolver reasons the user can clear without installing anything: trusting the
/// project, choosing between equal installations, or rerunning a probe.
const WARNING_REASONS: [&str; 3] = [
    "projectCodeRequiresAction",
    "ambiguousInstallation",
    "probeFailed",
];
/// Sized for thousands of enabled plugins; the 4 MiB command budget still applies.
const MAX_TOOLS: usize = 20_000;
const MAX_TERMS: usize = 16;
const MAX_GROUP_KEYS: usize = 20_000;
/// Grouping by plugin starts collapsed above this many groups, so the first screen
/// lists plugins rather than thousands of tools.
const PLUGIN_GROUPS_EXPANDED_UP_TO: usize = 40;
const DEFAULT_ISSUES: usize = 5;
const MAX_ISSUES: usize = 50;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Tool {
    id: String,
    display_name: String,
    #[serde(rename = "pluginID")]
    plugin_id: String,
    plugin_name: Option<String>,
    category: Option<String>,
    audience: Option<String>,
    /// The resolver's stable reason when no installation applies; absent when resolved.
    reason: Option<String>,
    #[serde(default)]
    pinned: bool,
    #[serde(default)]
    changed: bool,
    /// Extra searchable text such as versions and installation paths.
    #[serde(default)]
    terms: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Query {
    #[serde(default)]
    text: String,
    #[serde(default = "all")]
    filter: String,
    #[serde(default = "category")]
    group_by: String,
    /// Group keys the user collapsed or expanded; they override the defaults.
    #[serde(default)]
    collapsed: Vec<String>,
    #[serde(default)]
    expanded: Vec<String>,
}

fn all() -> String {
    "all".to_owned()
}

fn category() -> String {
    "category".to_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    tools: Vec<Tool>,
    query: Query,
    issue_limit: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Status {
    Error,
    Warning,
    Ok,
}

impl Status {
    fn of(reason: Option<&str>) -> Self {
        match reason {
            None => Self::Ok,
            Some(code) if WARNING_REASONS.contains(&code) => Self::Warning,
            Some(_) => Self::Error,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Ok => "ok",
        }
    }

    fn problem(self) -> bool {
        self != Self::Ok
    }
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn validate(request: &Request) -> Validation<()> {
    if request.tools.len() > MAX_TOOLS {
        return Err(invalid(
            "tools",
            "The settings view exceeds its tool limit.",
        ));
    }
    let mut ids = BTreeSet::new();
    for tool in &request.tools {
        let names_ok = bounded(&tool.id, 192)
            && bounded(&tool.display_name, 256)
            && bounded(&tool.plugin_id, 128)
            && tool
                .plugin_name
                .as_deref()
                .is_none_or(|name| bounded(name, 256));
        let enums_ok = tool
            .category
            .as_deref()
            .is_none_or(|value| CATEGORIES.contains(&value))
            && tool
                .audience
                .as_deref()
                .is_none_or(|value| AUDIENCES.contains(&value))
            && tool.reason.as_deref().is_none_or(|code| bounded(code, 64));
        let terms_ok = tool.terms.len() <= MAX_TERMS
            && tool
                .terms
                .iter()
                .all(|term| term.len() <= 1024 && !term.chars().any(char::is_control));
        if !names_ok || !enums_ok || !terms_ok {
            return Err(invalid("tools", "Expected bounded tool facts."));
        }
        // A tool ID appears once: the registry already dropped contested short IDs.
        if !ids.insert(tool.id.as_str()) {
            return Err(invalid("tools.id", "Tool IDs must be unique."));
        }
    }
    let query = &request.query;
    if query.text.len() > 1024
        || !matches!(
            query.filter.as_str(),
            "all" | "problems" | "pinned" | "changed"
        )
        || !matches!(query.group_by.as_str(), "category" | "plugin" | "none")
        || query.collapsed.len() + query.expanded.len() > MAX_GROUP_KEYS
        || query
            .collapsed
            .iter()
            .chain(&query.expanded)
            .any(|key| !bounded(key, 256))
    {
        return Err(invalid("query", "Expected a bounded settings view query."));
    }
    if request.issue_limit.is_some_and(|limit| limit > MAX_ISSUES) {
        return Err(invalid("issueLimit", "At most 50 issues are listed."));
    }
    Ok(())
}

fn matches(tool: &Tool, needle: &str) -> bool {
    needle.is_empty()
        || [
            Some(tool.display_name.as_str()),
            Some(tool.id.as_str()),
            Some(tool.plugin_id.as_str()),
            tool.plugin_name.as_deref(),
        ]
        .into_iter()
        .flatten()
        .chain(tool.terms.iter().map(String::as_str))
        .any(|text| text.to_lowercase().contains(needle))
}

fn plugin_title(tool: &Tool) -> &str {
    tool.plugin_name.as_deref().unwrap_or(&tool.plugin_id)
}

/// Problems first, then by name; the ID breaks ties so equal names never reorder.
fn tool_order(tools: &[Tool], statuses: &[Status], left: usize, right: usize) -> Ordering {
    statuses[left]
        .cmp(&statuses[right])
        .then_with(|| {
            tools[left]
                .display_name
                .to_lowercase()
                .cmp(&tools[right].display_name.to_lowercase())
        })
        .then_with(|| tools[left].id.cmp(&tools[right].id))
}

/// One group: its stable key, the fields that name it, and its member tools.
struct Group {
    key: String,
    head: Value,
    order: (usize, String, String),
    members: Vec<usize>,
}

fn group_of(tool: &Tool, by: &str) -> (String, Value, (usize, String, String)) {
    if by == "plugin" {
        let title = plugin_title(tool);
        return (
            format!("plugin:{}", tool.plugin_id),
            json!({"pluginID":tool.plugin_id,"title":title}),
            (0, title.to_lowercase(), tool.plugin_id.clone()),
        );
    }
    // A plugin's own runtime is grouped apart from user tools in every category.
    if tool.audience.as_deref() == Some("plugin") {
        return (
            "audience:plugin".to_owned(),
            json!({"audience":"plugin"}),
            (CATEGORIES.len(), String::new(), String::new()),
        );
    }
    let category = tool.category.as_deref().unwrap_or("other");
    let rank = CATEGORIES
        .iter()
        .position(|value| *value == category)
        .unwrap_or(CATEGORIES.len() - 1);
    (
        format!("category:{category}"),
        json!({"category":category}),
        (rank, String::new(), String::new()),
    )
}

/// Builds the projection. Filtering expands every group so a match is never hidden
/// behind a collapsed header; counts always cover every tool, not only matches.
pub fn settings_view(input: &Value) -> Validation<Value> {
    let request: Request = serde_json::from_value(input.clone()).map_err(|_| {
        invalid(
            "settingsView",
            "Expected a toolchain settings view request.",
        )
    })?;
    validate(&request)?;
    let tools = &request.tools;
    let query = &request.query;
    let statuses: Vec<Status> = tools
        .iter()
        .map(|tool| Status::of(tool.reason.as_deref()))
        .collect();
    let needle = query.text.trim().to_lowercase();
    let filtering = !needle.is_empty() || query.filter != "all";
    let visible: Vec<usize> = (0..tools.len())
        .filter(|index| {
            let tool = &tools[*index];
            matches(tool, &needle)
                && match query.filter.as_str() {
                    "problems" => statuses[*index].problem(),
                    "pinned" => tool.pinned,
                    "changed" => tool.changed,
                    _ => true,
                }
        })
        .collect();
    let tool_row =
        |index: usize| json!({"kind":"tool","index":index,"status":statuses[index].name()});
    let mut rows = Vec::new();
    if query.group_by == "none" {
        let mut sorted = visible.clone();
        sorted.sort_by(|left, right| tool_order(tools, &statuses, *left, *right));
        rows.extend(sorted.into_iter().map(tool_row));
    } else {
        let mut groups: BTreeMap<String, Group> = BTreeMap::new();
        for index in &visible {
            let (key, head, order) = group_of(&tools[*index], &query.group_by);
            groups
                .entry(key.clone())
                .or_insert_with(|| Group {
                    key,
                    head,
                    order,
                    members: Vec::new(),
                })
                .members
                .push(*index);
        }
        let mut groups: Vec<Group> = groups.into_values().collect();
        groups.sort_by(|left, right| left.order.cmp(&right.order));
        let collapsed: BTreeSet<&str> = query.collapsed.iter().map(String::as_str).collect();
        let expanded: BTreeSet<&str> = query.expanded.iter().map(String::as_str).collect();
        let many = query.group_by == "plugin" && groups.len() > PLUGIN_GROUPS_EXPANDED_UP_TO;
        for mut group in groups {
            group
                .members
                .sort_by(|left, right| tool_order(tools, &statuses, *left, *right));
            let problems = group
                .members
                .iter()
                .filter(|index| statuses[**index].problem())
                .count();
            let folded = !filtering
                && if collapsed.contains(group.key.as_str()) {
                    true
                } else if expanded.contains(group.key.as_str()) {
                    false
                } else {
                    many || group.key == "audience:plugin"
                };
            let mut head = json!({"kind":"group","key":group.key,"count":group.members.len(),"problems":problems,"collapsed":folded});
            if let (Some(target), Some(fields)) = (head.as_object_mut(), group.head.as_object()) {
                target.extend(fields.clone());
            }
            rows.push(head);
            if !folded {
                rows.extend(group.members.into_iter().map(tool_row));
            }
        }
    }
    let mut problems: Vec<usize> = (0..tools.len())
        .filter(|index| statuses[*index].problem())
        .collect();
    problems.sort_by(|left, right| tool_order(tools, &statuses, *left, *right));
    let errors = statuses.iter().filter(|s| **s == Status::Error).count();
    let warnings = statuses.iter().filter(|s| **s == Status::Warning).count();
    // An issue names the group its tool is listed under, so a host that opens the tool from
    // the list of issues can open that group without knowing how keys are formed.
    let issues: Vec<Value> = problems
        .iter()
        .take(request.issue_limit.unwrap_or(DEFAULT_ISSUES))
        .map(|index| {
            let mut issue = json!({"index":index,"status":statuses[*index].name()});
            if query.group_by != "none" {
                issue["group"] = Value::String(group_of(&tools[*index], &query.group_by).0);
            }
            issue
        })
        .collect();
    Ok(json!({
        "rows":rows,
        "issues":issues,
        "counts":{
            "total":tools.len(),
            "matched":visible.len(),
            "problems":errors + warnings,
            "errors":errors,
            "warnings":warnings,
            "pinned":tools.iter().filter(|tool| tool.pinned).count(),
            "changed":tools.iter().filter(|tool| tool.changed).count(),
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, name: &str, plugin: &str, extra: Value) -> Value {
        let mut value = json!({"id":id,"displayName":name,"pluginID":plugin});
        if let (Some(target), Some(fields)) = (value.as_object_mut(), extra.as_object()) {
            target.extend(fields.clone());
        }
        value
    }

    fn sample() -> Vec<Value> {
        vec![
            tool(
                "jdk",
                "JDK",
                "java",
                json!({"category":"runtime","pinned":true,"terms":["21.0.5","/opt/jdk"]}),
            ),
            tool(
                "go",
                "Go",
                "go",
                json!({"category":"runtime","reason":"notFound"}),
            ),
            tool(
                "maven",
                "Maven",
                "maven",
                json!({"category":"build","reason":"projectCodeRequiresAction"}),
            ),
            tool(
                "gradle",
                "Gradle",
                "gradle",
                json!({"category":"build","changed":true}),
            ),
            tool(
                "jdtls.jdk",
                "JDK for jdtls",
                "java",
                json!({"category":"runtime","audience":"plugin","reason":"versionUnsatisfied"}),
            ),
            tool("misc", "Misc", "other", json!({})),
        ]
    }

    fn view(tools: Vec<Value>, query: Value) -> Value {
        settings_view(&json!({"tools":tools,"query":query})).expect("view")
    }

    fn keys(rows: &Value) -> Vec<String> {
        rows.as_array()
            .expect("rows")
            .iter()
            .map(|row| match row["kind"].as_str() {
                Some("group") => row["key"].as_str().unwrap_or_default().to_owned(),
                _ => format!(
                    "{}:{}",
                    row["index"],
                    row["status"].as_str().unwrap_or_default()
                ),
            })
            .collect()
    }

    #[test]
    fn groups_by_fixed_category_order_with_problems_first() {
        let result = view(sample(), json!({}));
        assert_eq!(
            keys(&result["rows"]),
            [
                "category:runtime",
                "1:error",
                "0:ok",
                "category:build",
                "2:warning",
                "3:ok",
                "category:other",
                "5:ok",
                "audience:plugin",
            ]
        );
        let plugin_group = &result["rows"][8];
        assert_eq!(plugin_group["collapsed"], true);
        assert_eq!(plugin_group["problems"], 1);
        assert_eq!(
            result["counts"],
            json!({"total":6,"matched":6,"problems":3,"errors":2,"warnings":1,"pinned":1,"changed":1})
        );
        assert_eq!(keys(&result["issues"]), ["1:error", "4:error", "2:warning"]);
        let groups: Vec<&str> = result["issues"]
            .as_array()
            .expect("issues")
            .iter()
            .map(|issue| issue["group"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(groups, ["category:runtime", "audience:plugin", "category:build"]);
        let flat = view(sample(), json!({"groupBy":"none"}));
        assert!(flat["issues"][0].get("group").is_none());
    }

    #[test]
    fn filters_expand_groups_and_keep_counts_global() {
        let result = view(sample(), json!({"filter":"problems"}));
        assert_eq!(
            keys(&result["rows"]),
            [
                "category:runtime",
                "1:error",
                "category:build",
                "2:warning",
                "audience:plugin",
                "4:error"
            ]
        );
        assert_eq!(result["counts"]["matched"], 3);
        assert_eq!(result["counts"]["total"], 6);
        let text = view(sample(), json!({"text":"/OPT"}));
        assert_eq!(keys(&text["rows"]), ["category:runtime", "0:ok"]);
        let pinned = view(sample(), json!({"filter":"pinned","groupBy":"none"}));
        assert_eq!(keys(&pinned["rows"]), ["0:ok"]);
        let changed = view(sample(), json!({"filter":"changed","groupBy":"none"}));
        assert_eq!(keys(&changed["rows"]), ["3:ok"]);
    }

    #[test]
    fn user_overrides_and_plugin_grouping() {
        let result = view(
            sample(),
            json!({"collapsed":["category:runtime"],"expanded":["audience:plugin"]}),
        );
        assert_eq!(result["rows"][0]["collapsed"], true);
        assert!(
            keys(&result["rows"]).ends_with(&["audience:plugin".to_owned(), "4:error".to_owned()])
        );
        let by_plugin = view(sample(), json!({"groupBy":"plugin"}));
        assert_eq!(by_plugin["rows"][0]["key"], "plugin:go");
        assert_eq!(by_plugin["rows"][0]["title"], "go");
        let many: Vec<Value> = (0..41)
            .map(|index| {
                tool(
                    &format!("t{index}"),
                    "T",
                    &format!("p{index:02}"),
                    json!({}),
                )
            })
            .collect();
        let folded = view(many, json!({"groupBy":"plugin","expanded":["plugin:p00"]}));
        assert_eq!(folded["rows"][0]["collapsed"], false);
        assert_eq!(folded["rows"][2]["collapsed"], true);
    }

    #[test]
    fn rejects_unknown_values_and_duplicates() {
        for (tools, query) in [
            (
                vec![tool("a", "A", "p", json!({"category":"compiler"}))],
                json!({}),
            ),
            (
                vec![tool("a", "A", "p", json!({"audience":"system"}))],
                json!({}),
            ),
            (
                vec![
                    tool("a", "A", "p", json!({})),
                    tool("a", "B", "q", json!({})),
                ],
                json!({}),
            ),
            (vec![], json!({"filter":"broken"})),
            (vec![], json!({"groupBy":"version"})),
        ] {
            assert!(settings_view(&json!({"tools":tools,"query":query})).is_err());
        }
        assert!(settings_view(&json!({"tools":[],"query":{},"issueLimit":51})).is_err());
    }
}
