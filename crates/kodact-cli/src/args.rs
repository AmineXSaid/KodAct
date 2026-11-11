use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InputFormat {
    Text,
    #[value(name = "stream-json")]
    StreamJson,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PermissionPrompts {
    Host,
    None,
}

/// KodAct - an agentic coding CLI. Starts an interactive session by default;
/// use -p/--print for non-interactive output.
#[derive(Debug, Parser)]
#[command(name = "kodact", version = concat!(env!("CARGO_PKG_VERSION"), " (KodAct)"), disable_version_flag = true)]
pub struct Cli {
    /// Your prompt
    pub prompt: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,

    #[command(flatten)]
    pub opts: Opts,

    /// Output the version number
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: Option<bool>,
}

#[derive(Debug, Args, Clone)]
pub struct Opts {
    /// Print response and exit (useful for pipes)
    #[arg(short = 'p', long = "print")]
    pub print: bool,
    /// Output format (only with --print)
    #[arg(long = "output-format", value_enum, default_value = "text")]
    pub output_format: OutputFormat,
    /// Input format (only with --print)
    #[arg(long = "input-format", value_enum, default_value = "text")]
    pub input_format: InputFormat,
    /// Include partial message chunks as they arrive (stream-json only)
    #[arg(long = "include-partial-messages")]
    pub include_partial_messages: bool,
    /// Re-emit user messages from stdin on stdout (stream-json in and out)
    #[arg(long = "replay-user-messages")]
    pub replay_user_messages: bool,
    /// Include hook lifecycle events in the output stream
    #[arg(long = "include-hook-events")]
    pub include_hook_events: bool,
    /// More diagnostics on stderr (never on stdout)
    #[arg(long)]
    pub verbose: bool,
    /// Only errors on stderr; no warnings or progress
    #[arg(short = 'q', long, conflicts_with = "verbose")]
    pub quiet: bool,
    /// Never ask for input: permission prompts are denied and interactive mode refuses to start
    #[arg(long = "no-input", env = "KODACT_NO_INPUT", value_parser = clap::builder::BoolishValueParser::new(), num_args = 0..=1, require_equals = true, default_missing_value = "true", default_value = "false")]
    pub no_input: bool,
    /// Unattended run: no question or plan-approval tools, a prompt to finish without asking, a reminder to attempt before giving up, and retries of failed model calls (also the `autonomous` setting)
    #[arg(long = "autonomous", env = "KODACT_AUTONOMOUS", action = clap::ArgAction::SetTrue, value_parser = clap::builder::BoolishValueParser::new())]
    pub autonomous: bool,
    /// Interactive mode: use the line-based prompt instead of the terminal UI (also KODACT_TUI=0)
    #[arg(long = "no-tui")]
    pub no_tui: bool,
    /// When to use color on stderr: auto (default; honours NO_COLOR and FORCE_COLOR), always, never
    #[arg(long, value_enum, default_value = "auto")]
    pub color: crate::term::ColorChoice,
    /// Enable debug logging, optionally filtered (e.g. "api,hooks")
    #[arg(short = 'd', long = "debug", num_args = 0..=1, default_missing_value = "")]
    pub debug: Option<String>,
    /// Write debug logs to a file (implies --debug)
    #[arg(long = "debug-file")]
    pub debug_file: Option<PathBuf>,

    /// Model for the session: an alias (opus, sonnet, haiku, fable) or a full name
    #[arg(long)]
    pub model: Option<String>,
    /// Fallback model(s), comma-separated, used when the primary is overloaded
    #[arg(long = "fallback-model")]
    pub fallback_model: Option<String>,
    /// Effort level (low, medium, high, xhigh, max)
    #[arg(long)]
    pub effort: Option<String>,
    /// Beta headers to include in API requests
    #[arg(long, num_args = 1..)]
    pub betas: Vec<String>,

    /// Permission mode (acceptEdits, auto, bypassPermissions, manual, dontAsk, plan)
    #[arg(long = "permission-mode")]
    pub permission_mode: Option<String>,
    /// Bypass all permission checks (sandboxes only)
    #[arg(long = "dangerously-skip-permissions")]
    pub dangerously_skip_permissions: bool,
    /// Make bypassing permissions available as an option
    #[arg(long = "allow-dangerously-skip-permissions")]
    pub allow_dangerously_skip_permissions: bool,
    /// Who answers permission prompts in print mode
    #[arg(long = "permission-prompts", value_enum, default_value = "host")]
    pub permission_prompts: PermissionPrompts,
    /// Permission prompt channel (`stdio` = the SDK host)
    #[arg(long = "permission-prompt-tool")]
    pub permission_prompt_tool: Option<String>,
    /// Tools to allow, comma or space separated (e.g. "Bash(git *) Edit")
    #[arg(long = "allowedTools", alias = "allowed-tools", num_args = 1..)]
    pub allowed_tools: Vec<String>,
    /// Tools to deny, comma or space separated
    #[arg(long = "disallowedTools", alias = "disallowed-tools", num_args = 1..)]
    pub disallowed_tools: Vec<String>,
    /// Built-in tools to offer ("" = none, "default" = all)
    #[arg(long, num_args = 0..)]
    pub tools: Option<Vec<String>>,
    /// Run shell commands in an OS sandbox: off, read-only, or workspace-write (writable working directories, no network)
    #[arg(long)]
    pub sandbox: Option<String>,
    /// Additional directories to allow tool access to
    #[arg(long = "add-dir", num_args = 1..)]
    pub add_dir: Vec<PathBuf>,
    /// Load plugins from these directories (commands, agents, skills, hooks, MCP servers)
    #[arg(long = "plugin-dir", num_args = 1.., value_name = "DIRS")]
    pub plugin_dir: Vec<PathBuf>,
    /// Load MCP servers from JSON files or strings (space separated)
    #[arg(long = "mcp-config", num_args = 1.., value_name = "CONFIGS")]
    pub mcp_config: Vec<String>,
    /// Use only the MCP servers from --mcp-config
    #[arg(long = "strict-mcp-config")]
    pub strict_mcp_config: bool,

    /// System prompt for the session
    #[arg(long = "system-prompt")]
    pub system_prompt: Option<String>,
    /// Read the system prompt from a file
    #[arg(long = "system-prompt-file")]
    pub system_prompt_file: Option<PathBuf>,
    /// Append to the default system prompt
    #[arg(long = "append-system-prompt")]
    pub append_system_prompt: Option<String>,
    /// Append the contents of a file to the system prompt
    #[arg(long = "append-system-prompt-file")]
    pub append_system_prompt_file: Option<PathBuf>,
    /// Move per-machine sections from the system prompt into the first user message
    #[arg(long = "exclude-dynamic-system-prompt-sections")]
    pub exclude_dynamic_system_prompt_sections: bool,

    /// Continue the most recent conversation in this directory
    #[arg(short = 'c', long = "continue")]
    pub continue_: bool,
    /// Resume a conversation by session ID
    #[arg(short = 'r', long = "resume", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,
    /// When resuming, create a new session ID
    #[arg(long = "fork-session")]
    pub fork_session: bool,
    /// Use a specific session ID (a UUID)
    #[arg(long = "session-id")]
    pub session_id: Option<String>,
    /// Run in a new git worktree (`.kodact/worktrees/<name>`, branch `kodact/<name>`)
    #[arg(short = 'w', long = "worktree", num_args = 0..=1, default_missing_value = "", value_name = "NAME")]
    pub worktree: Option<String>,
    /// Do not save the session to disk (print mode)
    #[arg(long = "no-session-persistence")]
    pub no_session_persistence: bool,
    /// Display name for the session
    #[arg(short = 'n', long = "name")]
    pub name: Option<String>,

    /// Agent definitions as JSON, or a file holding them (e.g. '{"reviewer": {"description": "...", "prompt": "..."}}')
    #[arg(long)]
    pub agents: Option<String>,
    /// Run the session as this agent (its prompt and tools)
    #[arg(long)]
    pub agent: Option<String>,
    /// Auto-compact window size (auto, or 100k-1M tokens)
    #[arg(long)]
    pub autocompact: Option<String>,
    /// Maximum number of agentic turns (print mode)
    #[arg(long = "max-turns")]
    pub max_turns: Option<u32>,
    /// Maximum dollar amount to spend on API calls (print mode)
    #[arg(long = "max-budget-usd")]
    pub max_budget_usd: Option<f64>,
    /// Time limit for the run, e.g. 900, 15m or 1h (print mode): the model is told it and warned near the end
    #[arg(long = "max-time", value_parser = parse_duration)]
    pub max_time: Option<std::time::Duration>,
    /// JSON Schema for structured output
    #[arg(long = "json-schema")]
    pub json_schema: Option<String>,

    /// Settings file path or JSON string
    #[arg(long)]
    pub settings: Option<String>,
    /// Setting sources to load (user, project, local)
    #[arg(long = "setting-sources")]
    pub setting_sources: Option<String>,
    /// Minimal mode: no hooks or memory files
    #[arg(long)]
    pub bare: bool,
    /// Start with all customizations disabled
    #[arg(long = "safe-mode")]
    pub safe_mode: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check the health of the installation
    Doctor {
        /// Also contact the endpoint (its model list) and run the key helper, to check the URL and key
        #[arg(long)]
        probe: bool,
    },
    /// Inspect settings: merged values (secrets redacted), where they came from, and file locations
    Config {
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Print a shell completion script (bash, zsh, fish, elvish, powershell)
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Configure and manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Carry connections to a localhost port over stdin/stdout (for a host
    /// app that reaches this machine only through a process channel)
    #[command(hide = true)]
    Relay {
        /// The port to listen on, on 127.0.0.1
        #[arg(long)]
        port: u16,
    },
}

#[derive(Debug, Subcommand)]
pub enum McpAction {
    /// Run KodAct's built-in tools as an MCP server on stdio
    Serve,
    /// Add a server: `kodact mcp add name -- command args...` or `kodact mcp add -t http name https://...`
    Add {
        /// Where to save it: local (this project, private), project (.mcp.json, shared), or user
        #[arg(short, long, default_value = "local")]
        scope: String,
        /// stdio (default for commands), http (default for URLs) or sse
        #[arg(short, long)]
        transport: Option<String>,
        /// Environment for a stdio server, KEY=value (repeatable)
        #[arg(short, long = "env")]
        env: Vec<String>,
        /// Header for an HTTP server, "Name: value" (repeatable)
        #[arg(short = 'H', long = "header")]
        header: Vec<String>,
        name: String,
        /// The command (stdio) or URL (http, sse)
        command_or_url: String,
        /// Arguments for the command
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Add a server from its JSON config
    AddJson {
        #[arg(short, long, default_value = "local")]
        scope: String,
        name: String,
        json: String,
    },
    /// Remove a server (from the given scope, or from wherever it is)
    Remove {
        #[arg(short, long)]
        scope: Option<String>,
        name: String,
    },
    /// List servers and check that each one starts
    List,
    /// Show one server's configuration (secrets redacted) and status
    Get { name: String },
    /// Trust servers from this project's .mcp.json (saved in local settings)
    Approve {
        /// The server to trust
        name: Option<String>,
        /// Trust every server in this project's .mcp.json
        #[arg(long)]
        all: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Print the merged settings as JSON (secrets redacted)
    List {
        /// Also show which file each top-level key came from
        #[arg(long)]
        origin: bool,
    },
    /// Print one setting: a dotted key (permissions.defaultMode) or a JSON pointer (/permissions/defaultMode)
    Get { key: String },
    /// Print the configuration, state and cache directories and the settings files read
    Paths,
}

/// `--max-time`: whole seconds, or a number with `s`, `m` or `h`.
pub fn parse_duration(raw: &str) -> Result<std::time::Duration, String> {
    let raw = raw.trim();
    let (num, unit) = match raw.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&raw[..i], c.to_ascii_lowercase()),
        _ => (raw, 's'),
    };
    let n: u64 = num.trim().parse().map_err(|_| format!("expected a duration like 900, 15m or 1h, not {raw:?}"))?;
    let secs = match unit {
        's' => n,
        'm' => n * 60,
        'h' => n * 3600,
        _ => return Err(format!("unknown unit in {raw:?}: use s, m or h")),
    };
    if secs == 0 {
        return Err("the time limit must be more than 0".into());
    }
    Ok(std::time::Duration::from_secs(secs))
}
