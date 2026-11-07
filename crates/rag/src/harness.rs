//! Agent harness (Piece 88) — the plugin / hook / session layer
//! under the tool loop, porting the DeepSeek Harness architecture
//! (github.com/deepseek-ai/deepseek-harness, MIT) to Rust patterns.
//! Only the architecture is ported — plugin manifests, a guarded
//! execution pipeline, an append-only session log — no TypeScript
//! code. (The similarly named harness/harness repository is a Go
//! CI/CD platform and has nothing to do with this module.)
//!
//! DeepSeek Harness's core ideas, mapped onto what already exists
//! here (Piece 84's registry and loop are NOT rebuilt — they are
//! re-expressed through this layer):
//!
//! - **Everything is a plugin.** Every tool the loop can call
//!   registers as a [`PluginManifest`]: id, description, JSON
//!   schema, an admission class (which run audiences may be offered
//!   it, and which seam a top-level run must carry), and its scope
//!   behaviour. [`HarnessRegistry::admitted_names`] is the one
//!   place that computes a run's tool set — the Piece 84/86/87
//!   admission rules (seam tools top-level only, sub-agents get a
//!   subset minus spawn and minus every seam tool) live here as
//!   data, not as scattered conditionals.
//! - **Guarded tool pipeline.** Tool execution runs through
//!   registered pre-execute hooks (schema validation, budget
//!   guard, Workspace-scope guard) and post-execute hooks (trace
//!   recording, citation capture), the `tools/pre-execute →
//!   execute → tools/post-execute` waterfall. A pre-hook rejection
//!   is returned to the caller as a tool error and fed back to the
//!   model — it never crashes the loop.
//! - **Session log is the source of truth.** A chat thread keeps
//!   an append-only [`SessionEvent`] log beside its messages;
//!   [`derive_context`] projects the model-visible conversation
//!   from that log (DeepSeek's "model-visible means logged").
//!   Approval outcomes are events, so a decided edit enters the
//!   model's context as a fact of the log — the Piece 86
//!   decided-edit injection, expressed as a projection.
//! - **Prompt assembly.** The loop's system prompt is assembled
//!   from named, ordered sections ([`PromptAssembler`]): base
//!   instructions, the admitted plugins' schemas, the scope note,
//!   an approved plan, standing instructions. A plugin that is not
//!   admitted contributes no schema — its section is absent.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::{tool_def, ToolDef};

// ---------------------------------------------------------------------------
// Plugin manifests + admission
// ---------------------------------------------------------------------------

/// Which seam a top-level run must carry before a plugin is
/// offered to it (the Piece 86/87 seam gates, named).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seam {
    Edit,
    Draft,
    Analyze,
    Workflow,
    Team,
}

impl Seam {
    pub fn key(&self) -> &'static str {
        match self {
            Seam::Edit => "edit",
            Seam::Draft => "draft",
            Seam::Analyze => "analyze",
            Seam::Workflow => "workflow",
            Seam::Team => "team",
        }
    }
}

/// How a plugin treats a run's Workspace scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeBehavior {
    /// Retrieval/execution is confined to the scope when one is set.
    Confined,
    /// Runs whole-vault; a scoped run is refused (fed back).
    RefusesScoped,
    /// The scope is handed to the seam, which enforces it.
    SeamEnforced,
    /// Scope does not apply (e.g. spawning, which inherits it).
    Inherits,
}

/// One registered plugin: a tool, described for admission and
/// prompt assembly. The execution engine stays where it always
/// was (the loop's match arms and the server seams); the manifest
/// is the harness's view of it.
#[derive(Debug, Clone)]
pub struct PluginManifest {
    pub id: &'static str,
    pub display_name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    /// Offered to a plain top-level run with no seams (the Piece 84
    /// default set).
    pub in_default_set: bool,
    /// The seam a top-level run must carry to be offered this
    /// plugin; `None` = no seam needed.
    pub seam: Option<Seam>,
    /// A sub-agent may be offered this plugin. Always false for
    /// seam plugins and for `spawn_subagent` itself.
    pub subagent_allowed: bool,
    pub scope: ScopeBehavior,
}

/// The audience a run's tool set is computed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAudience {
    /// A top-level run: the default set plus the seam plugins
    /// whose seams the run actually carries.
    TopLevel,
    /// A sub-agent: the sub-agent subset only — no spawn, no seam
    /// plugins, regardless of what the parent carries.
    SubAgent,
}

/// The harness registry: the canonical plugin list, in prompt
/// order (the same order the registry has always rendered in).
pub struct HarnessRegistry {
    plugins: Vec<PluginManifest>,
}

fn manifest(def: ToolDef, display_name: &'static str, in_default_set: bool, seam: Option<Seam>, subagent_allowed: bool, scope: ScopeBehavior) -> PluginManifest {
    PluginManifest {
        id: def.name,
        display_name,
        description: def.description,
        input_schema: def.input_schema,
        in_default_set,
        seam,
        subagent_allowed,
        scope,
    }
}

/// Build the registry from the canonical tool definitions, so a
/// plugin's schema/description can never drift from the
/// definition the validator and the prompt have always used.
pub fn build_registry() -> HarnessRegistry {
    let mut plugins = Vec::new();
    for def in crate::tools::tool_defs() {
        let m = match def.name {
            "vault_search" => manifest(def, "Vault search", true, None, true, ScopeBehavior::Confined),
            "read_document" => manifest(def, "Read document", true, None, true, ScopeBehavior::Confined),
            "list_workspace" => manifest(def, "List workspace", true, None, true, ScopeBehavior::Confined),
            "web_research" => manifest(def, "Web research", true, None, true, ScopeBehavior::Confined),
            "deep_research" => manifest(def, "Deep research", true, None, true, ScopeBehavior::RefusesScoped),
            "graph_query" => manifest(def, "Graph query", true, None, true, ScopeBehavior::Confined),
            "spawn_subagent" => manifest(def, "Spawn sub-agent", true, None, false, ScopeBehavior::Inherits),
            "edit_document" => manifest(def, "Edit document", false, Some(Seam::Edit), false, ScopeBehavior::Confined),
            "draft_document" => manifest(def, "Draft document", false, Some(Seam::Draft), false, ScopeBehavior::SeamEnforced),
            "analyze_documents" => manifest(def, "Analyse documents", false, Some(Seam::Analyze), false, ScopeBehavior::SeamEnforced),
            "run_workflow" => manifest(def, "Run workflow", false, Some(Seam::Workflow), false, ScopeBehavior::SeamEnforced),
            "team_research" => manifest(def, "Research team", false, Some(Seam::Team), false, ScopeBehavior::SeamEnforced),
            _ => manifest(def, "Tool", true, None, true, ScopeBehavior::Confined),
        };
        plugins.push(m);
    }
    HarnessRegistry { plugins }
}

/// The process-wide registry (built once; manifests are static).
pub fn registry() -> &'static HarnessRegistry {
    static REG: std::sync::OnceLock<HarnessRegistry> = std::sync::OnceLock::new();
    REG.get_or_init(build_registry)
}

impl HarnessRegistry {
    pub fn plugins(&self) -> &[PluginManifest] {
        &self.plugins
    }

    pub fn get(&self, id: &str) -> Option<&PluginManifest> {
        self.plugins.iter().find(|p| p.id == id)
    }

    /// The tool names admitted for one run. `seams` lists the seam
    /// keys the run carries (e.g. `"edit"` when an edit proposer
    /// is attached and allowed). This is the ONLY computation of a
    /// run's tool set: the Piece 84 default set, the Piece 86/87
    /// seam gates, and the sub-agent subset all come from here.
    pub fn admitted_names(&self, audience: RunAudience, seams: &[&str]) -> Vec<String> {
        self.plugins
            .iter()
            .filter(|p| match audience {
                RunAudience::TopLevel => {
                    p.in_default_set
                        || p.seam.map(|s| seams.contains(&s.key())).unwrap_or(false)
                }
                RunAudience::SubAgent => p.subagent_allowed,
            })
            .map(|p| p.id.to_string())
            .collect()
    }

    /// May a sub-agent be offered this plugin at all? (The spawn
    /// arm's subset filter, as a registry question.)
    pub fn subagent_may_use(&self, id: &str) -> bool {
        self.get(id).map(|p| p.subagent_allowed).unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// Hook pipeline — tools/pre-execute → execute → tools/post-execute
// ---------------------------------------------------------------------------

/// Everything a pre-execute hook may judge a call by.
#[derive(Debug, Clone)]
pub struct ToolCallContext {
    pub tool: String,
    pub args: Value,
    /// The run's Workspace document ids; `None` = whole vault.
    pub scope_docs: Option<Vec<String>>,
    /// Calls already completed in this run.
    pub steps_done: usize,
    /// Estimated tokens already spent in this run.
    pub tokens_so_far: u64,
    pub max_steps: Option<usize>,
    pub max_tokens: Option<u64>,
}

/// A pre-execute hook: `Err(message)` rejects the call; the
/// message is fed back to the model as the tool result.
pub trait PreExecuteHook: Send + Sync {
    fn name(&self) -> &'static str;
    fn check(&self, ctx: &ToolCallContext) -> Result<(), String>;
}

/// A post-execute hook: observes every completed call (ok or
/// error) after execution. Observation never fails the run.
pub trait PostExecuteHook: Send + Sync {
    fn name(&self) -> &'static str;
    fn observe(&self, record: &ToolCallRecord);
}

/// The completed-call record post-hooks observe — the same facts
/// the run trace persists.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool: String,
    pub args_digest: String,
    /// ok | error
    pub status: String,
    pub error: Option<String>,
    pub result_chars: usize,
    pub tokens_estimate: u64,
    pub duration_ms: u64,
    pub citations: usize,
}

/// Schema validation as a hook (the Piece 84 validator, wired
/// into the pipeline — the same function, the same messages).
pub struct SchemaValidationHook;

impl PreExecuteHook for SchemaValidationHook {
    fn name(&self) -> &'static str {
        "schema_validation"
    }
    fn check(&self, ctx: &ToolCallContext) -> Result<(), String> {
        match tool_def(&ctx.tool) {
            Some(def) => crate::tools::validate_tool_args(&def, &ctx.args),
            None => Ok(()), // unknown tools are the executor's error, not a hook's
        }
    }
}

/// Budget guard: a call may not START once the run's step or
/// token budget is already spent. A run always makes its first
/// call (a cap is not a veto — the loop's own semantics).
pub struct BudgetGuardHook;

impl PreExecuteHook for BudgetGuardHook {
    fn name(&self) -> &'static str {
        "budget_guard"
    }
    fn check(&self, ctx: &ToolCallContext) -> Result<(), String> {
        if ctx.steps_done == 0 {
            return Ok(());
        }
        if let Some(cap) = ctx.max_steps {
            if ctx.steps_done >= cap {
                return Err(format!(
                    "the run's step budget ({cap}) is already spent — no further tool calls; answer from the findings so far"
                ));
            }
        }
        if let Some(cap) = ctx.max_tokens {
            if ctx.tokens_so_far >= cap {
                return Err(format!(
                    "the run's token budget ({cap}) is already spent — no further tool calls; answer from the findings so far"
                ));
            }
        }
        Ok(())
    }
}

/// Workspace-scope guard: the hard confinement rules that used to
/// live inside individual execution arms, as one pipeline gate —
/// a scoped run's `read_document` / `edit_document` may only name
/// documents inside the scope, and a whole-vault plugin
/// (`deep_research`) refuses a scoped run. Messages match the
/// arms' original wording exactly; the arms keep their own checks
/// as a second line of defence.
pub struct WorkspaceScopeHook;

impl PreExecuteHook for WorkspaceScopeHook {
    fn name(&self) -> &'static str {
        "workspace_scope"
    }
    fn check(&self, ctx: &ToolCallContext) -> Result<(), String> {
        let Some(scope) = &ctx.scope_docs else {
            return Ok(());
        };
        let in_scope = |doc_id: &str| scope.iter().any(|d| d == doc_id);
        match ctx.tool.as_str() {
            "read_document" => {
                let doc_id = ctx.args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
                if !in_scope(doc_id) {
                    return Err(format!(
                        "document {doc_id} is outside this run's Workspace — a tool run can only read documents inside its scope; use list_workspace to see what is available"
                    ));
                }
            }
            "edit_document" => {
                let doc_id = ctx.args.get("doc_id").and_then(|v| v.as_str()).unwrap_or("");
                if !in_scope(doc_id) {
                    return Err(format!(
                        "document {doc_id} is outside this run's Workspace — edits can only be proposed for documents inside the run's scope; use list_workspace to see what is available"
                    ));
                }
            }
            "deep_research" => {
                return Err(
                    "deep_research runs over the whole vault and cannot be confined to this run's Workspace — use vault_search or graph_query for a Workspace-scoped question, or run the research from an unscoped chat"
                        .to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }
}

/// Post-hook: records every completed call into a shared trace
/// sink (the run store persists the loop's own trace; this is the
/// pipeline-level record hooks and tests observe).
pub struct TraceRecordHook {
    pub records: Arc<Mutex<Vec<ToolCallRecord>>>,
}

impl TraceRecordHook {
    pub fn new() -> Self {
        Self {
            records: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl Default for TraceRecordHook {
    fn default() -> Self {
        Self::new()
    }
}

impl PostExecuteHook for TraceRecordHook {
    fn name(&self) -> &'static str {
        "trace_record"
    }
    fn observe(&self, record: &ToolCallRecord) {
        if let Ok(mut v) = self.records.lock() {
            v.push(record.clone());
        }
    }
}

/// Post-hook: counts the citations completed calls produced.
pub struct CitationCaptureHook {
    pub total: Arc<Mutex<usize>>,
}

impl CitationCaptureHook {
    pub fn new() -> Self {
        Self {
            total: Arc::new(Mutex::new(0)),
        }
    }
}

impl Default for CitationCaptureHook {
    fn default() -> Self {
        Self::new()
    }
}

impl PostExecuteHook for CitationCaptureHook {
    fn name(&self) -> &'static str {
        "citation_capture"
    }
    fn observe(&self, record: &ToolCallRecord) {
        if let Ok(mut t) = self.total.lock() {
            *t += record.citations;
        }
    }
}

/// The pipeline a run executes through: ordered pre-hooks (the
/// first rejection wins) and post-hooks (all observe).
pub struct HookPipeline {
    pub pre: Vec<Arc<dyn PreExecuteHook>>,
    pub post: Vec<Arc<dyn PostExecuteHook>>,
}

impl HookPipeline {
    pub fn new(pre: Vec<Arc<dyn PreExecuteHook>>, post: Vec<Arc<dyn PostExecuteHook>>) -> Self {
        Self { pre, post }
    }

    /// Run the pre-execute waterfall; the first rejection is the
    /// call's error.
    pub fn pre_execute(&self, ctx: &ToolCallContext) -> Result<(), String> {
        for hook in &self.pre {
            hook.check(ctx)?;
        }
        Ok(())
    }

    /// Run every post-execute hook over a completed call.
    pub fn post_execute(&self, record: &ToolCallRecord) {
        for hook in &self.post {
            hook.observe(record);
        }
    }
}

/// The standard pipeline wired into the tool loop: schema →
/// budget → scope before execution; trace + citations after.
pub fn default_pipeline() -> HookPipeline {
    HookPipeline::new(
        vec![
            Arc::new(SchemaValidationHook),
            Arc::new(BudgetGuardHook),
            Arc::new(WorkspaceScopeHook),
        ],
        vec![
            Arc::new(TraceRecordHook::new()),
            Arc::new(CitationCaptureHook::new()),
        ],
    )
}

// ---------------------------------------------------------------------------
// Session event log + context projection
// ---------------------------------------------------------------------------

/// One durable fact in a chat thread's append-only session log.
/// Model-visible context is DERIVED from these events
/// ([`derive_context`]) — an outcome that is not logged (an edit
/// decision, say) cannot enter the model's context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    TurnStart {
        goal: String,
    },
    UserMessage {
        text: String,
    },
    StepStart {
        n: usize,
    },
    AssistantMessage {
        text: String,
    },
    ToolCall {
        n: usize,
        tool: String,
        args_digest: String,
    },
    ToolResult {
        n: usize,
        tool: String,
        /// ok | error
        status: String,
        summary: String,
    },
    ApprovalRequired {
        edit_id: String,
        doc_id: String,
        doc_title: String,
        instruction: String,
    },
    ApprovalDecided {
        edit_id: String,
        doc_title: String,
        /// approved | rejected
        decision: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<u64>,
    },
    TurnEnd {
        /// finished | budget | budget_tokens | turn_cap | failed
        stopped: String,
    },
    Error {
        message: String,
    },
}

/// Project the model-visible conversation from a session log:
/// user and assistant turns in order, proposals as pending
/// facts, and — critically — decided edits as update lines in
/// exactly the wording the Piece 86 thread-context injection
/// used, so a stale "pending" turn can never outrank the log.
/// Tool calls/results are trace, not conversation, and are not
/// projected; neither are turn/step markers.
pub fn derive_context(events: &[SessionEvent]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for ev in events {
        match ev {
            SessionEvent::UserMessage { text } => {
                let text: String = text.chars().take(600).collect();
                lines.push(format!("User: {text}"));
            }
            SessionEvent::AssistantMessage { text } => {
                let text: String = text.chars().take(600).collect();
                lines.push(format!("Assistant: {text}"));
            }
            SessionEvent::ApprovalRequired {
                doc_title,
                instruction,
                ..
            } => {
                lines.push(format!(
                    "[An edit to “{doc_title}” was proposed (“{instruction}”) and is pending the user's approval.]"
                ));
            }
            SessionEvent::ApprovalDecided {
                doc_title,
                decision,
                version,
                ..
            } => {
                if decision == "approved" {
                    lines.push(format!(
                        "[Update: the proposed edit to “{doc_title}” was APPROVED and written back as version {} — the document's current text is the edited text.]",
                        version.unwrap_or(2)
                    ));
                } else {
                    lines.push(format!(
                        "[Update: the proposed edit to “{doc_title}” was REJECTED — nothing was written back.]"
                    ));
                }
            }
            _ => {}
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Prompt assembly — named, ordered sections
// ---------------------------------------------------------------------------

/// One named section of the assembled system prompt. Sections
/// carry their own leading separator (the loop's historic
/// format); assembly concatenates the non-empty sections in
/// `order` order.
#[derive(Debug, Clone)]
pub struct PromptSection {
    pub name: &'static str,
    pub order: usize,
    pub body: String,
}

/// Assembles a system prompt from registered sections. A section
/// whose body is empty contributes nothing — this is how a
/// non-admitted plugin's schema stays absent: it is simply never
/// rendered into the tools section.
#[derive(Debug, Default)]
pub struct PromptAssembler {
    sections: Vec<PromptSection>,
}

impl PromptAssembler {
    pub fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    pub fn add(&mut self, section: PromptSection) -> &mut Self {
        self.sections.push(section);
        self
    }

    /// Section names in assembly order — for tests and debugging.
    pub fn section_names(&self) -> Vec<&'static str> {
        let mut ordered: Vec<&PromptSection> = self.sections.iter().collect();
        ordered.sort_by_key(|s| s.order);
        ordered
            .into_iter()
            .filter(|s| !s.body.is_empty())
            .map(|s| s.name)
            .collect()
    }

    pub fn assemble(&self) -> String {
        let mut ordered: Vec<&PromptSection> = self.sections.iter().collect();
        ordered.sort_by_key(|s| s.order);
        ordered
            .into_iter()
            .filter(|s| !s.body.is_empty())
            .map(|s| s.body.clone())
            .collect::<Vec<_>>()
            .join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::HashSet;

    fn name_set(v: Vec<String>) -> HashSet<String> {
        v.into_iter().collect()
    }

    #[test]
    fn admission_matches_the_historic_tool_sets() {
        let reg = build_registry();
        // Top-level with no seams = the Piece 84 default set,
        // exactly (the historic `all_tool_names()`).
        assert_eq!(
            name_set(reg.admitted_names(RunAudience::TopLevel, &[])),
            name_set(crate::tools::all_tool_names())
        );
        // Top-level with every seam = the historic chat set plus
        // team_research (Piece 88).
        let mut want = name_set(crate::tools::chat_tool_names());
        want.insert("team_research".to_string());
        assert_eq!(
            name_set(reg.admitted_names(
                RunAudience::TopLevel,
                &["edit", "draft", "analyze", "workflow", "team"]
            )),
            want
        );
        // A single seam admits exactly its own plugin.
        let only_edit = reg.admitted_names(RunAudience::TopLevel, &["edit"]);
        assert!(only_edit.contains(&"edit_document".to_string()));
        assert!(!only_edit.contains(&"draft_document".to_string()));
        assert!(!only_edit.contains(&"team_research".to_string()));
        // Sub-agents: no spawn, no seam plugins, but the retrieval
        // legs (including deep_research) are available.
        let sub = name_set(reg.admitted_names(RunAudience::SubAgent, &["edit", "draft", "analyze", "workflow", "team"]));
        assert!(!sub.contains("spawn_subagent"));
        assert!(!sub.contains("edit_document"));
        assert!(!sub.contains("draft_document"));
        assert!(!sub.contains("analyze_documents"));
        assert!(!sub.contains("run_workflow"));
        assert!(!sub.contains("team_research"));
        assert!(sub.contains("vault_search"));
        assert!(sub.contains("deep_research"));
        assert!(reg.subagent_may_use("vault_search"));
        assert!(!reg.subagent_may_use("spawn_subagent"));
        assert!(!reg.subagent_may_use("no_such_tool"));
    }

    #[test]
    fn plugin_manifests_carry_schemas_and_scope_behaviour() {
        let reg = build_registry();
        let deep = reg.get("deep_research").unwrap();
        assert_eq!(deep.scope, ScopeBehavior::RefusesScoped);
        assert!(deep.input_schema.get("properties").is_some());
        let team = reg.get("team_research").expect("team_research plugin registered");
        assert_eq!(team.seam, Some(Seam::Team));
        assert!(!team.in_default_set);
        assert!(!team.subagent_allowed);
        assert!(reg.get("edit_document").is_some());
        assert_eq!(reg.plugins().len(), crate::tools::tool_defs().len());
    }

    struct RejectHook;

    impl PreExecuteHook for RejectHook {
        fn name(&self) -> &'static str {
            "reject_all"
        }
        fn check(&self, _ctx: &ToolCallContext) -> Result<(), String> {
            Err("rejected by the test hook".to_string())
        }
    }

    fn call_ctx(tool: &str) -> ToolCallContext {
        ToolCallContext {
            tool: tool.to_string(),
            args: json!({ "query": "deposit" }),
            scope_docs: None,
            steps_done: 1,
            tokens_so_far: 10,
            max_steps: None,
            max_tokens: None,
        }
    }

    #[test]
    fn a_hook_can_reject_a_call() {
        let pipeline = HookPipeline::new(vec![Arc::new(RejectHook)], vec![]);
        let err = pipeline.pre_execute(&call_ctx("vault_search")).unwrap_err();
        assert_eq!(err, "rejected by the test hook");
        // The standard pipeline passes a well-formed call…
        let standard = default_pipeline();
        assert!(standard.pre_execute(&call_ctx("vault_search")).is_ok());
        // …rejects a schema violation with the validator's message…
        let mut bad = call_ctx("vault_search");
        bad.args = json!({});
        let err = standard.pre_execute(&bad).unwrap_err();
        assert!(err.contains("missing required argument"), "{err}");
        // …and enforces the scope guard for deep_research.
        let mut scoped = call_ctx("deep_research");
        scoped.scope_docs = Some(vec!["doc-a".to_string()]);
        scoped.args = json!({ "question": "q" });
        let err = standard.pre_execute(&scoped).unwrap_err();
        assert!(err.contains("cannot be confined"), "{err}");
    }

    #[test]
    fn a_hook_observes_a_completed_call() {
        let trace = Arc::new(TraceRecordHook::new());
        let records = trace.records.clone();
        let cites = Arc::new(CitationCaptureHook::new());
        let total = cites.total.clone();
        let pipeline = HookPipeline::new(vec![], vec![trace, cites]);
        pipeline.post_execute(&ToolCallRecord {
            tool: "vault_search".to_string(),
            args_digest: "deposit".to_string(),
            status: "ok".to_string(),
            error: None,
            result_chars: 120,
            tokens_estimate: 42,
            duration_ms: 7,
            citations: 3,
        });
        pipeline.post_execute(&ToolCallRecord {
            tool: "read_document".to_string(),
            args_digest: "doc-a".to_string(),
            status: "error".to_string(),
            error: Some("nope".to_string()),
            result_chars: 10,
            tokens_estimate: 0,
            duration_ms: 1,
            citations: 0,
        });
        let seen = records.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].tool, "vault_search");
        assert_eq!(seen[1].status, "error");
        assert_eq!(*total.lock().unwrap(), 3);
    }

    #[test]
    fn budget_guard_allows_the_first_call_and_stops_a_spent_run() {
        let hook = BudgetGuardHook;
        let mut ctx = call_ctx("vault_search");
        ctx.steps_done = 0;
        ctx.max_tokens = Some(1);
        ctx.tokens_so_far = 500;
        assert!(hook.check(&ctx).is_ok(), "a cap is not a veto on the first call");
        ctx.steps_done = 1;
        let err = hook.check(&ctx).unwrap_err();
        assert!(err.contains("token budget"), "{err}");
    }

    #[test]
    fn session_projection_includes_decided_edits_as_updates() {
        let events = vec![
            SessionEvent::UserMessage {
                text: "What is the notice period in the tenancy note?".to_string(),
            },
            SessionEvent::ToolCall {
                n: 1,
                tool: "read_document".to_string(),
                args_digest: "doc-1".to_string(),
            },
            SessionEvent::AssistantMessage {
                text: "It is three months.".to_string(),
            },
            SessionEvent::UserMessage {
                text: "Change it to two months.".to_string(),
            },
            SessionEvent::ApprovalRequired {
                edit_id: "edit-1".to_string(),
                doc_id: "doc-1".to_string(),
                doc_title: "Tenancy note".to_string(),
                instruction: "make it two months".to_string(),
            },
            SessionEvent::AssistantMessage {
                text: "Proposed and awaiting approval.".to_string(),
            },
            SessionEvent::ApprovalDecided {
                edit_id: "edit-1".to_string(),
                doc_title: "Tenancy note".to_string(),
                decision: "approved".to_string(),
                version: Some(3),
            },
        ];
        let ctx = derive_context(&events);
        assert!(ctx.contains("User: What is the notice period"), "{ctx}");
        assert!(ctx.contains("Assistant: It is three months."), "{ctx}");
        assert!(ctx.contains("pending the user's approval"), "{ctx}");
        assert!(
            ctx.contains("[Update: the proposed edit to “Tenancy note” was APPROVED and written back as version 3 — the document's current text is the edited text.]"),
            "{ctx}"
        );
        // Tool calls are trace, not conversation: not projected.
        assert!(!ctx.contains("read_document"), "{ctx}");

        let rejected = vec![SessionEvent::ApprovalDecided {
            edit_id: "e".to_string(),
            doc_title: "Note".to_string(),
            decision: "rejected".to_string(),
            version: None,
        }];
        assert!(derive_context(&rejected).contains("REJECTED — nothing was written back"));
        // The log round-trips through JSON (the thread store's format).
        let bytes = serde_json::to_string(&events).unwrap();
        let back: Vec<SessionEvent> = serde_json::from_str(&bytes).unwrap();
        assert_eq!(back, events);
    }

    #[test]
    fn prompt_assembly_orders_sections_and_skips_empty_ones() {
        let mut asm = PromptAssembler::new();
        asm.add(PromptSection {
            name: "standing",
            order: 40,
            body: "\n\nSTANDING".to_string(),
        });
        asm.add(PromptSection {
            name: "base",
            order: 0,
            body: "BASE".to_string(),
        });
        asm.add(PromptSection {
            name: "plan",
            order: 30,
            body: String::new(),
        });
        asm.add(PromptSection {
            name: "tools",
            order: 10,
            body: "\n\nTOOLS".to_string(),
        });
        assert_eq!(asm.section_names(), vec!["base", "tools", "standing"]);
        assert_eq!(asm.assemble(), "BASE\n\nTOOLS\n\nSTANDING");
    }
}
