//! Versioned command requests and the stable set of dispatcher command names.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Versioned request envelope accepted by every shared Core entry point.
pub struct CoreRequest {
    /// Caller-provided correlation identifier copied to the response.
    #[serde(default)]
    pub id: Option<String>,
    /// Identifier used for cooperative cancellation and stale-result handling.
    #[serde(default)]
    pub operation_id: Option<String>,
    /// Optional deadline applied by operations that support bounded execution.
    #[serde(default)]
    pub timeout_milliseconds: Option<u64>,
    /// Host-owned Git preferences, scoped to this request only.
    #[serde(default)]
    pub git_execution: Option<lithe_git::execution_policy::GitExecutionOptions>,
    /// Remote host selected by the native workspace registry; absent for local workspaces.
    #[serde(default)]
    pub(crate) remote_workspace: Option<lithe_workspace::Workspace>,
    /// Host-reported provenance used only for diagnostics traces; never changes behavior.
    #[serde(default)]
    pub origin: Option<RequestOrigin>,
    /// Stable compatibility name resolved by [`CoreCommand::parse`].
    pub command: String,
    /// Command-specific JSON object; omitted payloads deserialize as JSON null.
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
/// Why the host sent a request. Invalid tokens are dropped by the trace, not rejected,
/// so a malformed diagnostics hint can never fail the operation it describes.
pub struct RequestOrigin {
    /// Whether a user action or an automatic refresh caused the request.
    pub source: Option<lithe_git::console::Source>,
    /// Stable identifier of the UI action that caused the request.
    pub trigger: Option<String>,
    /// Correlation identifier shared by the action and every request it caused.
    pub operation: Option<String>,
}

#[derive(Debug, Clone)]
/// Typed form of the stable command names accepted by the dispatcher.
///
/// Variants are grouped by domain, but their serialized compatibility names
/// live only in [`CoreCommand::parse`] so every host uses one mapping.
pub enum CoreCommand {
    /// Controls the native IDE capability broker (`ideHost.control`).
    IdeHostControl,
    /// Reports the Core and protocol versions (`core.ping`).
    Ping,
    /// Matches host-localized action text without workspace I/O (`search.actions`).
    SearchActions,
    /// AI commit, pull request and inline edit planning run by `lithe-ai` (`ai.*`).
    Ai(AiCommand),
    /// Plugin manifests, native UI and web content, plugin settings, installed
    /// plugins and the marketplace (`plugins.*`).
    Plugins(PluginsCommand),
    /// Debug adapters, transports, sessions, breakpoints and inspection (`debug.*`).
    Debug(DebugCommand),
    /// Keymap import, contexts, conflicts and schemes (`keymap.*`).
    Keymap(KeymapCommand),
    /// Toolchain registry, pins, consumer preferences, discovery and launch instantiation (`toolchains.*`).
    Toolchains(ToolchainsCommand),
    /// Validates a plugin backend's Run start request (`run.decodeStart`).
    RunDecodeStart,
    /// Finds the output links plugin matchers declare in console lines (`outputLinks.match`).
    OutputLinksMatch,
    /// Checks a plugin function's answer to an output link (`outputLinks.decodeResolution`).
    OutputLinksDecodeResolution,
    /// Build detection, options, plans and decoded tool output (`build.*`).
    Build(BuildCommand),
    /// Detects VCS roots and registered providers of a workspace (`vcs.detectRoots`).
    VcsDetectRoots,
    /// Assigns workspace paths to detected VCS roots (`vcs.ownerOf`).
    VcsOwnerOf,
    /// Validates and normalizes a VCS provider's reply (`vcs.decodeReply`).
    VcsDecodeReply,
    /// Lists plugin project generators and who provides their requirements (`project.generatorCatalog`).
    ProjectGeneratorCatalog,
    /// Derives a generator form's defaults, visibility, and field errors (`project.generatorForm`).
    ProjectGeneratorForm,
    /// Expands a generator's steps into files, argv, and URLs (`project.generationPlan`).
    ProjectGenerationPlan,
    /// Validates a new project's name against its parent directory (`project.checkName`).
    ProjectCheckName,
    /// Extracts a downloaded archive inside a bounded staging directory (`project.extractArchive`).
    ProjectExtractArchive,
    /// Workspace snapshots, indexing, directory actions, repositories and search (`workspace.*`).
    Workspace(WorkspaceCommand),
    /// Reads one workspace-relative UTF-8 file (`file.read`).
    FileRead,
    /// Replaces one workspace-relative UTF-8 file (`file.write`).
    FileWrite,
    /// Decodes bytes the host read, as `file.read` would (`text.decode`).
    TextDecode,
    /// Encodes text into bytes, as `file.write` would (`text.encode`).
    TextEncode,
    /// Reduces one shared document persistence event (`document.lifecycle`).
    DocumentLifecycle,
    /// Classifies decoded Unicode text independently of its filename (`document.classifyText`).
    DocumentClassifyText,
    /// Selects the open documents a save batch writes (`document.planSaveBatch`).
    DocumentPlanSaveBatch,
    /// Reduces save batch member outcomes and live documents (`document.reduceSaveBatch`).
    DocumentReduceSaveBatch,
    /// Local file history (`history.*`).
    History(HistoryCommand),
    /// Compares two host-supplied texts line by line (`diff.compareTexts`).
    DiffCompareTexts,
    /// Resolve environment precedence after platform tool discovery.
    ExecutionResolveEnvironment,
    /// Resolves deterministic process search-path ordering (`execution.resolveSearchPath`).
    ExecutionResolveSearchPath,
    /// Decides how each Lithe-owned process ends (`processLifecycle.resolve`).
    ProcessLifecycleResolve,
    /// Classifies a process snapshot into lifecycle items (`processLifecycle.inventory`).
    ProcessLifecycleInventory,
    /// Turns a plan and the user's answers into what ends (`processLifecycle.settle`).
    ProcessLifecycleSettle,
    /// Renders and sanitizes shared Markdown (`markdown.render`).
    MarkdownRender,
    /// Editor reducers: workbench state, text edits, find, navigation and preferences (`editor.*`).
    Editor(EditorCommand),
    /// Applies one transition to the rearrangeable workbench chrome: stripe, title bar and status bar
    /// button placement and open tool panels (`workbench.chromeLayout.reduce`), from `lithe-core-rules`.
    WorkbenchChromeLayoutReduce,
    /// Resolves terminal link candidates into file or external targets (`terminal.parseLinkTarget`).
    TerminalParseLinkTarget,
    /// Language server sessions, documents, edits, requests and events (`lsp.*`).
    Lsp(LspCommand),
    /// Normalizes the stored proxy settings into the policy a network stack
    /// must apply (`network.proxyPolicy`).
    NetworkProxyPolicy,
    /// Run configurations and their launch plans (`runConfig.*`).
    RunConfig(RunConfigCommand),
    /// Git status, writes, history, rebase, patches, diffs, references and pull requests (`git.*`).
    Git(GitCommand),
    /// Remote hosts: probing, authentication, connections, files and processes (`remote.*`).
    Remote(RemoteCommand),
    /// Parses a GitHub repository identity from a Git remote URL (`github.parseRemote`).
    GitHubParseRemote,
    /// Builds a platform-executable GitHub HTTP request (`github.requestPlan`).
    GitHubRequestPlan,
    /// Normalizes a GitHub HTTP response into the shared contract (`github.normalizeResponse`).
    GitHubNormalizeResponse,
    /// Plans the next bounded GitHub device-authorization transition.
    GitHubDeviceAuthorizationTransition,
    /// Plans a safe local branch for a GitHub pull request.
    GitHubPullRequestCheckoutPlan,
    /// Log events and queries, log filters and retention, redaction and diagnostic
    /// bundles (`diagnostics.*`).
    Diagnostics(DiagnosticsCommand),
}

#[derive(Debug, Clone)]
/// The `ai.*` commands; their names live in [`CoreCommand::parse`].
pub enum AiCommand {
    /// Budgets diff prefixes before crossing the native boundary (`ai.commitInputBudget`).
    CommitInputBudget,
    /// Plans a credential-free commit message request (`ai.planCommit`).
    PlanCommit,
    /// Decodes a provider commit message response (`ai.decodeCommitMessage`).
    DecodeCommitMessage,
    /// Names the native files of one AI CLI configuration source (`ai.configuration.sources`).
    ConfigurationSources,
    /// Parses AI CLI configuration into credential-free metadata (`ai.configuration.parse`).
    ConfigurationParse,
    /// Plans a credential-free pull request description request (`ai.pullRequest.plan`).
    PullRequestPlan,
    /// Decodes a strict pull request title and description (`ai.pullRequest.decode`).
    PullRequestDecode,
    /// Plans a credential-free inline edit request (`ai.inlineEdit.plan`).
    InlineEditPlan,
    /// Decodes the replacement text of an inline edit response (`ai.inlineEdit.decode`).
    InlineEditDecode,
}

#[derive(Debug, Clone)]
/// The `plugins.*` commands; their names live in [`CoreCommand::parse`].
pub enum PluginsCommand {
    /// Validates and normalizes a declarative plugin manifest (`plugins.declarative.validate`).
    DeclarativeValidate,
    /// Validates native plugin UI data with declaration identity and node budgets.
    NativeUIValidateData,
    /// Registers a plugin page for a web tool window (`plugins.web.open`).
    WebOpen,
    /// Forgets a plugin page and lists its calls to cancel (`plugins.web.close`).
    WebClose,
    /// Decides how a plugin page request is served (`plugins.web.resource`).
    WebResource,
    /// Prepends the policy and bridge to a page document (`plugins.web.document`).
    WebDocument,
    /// Checks one bridge message of a plugin page (`plugins.web.request`).
    WebRequest,
    /// Frames the host's answer to a bridge action (`plugins.web.reply`).
    WebReply,
    /// Frames an event for a plugin page (`plugins.web.event`).
    WebEvent,
    /// Describes the Core-owned backend host-method catalog.
    HostApiDescribe,
    /// Rechecks a backend host request against Core's grant rules.
    HostApiAuthorize,
    /// Expands a declarative plugin's run template (`plugins.declarative.runPlan`).
    DeclarativeRunPlan,
    /// Lists a declarative plugin's workspace and file tests (`plugins.declarative.discoverTests`).
    DeclarativeDiscoverTests,
    /// Expands a declarative plugin's test template (`plugins.declarative.testPlan`).
    DeclarativeTestPlan,
    /// Expands a declarative plugin's language-server launch (`plugins.declarative.languageServerLaunchPlan`).
    DeclarativeLanguageServerLaunchPlan,
    /// Checks a plugin backend's language-server launch answer (`plugins.languageServers.decodeLaunch`).
    LanguageServersDecodeLaunch,
    /// Picks the stored language-server state the host may delete (`plugins.languageServers.stateRetention`).
    LanguageServersStateRetention,
    /// Checks a plugin content provider's answer (`plugins.contentProviders.decode`).
    ContentProvidersDecode,
    /// Merges a plugin's stored setting layers into effective values (`plugins.declarative.resolveSettings`).
    DeclarativeResolveSettings,
    /// Projects stored documents into plugin setting rows and launch layers.
    DeclarativeSettingsState,
    /// Projects enabled plugin settings pages from installed manifests.
    SettingsCatalog,
    /// Projects searched settings navigation without platform policy.
    SettingsNavigation,
    /// Routes committed preference effects using native ownership facts.
    SettingsDelivery,
    /// Resolves an installed settings action and its native confirmation requirement.
    DeclarativeSettingsActionPlan,
    /// Selects numeric installed versions and reports malformed native facts.
    InstalledSelect,
    /// Reports installed plugins' enablement and plans a lifecycle action
    /// (`plugins.installed.lifecycle`).
    InstalledLifecycle,
    /// Validates an atomic plugin settings edit and preserves unrelated values.
    DeclarativeEditSettings,
    /// Expands a language server's `workspaceConfiguration` (`plugins.declarative.workspaceConfiguration`).
    DeclarativeWorkspaceConfiguration,
    /// Validates a language's grammar file (`plugins.declarative.decodeGrammar`).
    DeclarativeDecodeGrammar,
    /// Validates independent editor action registration or results.
    EditorActionsDecode,
    /// Validates editor provider registrations and provider results.
    EditorProvidersDecode,
    /// Validates and filters a marketplace index (`plugins.marketplace.validateIndex`).
    MarketplaceValidateIndex,
    /// Resolves a user-entered marketplace source URL (`plugins.marketplace.resolveSource`).
    MarketplaceResolveSource,
    /// Lists a package's permissions and programs before installation (`plugins.marketplace.installReview`).
    MarketplaceInstallReview,
}

#[derive(Debug, Clone)]
/// The `debug.*` commands; their names live in [`CoreCommand::parse`].
pub enum DebugCommand {
    /// Checks a debug start and returns the adapter's DAP arguments (`debug.adapters.startArguments`).
    AdaptersStartArguments,
    /// Validates an IDE-to-DAP TCP endpoint, not an attach target.
    ValidateAdapterEndpoint,
    /// Says where to look for the program a launch debugs (`debug.adapters.executablePlan`).
    AdaptersExecutablePlan,
    /// Returns the default byte-stream deadlines and queue budgets (`debug.transport.policy`).
    TransportPolicy,
    /// Applies one platform event to a byte-stream lifecycle and returns the effects to run
    /// (`debug.transport.handle`).
    TransportHandle,
    /// Forgets the lifecycle of a stopped byte stream (`debug.transport.release`).
    TransportRelease,
    /// Decides whether a combined debug shortcut starts, continues or does nothing.
    StartOrContinue,
    /// Validates a plugin backend's debug start request (`debug.decodeBackendConfiguration`).
    DecodeBackendConfiguration,
    /// Creates one transport-neutral Debug Adapter Protocol session (`debug.createSession`).
    CreateSession,
    /// Queues a launch or attach request for a debug session (`debug.launch`).
    Launch,
    /// Returns or normalizes portable stepping filters (`debug.steppingFilters`).
    SteppingFilters,
    /// Moves source breakpoints across one UTF-16 editor mutation (`debug.relocateBreakpoints`).
    RelocateBreakpoints,
    /// Plans the first page of one variable container (`debug.planVariablePage`).
    PlanVariablePage,
    /// Merges one variable page into its paging cursor (`debug.reduceVariablePage`).
    ReduceVariablePage,
    /// Plans the first request of one disassembly fetch (`debug.planDisassemblyWindow`).
    PlanDisassemblyWindow,
    /// Reduces one disassembly reply to the next bounded step (`debug.reduceDisassemblyPage`).
    ReduceDisassemblyPage,
    /// Replaces breakpoints for one source file (`debug.setBreakpoints`).
    SetBreakpoints,
    /// Replaces exception filters for one debug session (`debug.setExceptionBreakpoints`).
    SetExceptionBreakpoints,
    /// Replaces named function breakpoints for one debug session (`debug.setFunctionBreakpoints`).
    SetFunctionBreakpoints,
    /// Replaces instruction (address) breakpoints for one debug session (`debug.setInstructionBreakpoints`).
    SetInstructionBreakpoints,
    /// Resolves one adapter-owned data breakpoint identity (`debug.dataBreakpointInfo`).
    DataBreakpointInfo,
    /// Replaces data breakpoints for one debug session (`debug.setDataBreakpoints`).
    SetDataBreakpoints,
    /// Mutes or unmutes every breakpoint of one debug session (`debug.setBreakpointsMuted`).
    SetBreakpointsMuted,
    /// Places the memory view's readMemory window at an address (`debug.planMemoryWindow`).
    PlanMemoryWindow,
    /// Finds the byte read at in one readMemory reply (`debug.locateMemoryWindow`).
    LocateMemoryWindow,
    /// Replaces one visible variable value (`debug.setVariable`).
    SetVariable,
    /// Cancels or times out one pending debug operation (`debug.cancelOperation`).
    CancelOperation,
    /// Queues continue, pause, or stepping control (`debug.execute`).
    Execute,
    /// Queues one normalized debugger inspection request (`debug.inspect`).
    Inspect,
    /// Reduces bytes received from a platform-owned DAP transport (`debug.receive`).
    Receive,
    /// Completes one adapter-requested terminal launch (`debug.runInTerminalResponse`).
    RunInTerminalResponse,
    /// Completes one adapter-requested child session start (`debug.startDebuggingResponse`).
    StartDebuggingResponse,
    /// Begins the DAP disconnect handshake (`debug.disconnect`).
    Disconnect,
    /// Removes all state for a debug session (`debug.destroySession`).
    DestroySession,
}

#[derive(Debug, Clone)]
/// The `keymap.*` commands; their names live in [`CoreCommand::parse`].
pub enum KeymapCommand {
    /// Validates a normalized shortcut import against the host's action catalog.
    DecodeImport,
    /// Plans the selected provider's declared shortcut source inputs.
    PlanImportSources,
    /// Plans conflicts and default-key displacement for a shortcut import.
    PlanImportBindings,
    /// Compiles a bounded boolean shortcut context (`keymap.parseContext`).
    ParseContext,
    /// Evaluates a precompiled shortcut context (`keymap.evaluateContext`).
    EvaluateContext,
    /// Finds conflicts among the final layered shortcut bindings (`keymap.analyzeConflicts`).
    AnalyzeConflicts,
    /// Advances the double-Shift gesture recognizer by one event (`keymap.modifierGestureStep`).
    /// The rule itself lives in `lithe-core-rules` so the Windows frontend can run it synchronously.
    ModifierGestureStep,
    /// Reduces named shortcut schemes (`keymap.schemes.reduce`).
    SchemesReduce,
}

#[derive(Debug, Clone)]
/// The `toolchains.*` commands; their names live in [`CoreCommand::parse`].
pub enum ToolchainsCommand {
    /// Registers plugin-owned tools with verified package provenance.
    Registry,
    /// Validates provider answers using native evidence supplied by the host.
    DecodeReply,
    /// Builds the error reply for a provider query that timed out or found no backend.
    FailureReply,
    /// Resolves the nearest machine-local directory pin.
    ResolvePins,
    /// Edits a bounded batch of machine-local directory pins without native writes.
    EditPins,
    /// Plans native workspace pin source paths and byte budgets.
    PinStoragePlan,
    /// Parses a complete native pin document or its absent-source default.
    DecodePinDocument,
    /// Plans complete pin and ignore source edits without native writes.
    EditPinSources,
    /// Resolves consumer defaults while keeping workspace inheritance symbolic.
    ResolveConsumerPreferences,
    /// Edits one machine-local consumer preference and returns its effective input.
    EditConsumerPreferences,
    /// Resolves the four toolchain layers from a captured value snapshot.
    Resolve,
    /// Checks a plugin-defined version probe without per-tool parsing.
    CheckVersion,
    /// Plans system scans solely from enabled tool definitions.
    DiscoveryPlan,
    /// Lists checked installations and the automatic choice for one tool.
    Candidates,
    /// Expands a tool definition's launch template for a selected executable.
    InstantiateLaunch,
}

#[derive(Debug, Clone)]
/// The `build.*` commands; their names live in [`CoreCommand::parse`].
pub enum BuildCommand {
    /// Finds the build-system instances of a workspace (`build.detect`).
    Detect,
    /// Validates a native team's persisted Build options (`build.projectOptions`).
    ProjectOptions,
    /// Merges checked team and machine-local Build preferences without I/O.
    ResolveStoredOptions,
    /// Edits one declared option in a machine-local Build document without I/O.
    EditStoredOption,
    /// Lists a build system's executable candidates and version probes (`build.invocationPlan`).
    InvocationPlan,
    /// Compares a build tool's or runtime's version output with its minimum (`build.checkToolVersion`).
    CheckToolVersion,
    /// Resolves stored build options against the declaration (`build.resolveOptions`).
    ResolveOptions,
    /// Expands the command that prints a build model (`build.modelPlan`).
    ModelPlan,
    /// Expands one build task into an argv plan (`build.taskPlan`).
    TaskPlan,
    /// Expands a build test run and its result location (`build.testPlan`).
    TestPlan,
    /// Plans typed provider run preparation (`build.prepareRunPlan`).
    PrepareRunPlan,
    /// Plans source-backed configuration reads (`build.configurationPlan`).
    ConfigurationPlan,
    /// Validates non-sensitive native facts (`build.decodeConfiguration`).
    DecodeConfiguration,
    /// Plans provider-owned native configuration editing (`build.configurationEditPlan`).
    ConfigurationEditPlan,
    /// Validates a public edit receipt (`build.decodeConfigurationEdit`).
    DecodeConfigurationEdit,
    /// Resolves optional public build inputs (`build.resolveModelInputs`).
    ResolveModelInputs,
    /// Plans native debug preparation (`build.prepareDebugPlan`).
    PrepareDebugPlan,
    /// Validates native debug target facts (`build.decodeDebugPreparation`).
    DecodeDebugPreparation,
    /// Validates prepared launch data (`build.decodeRunPreparation`).
    DecodeRunPreparation,
    /// Plans provider-owned test discovery (`build.testDiscoveryPlan`).
    TestDiscoveryPlan,
    /// Validates provider-owned test selectors (`build.decodeTestDiscovery`).
    DecodeTestDiscovery,
    /// Expands the command that prints a project's dependencies (`build.dependencyPlan`).
    DependencyPlan,
    /// Normalizes a build model and derives its run entries (`build.decodeModel`).
    DecodeModel,
    /// Normalizes build test results (`build.decodeTestResults`).
    DecodeTestResults,
    /// Normalizes build diagnostics from process output (`build.decodeDiagnostics`).
    DecodeDiagnostics,
    /// Normalizes a project's dependency tree (`build.decodeDependencies`).
    DecodeDependencies,
    /// Digests the files a build model depends on (`build.syncFingerprint`).
    SyncFingerprint,
    /// Reduces a test run's termination and decoded summary to one verdict (`build.resolveTestRunOutcome`).
    ResolveTestRunOutcome,
    /// Merges a run's test results into the stored history inside the host-stated scope (`build.mergeTestOutcomes`).
    MergeTestOutcomes,
}

#[derive(Debug, Clone)]
/// The `workspace.*` commands; their names live in [`CoreCommand::parse`].
pub enum WorkspaceCommand {
    /// Builds the visible project tree (`workspace.snapshot`).
    Snapshot,
    /// Configure workspace indexing independently of tree visibility.
    ConfigureIndexing,
    /// Refreshes external indexing facts while retaining the owner's lease.
    RefreshIndexing,
    /// Releases an indexing generation when its project closes.
    ReleaseIndexing,
    /// Lists the visible children of project directories (`workspace.listDirectories`).
    ListDirectories,
    /// Says which loaded project tree nodes show as rows and keeps the selection on one
    /// (`workspace.projectTree.rows`).
    ProjectTreeRows,
    /// Applies one click to the multi-selection of rows of workspace paths
    /// (`workspace.rowSelection.reduce`).
    RowSelectionReduce,
    /// Orders natively enumerated directory siblings (`workspace.orderEntries`).
    OrderEntries,
    /// Ranks host-supplied candidate names with the shared matcher (`workspace.rankNames`).
    RankNames,
    /// Plans the name of a duplicated project item (`workspace.planDuplicate`).
    PlanDuplicate,
    /// Plans the destination names of pasted project items (`workspace.planPaste`).
    PlanPaste,
    /// Applies a directory role, exclusion or cancel action (`workspace.reduceDirectoryAction`).
    ReduceDirectoryAction,
    /// Migrates directory rules after a move or delete (`workspace.relocateDirectoryRules`).
    RelocateDirectoryRules,
    /// Filter ordinary workspace-relative event paths using shared defaults.
    FilterChanges,
    /// Finds Git repositories that belong to an opened workspace (`workspace.repositories`).
    Repositories,
    /// Applies one action to the recent project list (`workspace.recentProjects.reduce`).
    RecentProjectsReduce,
    /// Builds or reuses the workspace search index (`workspace.searchIndex.warm`).
    SearchIndexWarm,
    /// Applies changed paths to the search index (`workspace.searchIndex.update`).
    SearchIndexUpdate,
    /// Drops a workspace's cached search index (`workspace.searchIndex.invalidate`).
    SearchIndexInvalidate,
    /// Searches visible file paths and contents (`workspace.search`).
    Search,
    /// Searches file paths, contents, and symbols (`workspace.searchEverywhere`).
    SearchEverywhere,
    /// Previews replacements without writing files (`workspace.replacePreview`).
    ReplacePreview,
    /// Computes replacement on a supplied snapshot without writing (`workspace.replaceText`).
    ReplaceText,
    /// Searches provider-owned text snapshots in one request (`workspace.searchTextBatch`).
    SearchTextBatch,
    /// Classifies a batch of workspace-relative paths as hidden or visible (`workspace.classifyVisibility`).
    /// The rule lives in `lithe-core-rules` so the Windows frontend can run it synchronously.
    ClassifyVisibility,
    /// Returns the built-in hidden-path lists (`workspace.visibilityDefaults`), also from `lithe-core-rules`.
    VisibilityDefaults,
}

#[derive(Debug, Clone)]
/// The `history.*` commands; their names live in [`CoreCommand::parse`].
pub enum HistoryCommand {
    /// Records one local-history snapshot (`history.record`).
    Record,
    /// Lists retained local-history metadata (`history.entries`).
    Entries,
    /// Reads the contents of one history snapshot (`history.content`).
    Content,
    /// Moves history after a workspace path relocation (`history.relocate`).
    Relocate,
    /// Plans which snapshots an editing event records (`history.planEvent`).
    PlanEvent,
    /// Changes the optional label of a history snapshot (`history.rename`).
    Rename,
    /// Deletes one history snapshot (`history.delete`).
    Delete,
    /// Validates local-history ignore rules against a sample path (`history.ignorePreview`).
    IgnorePreview,
    /// Reports one project's history storage size (`history.usage`).
    Usage,
    /// Deletes every snapshot of one project (`history.clear`).
    Clear,
}

#[derive(Debug, Clone)]
/// The `editor.*` commands; their names live in [`CoreCommand::parse`].
pub enum EditorCommand {
    /// Applies one editor-group state transition: open, close, move, pin, preview, merge or normalize
    /// (`editor.workbenchState.reduce`). The rule lives in `lithe-core-rules` so the Windows frontend
    /// can run it synchronously inside store updates.
    WorkbenchStateReduce,
    /// Applies one transition to the editor groups and the tree that divides the editor area
    /// among them: a workbench state action, opening a tab, dropping a dragged one or removing
    /// empty groups (`editor.groupLayout.reduce`).
    GroupLayoutReduce,
    /// Says which places of each editor group accept a dragged tab
    /// (`editor.groupLayout.dropTargets`).
    GroupLayoutDropTargets,
    /// Applies a deterministic line-level editing transform
    /// (`editor.lineEdit`).
    LineEdit,
    /// Finds text or plans bounded replacements in a borrowed document (`editor.find`).
    Find,
    /// Resolves persisted editor presentation preferences (`editor.preferences.resolve`).
    PreferencesResolve,
    /// Reduces and queries bounded recent-file history (`editor.recent.reduce`).
    RecentReduce,
    /// Decides how a native checkpoint should recover (`editor.recovery.decide`).
    RecoveryDecide,
    /// Validates and orders a plugin document edit batch
    /// (`editor.planTextEdits`).
    PlanTextEdits,
    /// Parses a line or line:column navigation input (`editor.parseNavigationInput`).
    ParseNavigationInput,
    /// Reduces one bounded back/forward navigation history event
    /// (`editor.navigationHistory.reduce`).
    NavigationHistoryReduce,
    /// Resolves the line comment token for a file extension
    /// (`editor.lineCommentToken`).
    LineCommentToken,
}

#[derive(Debug, Clone)]
/// The `lsp.*` commands; their names live in [`CoreCommand::parse`].
pub enum LspCommand {
    /// Applies validated UTF-16 LSP text edits (`lsp.applyTextEdits`).
    ApplyTextEdits,
    /// Validates workspace-edit targets and orders their commit
    /// (`lsp.planWorkspaceEdit`).
    PlanWorkspaceEdit,
    /// Resolves a diagnostic identity inside its workspace (`lsp.documentIdentity`).
    DocumentIdentity,
    /// Decides whether a language-server jump target opens, opens read-only
    /// or is refused (`lsp.classifyNavigationTarget`).
    ClassifyNavigationTarget,
    /// Decides whether a go-to-declaration gesture opens the declaration or
    /// shows its usages (`lsp.declarationOrUsages`).
    DeclarationOrUsages,
    /// Reduces an LSP snippet to insertion text (`lsp.plainSnippet`).
    PlainSnippet,
    /// Provides same-document fallback completions (`lsp.builtinCompletions`).
    BuiltinCompletions,
    /// Provides a same-document fallback hover (`lsp.builtinHover`).
    BuiltinHover,
    /// Provides same-document fallback navigation (`lsp.builtinNavigation`).
    BuiltinNavigation,
    /// Merges completion lists from prioritized providers (`lsp.mergeCompletions`).
    MergeCompletions,
    /// Decides whether a language server may start automatically (`lsp.activationPolicy`).
    ActivationPolicy,
    /// Filters, orders, and groups Problems tool-window diagnostics (`lsp.problems.query`).
    ProblemsQuery,
    /// Starts and initializes a managed language server (`lsp.startServer`).
    StartServer,
    /// Gracefully shuts down a managed server (`lsp.stopServer`).
    StopServer,
    /// Replaces the plugin settings a plugin-provided server reads
    /// (`lsp.updateWorkspaceConfiguration`).
    UpdateWorkspaceConfiguration,
    /// Converts an absolute path to a file URI (`lsp.documentUri`).
    DocumentUri,
    /// Opens or updates a synchronized document (`lsp.syncDocument`).
    SyncDocument,
    /// Reports a successful native document save (`lsp.saveDocument`).
    SaveDocument,
    /// Publishes external workspace file changes (`lsp.workspaceFilesChanged`).
    WorkspaceFilesChanged,
    /// Closes a synchronized document (`lsp.closeDocument`).
    CloseDocument,
    /// Queues one semantic server request (`lsp.request`).
    Request,
    /// Queues one build-provider request (`lsp.buildRequest`).
    BuildRequest,
    /// Queues one VCS-provider request (`lsp.vcsRequest`).
    VcsRequest,
    /// Queues one toolchain-provider query (`lsp.toolchainRequest`).
    ToolchainRequest,
    /// Queues a shortcut import against a negotiated provider.
    KeymapImportRequest,
    /// Invokes one plugin function (`lsp.functionRequest`).
    FunctionRequest,
    /// Selects a registered function provider strictly inside one workspace.
    FunctionProvider,
    /// Replaces the plugins and workspace contexts the cross-plugin function
    /// relay checks calls against (`lsp.configureFunctionRelay`).
    ConfigureFunctionRelay,
    /// Answers a plugin backend's host-service request (`lsp.hostResponse`).
    HostResponse,
    /// Sends a plugin backend one host notification (`lsp.hostNotify`).
    HostNotify,
    /// Sends a plugin-owned language server one request of its backend (`lsp.pluginServerRequest`).
    PluginServerRequest,
    /// Sends a plugin-owned language server one notification of its backend (`lsp.pluginServerNotify`).
    PluginServerNotify,
    /// Cancels one pending semantic request (`lsp.cancelOperation`).
    CancelOperation,
    /// Drains queued session events (`lsp.pollEvents`).
    PollEvents,
    /// Waits for queued session events (`lsp.waitEvents`).
    WaitEvents,
    /// Toggles upstream verbose trace for one session (`lsp.setDetailedDiagnostics`).
    SetDetailedDiagnostics,
    /// Stops and removes a server session (`lsp.destroyServer`).
    DestroyServer,
}

#[derive(Debug, Clone)]
/// The `runConfig.*` commands; their names live in [`CoreCommand::parse`].
pub enum RunConfigCommand {
    /// Validates layered run-configuration documents (`runConfig.inspect`).
    Inspect,
    /// Reads effective saved tool paths for one Provider root (`runConfig.projectToolchain`).
    ProjectToolchain,
    /// Regenerates detected run configurations (`runConfig.generate`).
    Generate,
    /// Formats argument vectors as reversible editor text (`runConfig.argumentText`).
    ArgumentText,
    /// Merges configuration layers and toolchains (`runConfig.resolve`).
    Resolve,
    /// Selects a configuration for a provider-supplied context identity.
    SelectContextConfiguration,
    /// Decides what a press on the title bar's Run or Debug button does (`runConfig.decideTitleAction`).
    DecideTitleAction,
    /// Persists editable run options (`runConfig.updateOptions`).
    UpdateOptions,
    /// Replaces generic local tool preferences while preserving existing Run data.
    UpdateToolPreferences,
    /// Prepares one complete run-configuration editor save (`runConfig.saveEditorChanges`).
    SaveEditorChanges,
    /// Adds a user-authored run configuration (`runConfig.createUserConfiguration`).
    CreateUserConfiguration,
    /// Produces the process plan for one configuration (`runConfig.createLaunchPlan`).
    CreateLaunchPlan,
    /// Resolves user choices for a provider-owned launch (`runConfig.providerLaunchInputs`).
    ProviderLaunchInputs,
}

#[derive(Debug, Clone)]
/// The `git.*` commands; their names live in [`CoreCommand::parse`].
pub enum GitCommand {
    /// Reads normalized repository and working-tree state (`git.status`).
    Status,
    /// Lists one level of a summarized untracked directory (`git.untrackedDirectory`).
    UntrackedDirectory,
    /// Reads HEAD and index preconditions for workspace commits (`git.commitState`).
    CommitState,
    /// Builds a reviewed multi-repository plan (`git.workspaceCommitPrepare`).
    WorkspaceCommitPrepare,
    /// Executes one guarded workspace commit/push (`git.workspaceCommitStep`).
    WorkspaceCommitStep,
    /// Classifies a bounded page of Git watcher paths (`git.classifyMetadataChanges`).
    ClassifyMetadataChanges,
    /// Resolves paths a Git-aware watcher must observe (`git.watchContext`).
    WatchContext,
    /// Lists worktrees registered for the repository (`git.worktrees`).
    Worktrees,
    /// Describes the checked-out branch or detached worktree for PR creation (`git.pullRequestContext`).
    PullRequestContext,
    /// Executes a caller-supplied argument vector without a shell (`git.command`).
    Command,
    /// Resolves the containing repository root without acquiring a write lease (`git.repositoryRoot`).
    RepositoryRoot,
    /// Resolves a file to its owning Git checkout (`git.resolveFileContext`).
    ResolveFileContext,
    /// Plans workspace status publication across successful and failed roots (`git.workspaceStatusMerge`).
    WorkspaceStatusMerge,
    /// Performs one supported Git mutation (`git.write`).
    Write,
    /// Plans whole-file rollback from Git change identities (`git.rollbackPlan`).
    RollbackPlan,
    /// Rechecks Git identities and plans rollback (`git.rollbackPrepare`).
    RollbackPrepare,
    /// Prepares the real index for an explicit selected-file commit.
    PrepareCommitSelection,
    /// Reconciles missing selected additions after deletion.
    ReconcileDeletedSelection,
    /// Reduces local ChangeList metadata and projects membership and commit scope.
    Changelists,
    /// Captures or applies one guarded Shelf layer (`git.shelf`).
    Shelf,
    /// Captures and validates AI commit evidence (`git.commitMessageContext`).
    CommitMessageContext,
    /// Projects retained Git console records without executing Git (`git.consolePresentation`).
    ConsolePresentation,
    /// Reads an optional remote URL without a visible command (`git.remoteUrl`).
    RemoteUrl,
    /// Plans Fetch arguments without launching Git or changing configuration (`git.fetchPlan`).
    FetchPlan,
    /// Resolves the executable Git will run without requiring a repository (`git.executable`).
    Executable,
    /// Inspects Git configuration and its sources (`git.executionInspect`).
    ExecutionInspect,
    /// Saves one explicitly selected configuration field (`git.executionConfigure`).
    ExecutionConfigure,
    /// Answers a currently owned authentication prompt (`git.authRespond`).
    AuthRespond,
    /// Inspects repository initialization and scoped commit identity.
    RepositorySetup,
    /// Initializes an existing folder outside any repository.
    Initialize,
    /// Writes or clears one explicitly scoped commit identity field.
    ConfigureIdentity,
    /// Reviews a local history mutation and its immutable checkout snapshot (`git.historyRewritePreview`).
    HistoryRewritePreview,
    /// Lists configured fetch remotes.
    Remotes,
    /// Lists tags with object and target identities.
    Tags,
    /// Parses a bounded HEAD snapshot without filesystem access.
    ParseCheckoutMetadata,
    /// Parses a bounded gitdir pointer without filesystem access.
    ParseGitdirPointer,
    /// Resolves action eligibility against the actual checkout history.
    HistoryActionCapabilities,
    /// Preview a linear local range after an unchanged base commit.
    RebasePreview,
    /// Start a reviewed native interactive rebase with a durable session.
    RebaseStart,
    /// Read the current checkout's last owned interactive rebase session.
    RebaseSession,
    /// Continue, skip, or abort an identified native rebase session.
    RebaseControl,
    /// Exports a lossless UTF-8 exchange patch (`git.patchExport`).
    PatchExport,
    /// Reviews a patch with a forward applicability check (`git.patchPreview`).
    PatchPreview,
    /// Applies a patch tied to its reviewed snapshot (`git.patchApply`).
    PatchApply,
    /// Builds a structured Git diff (`git.diff`).
    Diff,
    /// Parses a saved patch into validated per-file documents (`git.parsePatch`).
    ParsePatch,
    /// Applies a patch to the index or working tree (`git.apply`).
    Apply,
    /// Plans commit-review blocks and their Core-owned patches (`git.planDiffBlocks`).
    PlanDiffBlocks,
    /// Includes, excludes or rolls back one planned block (`git.applyDiffBlock`).
    ApplyDiffBlock,
    /// Stages one editor gutter change with its original bytes (`git.stageGutterChange`).
    StageGutterChange,
    /// Lists references and bounded commit history (`git.history`).
    History,
    /// Filters commit dates against explicit absolute bounds (`git.historyQuery`).
    HistoryQuery,
    /// Lists Git references without repeating commit history (`git.references`).
    References,
    /// Returns one cursor-based commit page (`git.historyPage`).
    HistoryPage,
    /// Releases an incremental history cursor (`git.historyCursorClose`).
    HistoryCursorClose,
    /// Locates one commit while appending bounded history pages (`git.historyLocate`).
    HistoryLocate,
    /// Cleans and validates a new branch name in loaded references (`git.branchNamePreview`).
    BranchNamePreview,
    /// Assigns palette slots to a page of ordered repository identities.
    RepositoryColorAssignments,
    /// Plans a branch Update or Pull from collected facts (`git.updatePlan`).
    UpdatePlan,
    /// Plans history refresh windows, page merges, and selection retention (`git.historyWindow`).
    HistoryWindow,
    /// Projects the commit graph's lanes, colors, routes, and labels (`git.graphProjection`).
    GraphProjection,
    /// Drops targets already covered by a selected untracked summary (`git.normalizeChangeTargets`).
    NormalizeChangeTargets,
    /// Validates an interactive rebase draft and derives its messages (`git.rebasePlanPreview`).
    RebasePlanPreview,
    /// Groups one kind of loaded references into an ordered tree (`git.referenceTree`).
    ReferenceTree,
    /// Groups changed files by folder in natural order (`git.changedFileGroups`).
    ChangedFileGroups,
    /// Derives the worktree creation dialog's choices, branch, destination and readiness
    /// (`git.worktreeCreationPreview`).
    WorktreeCreationPreview,
    /// Resolves the destination and commits for a branch push (`git.pushPreview`).
    PushPreview,
    /// Resolves metadata for one commit (`git.commit`).
    Commit,
    /// Lists paths changed by one commit (`git.commitFiles`).
    CommitFiles,
    /// Compares a reference with the current checkout (`git.comparison`).
    Comparison,
    /// Lists repository stashes (`git.stashes`).
    Stashes,
    /// Finds edits that would block checkout (`git.checkoutPreflight`).
    CheckoutPreflight,
    /// Reports whether the tracked branch can fast-forward (`git.pullPreflight`).
    PullPreflight,
    /// Finds state that blocks merge, rebase, cherry-pick, or revert (`git.integrationPreflight`).
    IntegrationPreflight,
    /// Finds staged files containing conflict markers (`git.conflictMarkers`).
    ConflictMarkers,
    /// Inspects an interrupted sequential Git operation (`git.operationState`).
    OperationState,
    /// Returns normalized line attribution (`git.blame`).
    Blame,
    /// Traces which commits touched a snapshot line range (`git.lineHistory`).
    LineHistory,
    /// Previews a non-destructive GitHub pull-request checkout.
    PullRequestCheckoutPreview,
    /// Fetches and creates the reviewed pull-request branch without resetting refs.
    PullRequestCheckout,
}

#[derive(Debug, Clone)]
/// The `remote.*` commands; their names live in [`CoreCommand::parse`].
pub enum RemoteCommand {
    /// Checks that an SSH connection reaches a supported host (`remote.probe`).
    Probe,
    /// Answers a prompt relayed by a running `remote.probe` (`remote.authRespond`).
    AuthRespond,
    /// Opens a remote connection, installing the server when needed (`remote.connect`).
    Connect,
    /// Lists a directory over an open remote connection (`remote.readDir`).
    ReadDir,
    /// Closes a remote connection (`remote.disconnect`).
    Disconnect,
    /// Closes every open remote connection at application exit (`remote.closeAll`).
    CloseAll,
    /// Reads a file over an open remote connection (`remote.readFile`).
    ReadFile,
    /// Saves a file over an open remote connection (`remote.writeFile`).
    WriteFile,
    /// Performs a non-streaming operation on an existing workspace host.
    HostRequest,
    /// Owns a remote process and relays output until it exits or is cancelled.
    RunProcess,
    /// Captures a short remote process with explicit time and output budgets.
    CaptureProcess,
    /// Owns a remote PTY and relays output until it exits or is cancelled.
    RunTerminal,
    /// Owns a host watch and relays changes until cancellation or root removal.
    RunWatch,
    /// Streams a native source into integrity-checked host file staging.
    UploadFile,
    /// Returns cached handshake capabilities of one connection.
    HostInfo,
}

#[derive(Debug, Clone)]
/// The `diagnostics.*` commands; their names live in [`CoreCommand::parse`].
pub enum DiagnosticsCommand {
    /// Redacts credentials, tokens, and home-directory paths from diagnostic text (`diagnostics.redactText`).
    RedactText,
    /// Normalizes a bounded, redacted application log event (`diagnostics.prepareLogEvent`).
    PrepareLogEvent,
    /// Filters and merges bounded host-collected log tails (`diagnostics.queryLogs`).
    QueryLogs,
    /// Encodes structured log events independently in one batch (`diagnostics.encodeLogBatch`).
    EncodeLogBatch,
    /// Drains bounded process-owned Core log facts (`diagnostics.drainCoreEvents`).
    DrainCoreEvents,
    /// Selects which DEBUG Core facts the host will write (`diagnostics.configureCoreEvents`).
    ConfigureCoreEvents,
    /// Records host-observed declarative contribution outcomes (`diagnostics.recordPluginOperation`).
    RecordPluginOperation,
    /// Merges structured and legacy log evidence (`diagnostics.queryLogsV2`).
    QueryLogsV2,
    /// Parses filter terms and UTF-16 diagnostics (`diagnostics.parseLogFilter`).
    ParseLogFilter,
    /// Composes a context-menu predicate (`diagnostics.composeLogFilter`).
    ComposeLogFilter,
    /// Formats canonical redacted evidence (`diagnostics.formatLogRecords`).
    FormatLogRecords,
    /// Plans bounded native tail reads (`diagnostics.planLogCollection`).
    PlanLogCollection,
    /// Plans log cleanup without filesystem access (`diagnostics.planLogRetention`).
    PlanLogRetention,
    /// Validates a logical channel and resolves its session-relative path (`diagnostics.resolveLogChannel`).
    ResolveLogChannel,
    /// Resolves the engine routing identity without starting a session (`diagnostics.resolveEngineLogSubject`).
    ResolveEngineLogSubject,
    /// Shapes a deterministic diagnostic bundle manifest from host-gathered facts (`diagnostics.buildManifest`).
    BuildManifest,
}

impl CoreCommand {
    /// Resolves a compatibility command name without accepting aliases or
    /// case variations that could behave differently across hosts.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "core.ping" => Some(Self::Ping),
            "ideHost.control" => Some(Self::IdeHostControl),
            "ai.commitInputBudget" => Some(Self::Ai(AiCommand::CommitInputBudget)),
            "ai.planCommit" => Some(Self::Ai(AiCommand::PlanCommit)),
            "ai.decodeCommitMessage" => Some(Self::Ai(AiCommand::DecodeCommitMessage)),
            "ai.configuration.sources" => Some(Self::Ai(AiCommand::ConfigurationSources)),
            "ai.configuration.parse" => Some(Self::Ai(AiCommand::ConfigurationParse)),
            "ai.pullRequest.plan" => Some(Self::Ai(AiCommand::PullRequestPlan)),
            "ai.pullRequest.decode" => Some(Self::Ai(AiCommand::PullRequestDecode)),
            "ai.inlineEdit.plan" => Some(Self::Ai(AiCommand::InlineEditPlan)),
            "ai.inlineEdit.decode" => Some(Self::Ai(AiCommand::InlineEditDecode)),
            "plugins.declarative.validate" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeValidate))
            }
            "plugins.nativeUI.validateData" => {
                Some(Self::Plugins(PluginsCommand::NativeUIValidateData))
            }
            "plugins.web.open" => Some(Self::Plugins(PluginsCommand::WebOpen)),
            "plugins.web.close" => Some(Self::Plugins(PluginsCommand::WebClose)),
            "plugins.web.resource" => Some(Self::Plugins(PluginsCommand::WebResource)),
            "plugins.web.document" => Some(Self::Plugins(PluginsCommand::WebDocument)),
            "plugins.web.request" => Some(Self::Plugins(PluginsCommand::WebRequest)),
            "plugins.web.reply" => Some(Self::Plugins(PluginsCommand::WebReply)),
            "plugins.web.event" => Some(Self::Plugins(PluginsCommand::WebEvent)),
            "plugins.declarative.runPlan" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeRunPlan))
            }
            "plugins.declarative.discoverTests" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeDiscoverTests))
            }
            "plugins.declarative.testPlan" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeTestPlan))
            }
            "plugins.declarative.languageServerLaunchPlan" => Some(Self::Plugins(
                PluginsCommand::DeclarativeLanguageServerLaunchPlan,
            )),
            "plugins.languageServers.decodeLaunch" => {
                Some(Self::Plugins(PluginsCommand::LanguageServersDecodeLaunch))
            }
            "plugins.languageServers.stateRetention" => {
                Some(Self::Plugins(PluginsCommand::LanguageServersStateRetention))
            }
            "plugins.contentProviders.decode" => {
                Some(Self::Plugins(PluginsCommand::ContentProvidersDecode))
            }
            "debug.adapters.startArguments" => {
                Some(Self::Debug(DebugCommand::AdaptersStartArguments))
            }
            "debug.validateAdapterEndpoint" => {
                Some(Self::Debug(DebugCommand::ValidateAdapterEndpoint))
            }
            "debug.adapters.executablePlan" => {
                Some(Self::Debug(DebugCommand::AdaptersExecutablePlan))
            }
            "debug.transport.policy" => Some(Self::Debug(DebugCommand::TransportPolicy)),
            "debug.transport.handle" => Some(Self::Debug(DebugCommand::TransportHandle)),
            "debug.transport.release" => Some(Self::Debug(DebugCommand::TransportRelease)),
            "plugins.declarative.resolveSettings" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeResolveSettings))
            }
            "plugins.settings.catalog" => Some(Self::Plugins(PluginsCommand::SettingsCatalog)),
            "plugins.settings.navigation" => {
                Some(Self::Plugins(PluginsCommand::SettingsNavigation))
            }
            "plugins.settings.delivery" => Some(Self::Plugins(PluginsCommand::SettingsDelivery)),
            "plugins.declarative.settingsState" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeSettingsState))
            }
            "plugins.declarative.settingsActionPlan" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeSettingsActionPlan))
            }
            "plugins.installed.select" => Some(Self::Plugins(PluginsCommand::InstalledSelect)),
            "plugins.installed.lifecycle" => {
                Some(Self::Plugins(PluginsCommand::InstalledLifecycle))
            }
            "plugins.declarative.editSettings" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeEditSettings))
            }
            "plugins.declarative.workspaceConfiguration" => Some(Self::Plugins(
                PluginsCommand::DeclarativeWorkspaceConfiguration,
            )),
            "plugins.declarative.decodeGrammar" => {
                Some(Self::Plugins(PluginsCommand::DeclarativeDecodeGrammar))
            }
            "plugins.editorActions.decode" => {
                Some(Self::Plugins(PluginsCommand::EditorActionsDecode))
            }
            "plugins.hostApi.describe" => Some(Self::Plugins(PluginsCommand::HostApiDescribe)),
            "plugins.hostApi.authorize" => Some(Self::Plugins(PluginsCommand::HostApiAuthorize)),
            "plugins.editorProviders.decode" => {
                Some(Self::Plugins(PluginsCommand::EditorProvidersDecode))
            }
            "keymap.decodeImport" => Some(Self::Keymap(KeymapCommand::DecodeImport)),
            "keymap.planImportSources" => Some(Self::Keymap(KeymapCommand::PlanImportSources)),
            "keymap.planImportBindings" => Some(Self::Keymap(KeymapCommand::PlanImportBindings)),
            "debug.startOrContinue" => Some(Self::Debug(DebugCommand::StartOrContinue)),
            "toolchains.registry" => Some(Self::Toolchains(ToolchainsCommand::Registry)),
            "toolchains.decodeReply" => Some(Self::Toolchains(ToolchainsCommand::DecodeReply)),
            "toolchains.failureReply" => Some(Self::Toolchains(ToolchainsCommand::FailureReply)),
            "toolchains.resolvePins" => Some(Self::Toolchains(ToolchainsCommand::ResolvePins)),
            "toolchains.editPins" => Some(Self::Toolchains(ToolchainsCommand::EditPins)),
            "toolchains.pinStoragePlan" => {
                Some(Self::Toolchains(ToolchainsCommand::PinStoragePlan))
            }
            "toolchains.decodePinDocument" => {
                Some(Self::Toolchains(ToolchainsCommand::DecodePinDocument))
            }
            "toolchains.editPinSources" => {
                Some(Self::Toolchains(ToolchainsCommand::EditPinSources))
            }
            "toolchains.resolveConsumerPreferences" => Some(Self::Toolchains(
                ToolchainsCommand::ResolveConsumerPreferences,
            )),
            "toolchains.editConsumerPreferences" => {
                Some(Self::Toolchains(ToolchainsCommand::EditConsumerPreferences))
            }
            "toolchains.resolve" => Some(Self::Toolchains(ToolchainsCommand::Resolve)),
            "toolchains.checkVersion" => Some(Self::Toolchains(ToolchainsCommand::CheckVersion)),
            "toolchains.discoveryPlan" => Some(Self::Toolchains(ToolchainsCommand::DiscoveryPlan)),
            "toolchains.candidates" => Some(Self::Toolchains(ToolchainsCommand::Candidates)),
            "toolchains.instantiateLaunch" => {
                Some(Self::Toolchains(ToolchainsCommand::InstantiateLaunch))
            }
            "run.decodeStart" => Some(Self::RunDecodeStart),
            "debug.decodeBackendConfiguration" => {
                Some(Self::Debug(DebugCommand::DecodeBackendConfiguration))
            }
            "outputLinks.match" => Some(Self::OutputLinksMatch),
            "outputLinks.decodeResolution" => Some(Self::OutputLinksDecodeResolution),
            "build.detect" => Some(Self::Build(BuildCommand::Detect)),
            "build.projectOptions" => Some(Self::Build(BuildCommand::ProjectOptions)),
            "build.resolveStoredOptions" => Some(Self::Build(BuildCommand::ResolveStoredOptions)),
            "build.editStoredOption" => Some(Self::Build(BuildCommand::EditStoredOption)),
            "build.invocationPlan" => Some(Self::Build(BuildCommand::InvocationPlan)),
            "build.checkToolVersion" => Some(Self::Build(BuildCommand::CheckToolVersion)),
            "build.resolveOptions" => Some(Self::Build(BuildCommand::ResolveOptions)),
            "build.modelPlan" => Some(Self::Build(BuildCommand::ModelPlan)),
            "build.taskPlan" => Some(Self::Build(BuildCommand::TaskPlan)),
            "build.testPlan" => Some(Self::Build(BuildCommand::TestPlan)),
            "build.prepareDebugPlan" => Some(Self::Build(BuildCommand::PrepareDebugPlan)),
            "build.decodeDebugPreparation" => {
                Some(Self::Build(BuildCommand::DecodeDebugPreparation))
            }
            "build.resolveModelInputs" => Some(Self::Build(BuildCommand::ResolveModelInputs)),
            "build.configurationPlan" => Some(Self::Build(BuildCommand::ConfigurationPlan)),
            "build.configurationEditPlan" => Some(Self::Build(BuildCommand::ConfigurationEditPlan)),
            "build.decodeConfigurationEdit" => {
                Some(Self::Build(BuildCommand::DecodeConfigurationEdit))
            }
            "build.decodeConfiguration" => Some(Self::Build(BuildCommand::DecodeConfiguration)),
            "build.prepareRunPlan" => Some(Self::Build(BuildCommand::PrepareRunPlan)),
            "build.decodeRunPreparation" => Some(Self::Build(BuildCommand::DecodeRunPreparation)),
            "build.testDiscoveryPlan" => Some(Self::Build(BuildCommand::TestDiscoveryPlan)),
            "build.decodeTestDiscovery" => Some(Self::Build(BuildCommand::DecodeTestDiscovery)),
            "build.dependencyPlan" => Some(Self::Build(BuildCommand::DependencyPlan)),
            "build.decodeModel" => Some(Self::Build(BuildCommand::DecodeModel)),
            "build.decodeTestResults" => Some(Self::Build(BuildCommand::DecodeTestResults)),
            "build.decodeDiagnostics" => Some(Self::Build(BuildCommand::DecodeDiagnostics)),
            "build.decodeDependencies" => Some(Self::Build(BuildCommand::DecodeDependencies)),
            "build.syncFingerprint" => Some(Self::Build(BuildCommand::SyncFingerprint)),
            "build.resolveTestRunOutcome" => Some(Self::Build(BuildCommand::ResolveTestRunOutcome)),
            "build.mergeTestOutcomes" => Some(Self::Build(BuildCommand::MergeTestOutcomes)),
            "vcs.detectRoots" => Some(Self::VcsDetectRoots),
            "vcs.ownerOf" => Some(Self::VcsOwnerOf),
            "vcs.decodeReply" => Some(Self::VcsDecodeReply),
            "project.generatorCatalog" => Some(Self::ProjectGeneratorCatalog),
            "project.generatorForm" => Some(Self::ProjectGeneratorForm),
            "project.generationPlan" => Some(Self::ProjectGenerationPlan),
            "project.checkName" => Some(Self::ProjectCheckName),
            "project.extractArchive" => Some(Self::ProjectExtractArchive),
            "plugins.marketplace.validateIndex" => {
                Some(Self::Plugins(PluginsCommand::MarketplaceValidateIndex))
            }
            "plugins.marketplace.resolveSource" => {
                Some(Self::Plugins(PluginsCommand::MarketplaceResolveSource))
            }
            "plugins.marketplace.installReview" => {
                Some(Self::Plugins(PluginsCommand::MarketplaceInstallReview))
            }
            "workspace.snapshot" => Some(Self::Workspace(WorkspaceCommand::Snapshot)),
            "workspace.releaseIndexing" => Some(Self::Workspace(WorkspaceCommand::ReleaseIndexing)),
            "workspace.configureIndexing" => {
                Some(Self::Workspace(WorkspaceCommand::ConfigureIndexing))
            }
            "workspace.refreshIndexing" => Some(Self::Workspace(WorkspaceCommand::RefreshIndexing)),
            "workspace.filterChanges" => Some(Self::Workspace(WorkspaceCommand::FilterChanges)),
            "workspace.listDirectories" => Some(Self::Workspace(WorkspaceCommand::ListDirectories)),
            "workspace.projectTree.rows" => {
                Some(Self::Workspace(WorkspaceCommand::ProjectTreeRows))
            }
            "workspace.rowSelection.reduce" => {
                Some(Self::Workspace(WorkspaceCommand::RowSelectionReduce))
            }
            "workspace.orderEntries" => Some(Self::Workspace(WorkspaceCommand::OrderEntries)),
            "workspace.rankNames" => Some(Self::Workspace(WorkspaceCommand::RankNames)),
            "workspace.planDuplicate" => Some(Self::Workspace(WorkspaceCommand::PlanDuplicate)),
            "workspace.planPaste" => Some(Self::Workspace(WorkspaceCommand::PlanPaste)),
            "workspace.reduceDirectoryAction" => {
                Some(Self::Workspace(WorkspaceCommand::ReduceDirectoryAction))
            }
            "workspace.relocateDirectoryRules" => {
                Some(Self::Workspace(WorkspaceCommand::RelocateDirectoryRules))
            }
            "workspace.repositories" => Some(Self::Workspace(WorkspaceCommand::Repositories)),
            "workspace.recentProjects.reduce" => {
                Some(Self::Workspace(WorkspaceCommand::RecentProjectsReduce))
            }
            "workspace.searchIndex.warm" => {
                Some(Self::Workspace(WorkspaceCommand::SearchIndexWarm))
            }
            "workspace.searchIndex.update" => {
                Some(Self::Workspace(WorkspaceCommand::SearchIndexUpdate))
            }
            "workspace.searchIndex.invalidate" => {
                Some(Self::Workspace(WorkspaceCommand::SearchIndexInvalidate))
            }
            "search.actions" => Some(Self::SearchActions),
            "workspace.search" => Some(Self::Workspace(WorkspaceCommand::Search)),
            "workspace.searchEverywhere" => {
                Some(Self::Workspace(WorkspaceCommand::SearchEverywhere))
            }
            "workspace.replacePreview" => Some(Self::Workspace(WorkspaceCommand::ReplacePreview)),
            "workspace.replaceText" => Some(Self::Workspace(WorkspaceCommand::ReplaceText)),
            "workspace.searchTextBatch" => Some(Self::Workspace(WorkspaceCommand::SearchTextBatch)),
            "file.read" => Some(Self::FileRead),
            "file.write" => Some(Self::FileWrite),
            "text.decode" => Some(Self::TextDecode),
            "text.encode" => Some(Self::TextEncode),
            "document.lifecycle" => Some(Self::DocumentLifecycle),
            "document.classifyText" => Some(Self::DocumentClassifyText),
            "document.planSaveBatch" => Some(Self::DocumentPlanSaveBatch),
            "document.reduceSaveBatch" => Some(Self::DocumentReduceSaveBatch),
            "history.record" => Some(Self::History(HistoryCommand::Record)),
            "history.entries" => Some(Self::History(HistoryCommand::Entries)),
            "history.content" => Some(Self::History(HistoryCommand::Content)),
            "history.relocate" => Some(Self::History(HistoryCommand::Relocate)),
            "history.planEvent" => Some(Self::History(HistoryCommand::PlanEvent)),
            "history.rename" => Some(Self::History(HistoryCommand::Rename)),
            "history.delete" => Some(Self::History(HistoryCommand::Delete)),
            "history.ignorePreview" => Some(Self::History(HistoryCommand::IgnorePreview)),
            "history.usage" => Some(Self::History(HistoryCommand::Usage)),
            "history.clear" => Some(Self::History(HistoryCommand::Clear)),
            "diff.compareTexts" => Some(Self::DiffCompareTexts),
            "execution.resolveEnvironment" => Some(Self::ExecutionResolveEnvironment),
            "execution.resolveSearchPath" => Some(Self::ExecutionResolveSearchPath),
            "processLifecycle.resolve" => Some(Self::ProcessLifecycleResolve),
            "processLifecycle.inventory" => Some(Self::ProcessLifecycleInventory),
            "processLifecycle.settle" => Some(Self::ProcessLifecycleSettle),
            "markdown.render" => Some(Self::MarkdownRender),
            "keymap.parseContext" => Some(Self::Keymap(KeymapCommand::ParseContext)),
            "keymap.evaluateContext" => Some(Self::Keymap(KeymapCommand::EvaluateContext)),
            "keymap.analyzeConflicts" => Some(Self::Keymap(KeymapCommand::AnalyzeConflicts)),
            "keymap.modifierGestureStep" => Some(Self::Keymap(KeymapCommand::ModifierGestureStep)),
            "editor.workbenchState.reduce" => {
                Some(Self::Editor(EditorCommand::WorkbenchStateReduce))
            }
            "editor.groupLayout.reduce" => Some(Self::Editor(EditorCommand::GroupLayoutReduce)),
            "editor.groupLayout.dropTargets" => {
                Some(Self::Editor(EditorCommand::GroupLayoutDropTargets))
            }
            "workspace.classifyVisibility" => {
                Some(Self::Workspace(WorkspaceCommand::ClassifyVisibility))
            }
            "workspace.visibilityDefaults" => {
                Some(Self::Workspace(WorkspaceCommand::VisibilityDefaults))
            }
            "workbench.chromeLayout.reduce" => Some(Self::WorkbenchChromeLayoutReduce),
            "terminal.parseLinkTarget" => Some(Self::TerminalParseLinkTarget),
            "debug.createSession" => Some(Self::Debug(DebugCommand::CreateSession)),
            "debug.launch" => Some(Self::Debug(DebugCommand::Launch)),
            "debug.steppingFilters" => Some(Self::Debug(DebugCommand::SteppingFilters)),
            "debug.relocateBreakpoints" => Some(Self::Debug(DebugCommand::RelocateBreakpoints)),
            "debug.planVariablePage" => Some(Self::Debug(DebugCommand::PlanVariablePage)),
            "debug.reduceVariablePage" => Some(Self::Debug(DebugCommand::ReduceVariablePage)),
            "debug.planDisassemblyWindow" => Some(Self::Debug(DebugCommand::PlanDisassemblyWindow)),
            "debug.reduceDisassemblyPage" => Some(Self::Debug(DebugCommand::ReduceDisassemblyPage)),
            "debug.setBreakpoints" => Some(Self::Debug(DebugCommand::SetBreakpoints)),
            "debug.setExceptionBreakpoints" => {
                Some(Self::Debug(DebugCommand::SetExceptionBreakpoints))
            }
            "debug.setFunctionBreakpoints" => {
                Some(Self::Debug(DebugCommand::SetFunctionBreakpoints))
            }
            "debug.setInstructionBreakpoints" => {
                Some(Self::Debug(DebugCommand::SetInstructionBreakpoints))
            }
            "debug.dataBreakpointInfo" => Some(Self::Debug(DebugCommand::DataBreakpointInfo)),
            "debug.setDataBreakpoints" => Some(Self::Debug(DebugCommand::SetDataBreakpoints)),
            "debug.setBreakpointsMuted" => Some(Self::Debug(DebugCommand::SetBreakpointsMuted)),
            "debug.planMemoryWindow" => Some(Self::Debug(DebugCommand::PlanMemoryWindow)),
            "debug.locateMemoryWindow" => Some(Self::Debug(DebugCommand::LocateMemoryWindow)),
            "debug.setVariable" => Some(Self::Debug(DebugCommand::SetVariable)),
            "debug.cancelOperation" => Some(Self::Debug(DebugCommand::CancelOperation)),
            "debug.execute" => Some(Self::Debug(DebugCommand::Execute)),
            "debug.inspect" => Some(Self::Debug(DebugCommand::Inspect)),
            "debug.receive" => Some(Self::Debug(DebugCommand::Receive)),
            "debug.runInTerminalResponse" => Some(Self::Debug(DebugCommand::RunInTerminalResponse)),
            "debug.startDebuggingResponse" => {
                Some(Self::Debug(DebugCommand::StartDebuggingResponse))
            }
            "debug.disconnect" => Some(Self::Debug(DebugCommand::Disconnect)),
            "debug.destroySession" => Some(Self::Debug(DebugCommand::DestroySession)),
            "lsp.applyTextEdits" => Some(Self::Lsp(LspCommand::ApplyTextEdits)),
            "lsp.planWorkspaceEdit" => Some(Self::Lsp(LspCommand::PlanWorkspaceEdit)),
            "lsp.documentIdentity" => Some(Self::Lsp(LspCommand::DocumentIdentity)),
            "lsp.classifyNavigationTarget" => Some(Self::Lsp(LspCommand::ClassifyNavigationTarget)),
            "lsp.declarationOrUsages" => Some(Self::Lsp(LspCommand::DeclarationOrUsages)),
            "editor.lineEdit" => Some(Self::Editor(EditorCommand::LineEdit)),
            "editor.find" => Some(Self::Editor(EditorCommand::Find)),
            "editor.preferences.resolve" => Some(Self::Editor(EditorCommand::PreferencesResolve)),
            "editor.recent.reduce" => Some(Self::Editor(EditorCommand::RecentReduce)),
            "editor.recovery.decide" => Some(Self::Editor(EditorCommand::RecoveryDecide)),
            "keymap.schemes.reduce" => Some(Self::Keymap(KeymapCommand::SchemesReduce)),
            "editor.planTextEdits" => Some(Self::Editor(EditorCommand::PlanTextEdits)),
            "editor.parseNavigationInput" => {
                Some(Self::Editor(EditorCommand::ParseNavigationInput))
            }
            "editor.navigationHistory.reduce" => {
                Some(Self::Editor(EditorCommand::NavigationHistoryReduce))
            }
            "editor.lineCommentToken" => Some(Self::Editor(EditorCommand::LineCommentToken)),
            "network.proxyPolicy" => Some(Self::NetworkProxyPolicy),
            "lsp.plainSnippet" => Some(Self::Lsp(LspCommand::PlainSnippet)),
            "lsp.builtinCompletions" => Some(Self::Lsp(LspCommand::BuiltinCompletions)),
            "lsp.builtinHover" => Some(Self::Lsp(LspCommand::BuiltinHover)),
            "lsp.builtinNavigation" => Some(Self::Lsp(LspCommand::BuiltinNavigation)),
            "lsp.mergeCompletions" => Some(Self::Lsp(LspCommand::MergeCompletions)),
            "lsp.activationPolicy" => Some(Self::Lsp(LspCommand::ActivationPolicy)),
            "lsp.problems.query" => Some(Self::Lsp(LspCommand::ProblemsQuery)),
            "lsp.startServer" => Some(Self::Lsp(LspCommand::StartServer)),
            "lsp.stopServer" => Some(Self::Lsp(LspCommand::StopServer)),
            "lsp.updateWorkspaceConfiguration" => {
                Some(Self::Lsp(LspCommand::UpdateWorkspaceConfiguration))
            }
            "lsp.documentUri" => Some(Self::Lsp(LspCommand::DocumentUri)),
            "lsp.syncDocument" => Some(Self::Lsp(LspCommand::SyncDocument)),
            "lsp.saveDocument" => Some(Self::Lsp(LspCommand::SaveDocument)),
            "lsp.workspaceFilesChanged" => Some(Self::Lsp(LspCommand::WorkspaceFilesChanged)),
            "lsp.closeDocument" => Some(Self::Lsp(LspCommand::CloseDocument)),
            "lsp.request" => Some(Self::Lsp(LspCommand::Request)),
            "lsp.buildRequest" => Some(Self::Lsp(LspCommand::BuildRequest)),
            "lsp.vcsRequest" => Some(Self::Lsp(LspCommand::VcsRequest)),
            "lsp.toolchainRequest" => Some(Self::Lsp(LspCommand::ToolchainRequest)),
            "lsp.keymapImportRequest" => Some(Self::Lsp(LspCommand::KeymapImportRequest)),
            "lsp.functionRequest" => Some(Self::Lsp(LspCommand::FunctionRequest)),
            "lsp.functionProvider" => Some(Self::Lsp(LspCommand::FunctionProvider)),
            "lsp.configureFunctionRelay" => Some(Self::Lsp(LspCommand::ConfigureFunctionRelay)),
            "lsp.hostResponse" => Some(Self::Lsp(LspCommand::HostResponse)),
            "lsp.hostNotify" => Some(Self::Lsp(LspCommand::HostNotify)),
            "lsp.pluginServerRequest" => Some(Self::Lsp(LspCommand::PluginServerRequest)),
            "lsp.pluginServerNotify" => Some(Self::Lsp(LspCommand::PluginServerNotify)),
            "lsp.cancelOperation" => Some(Self::Lsp(LspCommand::CancelOperation)),
            "lsp.pollEvents" => Some(Self::Lsp(LspCommand::PollEvents)),
            "lsp.setDetailedDiagnostics" => Some(Self::Lsp(LspCommand::SetDetailedDiagnostics)),
            "lsp.waitEvents" => Some(Self::Lsp(LspCommand::WaitEvents)),
            "lsp.destroyServer" => Some(Self::Lsp(LspCommand::DestroyServer)),
            "runConfig.inspect" => Some(Self::RunConfig(RunConfigCommand::Inspect)),
            "runConfig.projectToolchain" => {
                Some(Self::RunConfig(RunConfigCommand::ProjectToolchain))
            }
            "runConfig.generate" => Some(Self::RunConfig(RunConfigCommand::Generate)),
            "runConfig.argumentText" => Some(Self::RunConfig(RunConfigCommand::ArgumentText)),
            "runConfig.resolve" => Some(Self::RunConfig(RunConfigCommand::Resolve)),
            "runConfig.selectContextConfiguration" => Some(Self::RunConfig(
                RunConfigCommand::SelectContextConfiguration,
            )),
            "runConfig.decideTitleAction" => {
                Some(Self::RunConfig(RunConfigCommand::DecideTitleAction))
            }
            "runConfig.updateOptions" => Some(Self::RunConfig(RunConfigCommand::UpdateOptions)),
            "runConfig.updateToolPreferences" => {
                Some(Self::RunConfig(RunConfigCommand::UpdateToolPreferences))
            }
            "runConfig.saveEditorChanges" => {
                Some(Self::RunConfig(RunConfigCommand::SaveEditorChanges))
            }
            "runConfig.createUserConfiguration" => {
                Some(Self::RunConfig(RunConfigCommand::CreateUserConfiguration))
            }
            "runConfig.createLaunchPlan" => {
                Some(Self::RunConfig(RunConfigCommand::CreateLaunchPlan))
            }
            "runConfig.providerLaunchInputs" => {
                Some(Self::RunConfig(RunConfigCommand::ProviderLaunchInputs))
            }
            "git.status" => Some(Self::Git(GitCommand::Status)),
            "git.untrackedDirectory" => Some(Self::Git(GitCommand::UntrackedDirectory)),
            "git.commitState" => Some(Self::Git(GitCommand::CommitState)),
            "git.workspaceCommitPrepare" => Some(Self::Git(GitCommand::WorkspaceCommitPrepare)),
            "git.workspaceCommitStep" => Some(Self::Git(GitCommand::WorkspaceCommitStep)),
            "git.classifyMetadataChanges" => Some(Self::Git(GitCommand::ClassifyMetadataChanges)),
            "git.watchContext" => Some(Self::Git(GitCommand::WatchContext)),
            "git.worktrees" => Some(Self::Git(GitCommand::Worktrees)),
            "git.pullRequestContext" => Some(Self::Git(GitCommand::PullRequestContext)),
            "git.command" => Some(Self::Git(GitCommand::Command)),
            "git.repositoryRoot" => Some(Self::Git(GitCommand::RepositoryRoot)),
            "git.resolveFileContext" => Some(Self::Git(GitCommand::ResolveFileContext)),
            "git.workspaceStatusMerge" => Some(Self::Git(GitCommand::WorkspaceStatusMerge)),
            "git.write" => Some(Self::Git(GitCommand::Write)),
            "git.rollbackPlan" => Some(Self::Git(GitCommand::RollbackPlan)),
            "git.rollbackPrepare" => Some(Self::Git(GitCommand::RollbackPrepare)),
            "git.prepareCommitSelection" => Some(Self::Git(GitCommand::PrepareCommitSelection)),
            "git.reconcileDeletedSelection" => {
                Some(Self::Git(GitCommand::ReconcileDeletedSelection))
            }
            "git.changelists" => Some(Self::Git(GitCommand::Changelists)),
            "git.shelf" => Some(Self::Git(GitCommand::Shelf)),
            "git.commitMessageContext" => Some(Self::Git(GitCommand::CommitMessageContext)),
            "git.consolePresentation" => Some(Self::Git(GitCommand::ConsolePresentation)),
            "git.remoteUrl" => Some(Self::Git(GitCommand::RemoteUrl)),
            "git.fetchPlan" => Some(Self::Git(GitCommand::FetchPlan)),
            "git.executable" => Some(Self::Git(GitCommand::Executable)),
            "git.executionInspect" => Some(Self::Git(GitCommand::ExecutionInspect)),
            "git.executionConfigure" => Some(Self::Git(GitCommand::ExecutionConfigure)),
            "git.authRespond" => Some(Self::Git(GitCommand::AuthRespond)),
            "remote.probe" => Some(Self::Remote(RemoteCommand::Probe)),
            "remote.authRespond" => Some(Self::Remote(RemoteCommand::AuthRespond)),
            "remote.connect" => Some(Self::Remote(RemoteCommand::Connect)),
            "remote.readDir" => Some(Self::Remote(RemoteCommand::ReadDir)),
            "remote.disconnect" => Some(Self::Remote(RemoteCommand::Disconnect)),
            "remote.closeAll" => Some(Self::Remote(RemoteCommand::CloseAll)),
            "remote.readFile" => Some(Self::Remote(RemoteCommand::ReadFile)),
            "remote.writeFile" => Some(Self::Remote(RemoteCommand::WriteFile)),
            "remote.hostRequest" => Some(Self::Remote(RemoteCommand::HostRequest)),
            "remote.runProcess" => Some(Self::Remote(RemoteCommand::RunProcess)),
            "remote.captureProcess" => Some(Self::Remote(RemoteCommand::CaptureProcess)),
            "remote.runTerminal" => Some(Self::Remote(RemoteCommand::RunTerminal)),
            "remote.runWatch" => Some(Self::Remote(RemoteCommand::RunWatch)),
            "remote.uploadFile" => Some(Self::Remote(RemoteCommand::UploadFile)),
            "remote.hostInfo" => Some(Self::Remote(RemoteCommand::HostInfo)),
            "git.historyRewritePreview" => Some(Self::Git(GitCommand::HistoryRewritePreview)),
            "git.remotes" => Some(Self::Git(GitCommand::Remotes)),
            "git.tags" => Some(Self::Git(GitCommand::Tags)),
            "git.parseCheckoutMetadata" => Some(Self::Git(GitCommand::ParseCheckoutMetadata)),
            "git.parseGitdirPointer" => Some(Self::Git(GitCommand::ParseGitdirPointer)),
            "git.historyActionCapabilities" => {
                Some(Self::Git(GitCommand::HistoryActionCapabilities))
            }
            "git.rebasePreview" => Some(Self::Git(GitCommand::RebasePreview)),
            "git.repositorySetup" => Some(Self::Git(GitCommand::RepositorySetup)),
            "git.initialize" => Some(Self::Git(GitCommand::Initialize)),
            "git.configureIdentity" => Some(Self::Git(GitCommand::ConfigureIdentity)),
            "git.rebaseStart" => Some(Self::Git(GitCommand::RebaseStart)),
            "git.rebaseSession" => Some(Self::Git(GitCommand::RebaseSession)),
            "git.rebaseControl" => Some(Self::Git(GitCommand::RebaseControl)),
            "git.patchExport" => Some(Self::Git(GitCommand::PatchExport)),
            "git.patchPreview" => Some(Self::Git(GitCommand::PatchPreview)),
            "git.patchApply" => Some(Self::Git(GitCommand::PatchApply)),
            "git.diff" => Some(Self::Git(GitCommand::Diff)),
            "git.parsePatch" => Some(Self::Git(GitCommand::ParsePatch)),
            "git.apply" => Some(Self::Git(GitCommand::Apply)),
            "git.planDiffBlocks" => Some(Self::Git(GitCommand::PlanDiffBlocks)),
            "git.applyDiffBlock" => Some(Self::Git(GitCommand::ApplyDiffBlock)),
            "git.stageGutterChange" => Some(Self::Git(GitCommand::StageGutterChange)),
            "git.history" => Some(Self::Git(GitCommand::History)),
            "git.historyQuery" => Some(Self::Git(GitCommand::HistoryQuery)),
            "git.references" => Some(Self::Git(GitCommand::References)),
            "git.historyPage" => Some(Self::Git(GitCommand::HistoryPage)),
            "git.historyCursorClose" => Some(Self::Git(GitCommand::HistoryCursorClose)),
            "git.historyLocate" => Some(Self::Git(GitCommand::HistoryLocate)),
            "git.branchNamePreview" => Some(Self::Git(GitCommand::BranchNamePreview)),
            "git.repositoryColorAssignments" => {
                Some(Self::Git(GitCommand::RepositoryColorAssignments))
            }
            "git.updatePlan" => Some(Self::Git(GitCommand::UpdatePlan)),
            "git.historyWindow" => Some(Self::Git(GitCommand::HistoryWindow)),
            "git.graphProjection" => Some(Self::Git(GitCommand::GraphProjection)),
            "git.normalizeChangeTargets" => Some(Self::Git(GitCommand::NormalizeChangeTargets)),
            "git.rebasePlanPreview" => Some(Self::Git(GitCommand::RebasePlanPreview)),
            "git.referenceTree" => Some(Self::Git(GitCommand::ReferenceTree)),
            "git.changedFileGroups" => Some(Self::Git(GitCommand::ChangedFileGroups)),
            "git.worktreeCreationPreview" => Some(Self::Git(GitCommand::WorktreeCreationPreview)),
            "git.pushPreview" => Some(Self::Git(GitCommand::PushPreview)),
            "git.commit" => Some(Self::Git(GitCommand::Commit)),
            "git.commitFiles" => Some(Self::Git(GitCommand::CommitFiles)),
            "git.comparison" => Some(Self::Git(GitCommand::Comparison)),
            "git.stashes" => Some(Self::Git(GitCommand::Stashes)),
            "git.checkoutPreflight" => Some(Self::Git(GitCommand::CheckoutPreflight)),
            "git.pullPreflight" => Some(Self::Git(GitCommand::PullPreflight)),
            "git.integrationPreflight" => Some(Self::Git(GitCommand::IntegrationPreflight)),
            "git.conflictMarkers" => Some(Self::Git(GitCommand::ConflictMarkers)),
            "git.operationState" => Some(Self::Git(GitCommand::OperationState)),
            "git.blame" => Some(Self::Git(GitCommand::Blame)),
            "git.lineHistory" => Some(Self::Git(GitCommand::LineHistory)),
            "github.parseRemote" => Some(Self::GitHubParseRemote),
            "github.requestPlan" => Some(Self::GitHubRequestPlan),
            "github.normalizeResponse" => Some(Self::GitHubNormalizeResponse),
            "github.deviceAuthorizationTransition" => {
                Some(Self::GitHubDeviceAuthorizationTransition)
            }
            "github.pullRequestCheckoutPlan" => Some(Self::GitHubPullRequestCheckoutPlan),
            "git.pullRequestCheckoutPreview" => {
                Some(Self::Git(GitCommand::PullRequestCheckoutPreview))
            }
            "git.pullRequestCheckout" => Some(Self::Git(GitCommand::PullRequestCheckout)),
            "diagnostics.redactText" => Some(Self::Diagnostics(DiagnosticsCommand::RedactText)),
            "diagnostics.prepareLogEvent" => {
                Some(Self::Diagnostics(DiagnosticsCommand::PrepareLogEvent))
            }
            "diagnostics.queryLogs" => Some(Self::Diagnostics(DiagnosticsCommand::QueryLogs)),
            "diagnostics.recordPluginOperation" => {
                Some(Self::Diagnostics(DiagnosticsCommand::RecordPluginOperation))
            }
            "diagnostics.drainCoreEvents" => {
                Some(Self::Diagnostics(DiagnosticsCommand::DrainCoreEvents))
            }
            "diagnostics.configureCoreEvents" => {
                Some(Self::Diagnostics(DiagnosticsCommand::ConfigureCoreEvents))
            }
            "diagnostics.encodeLogBatch" => {
                Some(Self::Diagnostics(DiagnosticsCommand::EncodeLogBatch))
            }
            "diagnostics.parseLogFilter" => {
                Some(Self::Diagnostics(DiagnosticsCommand::ParseLogFilter))
            }
            "diagnostics.composeLogFilter" => {
                Some(Self::Diagnostics(DiagnosticsCommand::ComposeLogFilter))
            }
            "diagnostics.formatLogRecords" => {
                Some(Self::Diagnostics(DiagnosticsCommand::FormatLogRecords))
            }
            "diagnostics.queryLogsV2" => Some(Self::Diagnostics(DiagnosticsCommand::QueryLogsV2)),
            "diagnostics.planLogCollection" => {
                Some(Self::Diagnostics(DiagnosticsCommand::PlanLogCollection))
            }
            "diagnostics.planLogRetention" => {
                Some(Self::Diagnostics(DiagnosticsCommand::PlanLogRetention))
            }
            "diagnostics.resolveLogChannel" => {
                Some(Self::Diagnostics(DiagnosticsCommand::ResolveLogChannel))
            }
            "diagnostics.resolveEngineLogSubject" => Some(Self::Diagnostics(
                DiagnosticsCommand::ResolveEngineLogSubject,
            )),
            "diagnostics.buildManifest" => {
                Some(Self::Diagnostics(DiagnosticsCommand::BuildManifest))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CoreCommand, GitCommand, WorkspaceCommand};
    use std::collections::BTreeSet;
    use std::path::Path;

    /// Returns the command names that `CoreCommand::parse` matches, read from
    /// this file so the test cannot drift from the parser it checks.
    fn parsed_command_names(source: &str) -> Vec<String> {
        let body = source
            .split_once("pub fn parse(value: &str) -> Option<Self> {")
            .and_then(|(_, rest)| rest.split_once("_ => None,"))
            .map(|(body, _)| body)
            .expect("CoreCommand::parse body");
        body.lines()
            .filter_map(|line| line.trim().strip_prefix('"'))
            .filter_map(|line| line.split_once("\" =>"))
            .map(|(name, _)| name.to_owned())
            .collect()
    }

    /// Returns the first-column names of the `| Command | Purpose |` table.
    fn documented_command_names(document: &str) -> Vec<String> {
        document
            .lines()
            .skip_while(|line| !line.starts_with("| Command | Purpose |"))
            .skip(2)
            .take_while(|line| line.starts_with("| `"))
            .map(|line| {
                line["| `".len()..]
                    .split_once('`')
                    .expect("backticked command name")
                    .0
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn api_command_table_lists_exactly_the_parsed_commands() {
        let crate_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = std::fs::read_to_string(crate_root.join("src/protocol/command.rs"))
            .expect("read command.rs");
        let document = std::fs::read_to_string(crate_root.join("../../contracts/rust-core-api.md"))
            .expect("read rust-core-api.md");
        let parsed = parsed_command_names(&source);
        let documented = documented_command_names(&document);

        assert!(
            parsed.len() > 300,
            "parser scan found {} names",
            parsed.len()
        );
        for name in &parsed {
            assert!(
                CoreCommand::parse(name).is_some(),
                "scanned {name} does not parse"
            );
        }
        let unique: BTreeSet<_> = documented.iter().collect();
        assert_eq!(
            unique.len(),
            documented.len(),
            "duplicate rows in the command table"
        );

        let parsed: BTreeSet<_> = parsed.iter().collect();
        let undocumented: Vec<_> = parsed.difference(&unique).collect();
        let unknown: Vec<_> = unique.difference(&parsed).collect();
        assert!(
            undocumented.is_empty() && unknown.is_empty(),
            "rust-core-api.md command table is out of date: missing {undocumented:?}, \
             not parsed {unknown:?}"
        );
    }

    #[test]
    fn parses_semantic_lsp_runtime_commands() {
        for command in [
            "lsp.startServer",
            "lsp.stopServer",
            "lsp.updateWorkspaceConfiguration",
            "lsp.syncDocument",
            "lsp.closeDocument",
            "lsp.request",
            "lsp.buildRequest",
            "lsp.vcsRequest",
            "lsp.toolchainRequest",
            "lsp.keymapImportRequest",
            "lsp.functionRequest",
            "lsp.configureFunctionRelay",
            "lsp.hostResponse",
            "lsp.hostNotify",
            "lsp.pluginServerRequest",
            "lsp.pluginServerNotify",
            "plugins.languageServers.decodeLaunch",
            "plugins.languageServers.stateRetention",
            "plugins.contentProviders.decode",
            "lsp.cancelOperation",
            "lsp.pollEvents",
            "lsp.waitEvents",
            "lsp.destroyServer",
        ] {
            assert!(CoreCommand::parse(command).is_some(), "missing {command}");
        }
    }

    #[test]
    fn parses_network_proxy_policy_command() {
        assert!(CoreCommand::parse("network.proxyPolicy").is_some());
        assert!(CoreCommand::parse("network.unknown").is_none());
    }

    #[test]
    fn parses_document_lifecycle_command() {
        assert!(CoreCommand::parse("document.lifecycle").is_some());
    }

    #[test]
    fn parses_process_lifecycle_command() {
        assert!(matches!(
            CoreCommand::parse("processLifecycle.resolve"),
            Some(CoreCommand::ProcessLifecycleResolve)
        ));
        assert!(matches!(
            CoreCommand::parse("processLifecycle.inventory"),
            Some(CoreCommand::ProcessLifecycleInventory)
        ));
        assert!(matches!(
            CoreCommand::parse("processLifecycle.settle"),
            Some(CoreCommand::ProcessLifecycleSettle)
        ));
    }

    #[test]
    fn parses_debug_runtime_commands() {
        for command in [
            "debug.createSession",
            "debug.launch",
            "debug.steppingFilters",
            "debug.relocateBreakpoints",
            "debug.planVariablePage",
            "debug.reduceVariablePage",
            "debug.planDisassemblyWindow",
            "debug.reduceDisassemblyPage",
            "debug.setBreakpoints",
            "debug.setExceptionBreakpoints",
            "debug.setFunctionBreakpoints",
            "debug.setInstructionBreakpoints",
            "debug.dataBreakpointInfo",
            "debug.setDataBreakpoints",
            "debug.setBreakpointsMuted",
            "debug.planMemoryWindow",
            "debug.locateMemoryWindow",
            "debug.setVariable",
            "debug.cancelOperation",
            "debug.execute",
            "debug.inspect",
            "debug.receive",
            "debug.runInTerminalResponse",
            "debug.startDebuggingResponse",
            "debug.disconnect",
            "debug.destroySession",
            "debug.transport.policy",
            "debug.transport.handle",
            "debug.transport.release",
        ] {
            assert!(CoreCommand::parse(command).is_some(), "missing {command}");
        }
    }

    #[test]
    fn parses_plugin_run_debug_and_output_link_commands() {
        for command in [
            "toolchains.instantiateLaunch",
            "run.decodeStart",
            "debug.decodeBackendConfiguration",
            "outputLinks.match",
            "outputLinks.decodeResolution",
        ] {
            assert!(CoreCommand::parse(command).is_some(), "missing {command}");
        }
    }

    #[test]
    fn parses_git_push_preview_command() {
        assert!(matches!(
            CoreCommand::parse("git.pushPreview"),
            Some(CoreCommand::Git(GitCommand::PushPreview))
        ));
    }

    #[test]
    fn parses_workspace_repositories_command() {
        assert!(matches!(
            CoreCommand::parse("workspace.repositories"),
            Some(CoreCommand::Workspace(WorkspaceCommand::Repositories))
        ));
    }

    #[test]
    fn parses_git_repository_root_command() {
        assert!(matches!(
            CoreCommand::parse("git.repositoryRoot"),
            Some(CoreCommand::Git(GitCommand::RepositoryRoot))
        ));
    }
}
