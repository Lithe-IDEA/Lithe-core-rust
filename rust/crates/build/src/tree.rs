//! `build.modelTree`: the tree a build tool window shows for one loaded
//! model, nested the way the build tool nests its projects.
//!
//! A project holds, in this order, its tasks filed under folders, its
//! dependencies, its targets, and its child projects, which are found through
//! `parentID`; a multi-module build therefore reads as the tool's own
//! hierarchy instead of one long list per instance. Tasks that run on the
//! whole instance appear once, beside the root projects. When the instance
//! has exactly one root project, the root's contents become the instance's
//! contents, because the host already shows the instance as a row.
//!
//! The rules depend only on the declaration's scopes, groups and categories
//! and on the model, never on a build tool. Hosts render the nodes and keep
//! which ones are open by `key`; they neither nest projects nor file tasks
//! themselves. Nothing here reads a file or starts a process.

use super::manifest::{
    BuildSystemDeclaration, DependenciesSource, OptionPresentation, Scope, TaskDeclaration,
    TaskGroup,
};
use super::model::{MAX_ITEMS, MAX_PROJECTS};
use super::options::{self, ModelOptionFacts};
use lithe_plugin_model::Validation;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Most nodes one tree holds; a larger tree is cut and marked `truncated`.
const MAX_NODES: usize = 50_000;
/// Deepest project nesting kept; a project nested deeper becomes a root.
const MAX_NESTING: usize = 64;
const MAX_QUERY: usize = 256;
const MAX_HEADING: usize = 120;
/// Joins the parts of a key. Labels and IDs never contain control
/// characters, so the parts cannot run into each other.
const KEY_SEPARATOR: char = '\u{1f}';

/// A loaded model to show and the text the user filters it by.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ModelTreeRequest {
    model: TreeModel,
    /// Stored option values of the instance, for `presentation: "tree"` options.
    #[serde(default)]
    options: BTreeMap<String, Value>,
    /// Case-insensitive filter; empty or blank keeps every node.
    #[serde(default, deserialize_with = "query")]
    query: String,
    /// Host-localized label of each group folder; a group without one leaves
    /// the folder's label to the host. The filter matches these labels.
    #[serde(default, deserialize_with = "headings")]
    group_headings: BTreeMap<TaskGroup, String>,
}

/// The parts of a decoded `BuildModel` the tree reads; other fields are
/// accepted and ignored, so a host can send back the model it received.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeModel {
    #[serde(default, deserialize_with = "projects")]
    projects: Vec<TreeProject>,
    #[serde(default, deserialize_with = "items")]
    targets: Vec<TreeTarget>,
    #[serde(default, deserialize_with = "items")]
    tasks: Vec<TreeTask>,
    #[serde(default)]
    option_values: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    option_defaults: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    option_exclude_prefixes: BTreeMap<String, String>,
    #[serde(default)]
    option_selection: Option<options::ModelOptionSelection>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeProject {
    id: String,
    path: String,
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default, rename = "parentID")]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeTarget {
    id: String,
    #[serde(rename = "projectID")]
    project_id: String,
    name: String,
    path: String,
    #[serde(default, deserialize_with = "nullable")]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeTask {
    id: String,
    #[serde(rename = "projectID")]
    project_id: String,
    name: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    group: TaskGroup,
    #[serde(default, deserialize_with = "nullable")]
    category: Vec<String>,
}

/// The nodes of one instance.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelTree {
    /// The instance's only root project, without children; its contents and
    /// the instance's own are `nodes`.
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<TreeNode>,
    nodes: Vec<TreeNode>,
    /// True when [`MAX_NODES`] cut the tree.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

/// What a node stands for. `targets`, `dependencies` and `tests` nodes have
/// no label: the host names them, and loads a `dependencies` or `tests`
/// node's entries itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
enum NodeKind {
    /// A `multiSelect` option with `presentation: "tree"`, such as build profiles.
    Option,
    /// One toggleable choice under an `Option` node.
    OptionChoice,
    Project,
    /// A category heading of the plugin, or a task group.
    Folder,
    Task,
    Targets,
    Target,
    Dependencies,
    Tests,
}

/// One row of the tree and the rows nested in it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeNode {
    kind: NodeKind,
    /// Unique within the instance and stable across reloads and filters, so
    /// the host can remember open nodes by it. Opaque to the host.
    key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    label: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    detail: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    tooltip: String,
    /// The group a folder stands for; absent on category folders.
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<TaskGroup>,
    /// The project a node acts on, its own for a project; absent on what
    /// acts on the whole instance.
    #[serde(rename = "projectID", skip_serializing_if = "String::is_empty")]
    project_id: String,
    #[serde(rename = "targetID", skip_serializing_if = "String::is_empty")]
    target_id: String,
    /// A declared task's ID or a model task's ID.
    #[serde(rename = "taskID", skip_serializing_if = "String::is_empty")]
    task_id: String,
    /// The declared option an `option` or `optionChoice` node belongs to.
    #[serde(rename = "optionID", skip_serializing_if = "String::is_empty")]
    option_id: String,
    /// The choice value an `optionChoice` node toggles.
    #[serde(rename = "optionValue", skip_serializing_if = "String::is_empty")]
    option_value: String,
    /// True on an `optionChoice` node that is currently active.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    checked: bool,
    /// True on an `optionChoice` node that is active when the option is unset.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    default_active: bool,
    /// True on a node the query matches itself, so the host opens the nodes
    /// on the way to it and shows the match as the user left it.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    matched: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<TreeNode>,
}

impl TreeNode {
    fn new(kind: NodeKind, key: String) -> Self {
        Self {
            kind,
            key,
            label: String::new(),
            detail: String::new(),
            tooltip: String::new(),
            group: None,
            project_id: String::new(),
            target_id: String::new(),
            task_id: String::new(),
            option_id: String::new(),
            option_value: String::new(),
            checked: false,
            default_active: false,
            matched: false,
            children: Vec::new(),
        }
    }

    fn matches(&self, query: &str) -> bool {
        // An option node's detail summarizes its checked children; matching it
        // there would keep the folder closed instead of opening onto the child.
        if self.kind == NodeKind::Option {
            return [&self.label, &self.tooltip]
                .iter()
                .any(|text| text.to_lowercase().contains(query));
        }
        [&self.label, &self.detail, &self.tooltip]
            .iter()
            .any(|text| text.to_lowercase().contains(query))
    }
}

/// `build.modelTree`.
pub(crate) fn model_tree(
    build_system: &BuildSystemDeclaration,
    request: &ModelTreeRequest,
) -> Validation<Value> {
    let index = Index::new(build_system, request);
    let mut budget = Budget {
        left: MAX_NODES,
        truncated: false,
    };
    let mut nodes = tree_option_nodes(build_system, request, &mut budget);
    let (mut project, contents) = match index.roots.as_slice() {
        &[root] => (
            index.project_node(root, &mut budget),
            index.contents(Some(root), true, &mut budget),
        ),
        _ => (None, index.contents(None, true, &mut budget)),
    };
    nodes.extend(contents);
    let query = request.query.trim().to_lowercase();
    if let Some(node) = project.as_mut() {
        node.matched = !query.is_empty() && node.matches(&query);
    }
    let unfiltered = query.is_empty() || project.as_ref().is_some_and(|node| node.matched);
    let tree = ModelTree {
        project,
        nodes: if unfiltered {
            nodes
        } else {
            filter(nodes, &query)
        },
        truncated: budget.truncated,
    };
    Ok(serde_json::to_value(tree).expect("a model tree serializes"))
}

/// Checklist nodes at the top of the tree for `presentation: "tree"` options
/// that currently offer at least one choice.
fn tree_option_nodes(
    build_system: &BuildSystemDeclaration,
    request: &ModelTreeRequest,
    budget: &mut Budget,
) -> Vec<TreeNode> {
    if !build_system
        .options
        .iter()
        .any(|option| option.presentation == Some(OptionPresentation::Tree))
    {
        return Vec::new();
    }
    let facts = ModelOptionFacts {
        values: Some(&request.model.option_values),
        defaults: Some(&request.model.option_defaults),
        exclude_prefixes: Some(&request.model.option_exclude_prefixes),
    };
    let resolved = options::resolve_with_facts(build_system, &request.options, facts);
    let active = request
        .model
        .option_selection
        .as_ref()
        .and_then(|selection| selection.matching_choices(build_system, &resolved));
    let mut nodes = Vec::new();
    for option in build_system
        .options
        .iter()
        .filter(|option| option.presentation == Some(OptionPresentation::Tree))
    {
        let active = active.and_then(|choices| choices.get(&option.id).map(Vec::as_slice));
        let choices = options::choice_state(option, &resolved, &facts, active);
        if choices.is_empty() {
            continue;
        }
        if !budget.take() {
            break;
        }
        let key = format!("o{KEY_SEPARATOR}{}", option.id);
        let mut parent = TreeNode::new(NodeKind::Option, key.clone());
        parent.label = option.display_name.clone();
        parent.option_id = option.id.clone();
        let active: Vec<&str> = choices
            .iter()
            .filter(|choice| choice.checked)
            .map(|choice| choice.title.as_deref().unwrap_or(choice.value.as_str()))
            .collect();
        if !active.is_empty() {
            parent.detail = active.join(", ");
        }
        for choice in choices {
            if !budget.take() {
                break;
            }
            let mut item = TreeNode::new(
                NodeKind::OptionChoice,
                format!("{key}{KEY_SEPARATOR}{}", choice.value),
            );
            item.label = choice.title.unwrap_or_else(|| choice.value.clone());
            item.option_id = option.id.clone();
            item.option_value = choice.value;
            item.checked = choice.checked;
            item.default_active = choice.default_active;
            parent.children.push(item);
        }
        nodes.push(parent);
    }
    nodes
}

/// Keeps the nodes that match `query`, marked and each with its whole
/// subtree, and the nodes on the way to them. Dependencies are loaded by the
/// host, so their node stays only inside a match; tests are filtered by the
/// host, so their node always stays.
fn filter(nodes: Vec<TreeNode>, query: &str) -> Vec<TreeNode> {
    nodes
        .into_iter()
        .filter_map(|mut node| {
            if node.kind == NodeKind::Tests {
                return Some(node);
            }
            if node.matches(query) {
                node.matched = true;
                return Some(node);
            }
            if node.kind == NodeKind::Dependencies {
                return None;
            }
            node.children = filter(std::mem::take(&mut node.children), query);
            (!node.children.is_empty()).then_some(node)
        })
        .collect()
}

/// Nodes left to build; reaching zero cuts the tree.
struct Budget {
    left: usize,
    truncated: bool,
}

impl Budget {
    /// Takes one node; false, and the tree marked cut, when none is left.
    fn take(&mut self) -> bool {
        if self.left == 0 {
            self.truncated = true;
            return false;
        }
        self.left -= 1;
        true
    }
}

/// The model indexed by project.
struct Index<'a> {
    build_system: &'a BuildSystemDeclaration,
    headings: &'a BTreeMap<TaskGroup, String>,
    projects: &'a [TreeProject],
    /// Children shown under each project, in model order, once cycles,
    /// duplicate IDs and too deep nesting are resolved.
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
    targets: Vec<Vec<&'a TreeTarget>>,
    tasks: Vec<Vec<&'a TreeTask>>,
}

impl<'a> Index<'a> {
    fn new(build_system: &'a BuildSystemDeclaration, request: &'a ModelTreeRequest) -> Self {
        let model = &request.model;
        let count = model.projects.len();
        let mut by_id = BTreeMap::new();
        for (at, project) in model.projects.iter().enumerate() {
            by_id.entry(project.id.as_str()).or_insert(at);
        }
        // A project repeating an earlier project's ID is left out; targets
        // and tasks naming that ID belong to the first.
        let kept: Vec<usize> = (0..count)
            .filter(|&at| by_id.get(model.projects[at].id.as_str()) == Some(&at))
            .collect();
        let mut declared = vec![Vec::new(); count];
        let mut first_roots = Vec::new();
        for &at in &kept {
            let parent = model.projects[at]
                .parent_id
                .as_deref()
                .and_then(|id| by_id.get(id).copied())
                .filter(|&parent| parent != at);
            match parent {
                Some(parent) => declared[parent].push(at),
                None => first_roots.push(at),
            }
        }
        let mut children = vec![Vec::new(); count];
        let mut roots = Vec::new();
        let mut reached = vec![false; count];
        // The projects without a parent first; then, in model order, each
        // project a cycle or the nesting bound left unreached becomes a root.
        for start in first_roots.into_iter().chain(kept) {
            if reached[start] {
                continue;
            }
            reached[start] = true;
            roots.push(start);
            let mut stack = vec![(start, 1)];
            while let Some((at, depth)) = stack.pop() {
                if depth == MAX_NESTING {
                    continue;
                }
                for &child in &declared[at] {
                    if !reached[child] {
                        reached[child] = true;
                        children[at].push(child);
                        stack.push((child, depth + 1));
                    }
                }
            }
        }
        // Targets and tasks of an unknown project have nowhere to show.
        let mut targets = vec![Vec::new(); count];
        for target in &model.targets {
            if let Some(&at) = by_id.get(target.project_id.as_str()) {
                targets[at].push(target);
            }
        }
        let mut tasks = vec![Vec::new(); count];
        for task in &model.tasks {
            if let Some(&at) = by_id.get(task.project_id.as_str()) {
                tasks[at].push(task);
            }
        }
        Self {
            build_system,
            headings: &request.group_headings,
            projects: &model.projects,
            children,
            roots,
            targets,
            tasks,
        }
    }

    fn project_node(&self, at: usize, budget: &mut Budget) -> Option<TreeNode> {
        if !budget.take() {
            return None;
        }
        let project = &self.projects[at];
        let mut node = TreeNode::new(NodeKind::Project, project_key(&project.id));
        node.label = shown_name(project.display_name.as_deref(), &project.name);
        // The project at the build root says nothing more with a path of ".".
        if project.path != "." {
            node.detail = project.path.clone();
        }
        node.project_id = project.id.clone();
        Some(node)
    }

    /// The nodes inside a project, or inside the instance when `project` is
    /// `None`. `instance` adds what acts on the whole instance; with a
    /// project as well, the instance's tasks share that project's folders.
    fn contents(
        &self,
        project: Option<usize>,
        instance: bool,
        budget: &mut Budget,
    ) -> Vec<TreeNode> {
        let tasks = &self.build_system.tasks;
        let container = project.map_or_else(String::new, |at| project_key(&self.projects[at].id));
        let mut folders = Folders::default();
        if let Some(at) = project {
            let id = &self.projects[at].id;
            for task in tasks.iter().filter(|task| runs_on_each_project(task)) {
                let node = declared_task(task, id, "");
                self.file(
                    &mut folders,
                    &container,
                    &task.category,
                    task.group,
                    node,
                    budget,
                );
            }
            for task in &self.tasks[at] {
                let mut node = TreeNode::new(NodeKind::Task, String::new());
                node.label = shown_name(task.display_name.as_deref(), &task.name);
                node.tooltip = task.description.clone().unwrap_or_default();
                node.project_id = task.project_id.clone();
                node.task_id = task.id.clone();
                self.file(
                    &mut folders,
                    &container,
                    &task.category,
                    task.group,
                    node,
                    budget,
                );
            }
        }
        if instance {
            let without_projects = self.projects.is_empty();
            for task in tasks
                .iter()
                .filter(|task| runs_once(task, without_projects))
            {
                let node = declared_task(task, "", "");
                self.file(
                    &mut folders,
                    &container,
                    &task.category,
                    task.group,
                    node,
                    budget,
                );
            }
        }
        let mut nodes = folders.into_nodes();

        let scopes = dependency_scopes(self.build_system);
        let per_project = scopes.contains(&Scope::Project);
        let dependencies = match project {
            Some(at) if per_project => Some(self.projects[at].id.clone()),
            _ if instance
                && scopes.contains(&Scope::Workspace)
                && (!per_project || self.projects.is_empty()) =>
            {
                Some(String::new())
            }
            _ => None,
        };
        if let Some(project_id) = dependencies {
            if budget.take() {
                let key = format!("{container}{KEY_SEPARATOR}dependencies");
                let mut node = TreeNode::new(NodeKind::Dependencies, key);
                node.project_id = project_id;
                nodes.push(node);
            }
        }

        if let Some(at) = project {
            if !self.targets[at].is_empty() && budget.take() {
                let key = format!("{container}{KEY_SEPARATOR}targets");
                let mut folder = TreeNode::new(NodeKind::Targets, key);
                folder.project_id = self.projects[at].id.clone();
                for target in &self.targets[at] {
                    if let Some(node) = self.target_node(target, &folder.key, budget) {
                        folder.children.push(node);
                    }
                }
                nodes.push(folder);
            }
        }

        let discovers_tests = self
            .build_system
            .test
            .as_ref()
            .is_some_and(|test| test.discovers_tests());
        if instance && discovers_tests && budget.take() {
            let key = format!("{container}{KEY_SEPARATOR}tests");
            nodes.push(TreeNode::new(NodeKind::Tests, key));
        }

        let projects = match project {
            Some(at) => &self.children[at],
            None => &self.roots,
        };
        for &at in projects {
            if let Some(mut node) = self.project_node(at, budget) {
                node.children = self.contents(Some(at), false, budget);
                nodes.push(node);
            }
        }
        nodes
    }

    /// A target with the declared tasks that run on a target carrying one of
    /// their tags.
    fn target_node(
        &self,
        target: &TreeTarget,
        container: &str,
        budget: &mut Budget,
    ) -> Option<TreeNode> {
        if !budget.take() {
            return None;
        }
        let mut node = TreeNode::new(
            NodeKind::Target,
            format!("{container}{KEY_SEPARATOR}{}", target.id),
        );
        (node.label, node.detail, node.tooltip) = target_label(target);
        node.project_id = target.project_id.clone();
        node.target_id = target.id.clone();
        let applies = |task: &&TaskDeclaration| {
            !task.from_model
                && task.scopes.contains(&Scope::Target)
                && (task.target_tags.is_empty()
                    || target.tags.iter().any(|tag| task.target_tags.contains(tag)))
        };
        for task in self.build_system.tasks.iter().filter(applies) {
            if !budget.take() {
                break;
            }
            let mut child = declared_task(task, &target.project_id, &target.id);
            child.key = task_key(&node.key, &task.id);
            node.children.push(child);
        }
        Some(node)
    }

    /// Files a task under its category path, else under its group, making
    /// the folders it needs. A task the budget cannot hold is left out.
    fn file(
        &self,
        folders: &mut Folders,
        container: &str,
        category: &[String],
        group: TaskGroup,
        mut task: TreeNode,
        budget: &mut Budget,
    ) {
        let path: Vec<Segment> = if category.is_empty() {
            vec![Segment::Group(group)]
        } else {
            category.iter().cloned().map(Segment::Category).collect()
        };
        let mut key = container.to_string();
        let mut level = folders;
        for segment in path {
            key.push(KEY_SEPARATOR);
            key.push_str(&segment.key());
            let at = match level
                .folders
                .iter()
                .position(|folder| folder.segment == segment)
            {
                Some(at) => at,
                None => {
                    if !budget.take() {
                        return;
                    }
                    let mut node = TreeNode::new(NodeKind::Folder, key.clone());
                    match &segment {
                        Segment::Category(label) => node.label = label.clone(),
                        Segment::Group(group) => {
                            node.group = Some(*group);
                            node.label = self.headings.get(group).cloned().unwrap_or_default();
                        }
                    }
                    level.folders.push(Folder {
                        segment,
                        node,
                        contents: Folders::default(),
                    });
                    level.folders.len() - 1
                }
            };
            level = &mut level.folders[at].contents;
        }
        if budget.take() {
            task.key = task_key(&key, &task.task_id);
            level.tasks.push(task);
        }
    }
}

/// A declared task's node, without its key.
fn declared_task(task: &TaskDeclaration, project_id: &str, target_id: &str) -> TreeNode {
    let mut node = TreeNode::new(NodeKind::Task, String::new());
    node.label = task.display_name.clone();
    node.tooltip = task.description.clone().unwrap_or_default();
    node.project_id = project_id.to_string();
    node.target_id = target_id.to_string();
    node.task_id = task.id.clone();
    node
}

fn shown_name(display_name: Option<&str>, name: &str) -> String {
    display_name
        .filter(|shown| !shown.trim().is_empty())
        .unwrap_or(name)
        .to_string()
}

/// One folder level: its subfolders by first appearance, then the tasks
/// filed directly in it.
#[derive(Default)]
struct Folders {
    folders: Vec<Folder>,
    tasks: Vec<TreeNode>,
}

struct Folder {
    segment: Segment,
    node: TreeNode,
    contents: Folders,
}

impl Folders {
    fn into_nodes(self) -> Vec<TreeNode> {
        let mut nodes: Vec<TreeNode> = self
            .folders
            .into_iter()
            .map(|folder| {
                let mut node = folder.node;
                node.children = folder.contents.into_nodes();
                node
            })
            .collect();
        nodes.extend(self.tasks);
        nodes
    }
}

/// One heading of a task's folder path.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Category(String),
    Group(TaskGroup),
}

impl Segment {
    /// The key part, by group ID rather than by heading so a change of
    /// language keeps open folders open.
    fn key(&self) -> String {
        match self {
            Self::Category(label) => format!("c{KEY_SEPARATOR}{label}"),
            Self::Group(group) => format!("g{KEY_SEPARATOR}{}", group_id(*group)),
        }
    }
}

fn group_id(group: TaskGroup) -> &'static str {
    match group {
        TaskGroup::Lifecycle => "lifecycle",
        TaskGroup::Build => "build",
        TaskGroup::Test => "test",
        TaskGroup::Run => "run",
        TaskGroup::Verification => "verification",
        TaskGroup::Documentation => "documentation",
        TaskGroup::Other => "other",
    }
}

fn project_key(id: &str) -> String {
    format!("p{KEY_SEPARATOR}{id}")
}

/// The key of a task in the folder or target with key `container`.
fn task_key(container: &str, id: &str) -> String {
    format!("{container}{KEY_SEPARATOR}t{KEY_SEPARATOR}{id}")
}

/// A declared task that each project lists: it runs on a project, unless it
/// asks for arguments and also runs on the instance, which lists it once.
fn runs_on_each_project(task: &TaskDeclaration) -> bool {
    !task.from_model
        && task.scopes.contains(&Scope::Project)
        && !(task.requires_user_arguments && task.scopes.contains(&Scope::Workspace))
}

/// A declared task the instance lists once.
fn runs_once(task: &TaskDeclaration, without_projects: bool) -> bool {
    !task.from_model
        && task.scopes.contains(&Scope::Workspace)
        && (!task.scopes.contains(&Scope::Project)
            || task.requires_user_arguments
            || without_projects)
}

fn dependency_scopes(build_system: &BuildSystemDeclaration) -> &[Scope] {
    match &build_system.dependencies {
        Some(
            DependenciesSource::Command { scopes, .. } | DependenciesSource::Provider { scopes },
        ) => scopes,
        None => &[],
    }
}

/// What a target's node shows: its name with its path beside it, or the path
/// alone when the name already ends with it, as a Go import path ends with
/// the folder of its package; the full name is then the tooltip.
fn target_label(target: &TreeTarget) -> (String, String, String) {
    let name = &target.name;
    let path = target.path.trim_matches('/');
    if !path.is_empty() && path != "." {
        if name == path {
            return (path.to_string(), String::new(), String::new());
        }
        if name.ends_with(&format!("/{path}")) {
            return (path.to_string(), String::new(), name.clone());
        }
    }
    (name.clone(), target.path.clone(), String::new())
}

fn nullable<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}

fn bounded<'de, D, T>(deserializer: D, limit: usize) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let list: Vec<T> = nullable(deserializer)?;
    if list.len() > limit {
        return Err(D::Error::invalid_length(
            list.len(),
            &format!("at most {limit} entries").as_str(),
        ));
    }
    Ok(list)
}

fn projects<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    bounded(deserializer, MAX_PROJECTS)
}

fn items<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    bounded(deserializer, MAX_ITEMS)
}

fn query<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let query = String::deserialize(deserializer)?;
    if query.len() > MAX_QUERY || query.chars().any(char::is_control) {
        return Err(D::Error::custom(format!(
            "query must be at most {MAX_QUERY} bytes without control characters"
        )));
    }
    Ok(query)
}

fn headings<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<TaskGroup, String>, D::Error> {
    let headings = BTreeMap::<TaskGroup, String>::deserialize(deserializer)?;
    for heading in headings.values() {
        let text = heading.trim();
        if text.is_empty() || text.len() > MAX_HEADING || text.chars().any(char::is_control) {
            return Err(D::Error::custom(format!(
                "group headings must be 1 to {MAX_HEADING} bytes without control characters"
            )));
        }
    }
    Ok(headings)
}
