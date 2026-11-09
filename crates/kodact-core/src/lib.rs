//! Builds a ready-to-run session from launch options and settings.

pub mod advisor;
pub mod commands;
pub mod debug;
pub mod doctor;
pub mod driver;
pub mod endpoint;
pub mod glyphs;
pub mod goal;
pub mod import;
pub mod prompt;
pub mod schedule;
pub mod schedule_tools;
pub mod subtask;
pub mod view;
pub mod web;

pub use driver::{Driver, Flow, Report};
pub use prompt::PromptSpec;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kodact_api::{
    GeminiConfig, GeminiProvider, MessagesConfig, MessagesProvider, OpenAiConfig, OpenAiProvider, Provider,
};
use kodact_config::{load_memory, load_settings, LoadedSettings, MemoryKind, SettingSource, SettingsOptions};
use kodact_engine::{Engine, EngineConfig, EngineParts, EnvInfo, EventSink, PermissionPrompter, Pricing};
use kodact_hooks::{HookBase, HookRunner, HooksConfig};
use kodact_permissions::{PermissionMode, Rule, RuleSet};
use kodact_session::{FileHistory, LoadedSession, SessionStore, Transcript};
use kodact_tools::{ToolContext, ToolRegistry};
use kodact_types::sdk::{InitInfo, McpServerStatus};
use serde_json::{json, Value};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("{0}")]
    Config(String),
    /// No usable credentials or endpoint.
    #[error("{0}")]
    Auth(String),
    #[error(transparent)]
    Session(#[from] kodact_session::SessionError),
    #[error(transparent)]
    Engine(#[from] kodact_engine::EngineError),
    #[error(transparent)]
    Api(#[from] kodact_api::ApiError),
}

/// How to resume (`--continue`, `--resume [id]`).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Resume {
    #[default]
    New,
    Latest,
    Id(String),
    /// A conversation already in memory (`/branch`, `/cd`, `/reload-*`).
    Loaded(Box<LoadedSession>),
}

/// Everything the command line can say about a session.
#[derive(Clone, Default)]
pub struct LaunchOptions {
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub fallback_models: Vec<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<String>,
    pub dangerously_skip_permissions: bool,
    pub allow_dangerously_skip_permissions: bool,
    pub allowed_tools: Vec<String>,
    pub disallowed_tools: Vec<String>,
    /// `--tools`: `None` = all built-ins, `Some([])` = none.
    pub tools: Option<Vec<String>>,
    pub add_dirs: Vec<PathBuf>,
    pub system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub exclude_dynamic_system_prompt_sections: bool,
    pub settings: Option<String>,
    pub setting_sources: Option<Vec<SettingSource>>,
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    /// `--max-time`, counted from the start of the session.
    pub max_time: Option<std::time::Duration>,
    /// `--autonomous` (or the `autonomous` setting): an unattended run.
    pub autonomous: bool,
    pub json_schema: Option<Value>,
    pub resume: Resume,
    pub fork_session: bool,
    pub session_id: Option<String>,
    pub no_session_persistence: bool,
    /// `--bare` / `--safe-mode`: no hooks, no memory files.
    pub bare: bool,
    pub betas: Vec<String>,
    /// `--autocompact <auto|tokens>`: the window compaction thresholds use.
    pub autocompact: Option<String>,
    /// `--agents <json>`: extra agent definitions.
    pub agents_json: Option<String>,
    /// `--agent <name>`: run the main session as this agent.
    pub agent: Option<String>,
    /// `--sandbox <off|read-only|workspace-write>` for shell commands.
    pub sandbox: Option<String>,
    /// `--plugin-dir`: plugin directories (with the `pluginDirs` setting).
    pub plugin_dirs: Vec<PathBuf>,
    /// `--worktree [name]`: run in a new git worktree (`Some("")` = a generated name).
    pub worktree: Option<String>,
    /// `--mcp-config`: server config files or JSON strings.
    pub mcp_configs: Vec<String>,
    /// `--strict-mcp-config`: only the servers from `mcp_configs`.
    pub strict_mcp_config: bool,
    /// Connected MCP servers (from [`connect_mcp`]); their tools join the session.
    pub mcp: Option<Arc<kodact_mcp::McpManager>>,
    /// Background shells to keep (`/reload-*` rebuilds the session around them).
    pub shells: Option<Arc<kodact_tools::shells::ShellManager>>,
    /// Replace the provider (tests, embedding).
    pub provider: Option<Arc<dyn Provider>>,
    /// Where sessions live (default `~/.kodact/projects`).
    pub store_root: Option<PathBuf>,
}

/// WebFetch (always) and WebSearch (when the provider can search), backed by a small model.
fn web_tools(provider: &Arc<dyn Provider>, settings: &LoadedSettings, model: &str) -> Vec<Arc<dyn kodact_tools::Tool>> {
    let backend = Arc::new(web::ProviderWeb {
        provider: provider.clone(),
        // An OpenAI-compatible endpoint may not serve the default small model: use the session's.
        model: match settings.str("/smallFastModel") {
            Some(m) => kodact_api::resolve_model(m),
            None if provider.name() == "openai" => model.to_string(),
            None => kodact_api::models::SMALL_FAST_MODEL.to_string(),
        },
        search: provider.name() != "openai",
        search_tool: settings.str("/webSearch/toolType").unwrap_or("web_search_20250305").to_string(),
    });
    let mut out: Vec<Arc<dyn kodact_tools::Tool>> =
        vec![Arc::new(kodact_tools::builtin::WebFetch::new(Some(backend.clone())))];
    if kodact_tools::builtin::WebBackend::can_search(backend.as_ref()) {
        out.push(Arc::new(kodact_tools::builtin::WebSearch { backend }));
    }
    out
}

struct Worktree {
    name: String,
    path: PathBuf,
    main_repo: PathBuf,
}

/// `--worktree [name]` (contract C10): `<repo>/.kodact/worktrees/<name>` on branch `kodact/<name>`,
/// created from HEAD, or reused when it already exists.
fn enter_worktree(cwd: &Path, name: &str, session_id: Option<&str>) -> Result<Worktree, CoreError> {
    let root = kodact_git::repo_root(cwd)
        .ok_or_else(|| CoreError::Config("--worktree needs a git repository; run kodact inside one".into()))?;
    let name = match name.trim() {
        "" => {
            let id = session_id.map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            format!("session-{}", &id[..8.min(id.len())])
        }
        n => n.to_string(),
    };
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') || name.starts_with('.') {
        return Err(CoreError::Config(format!("--worktree name {name:?}: use letters, digits, '-', '_' and '.'")));
    }
    let path = root.join(".kodact").join("worktrees").join(&name);
    if !path.exists() {
        kodact_git::add_worktree(&root, &path, &format!("kodact/{name}"))
            .map_err(|e| CoreError::Config(format!("--worktree: git could not create it: {e}")))?;
        let _ = kodact_git::exclude(&root, ".kodact/worktrees/");
    }
    Ok(Worktree { name, path: kodact_platform::path::canonicalize(&path).unwrap_or(path), main_repo: root })
}

/// Load the session's settings the way [`build_session`] does.
fn session_settings(opts: &LaunchOptions, cwd: &Path) -> LoadedSettings {
    let mut sopts = SettingsOptions::new(cwd);
    if let Some(s) = &opts.setting_sources {
        sopts.sources = s.clone();
    }
    sopts.flag = opts.settings.clone();
    load_settings(&sopts)
}

/// Settings that shape the skills, as the reference CLI reads them:
/// `skillOverrides` (`{"name": "on" | "name-only" | "user-invocable-only" |
/// "off"}`, plugin skills excepted) and `disableSkillShellExecution` (their
/// ``!`command` `` lines become a placeholder; KodAct's bundled skills excepted).
pub(crate) fn apply_skill_settings(skills: &mut Vec<kodact_agents::SkillDef>, settings: &LoadedSettings) {
    let from_plugin = |s: &kodact_agents::SkillDef| s.plugin_dir.is_some();
    if let Some(overrides) = settings.get("/skillOverrides").and_then(serde_json::Value::as_object) {
        skills.retain(|s| from_plugin(s) || overrides.get(&s.name).and_then(serde_json::Value::as_str) != Some("off"));
        for s in skills.iter_mut().filter(|s| !from_plugin(s)) {
            match overrides.get(&s.name).and_then(serde_json::Value::as_str) {
                Some("name-only") => s.name_only = true,
                Some("user-invocable-only") => s.model_invocable = false,
                _ => {}
            }
        }
    }
    if settings.bool("/disableSkillShellExecution") == Some(true) {
        for s in skills.iter_mut().filter(|s| s.source != "bundled") {
            s.no_shell = true;
        }
    }
}

/// Which MCP servers this session would start (contract C16). `--bare` keeps only `--mcp-config`.
pub fn resolve_mcp(opts: &LaunchOptions) -> kodact_mcp::Resolved {
    let cwd = kodact_platform::path::canonicalize(&opts.cwd).unwrap_or(opts.cwd.clone());
    let settings = session_settings(opts, &cwd);
    // Plugin servers count as explicitly chosen, like --mcp-config.
    let mut configs: Vec<String> =
        session_plugins(opts, &settings, &cwd, &mut vec![]).iter().filter_map(|p| p.mcp_config()).collect();
    configs.extend(opts.mcp_configs.iter().cloned());
    kodact_mcp::resolve(&settings, &cwd, &configs, opts.strict_mcp_config || opts.bare)
}

/// `--plugin-dir` plus the `pluginDirs` setting (relative to the project). None in `--bare`.
fn session_plugins(
    opts: &LaunchOptions,
    settings: &LoadedSettings,
    cwd: &Path,
    warnings: &mut Vec<String>,
) -> Vec<kodact_agents::Plugin> {
    if opts.bare {
        return vec![];
    }
    let mut dirs = opts.plugin_dirs.clone();
    dirs.extend(settings.strings("/pluginDirs").into_iter().map(|d| kodact_permissions::normalize(Path::new(&d), cwd)));
    kodact_agents::load_plugins(&dirs, warnings)
}

/// Connect the session's MCP servers. Failures are reported in the manager, never fatal.
pub async fn connect_mcp(opts: &LaunchOptions) -> (Arc<kodact_mcp::McpManager>, Vec<String>) {
    let resolved = resolve_mcp(opts);
    let cwd = kodact_platform::path::canonicalize(&opts.cwd).unwrap_or(opts.cwd.clone());
    let manager = kodact_mcp::McpManager::connect(&resolved, &kodact_mcp::ConnectOptions::new(&cwd)).await;
    let mut warnings = resolved.warnings;
    warnings.extend(manager.warnings());
    (Arc::new(manager), warnings)
}

/// A built session, ready for `engine.submit`.
pub struct Session {
    pub engine: Engine,
    pub init: InitInfo,
    /// What the system prompt is built from, for rebuilding it later.
    pub prompt: PromptSpec,
    /// Scheduled prompts (`None` when KODACT_DISABLE_CRON is set).
    pub scheduler: Option<schedule_tools::SharedScheduler>,
    /// The advisor model, while one is set.
    pub advisor: advisor::AdvisorCell,
    /// Builds sub-agents: the Task tool's and `/subtask`'s (C20).
    pub agent_rt: Arc<kodact_agents::AgentRuntime>,
    /// Custom slash commands, skills, output styles and plugins (M5).
    pub commands: Vec<kodact_agents::CommandDef>,
    pub skills: Vec<kodact_agents::SkillDef>,
    pub styles: Vec<kodact_agents::OutputStyle>,
    pub plugins: Vec<kodact_agents::Plugin>,
    pub agents: Vec<kodact_agents::AgentDef>,
    pub settings: LoadedSettings,
    pub warnings: Vec<String>,
    pub session_id: String,
    pub resumed: Option<LoadedSession>,
}

/// The verification loop's settings: `verification.{enabled, commands,
/// maxReminders}`, with commands detected from the project's manifests when
/// none are set. `KODACT_VERIFY=0` turns it off (for A/B runs).
pub(crate) fn verify_config(
    settings: &LoadedSettings,
    cwd: &Path,
    autonomous: bool,
) -> Option<kodact_engine::VerifyConfig> {
    let off =
        env_nonempty("KODACT_VERIFY").map(|v| matches!(v.as_str(), "0" | "false" | "off" | "no")).unwrap_or(false);
    if off || settings.bool("/verification/enabled") == Some(false) {
        return None;
    }
    let configured = settings.strings("/verification/commands");
    Some(kodact_engine::VerifyConfig {
        commands: if configured.is_empty() { kodact_engine::detect_checks(cwd) } else { configured },
        max_reminders: settings
            .get("/verification/maxReminders")
            .and_then(Value::as_u64)
            .map(|n| n as u32)
            // Unattended runs get a second reminder: nobody else will catch an unchecked change.
            .unwrap_or(if autonomous { 2 } else { 1 }),
    })
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

/// The session's model: `--model`, `KODACT_MODEL`, the `model` setting, else the default.
fn session_model(opts: &LaunchOptions, settings: &LoadedSettings) -> String {
    kodact_api::resolve_model(
        &opts
            .model
            .clone()
            .or_else(|| env_nonempty("KODACT_MODEL"))
            .or_else(|| settings.str("/model").map(str::to_string))
            .unwrap_or_else(|| "default".into()),
    )
}

/// Concurrent model requests per session unless `maxConcurrentRequests` or
/// `KODACT_MAX_CONCURRENT_REQUESTS` says otherwise; 429s lower it further.
pub const DEFAULT_CONCURRENT_REQUESTS: usize = 4;

/// Task agents running at once unless `maxParallelAgents` or `KODACT_MAX_PARALLEL_AGENTS` says otherwise.
pub const DEFAULT_PARALLEL_AGENTS: usize = 4;

/// The provider for this session: an OpenAI-compatible endpoint when one is
/// configured, the Messages API otherwise (see [`endpoint`] for the rules).
pub fn make_provider(settings: &LoadedSettings, betas: &[String]) -> Result<Arc<dyn Provider>, CoreError> {
    make_provider_with_shell(settings, betas, &session_shell(settings), &mut vec![])
}

fn make_provider_with_shell(
    settings: &LoadedSettings,
    betas: &[String],
    shell: &kodact_platform::shell::ShellChoice,
    warnings: &mut Vec<String>,
) -> Result<Arc<dyn Provider>, CoreError> {
    let r = endpoint::resolve(settings, &|k| std::env::var(k).ok());
    if let Some(p) = r.problems.first() {
        // The session won't start, so its warnings would never show: they may be the cause.
        let notes: String = r.warnings.iter().map(|w| format!("\n  note: {w}")).collect();
        return Err(CoreError::Auth(format!("{p}{notes}")));
    }
    warnings.extend(r.warnings.iter().cloned());
    let mut key = r.key.clone();
    let mut helper_name = None;
    if key.is_none() && r.token.is_none() {
        if let Some((cmd, setting, _)) = &r.helper {
            key = Some(run_key_helper(cmd, setting, shell).map_err(CoreError::Auth)?);
            helper_name = Some(*setting);
        }
    }
    match r.backend() {
        kodact_api::auth::Backend::OpenAi => {
            let mut cfg = OpenAiConfig {
                base_url: r.url.clone().unwrap_or_default(),
                api_key: key,
                url_from: r.url_from,
                key_helper: helper_name,
                ..Default::default()
            };
            if let Some(n) = env_nonempty("KODACT_MAX_RETRIES").and_then(|v| v.trim().parse().ok()) {
                cfg.max_retries = n;
            }
            Ok(Arc::new(OpenAiProvider::new(cfg)?))
        }
        kodact_api::auth::Backend::Gemini => {
            use kodact_api::gemini::AuthHeader;
            let header = env_nonempty("KODACT_GEMINI_AUTH_HEADER")
                .or_else(|| settings.get("/gemini/authHeader").and_then(|v| v.as_str()).map(str::to_string));
            let auth_header = match header {
                Some(h) => AuthHeader::parse(&h).ok_or_else(|| {
                    CoreError::Auth(format!(
                        "gemini.authHeader is \"{h}\": it takes x-goog-api-key, bearer, both or auto"
                    ))
                })?,
                None => AuthHeader::Auto,
            };
            let mut cfg = GeminiConfig {
                base_url: r.url.clone().unwrap_or_else(|| endpoint::GEMINI_DEFAULT_URL.to_string()),
                api_key: key,
                auth_header,
                url_from: r.url_from,
                key_helper: helper_name,
                ..Default::default()
            };
            if let Some(n) = env_nonempty("KODACT_MAX_RETRIES").and_then(|v| v.trim().parse().ok()) {
                cfg.max_retries = n;
            }
            Ok(Arc::new(GeminiProvider::new(cfg)?))
        }
        kodact_api::auth::Backend::Messages => {
            let mut cfg = MessagesConfig::from_env();
            if let Some(u) = &r.url {
                cfg.base_url = u.clone();
            }
            cfg.url_from = r.url_from;
            cfg.api_key = key;
            cfg.auth_token = r.token.clone();
            cfg.key_helper = helper_name;
            cfg.betas = betas.to_vec();
            Ok(Arc::new(MessagesProvider::new(cfg)?))
        }
    }
}

/// A key helper (`apiKeyHelper`, `openai.apiKeyHelper`): a command that prints
/// the key. Every way it can fail is named, with the command redacted.
fn run_key_helper(cmd: &str, setting: &str, shell: &kodact_platform::shell::ShellChoice) -> Result<String, String> {
    let c = kodact_config::redact_text(cmd);
    let fail = |why: String| format!("{setting} failed: {why}; run the command yourself to see what it prints");
    let shell = shell.as_ref().map_err(|m| fail(format!("could not run `{c}`: {m}")))?;
    let (mut command, _script) =
        shell.command(&shell.script(cmd, None)).map_err(|e| fail(format!("could not run `{c}`: {e}")))?;
    kodact_platform::process::no_window(&mut command);
    let out =
        command.stdin(std::process::Stdio::null()).output().map_err(|e| fail(format!("could not run `{c}`: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().map(str::trim).find(|l| !l.is_empty()).map(kodact_config::redact_text);
        let status = match out.status.code() {
            Some(n) => format!("exited with status {n}"),
            None => "was stopped by a signal".into(),
        };
        return Err(fail(match first {
            Some(l) => format!("`{c}` {status}: {l}"),
            None => format!("`{c}` {status}"),
        }));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    match lines.as_slice() {
        [] => Err(fail(format!("`{c}` printed nothing; it must print the key"))),
        [key] if key.chars().all(|ch| ch.is_ascii_graphic()) => Ok(key.to_string()),
        [_] => Err(fail(format!(
            "what `{c}` printed contains a space or a control character, so it can't be sent as a key"
        ))),
        more => Err(fail(format!("`{c}` printed {} lines; it must print only the key", more.len()))),
    }
}

/// The shell for a session: `KODACT_SHELL` from the environment, else from the
/// settings `env` block, else the platform's search (kodact_platform::shell).
pub fn session_shell(settings: &LoadedSettings) -> kodact_platform::shell::ShellChoice {
    let from_settings = settings.env().into_iter().find(|(k, _)| k == "KODACT_SHELL").map(|(_, v)| v);
    kodact_platform::shell::resolve(&kodact_platform::shell::ShellConfig {
        env_override: env_nonempty("KODACT_SHELL"),
        settings_override: from_settings.filter(|v| !v.trim().is_empty()),
        posix_only: false,
    })
}

/// Tools named in `--disallowedTools` without a specifier are removed outright.
fn removed_tools(disallowed: &[String]) -> Vec<String> {
    disallowed.iter().flat_map(|s| kodact_permissions::split_rule_list(s)).filter(|r| !r.contains('(')).collect()
}

/// `modelLimits`: `{"<model id>": {"contextWindow": 131072, "maxOutputTokens": 8192}}`.
/// Returns warnings for entries that can't be used.
fn apply_model_limits(s: &LoadedSettings) -> Vec<String> {
    let mut warnings = vec![];
    let Some(m) = s.get("/modelLimits").and_then(Value::as_object) else { return warnings };
    for (model, v) in m {
        let n = |k: &str| v.get(k).and_then(Value::as_u64).filter(|n| *n > 0);
        let l = kodact_api::models::Limits {
            context_window: n("contextWindow"),
            max_output: n("maxOutputTokens").map(|n| n.min(u64::from(u32::MAX)) as u32),
        };
        if l == kodact_api::models::Limits::default() {
            warnings.push(format!(
                "modelLimits.{model}: set \"contextWindow\" and/or \"maxOutputTokens\" to positive token counts"
            ));
            continue;
        }
        kodact_api::models::configure_limits(&kodact_api::resolve_model(model), l);
    }
    warnings
}

/// How [`unknown_model_notice`] starts, so `/doctor` can tell it apart.
pub const UNKNOWN_MODEL_PREFIX: &str = "KodAct doesn't know the model ";

/// How long a session waits, as it starts, for its endpoint to report an unknown
/// model's context window.
pub const LISTED_WINDOW_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

impl Session {
    /// A model with no window of its own: wait a moment ([`LISTED_WINDOW_WAIT`]) for the
    /// endpoint's model list, which may report it, so the first request and the startup
    /// notice use it instead of `KODACT_CONTEXT_WINDOW` or the 200,000-token guess. The
    /// variable, one value for every model, then never stands in for a window the endpoint
    /// gives, so a large model keeps its own. The notice goes once the window is known.
    pub async fn learn_window(&mut self) {
        let model = self.engine.handle().model();
        if kodact_api::models::window_unknown(&model) {
            let provider = self.engine.provider();
            let _ = tokio::time::timeout(LISTED_WINDOW_WAIT, provider.list_models()).await;
        }
        // Also when the listing came in before this call, from the request made at start.
        if kodact_api::models::limits_source(&model) == kodact_api::models::LimitsSource::Listed {
            self.warnings.retain(|w| !w.starts_with(UNKNOWN_MODEL_PREFIX));
        }
    }
}

/// Shown once when the session's model is in no table and nothing set its limits.
pub fn unknown_model_notice(model: &str, priced: bool) -> String {
    let info = kodact_api::models::model_info_or_default(model);
    let price = if priced { "" } else { ", and has no price for it" };
    format!(
        "{UNKNOWN_MODEL_PREFIX}{model}: it assumes a {}-token context window and {} output tokens{price}. \
         Set its limits with \"modelLimits\" in settings{}, or KODACT_CONTEXT_WINDOW for every unknown model.",
        group(info.context_window),
        group(u64::from(info.max_output)),
        if priced { "" } else { " (and its price with \"modelPricing\")" }
    )
}

/// 200000 -> "200,000".
fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn pricing_from_settings(s: &LoadedSettings) -> HashMap<String, Pricing> {
    let mut out = HashMap::new();
    if let Some(m) = s.get("/modelPricing").and_then(Value::as_object) {
        for (model, p) in m {
            let f = |k: &str| p.get(k).and_then(Value::as_f64);
            if let (Some(input), Some(output)) = (f("input"), f("output")) {
                out.insert(
                    model.clone(),
                    Pricing {
                        input,
                        output,
                        cache_read: f("cacheRead").unwrap_or(input * 0.1),
                        cache_write: f("cacheWrite").unwrap_or(input * 1.25),
                    },
                );
            }
        }
    }
    out
}

fn memory_context(cwd: &Path) -> Option<String> {
    let files = load_memory(cwd);
    if files.is_empty() {
        return None;
    }
    let mut s = String::from(
        "As you answer, follow the instructions below. They override default behaviour; follow them exactly.\n",
    );
    for f in files {
        let what = match f.kind {
            MemoryKind::User => "the user's private global instructions for all projects",
            MemoryKind::Project => "project instructions, checked into the codebase",
            MemoryKind::Local => "the user's private project instructions, not checked in",
        };
        s.push_str(&format!("\nContents of {} ({what}):\n\n{}\n", f.path.display(), f.content.trim_end()));
    }
    Some(s)
}

/// Build a session. `sink` receives engine events; `prompter` answers permission prompts.
pub fn build_session(
    opts: LaunchOptions,
    sink: Arc<dyn EventSink>,
    prompter: Arc<dyn PermissionPrompter>,
) -> Result<Session, CoreError> {
    let mut cwd = kodact_platform::path::canonicalize(&opts.cwd).unwrap_or(opts.cwd.clone());
    let mut warnings = vec![];
    let worktree = match &opts.worktree {
        Some(name) => {
            let w = enter_worktree(&cwd, name, opts.session_id.as_deref())?;
            cwd = w.path.clone();
            Some(w)
        }
        None => None,
    };

    // Settings.
    let settings = session_settings(&opts, &cwd);
    warnings.extend(settings.errors.iter().map(|e| format!("settings: {e}")));

    // The shell every command runs in (Bash, `!`, hooks, apiKeyHelper, status line).
    let shell = session_shell(&settings);
    if let Err(m) = &shell {
        warnings.push(format!("shell: {m}"));
    }

    let provider = match opts.provider.clone() {
        Some(p) => p,
        None => make_provider_with_shell(&settings, &opts.betas, &shell, &mut warnings)?,
    };
    // One limit on concurrent requests for everything in the session (main agent, sub-agents,
    // compaction, goal checks): requests wait for a slot, and 429s lower the limit.
    let provider: Arc<dyn Provider> = {
        let max = env_nonempty("KODACT_MAX_CONCURRENT_REQUESTS")
            .and_then(|v| v.trim().parse::<usize>().ok())
            .or_else(|| settings.get("/maxConcurrentRequests").and_then(Value::as_u64).map(|n| n as usize))
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_CONCURRENT_REQUESTS);
        let sink = sink.clone();
        let notify: kodact_api::limit::Notifier = Arc::new(move |text| {
            sink.emit(kodact_engine::EngineEvent::Notice { level: kodact_engine::NoticeLevel::Warning, text })
        });
        Arc::new(kodact_api::limit::LimitedProvider::new(provider, kodact_api::limit::Limiter::new(max), Some(notify)))
    };

    // Plugins, commands, skills and output styles (M5).
    let plugins = session_plugins(&opts, &settings, &cwd, &mut warnings);
    let commands = if opts.bare { vec![] } else { kodact_agents::load_commands(&cwd, &plugins, &mut warnings) };
    let mut skills = if opts.bare { vec![] } else { kodact_agents::load_skills(&cwd, &plugins, &mut warnings) };
    apply_skill_settings(&mut skills, &settings);
    let styles = kodact_agents::load_styles(&cwd, &plugins, &mut warnings);
    let style_name = settings.str("/outputStyle").unwrap_or("default").to_string();
    let style = match kodact_agents::styles::find(&styles, &style_name) {
        Some(s) => s.clone(),
        None => {
            warnings.push(format!("outputStyle {style_name:?} not found; using default"));
            styles[0].clone()
        }
    };
    let flag_agents = match &opts.agents_json {
        Some(raw) => {
            let raw = if raw.trim_start().starts_with('{') {
                raw.clone()
            } else {
                std::fs::read_to_string(raw)
                    .map_err(|e| CoreError::Config(format!("--agents: cannot read {raw}: {e}")))?
            };
            kodact_agents::parse_agents_json(&raw).map_err(CoreError::Config)?
        }
        None => vec![],
    };
    let agents = if opts.bare {
        kodact_agents::builtin_agents()
    } else {
        kodact_agents::load_agents(&cwd, &plugins, &flag_agents, &mut warnings)
    };
    let main_agent = match &opts.agent {
        Some(name) => Some(
            agents
                .iter()
                .find(|a| &a.name == name)
                .cloned()
                .ok_or_else(|| CoreError::Config(format!("--agent: no agent named {name:?}")))?,
        ),
        None => None,
    };

    // Session identity and history.
    let store =
        SessionStore::new(opts.store_root.clone().unwrap_or_else(|| kodact_session::kodact_home().join("projects")));
    let mut resumed: Option<LoadedSession> = match &opts.resume {
        Resume::New => None,
        Resume::Latest => match store.latest(&cwd) {
            Some(s) => Some(LoadedSession::load(&s.path, None)?),
            None => {
                warnings.push("No conversation found to continue; starting a new one.".into());
                None
            }
        },
        Resume::Id(id) => {
            let path = store.find(id)?;
            let loaded = LoadedSession::load(&path, None)?;
            // Its file and tools belong to its own directory; writing it from here
            // would split it into two files with the same id.
            if !opts.fork_session && path.parent() != Some(store.project_dir(&cwd).as_path()) {
                let home = if loaded.cwd.as_os_str().is_empty() {
                    "another directory".to_string()
                } else {
                    loaded.cwd.display().to_string()
                };
                return Err(CoreError::Config(format!(
                    "session {id} belongs to {home}: resume it from there, or add --fork-session to continue a copy here"
                )));
            }
            Some(loaded)
        }
        Resume::Loaded(l) => Some((**l).clone()),
    };
    let session_id = match (&resumed, opts.fork_session, &opts.session_id) {
        (Some(_), true, Some(id)) | (None, _, Some(id)) => id.clone(),
        (Some(r), false, _) => r.session_id.clone(),
        _ => uuid::Uuid::new_v4().to_string(),
    };
    kodact_session::validate_session_id(&session_id)?;
    let git_branch = kodact_git::current_branch(&cwd);
    let transcript = Arc::new(Transcript::create(&store, &cwd, &session_id, git_branch, !opts.no_session_persistence)?);
    if let Some(r) = &resumed {
        if r.session_id != session_id {
            transcript.write_fork_of(r);
        }
    }
    if let Some(w) = &worktree {
        transcript.append_meta(json!({"worktree": {"name": w.name, "path": w.path, "mainRepo": w.main_repo}}));
        if !opts.no_session_persistence {
            store.register_project(&cwd, Some(&w.main_repo))?;
        }
    }

    // Working directories: flags, settings, and whatever the resumed session had.
    let mut add_dirs: Vec<PathBuf> = opts.add_dirs.iter().map(|d| kodact_permissions::normalize(d, &cwd)).collect();
    for d in settings.strings("/permissions/additionalDirectories") {
        add_dirs.push(kodact_permissions::normalize(Path::new(&d), &cwd));
    }
    if let Some(r) = &resumed {
        add_dirs.extend(r.additional_dirs.iter().cloned());
    }
    // Their skills load too, in the order they were added.
    if !opts.bare {
        let mut seen = std::collections::HashSet::new();
        let ordered: Vec<PathBuf> = add_dirs.iter().filter(|d| seen.insert((*d).clone())).cloned().collect();
        kodact_agents::skills::add_dir_skills(&mut skills, &ordered, &mut warnings);
        apply_skill_settings(&mut skills, &settings);
    }
    add_dirs.sort();
    add_dirs.dedup();
    if !add_dirs.is_empty() {
        transcript.append_meta(json!({"additionalDirectories": add_dirs}));
    }

    // Permissions.
    let mode_name = if opts.dangerously_skip_permissions {
        Some("bypassPermissions".to_string())
    } else {
        opts.permission_mode.clone().or_else(|| settings.str("/permissions/defaultMode").map(str::to_string))
    };
    let mut mode = match mode_name.as_deref() {
        Some(m) => {
            PermissionMode::parse(m).ok_or_else(|| CoreError::Config(format!("unknown permission mode {m:?}")))?
        }
        None => PermissionMode::Default,
    };
    let bypass_disabled =
        settings.managed().and_then(|m| m.pointer("/permissions/disableBypassPermissionsMode")).and_then(Value::as_str)
            == Some("disable");
    if mode == PermissionMode::BypassPermissions && bypass_disabled {
        warnings.push("bypassPermissions is disabled by managed settings; using default mode.".into());
        mode = PermissionMode::Default;
    }
    let mut allow = settings.strings("/permissions/allow");
    allow.extend(opts.allowed_tools.iter().cloned());
    let ask = settings.strings("/permissions/ask");
    let mut deny = settings.strings("/permissions/deny");
    deny.extend(
        opts.disallowed_tools.iter().flat_map(|s| kodact_permissions::split_rule_list(s)).filter(|r| r.contains('(')),
    );
    let (rules, rule_errors): (RuleSet, _) = RuleSet::from_strings(&allow, &ask, &deny);
    warnings.extend(rule_errors.iter().map(|e| e.to_string()));
    let mut permissions = kodact_permissions::Engine::new(mode, rules, &cwd, &add_dirs);
    // Output too long for a tool result is saved here, and may be read back without a prompt.
    let spill_dir = kodact_config::cache_dir().join("tool-output").join(&session_id);
    permissions.add_read_dir(&spill_dir);

    // Tools.
    let mut tools = ToolRegistry::new();
    kodact_tools::builtin::register_core(&mut tools);
    kodact_tools::builtin::set_shell(&mut tools, &shell);
    let web_tools = web_tools(&provider, &settings, &session_model(&opts, &settings));
    for t in &web_tools {
        tools.register(t.clone());
    }
    // The Skill tool reaches the session (sub-agents, hooks, files) once it exists.
    let skill_links: Arc<std::sync::OnceLock<kodact_agents::skills::SkillLinks>> = Default::default();
    if skills.iter().any(|s| s.model_invocable) {
        tools.register(Arc::new(kodact_agents::SkillTool::with_links(skills.clone(), skill_links.clone())));
    }
    // Scheduled prompts (C19), unless KODACT_DISABLE_CRON is set.
    let scheduler = (!schedule::disabled()).then(|| {
        let mut s = schedule::Scheduler::default();
        if let Some(rec) = resumed.as_ref().and_then(|r| r.schedule.as_ref()) {
            s.restore(rec);
        }
        Arc::new(std::sync::Mutex::new(s))
    });
    if let Some(s) = &scheduler {
        for t in schedule_tools::tools(s, &transcript) {
            tools.register(t);
        }
    }
    // The Advisor tool, shown only while an advisor model is set (`/advisor`).
    let advisor: advisor::AdvisorCell =
        Arc::new(std::sync::RwLock::new(settings.str("/advisorModel").map(kodact_api::resolve_model)));
    tools.register(Arc::new(advisor::Advisor { provider: provider.clone(), model: advisor.clone() }));
    let agent_store =
        (!opts.no_session_persistence).then(|| SessionStore::new(store.root.join("agents").join(&session_id)));
    if let Some(list) = &opts.tools {
        let names: Vec<String> = list.iter().flat_map(|s| kodact_permissions::split_rule_list(s)).collect();
        if !(names.len() == 1 && names[0] == "default") {
            tools.retain(|n| names.iter().any(|x| x == n));
        }
    }
    // MCP tools join after `--tools` (which names built-ins) and before `--disallowedTools`.
    if let Some(m) = &opts.mcp {
        for t in m.tools() {
            tools.register(t);
        }
    }
    let autonomous = opts.autonomous || settings.bool("/autonomous") == Some(true);
    let mut removed = removed_tools(&opts.disallowed_tools);
    if autonomous {
        // Nobody answers questions or approves plans in an unattended run.
        removed.extend(["AskUserQuestion", "EnterPlanMode", "ExitPlanMode"].map(String::from));
    }
    tools.retain(|n| !removed.iter().any(|r| Rule::parse(r).map(|rule| rule.covers_tool(n)).unwrap_or(false)));

    let mut tool_ctx = ToolContext::new(&cwd);
    let session_files = tool_ctx.files.clone();
    tool_ctx.shell = shell.clone();
    if let Some(shells) = &opts.shells {
        tool_ctx.shells = shells.clone();
    }
    tool_ctx.session_id = session_id.clone();
    tool_ctx.spill_dir = Some(spill_dir);
    tool_ctx.transcript_path = transcript.path().map(Path::to_path_buf);
    {
        let mut wd = tool_ctx.working_dirs.write().unwrap();
        wd.extend(add_dirs.iter().cloned());
    }
    // OS sandbox for shell commands: flag > KODACT_SANDBOX > settings.
    let sandbox_mode = opts
        .sandbox
        .clone()
        .or_else(|| env_nonempty("KODACT_SANDBOX"))
        .or_else(|| settings.str("/sandbox/mode").map(str::to_string));
    if let Some(m) = sandbox_mode {
        let parsed = kodact_tools::sandbox::SandboxMode::parse(&m).ok_or_else(|| {
            CoreError::Config(format!("unknown sandbox mode {m:?}: expected off, read-only or workspace-write"))
        })?;
        if let Some(mode) = parsed {
            if kodact_tools::sandbox::backend().is_none() {
                warnings.push(format!(
                    "sandbox {} requested but unavailable ({}); shell commands will ask for approval instead",
                    mode.as_str(),
                    kodact_tools::sandbox::unavailable_reason()
                ));
            } else {
                tool_ctx.set_sandbox(Some(kodact_tools::sandbox::SandboxPolicy {
                    mode,
                    network: settings.bool("/sandbox/network").unwrap_or(false),
                    writable_roots: vec![],
                    extra_writable: settings.strings("/sandbox/writableRoots").into_iter().map(PathBuf::from).collect(),
                }));
            }
        }
    }
    let mut env: HashMap<String, String> = settings.env().into_iter().collect();
    env.insert("KODACT_SESSION_ID".into(), session_id.clone());
    // The session's effort, for skills that ask for `${KODACT_EFFORT}`.
    if let Some(e) = opts.effort.clone().or_else(|| settings.str("/effortLevel").map(str::to_string)) {
        env.insert("KODACT_EFFORT".into(), e);
    }
    tool_ctx.env = Arc::new(env);

    // Hooks.
    let mut hooks_value = settings.get("/hooks").cloned().unwrap_or_else(|| json!({}));
    for p in &plugins {
        if let Some(h) = p.hooks() {
            kodact_config::deep_merge(&mut hooks_value, &h);
        }
    }
    let (hooks_cfg, hook_errors) = HooksConfig::from_settings(Some(&hooks_value));
    warnings.extend(hook_errors.into_iter().map(|e| format!("hooks: {e}")));
    let mut hooks = HookRunner::new(
        hooks_cfg,
        HookBase {
            session_id: session_id.clone(),
            transcript_path: transcript.path().map(Path::to_path_buf),
            cwd: cwd.clone(),
            project_dir: cwd.clone(),
        },
    );
    hooks.disabled = opts.bare || settings.bool("/disableAllHooks") == Some(true);
    hooks.shell = Some(shell.clone());
    if let Some(agent) = &main_agent {
        if let Some(allowed) = &agent.tools {
            tools.retain(|n| allowed.iter().any(|a| a == n));
        }
    }

    // Model and system prompt.
    let model = session_model(&opts, &settings);
    // Limits for models the built-in table doesn't know (C9 compaction needs the real window).
    let pricing = pricing_from_settings(&settings);
    let autocompact_window = match opts.autocompact.as_deref() {
        Some(v) => parse_autocompact(v)?,
        None => settings.get("/autoCompactWindow").and_then(Value::as_u64),
    };
    warnings.extend(apply_model_limits(&settings));
    // No window of the model's own: KODACT_CONTEXT_WINDOW or the guess stands in, and the
    // endpoint may still report the real one, which wins over both.
    if kodact_api::models::window_unknown(&model) && opts.provider.is_none() {
        if kodact_api::models::limits_source(&model) == kodact_api::models::LimitsSource::Guessed {
            warnings.push(unknown_model_notice(&model, pricing.contains_key(&model)));
        }
        // An OpenAI-compatible endpoint may report the window in its model list.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let p = provider.clone();
            rt.spawn(async move {
                let _ = p.list_models().await;
            });
        }
    }
    let fallback_models: Vec<String> = opts
        .fallback_models
        .iter()
        .flat_map(|s| s.split(','))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(kodact_api::resolve_model)
        .collect();
    let verify = verify_config(&settings, &cwd, autonomous);
    let mut env_info = EnvInfo::collect(&cwd, &add_dirs, &model);
    env_info.shell = kodact_platform::shell::env_line(&shell);
    if let Some(v) = &verify {
        env_info.checks = v.commands.clone();
    }
    let prompt = PromptSpec {
        replace: opts.system_prompt.clone(),
        append: opts.append_system_prompt.clone(),
        agent: main_agent.as_ref().map(|a| a.prompt.clone()),
        mcp_instructions: opts.mcp.as_ref().and_then(|m| m.instructions()),
        prompts_dir: env_nonempty("KODACT_PROMPTS_DIR").map(PathBuf::from),
        exclude_dynamic: opts.exclude_dynamic_system_prompt_sections,
        output_style: Some(style.prompt.clone()).filter(|p| !p.is_empty()),
        autonomous,
        env: env_info,
    };
    let sp_opts = prompt.options();
    for (tool, text) in kodact_engine::prompts::tool_description_overrides(&sp_opts) {
        tools.set_description(&tool, text);
    }
    let (system, deferred) = prompt.build();
    let mut initial = vec![];
    if let Some(d) = deferred {
        initial.push(d);
    }
    if !opts.bare {
        if let Some(m) = memory_context(&cwd) {
            initial.push(m);
        }
    }

    // Sub-agents: the Task tool and `/subtask` share this session's provider, rules, hooks and budget.
    let memory = if opts.bare { None } else { memory_context(&cwd) };
    let agent_rt = Arc::new(kodact_agents::AgentRuntime {
        provider: provider.clone(),
        agents: agents.clone(),
        project_dir: cwd.clone(),
        working_dirs: tool_ctx.working_dirs.clone(),
        env: tool_ctx.env.clone(),
        sandbox: tool_ctx.sandbox.clone(),
        shell: shell.clone(),
        agent_slots: Arc::new(tokio::sync::Semaphore::new(
            env_nonempty("KODACT_MAX_PARALLEL_AGENTS")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .or_else(|| settings.get("/maxParallelAgents").and_then(Value::as_u64).map(|n| n as usize))
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_PARALLEL_AGENTS),
        )),
        extra_tools: web_tools
            .iter()
            .cloned()
            .chain(opts.mcp.as_ref().map(|m| m.tools()).unwrap_or_default())
            .collect(),
        store: agent_store,
        session_id: session_id.clone(),
        hooks: hooks.clone(),
        sink: sink.clone(),
        base: EngineConfig {
            max_output_tokens: env_nonempty("KODACT_MAX_OUTPUT_TOKENS").and_then(|v| v.parse().ok()).unwrap_or(32_000),
            pricing: pricing.clone(),
            autocompact_window,
            auto_compact: settings.bool("/autoCompactEnabled").unwrap_or(true),
            ..Default::default()
        },
        memory_context: memory.clone(),
        parent: std::sync::OnceLock::new(),
    });
    if tools.get("Task").is_none()
        && opts.tools.as_ref().map(|t| t.iter().any(|x| x.contains("Task") || x == "default")).unwrap_or(true)
        && main_agent.as_ref().and_then(|a| a.tools.as_ref()).map(|t| t.iter().any(|x| x == "Task")).unwrap_or(true)
        && !removed.iter().any(|r| r == "Task")
    {
        tools.register(Arc::new(kodact_agents::TaskTool { rt: agent_rt.clone() }));
    }
    let tool_names = tools.names();
    let cfg = EngineConfig {
        model: model.clone(),
        fallback_models,
        max_output_tokens: env_nonempty("KODACT_MAX_OUTPUT_TOKENS").and_then(|v| v.parse().ok()).unwrap_or(32_000),
        effort: opts.effort.clone().or_else(|| settings.str("/effortLevel").map(str::to_string)),
        max_thinking_tokens: env_nonempty("KODACT_MAX_THINKING_TOKENS")
            .and_then(|v| v.parse().ok())
            .or_else(|| (settings.bool("/alwaysThinkingEnabled") == Some(false)).then_some(0)),
        max_turns: opts.max_turns,
        max_budget_usd: opts.max_budget_usd,
        time_limit: opts.max_time.map(kodact_engine::TimeLimit::starting_now),
        autonomous,
        api_retry_wait: EngineConfig::default().api_retry_wait,
        json_schema: opts.json_schema.clone(),
        pricing,
        initial_context: (!initial.is_empty()).then(|| initial.join("\n\n")),
        metadata_user_id: None,
        autocompact_window,
        auto_compact: settings.bool("/autoCompactEnabled").unwrap_or(true),
        is_subagent: false,
        verify,
        escalate_output: env_nonempty("KODACT_MAX_OUTPUT_TOKENS").is_none(),
        fast: settings.bool("/fastMode").unwrap_or(false),
    };
    let parts = EngineParts {
        provider: provider.clone(),
        tools,
        tool_ctx,
        permissions,
        hooks,
        prompter,
        sink,
        transcript: transcript.clone(),
        history: Arc::new({
            let dir_of = |id: &str| match &opts.store_root {
                Some(root) => root.with_file_name("file-history").join(id),
                None => kodact_session::kodact_home().join("file-history").join(id),
            };
            // A fork keeps the checkpoints of the prompts it copied.
            if let Some(r) = resumed.as_ref().filter(|r| r.session_id != session_id) {
                kodact_session::copy_history(&dir_of(&r.session_id), &dir_of(&session_id));
            }
            FileHistory::new(dir_of(&session_id))
        }),
        system,
    };
    let mut engine = Engine::new(cfg, parts)?;
    let _ = agent_rt.parent.set(kodact_agents::ParentLink {
        handle: engine.handle(),
        prompter: engine.prompter(),
        history: engine.history().clone(),
    });
    let _ = skill_links.set(kodact_agents::skills::SkillLinks { rt: agent_rt.clone(), files: session_files });
    if let Some(r) = resumed.as_mut() {
        engine.restore(r);
        engine.set_start_source("resume");
    }

    // Persist "always allow" answers to the settings file they name.
    let cwd_for_updates = cwd.clone();
    engine.set_permission_update_handler(Box::new(move |upd| persist_permission_update(&cwd_for_updates, upd)));

    let init = InitInfo {
        cwd: cwd.display().to_string(),
        session_id: session_id.clone(),
        tools: tool_names,
        mcp_servers: opts
            .mcp
            .as_ref()
            .map(|m| m.status().into_iter().map(|(name, status)| McpServerStatus { name, status }).collect())
            .unwrap_or_default(),
        model,
        permission_mode: mode.as_str().into(),
        slash_commands: commands::Catalog {
            commands: commands.clone(),
            skills: skills.clone(),
            mcp: opts.mcp.clone(),
            ..Default::default()
        }
        .names(commands::Surface::Stream),
        api_key_source: provider.origin().map(|o| o.api_key_source()).unwrap_or("none").into(),
        kodact_version: VERSION.into(),
        output_style: style.name.clone(),
        agents: agents.iter().map(|a| a.name.clone()).collect(),
        skills: skills.iter().map(|s| s.name.clone()).collect(),
        plugins: plugins.iter().map(|p| json!({"name": p.name, "path": p.dir})).collect(),
        uuid: uuid::Uuid::new_v4().to_string(),
    };
    Ok(Session {
        engine,
        init,
        prompt,
        scheduler,
        advisor,
        agent_rt,
        commands,
        skills,
        styles,
        plugins,
        agents,
        settings,
        warnings,
        session_id,
        resumed,
    })
}

/// `auto` or a token count between 100k and 1M (`200000`, `200k`, `1m`).
pub fn parse_autocompact(v: &str) -> Result<Option<u64>, CoreError> {
    let t = v.trim().to_ascii_lowercase();
    if t == "auto" {
        return Ok(None);
    }
    let n = if let Some(k) = t.strip_suffix('k') {
        k.parse::<f64>().ok().map(|n| (n * 1_000.0) as u64)
    } else if let Some(m) = t.strip_suffix('m') {
        m.parse::<f64>().ok().map(|n| (n * 1_000_000.0) as u64)
    } else {
        t.replace('_', "").parse::<u64>().ok()
    };
    match n {
        Some(n) if (100_000..=1_000_000).contains(&n) => Ok(Some(n)),
        _ => Err(CoreError::Config(format!("--autocompact must be auto or 100k-1M tokens, got {v:?}"))),
    }
}

/// Keep the per-person `.kodact/settings.local.json` out of git (as worktrees are),
/// so its rules and paths aren't committed and shared by accident.
pub fn ignore_local_settings(project: &Path) {
    if let Some(root) = kodact_git::repo_root(project) {
        let _ = kodact_git::exclude(&root, "**/.kodact/settings.local.json");
    }
}

/// Write an accepted `addRules` / `setMode` / `addDirectories` update to its settings file.
pub fn persist_permission_update(cwd: &Path, upd: &Value) {
    let dest = upd.get("destination").and_then(Value::as_str).unwrap_or("session");
    let file = match dest {
        "userSettings" => kodact_config::kodact_home().join("settings.json"),
        "projectSettings" => kodact_config::project_dir(cwd).join("settings.json"),
        "localSettings" => {
            ignore_local_settings(cwd);
            kodact_config::project_dir(cwd).join("settings.local.json")
        }
        _ => return,
    };
    let current: Value =
        std::fs::read_to_string(&file).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_else(|| json!({}));
    let result = match upd.get("type").and_then(Value::as_str) {
        Some("addRules") => {
            let behavior = upd.get("behavior").and_then(Value::as_str).unwrap_or("allow");
            let mut list: Vec<Value> = current
                .pointer(&format!("/permissions/{behavior}"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for r in upd.get("rules").and_then(Value::as_array).into_iter().flatten() {
                if let Ok(rv) = serde_json::from_value::<kodact_permissions::RuleValue>(r.clone()) {
                    let s = json!(rv.to_rule_string());
                    if !list.contains(&s) {
                        list.push(s);
                    }
                }
            }
            kodact_config::write_setting(&file, &["permissions", behavior], Value::Array(list))
        }
        Some("setMode") => kodact_config::write_setting(&file, &["permissions", "defaultMode"], upd["mode"].clone()),
        Some("addDirectories") => {
            let mut list: Vec<Value> = current
                .pointer("/permissions/additionalDirectories")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for d in upd.get("directories").and_then(Value::as_array).into_iter().flatten() {
                if !list.contains(d) {
                    list.push(d.clone());
                }
            }
            kodact_config::write_setting(&file, &["permissions", "additionalDirectories"], Value::Array(list))
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, file = %file.display(), "could not save permission update");
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod driver_tests;
