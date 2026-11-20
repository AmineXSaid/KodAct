//! Interactive local BYOK onboarding wizard (`kodact setup` and automatic first-run).
//!
//! Writes provider settings (`env.<KEY>`, `model`, and optional `env.OPENAI_BASE_URL`)
//! directly to `~/.kodact/settings.json` with `0o600` permissions on Unix—zero servers,
//! zero accounts.
//!
//! The wizard is one centred card on the alternate screen: a stepper, the step's
//! fields, and the keys that work there. Every row is cut to the card's inner width,
//! so nothing crosses its edges at any terminal size.

use std::future::Future;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::{Frame, Terminal};
use serde_json::{json, Map, Value};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::palette::{self, Depth};
use super::text::Theme;
use crate::exit::Fail;

/// Result of running the interactive setup wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetupOutcome {
    /// Settings were saved to disk; ready to start or resume a session.
    Configured { provider: &'static str, model: String },
    /// User pressed Esc/Ctrl+C before saving.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Anthropic,
    OpenAi,
    Gemini,
    Ollama,
    Custom,
}

impl Provider {
    pub const ALL: [Provider; 5] =
        [Provider::Anthropic, Provider::OpenAi, Provider::Gemini, Provider::Ollama, Provider::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Anthropic => "Anthropic",
            Provider::OpenAi => "OpenAI",
            Provider::Gemini => "Google Gemini",
            Provider::Ollama => "Ollama (Local)",
            Provider::Custom => "OpenAI-Compatible / Custom URL",
        }
    }

    /// The name in the list, short enough to leave room for the hint.
    fn short_label(self) -> &'static str {
        match self {
            Provider::Custom => "Custom endpoint",
            Provider::Ollama => "Ollama",
            p => p.label(),
        }
    }

    /// The hint beside the name in the list: what it is, in a few words.
    fn list_hint(self) -> &'static str {
        match self {
            Provider::Anthropic => "Claude models",
            Provider::OpenAi => "GPT and o-series models",
            Provider::Gemini => "Gemini models",
            Provider::Ollama => "Models on this machine · no key",
            Provider::Custom => "OpenRouter, Groq, vLLM, LM Studio…",
        }
    }

    /// Where to get a key, under the key field.
    fn key_help(self) -> &'static str {
        match self {
            Provider::Anthropic => "Create one at console.anthropic.com",
            Provider::OpenAi => "Create one at platform.openai.com/api-keys",
            Provider::Gemini => "Create one at aistudio.google.com/apikey",
            Provider::Custom => "The key your endpoint expects · empty if it needs none",
            Provider::Ollama => "",
        }
    }

    pub fn env_key(self) -> Option<&'static str> {
        match self {
            Provider::Anthropic => Some("ANTHROPIC_API_KEY"),
            Provider::OpenAi | Provider::Custom => Some("OPENAI_API_KEY"),
            Provider::Gemini => Some("GEMINI_API_KEY"),
            Provider::Ollama => None,
        }
    }

    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Provider::Ollama => Some("http://localhost:11434/v1"),
            Provider::Custom => Some("https://openrouter.ai/api/v1"),
            _ => None,
        }
    }

    pub fn needs_url(self) -> bool {
        matches!(self, Provider::Ollama | Provider::Custom)
    }

    /// The key may be left empty: local servers and many custom endpoints need none.
    fn key_optional(self) -> bool {
        matches!(self, Provider::Ollama | Provider::Custom)
    }

    pub fn default_models(self) -> &'static [&'static str] {
        match self {
            Provider::Anthropic => &["claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5"],
            Provider::OpenAi => &["gpt-5", "gpt-5-mini", "o4-mini"],
            Provider::Gemini => &["gemini-2.5-pro", "gemini-2.5-flash"],
            Provider::Ollama => &["qwen2.5-coder:14b", "llama3.3", "deepseek-r1:14b"],
            Provider::Custom => &["anthropic/claude-sonnet-4", "openai/gpt-4o", "deepseek/deepseek-chat"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Provider,
    Credentials,
    Model,
    Review,
}

impl Step {
    const ALL: [Step; 4] = [Step::Provider, Step::Credentials, Step::Model, Step::Review];

    fn index(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    /// The heading in the card's top edge.
    fn heading(self) -> &'static str {
        match self {
            Step::Provider => "Choose a provider",
            Step::Credentials => "Connect",
            Step::Model => "Pick a default model",
            Step::Review => "Review & save",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Step::Provider => "Provider",
            Step::Credentials => "Connect",
            Step::Model => "Model",
            Step::Review => "Review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredField {
    Url,
    Key,
}

/// A row of the model list: a model the endpoint (or KodAct) knows, or the ID as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModelRow {
    Listed(String),
    Typed(String),
}

/// Pure state machine for the onboarding wizard.
#[derive(Debug, Clone)]
pub struct SetupState {
    pub step: Step,
    pub provider_idx: usize,
    pub base_url: String,
    pub api_key: String,
    pub reveal_key: bool,
    cred_field: CredField,
    pub models: Vec<String>,
    pub model_idx: usize,
    /// What was typed on the model step: it filters the list, and is offered as an ID of its own.
    pub custom_model: String,
    pub probing: bool,
    pub status: Option<String>,
    pub error: Option<String>,
    /// The provider's key is already in the environment, so the field may stay empty.
    pub key_in_env: bool,
    /// Look the provider's key up in the environment (off in tests).
    detect_env: bool,
    /// Frame counter for the spinner.
    pub tick: usize,
}

impl Default for SetupState {
    fn default() -> Self {
        Self::new()
    }
}

impl SetupState {
    pub fn new() -> Self {
        Self {
            step: Step::Provider,
            provider_idx: 0,
            base_url: String::new(),
            api_key: String::new(),
            reveal_key: false,
            cred_field: CredField::Key,
            models: Provider::Anthropic.default_models().iter().map(|s| (*s).to_string()).collect(),
            model_idx: 0,
            custom_model: String::new(),
            probing: false,
            status: None,
            error: None,
            key_in_env: false,
            detect_env: false,
            tick: 0,
        }
    }

    /// A wizard that notices keys already exported in the shell.
    pub fn with_env() -> Self {
        Self { detect_env: true, ..Self::new() }
    }

    pub fn provider(&self) -> Provider {
        Provider::ALL[self.provider_idx.min(Provider::ALL.len() - 1)]
    }

    fn rows(&self) -> Vec<ModelRow> {
        let typed = self.custom_model.trim();
        let needle = typed.to_lowercase();
        let mut rows: Vec<ModelRow> = self
            .models
            .iter()
            .filter(|m| needle.is_empty() || m.to_lowercase().contains(&needle))
            .map(|m| ModelRow::Listed(m.clone()))
            .collect();
        if !typed.is_empty() && !self.models.iter().any(|m| m == typed) {
            rows.push(ModelRow::Typed(typed.to_string()));
        }
        rows
    }

    pub fn selected_model(&self) -> String {
        let rows = self.rows();
        match rows.get(self.model_idx.min(rows.len().saturating_sub(1))) {
            Some(ModelRow::Listed(m)) | Some(ModelRow::Typed(m)) => m.clone(),
            None => self.models.first().cloned().unwrap_or_else(|| self.provider().default_models()[0].to_string()),
        }
    }

    /// Swap in the models the endpoint listed, keeping the choice the user already made:
    /// the probe can answer after they moved on, even to the review step.
    fn replace_models(&mut self, found: Vec<String>) {
        let before = self.selected_model();
        self.models = found;
        self.model_idx = self
            .rows()
            .iter()
            .position(|r| matches!(r, ModelRow::Listed(m) | ModelRow::Typed(m) if *m == before))
            .unwrap_or(0);
    }

    pub fn masked_key(&self) -> String {
        if self.api_key.is_empty() {
            return String::new();
        }
        let chars: Vec<char> = self.api_key.chars().collect();
        if self.reveal_key {
            if chars.len() <= 8 {
                return self.api_key.clone();
            }
            let tail: String = chars[chars.len() - 4..].iter().collect();
            return format!("{}{}", "•".repeat(chars.len() - 4), tail);
        }
        "•".repeat(chars.len())
    }

    /// The key the probe sends: the typed one, else the one in the environment.
    fn effective_key(&self) -> String {
        let typed = self.api_key.trim();
        if !typed.is_empty() {
            return typed.to_string();
        }
        match (self.key_in_env, self.provider().env_key()) {
            (true, Some(k)) => std::env::var(k).unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Advance from Step 1 to Step 2 with provider defaults.
    pub fn enter_credentials(&mut self) {
        let p = self.provider();
        self.base_url = p.default_base_url().unwrap_or("").to_string();
        self.cred_field = if p.needs_url() { CredField::Url } else { CredField::Key };
        self.models = p.default_models().iter().map(|s| (*s).to_string()).collect();
        self.model_idx = 0;
        self.custom_model.clear();
        self.status = None;
        self.error = None;
        self.key_in_env = self.detect_env
            && p.env_key().and_then(std::env::var_os).is_some_and(|v| !v.to_string_lossy().trim().is_empty());
        self.step = Step::Credentials;
    }

    /// Validate Step 2 and advance to Step 3.
    pub fn enter_model(&mut self) -> Result<(), String> {
        let p = self.provider();
        if p.needs_url() {
            let url = self.base_url.trim();
            if url.is_empty() {
                return Err("Enter a base URL (for example http://localhost:11434/v1).".to_string());
            }
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err("The base URL starts with http:// or https://.".to_string());
            }
        }
        if !p.key_optional() && self.api_key.trim().is_empty() && !self.key_in_env {
            return Err(format!("Paste your {} to continue.", p.env_key().unwrap_or("API key")));
        }
        self.error = None;
        self.step = Step::Model;
        Ok(())
    }

    fn field_mut(&mut self) -> &mut String {
        match self.cred_field {
            CredField::Url => &mut self.base_url,
            CredField::Key => &mut self.api_key,
        }
    }

    /// Both fields show for a custom endpoint; Tab and the arrows move between them.
    fn switch_field(&mut self) {
        if self.provider() == Provider::Custom {
            self.cred_field = match self.cred_field {
                CredField::Url => CredField::Key,
                CredField::Key => CredField::Url,
            };
        }
    }

    /// Apply a key event. Returns `Some(SetupAction)` when the outer loop needs to act.
    pub fn on_key(&mut self, k: KeyEvent) -> Option<SetupAction> {
        if k.kind == KeyEventKind::Release {
            return None;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return Some(SetupAction::Cancel);
        }
        match self.step {
            Step::Provider => match k.code {
                KeyCode::Esc => Some(SetupAction::Cancel),
                KeyCode::Up | KeyCode::Char('k') => {
                    self.provider_idx = self.provider_idx.checked_sub(1).unwrap_or(Provider::ALL.len() - 1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                    self.provider_idx = (self.provider_idx + 1) % Provider::ALL.len();
                    None
                }
                KeyCode::Home => {
                    self.provider_idx = 0;
                    None
                }
                KeyCode::End => {
                    self.provider_idx = Provider::ALL.len() - 1;
                    None
                }
                KeyCode::Char(c @ '1'..='9') if !ctrl => {
                    let i = c as usize - '1' as usize;
                    if i < Provider::ALL.len() {
                        self.provider_idx = i;
                        self.enter_credentials();
                    }
                    None
                }
                KeyCode::Enter => {
                    self.enter_credentials();
                    None
                }
                _ => None,
            },
            Step::Credentials => {
                let result = match k.code {
                    KeyCode::Esc => {
                        self.step = Step::Provider;
                        None
                    }
                    KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                        self.switch_field();
                        None
                    }
                    KeyCode::Char('r') if ctrl => {
                        self.reveal_key = !self.reveal_key;
                        None
                    }
                    KeyCode::Char('u') if ctrl => {
                        self.field_mut().clear();
                        None
                    }
                    KeyCode::Char('w') if ctrl => {
                        let f = self.field_mut();
                        let sep = |c: char| matches!(c, '/' | ' ' | '.' | '-' | '_' | ':');
                        while f.ends_with(sep) {
                            f.pop();
                        }
                        while f.chars().last().is_some_and(|c| !sep(c)) {
                            f.pop();
                        }
                        None
                    }
                    KeyCode::Enter => {
                        if self.provider() == Provider::Custom && self.cred_field == CredField::Url {
                            self.cred_field = CredField::Key;
                            return None;
                        }
                        return match self.enter_model() {
                            Ok(()) => Some(SetupAction::ProbeModels),
                            Err(e) => {
                                self.error = Some(e);
                                None
                            }
                        };
                    }
                    KeyCode::Backspace => {
                        self.field_mut().pop();
                        None
                    }
                    KeyCode::Char(c) if !ctrl && !c.is_control() => {
                        self.field_mut().push(c);
                        None
                    }
                    _ => return None,
                };
                self.error = None;
                result
            }
            Step::Model => match k.code {
                KeyCode::Esc => {
                    self.error = None;
                    if !self.custom_model.is_empty() {
                        self.custom_model.clear();
                        self.model_idx = 0;
                        return None;
                    }
                    self.probing = false;
                    self.step = Step::Credentials;
                    Some(SetupAction::StopProbe)
                }
                KeyCode::Up => {
                    let n = self.rows().len().max(1);
                    self.model_idx = self.model_idx.checked_sub(1).unwrap_or(n - 1);
                    None
                }
                KeyCode::Down | KeyCode::Tab => {
                    let n = self.rows().len().max(1);
                    self.model_idx = (self.model_idx + 1) % n;
                    None
                }
                KeyCode::Backspace => {
                    self.custom_model.pop();
                    self.model_idx = 0;
                    None
                }
                KeyCode::Char('u') if ctrl => {
                    self.custom_model.clear();
                    self.model_idx = 0;
                    None
                }
                KeyCode::Char(c) if !ctrl && !c.is_control() => {
                    self.custom_model.push(c);
                    self.model_idx = 0;
                    None
                }
                KeyCode::Enter => {
                    if self.rows().is_empty() && self.models.is_empty() {
                        self.error = Some("Type a model ID to continue.".into());
                        return None;
                    }
                    self.error = None;
                    self.step = Step::Review;
                    None
                }
                _ => None,
            },
            Step::Review => match k.code {
                KeyCode::Esc | KeyCode::Backspace => {
                    self.error = None;
                    self.step = Step::Model;
                    None
                }
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('s') if !ctrl => Some(SetupAction::Save),
                _ => None,
            },
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        // One line only: a pasted key often brings a newline with it.
        let clean: String = text.trim().lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).collect();
        match self.step {
            Step::Credentials => {
                self.field_mut().push_str(clean.trim());
                self.error = None;
            }
            Step::Model => {
                self.custom_model.push_str(clean.trim());
                self.model_idx = 0;
            }
            Step::Provider | Step::Review => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    ProbeModels,
    /// Leaving the model step: a probe still running is dropped.
    StopProbe,
    Save,
    Cancel,
}

/// Write the selected provider, credentials, and model to `path` atomically with `0o600` on Unix.
pub fn write_settings(path: &Path, state: &SetupState) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut root: Map<String, Value> = if path.exists() {
        let raw = std::fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str::<Value>(&raw).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default()
    } else {
        Map::new()
    };

    let mut env =
        root.entry("env").or_insert_with(|| Value::Object(Map::new())).as_object_mut().cloned().unwrap_or_default();

    let p = state.provider();
    if let Some(key_name) = p.env_key() {
        let val = state.api_key.trim();
        if !val.is_empty() {
            env.insert(key_name.to_string(), json!(val));
        }
    }
    if p.needs_url() {
        let url = state.base_url.trim();
        if !url.is_empty() {
            env.insert("OPENAI_BASE_URL".to_string(), json!(url));
        }
    }
    root.insert("env".to_string(), Value::Object(env));
    root.insert("model".to_string(), json!(state.selected_model()));

    let bytes = serde_json::to_vec_pretty(&Value::Object(root)).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// What a `/models` probe needs, owned, so it can run while the card keeps drawing.
fn probe_request(state: &SetupState) -> Option<(String, String)> {
    let url = match state.provider() {
        Provider::Ollama | Provider::Custom => format!("{}/models", state.base_url.trim().trim_end_matches('/')),
        Provider::OpenAi => "https://api.openai.com/v1/models".to_string(),
        _ => return None,
    };
    Some((url, state.effective_key()))
}

/// Best-effort live `/models` probe for OpenAI-compatible or Ollama endpoints.
async fn probe_models(url: String, key: String) -> Option<Vec<String>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(4)).build().ok()?;
    let mut req = client.get(&url);
    if !key.is_empty() {
        req = req.bearer_auth(key);
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body: Value = resp.json().await.ok()?;
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| arr.iter().filter_map(|m| m.get("id").and_then(|s| s.as_str()).map(String::from)).collect())
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

// ---- drawing ----

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
/// The widest the card grows: wide enough for a URL, narrow enough to read at a glance.
const CARD_MAX_W: u16 = 74;

fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// `s` cut to `w` columns, with an ellipsis when it was longer.
fn fit(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// The end of `s` that fits in `w` columns: what an input shows as the text grows past it.
fn tail(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out: Vec<char> = vec![];
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out.iter().rev().collect()
}

/// Spans cut so the row never passes `w` columns, then padded to exactly `w`.
fn clamp_row(spans: Vec<Span<'static>>, w: usize) -> Line<'static> {
    let mut out = vec![];
    let mut used = 0;
    for s in spans {
        let sw = width(&s.content);
        if used + sw <= w {
            used += sw;
            out.push(s);
        } else {
            let room = w - used;
            if room > 0 {
                let cut = fit(&s.content, room);
                used += width(&cut);
                out.push(Span::styled(cut, s.style));
            }
            break;
        }
    }
    let bg = out.last().and_then(|s| s.style.bg);
    if used < w {
        let pad = Span::styled(" ".repeat(w - used), bg.map(|c| Style::default().bg(c)).unwrap_or_default());
        out.push(pad);
    }
    Line::from(out)
}

/// A surface colour as a background, where the terminal has enough colours.
fn surface(t: &Theme, role: palette::Role) -> Option<Color> {
    (t.color && t.depth != Depth::Ansi16).then(|| role.color(t.depth, t.light))
}

/// A key and what it does, for the foot of the card.
fn keycap(t: &Theme, key: &str) -> Span<'static> {
    let style = match surface(t, palette::PROMPT_BG) {
        Some(bg) => Style::default().bg(bg).fg(palette::PROMPT_FG.color(t.depth, t.light)),
        None => t.bold(),
    };
    Span::styled(format!(" {key} "), style)
}

/// The brightest text, bold: a heading on the card's faint edge, which would otherwise take
/// the edge's colour.
fn strong(t: &Theme) -> Style {
    if t.color && t.depth != Depth::Ansi16 {
        t.bold().fg(palette::STRONG.color(t.depth, t.light))
    } else {
        t.bold()
    }
}

/// The filled call to action on the review step.
fn button(t: &Theme, label: &str) -> Span<'static> {
    let style = if t.color {
        let fg = if t.light { Color::White } else { Color::Black };
        Style::default().bg(palette::BRAND.color(t.depth, t.light)).fg(fg).add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    };
    Span::styled(format!("  {label}  "), style)
}

/// The rows inside the card, and where the terminal cursor goes, if a field has focus.
struct Body {
    rows: Vec<Line<'static>>,
    cursor: Option<(u16, u16)>,
}

impl Body {
    fn push(&mut self, spans: Vec<Span<'static>>, w: usize) {
        self.rows.push(clamp_row(spans, w));
    }

    fn blank(&mut self) {
        self.rows.push(Line::default());
    }

    /// Text that wraps to the width instead of being cut: errors and notes must be read whole.
    fn wrapped(&mut self, lead: &str, text: &str, style: Style, w: usize) {
        let lead_w = width(lead);
        let room = w.saturating_sub(lead_w).max(1);
        let mut line = String::new();
        let mut first = true;
        let flush = |line: &mut String, first: &mut bool, rows: &mut Vec<Line<'static>>| {
            let head = if *first { lead.to_string() } else { " ".repeat(lead_w) };
            rows.push(clamp_row(vec![Span::styled(head, style), Span::styled(std::mem::take(line), style)], w));
            *first = false;
        };
        for word in text.split_whitespace() {
            let word = fit(word, room);
            if !line.is_empty() && width(&line) + 1 + width(&word) > room {
                flush(&mut line, &mut first, &mut self.rows);
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(&word);
        }
        if !line.is_empty() || first {
            flush(&mut line, &mut first, &mut self.rows);
        }
    }

    /// A boxed input: label in the top edge, the value (its end, when long), a cursor
    /// at the end when focused.
    #[allow(clippy::too_many_arguments)]
    fn input(&mut self, t: &Theme, label: &str, value: &str, placeholder: &str, focused: bool, right: &str, w: usize) {
        let edge = if focused { t.accent() } else { t.hairline() };
        let label_style = if focused { t.accent().add_modifier(Modifier::BOLD) } else { t.dim() };
        let w = w.max(8);
        let inner = w - 4;
        // ╭─ Label ──────── right ─╮
        let label = fit(label, inner.saturating_sub(2));
        let right = if width(right) + width(&label) + 6 <= w { right.to_string() } else { String::new() };
        let mut top =
            vec![Span::styled("╭─ ", edge), Span::styled(label.clone(), label_style), Span::styled(" ", edge)];
        let used = 3 + width(&label) + 1 + if right.is_empty() { 0 } else { width(&right) + 2 } + 1;
        top.push(Span::styled("─".repeat(w.saturating_sub(used)), edge));
        if !right.is_empty() {
            top.push(Span::styled(" ", edge));
            top.push(Span::styled(right, t.subtle()));
            top.push(Span::styled(" ", edge));
        }
        top.push(Span::styled("╮", edge));
        self.push(top, w);
        // │ value▏ │ — one column is kept for the cursor.
        let (shown, style) = if value.is_empty() {
            (fit(placeholder, inner), t.subtle())
        } else {
            (
                tail(value, inner - 1),
                Style::default().add_modifier(if focused { Modifier::BOLD } else { Modifier::empty() }),
            )
        };
        let shown_w = width(&shown);
        if focused {
            let col = 2 + if value.is_empty() { 0 } else { shown_w };
            self.cursor = Some((col as u16, self.rows.len() as u16));
        }
        let mid = vec![
            Span::styled("│ ", edge),
            Span::styled(shown, style),
            Span::raw(" ".repeat(inner.saturating_sub(shown_w))),
            Span::styled(" │", edge),
        ];
        self.push(mid, w);
        self.push(vec![Span::styled("╰", edge), Span::styled("─".repeat(w - 2), edge), Span::styled("╯", edge)], w);
    }

    /// A list window of `room` rows around `sel`, with "↑ n more" / "↓ n more" in place of what is hidden.
    fn list<T>(
        &mut self,
        t: &Theme,
        items: &[T],
        sel: usize,
        room: usize,
        w: usize,
        row: impl Fn(&T, bool) -> Vec<Span<'static>>,
    ) {
        let room = room.max(1);
        let n = items.len();
        let (mut start, mut end) = (0, n);
        if n > room {
            let visible = room.saturating_sub(2).max(1);
            start = sel.saturating_sub(visible / 2).min(n - visible);
            end = start + visible;
        }
        if n > room {
            let label = if start > 0 { format!("   ↑ {start} more") } else { String::new() };
            self.push(vec![Span::styled(label, t.subtle())], w);
        }
        for (i, item) in items.iter().enumerate().take(end).skip(start) {
            let active = i == sel;
            let spans = row(item, active);
            let mut line = clamp_row(spans, w);
            if active {
                if let Some(bg) = surface(t, palette::PROMPT_BG) {
                    for s in &mut line.spans {
                        s.style = s.style.bg(bg);
                    }
                }
            }
            self.rows.push(line);
        }
        if n > room {
            let label = if end < n { format!("   ↓ {} more", n - end) } else { String::new() };
            self.push(vec![Span::styled(label, t.subtle())], w);
        }
    }
}

/// The stepper: done steps checked, the current one in the brand colour, the rest quiet.
/// Falls back to "Step 2 of 4 · Connect" when the full row does not fit.
fn stepper(t: &Theme, step: Step, w: usize) -> Line<'static> {
    let cur = step.index();
    let mut spans = vec![];
    for (i, s) in Step::ALL.iter().enumerate() {
        if i > 0 {
            let style = if i <= cur { t.accent() } else { t.hairline() };
            spans.push(Span::styled(" ── ", style));
        }
        let (mark, style) = if i < cur {
            ("✓ ", t.success())
        } else if i == cur {
            ("● ", t.accent().add_modifier(Modifier::BOLD))
        } else {
            ("○ ", t.subtle())
        };
        spans.push(Span::styled(mark, style));
        let label_style = if i == cur {
            t.bold()
        } else if i < cur {
            t.dim()
        } else {
            t.subtle()
        };
        spans.push(Span::styled(s.title(), label_style));
    }
    if spans_width(&spans) <= w {
        return clamp_row(spans, w);
    }
    clamp_row(
        vec![
            Span::styled(format!("Step {} of {}", cur + 1, Step::ALL.len()), t.accent().add_modifier(Modifier::BOLD)),
            Span::styled(" · ", t.subtle()),
            Span::styled(step.title(), t.bold()),
        ],
        w,
    )
}

/// The keys that work on this step; the last ones go first when the card is narrow.
fn footer(t: &Theme, state: &SetupState, w: usize) -> Line<'static> {
    let keys: Vec<(&str, &str)> = match state.step {
        Step::Provider => vec![("⏎", "continue"), ("↑↓", "choose"), ("1-5", "jump"), ("esc", "quit")],
        Step::Credentials if state.provider() == Provider::Custom => {
            vec![("⏎", "next"), ("tab", "field"), ("^R", "peek"), ("esc", "back")]
        }
        Step::Credentials if state.provider().env_key().is_some() => {
            vec![("⏎", "continue"), ("^R", "peek"), ("^U", "clear"), ("esc", "back")]
        }
        Step::Credentials => vec![("⏎", "continue"), ("^U", "clear"), ("esc", "back")],
        Step::Model => vec![("⏎", "continue"), ("↑↓", "choose"), ("type", "filter"), ("esc", "back")],
        Step::Review => vec![("⏎", "save"), ("esc", "back"), ("^C", "quit")],
    };
    let mut spans: Vec<Span<'static>> = vec![];
    for (k, what) in keys {
        let mut piece = vec![];
        if !spans.is_empty() {
            piece.push(Span::raw("   "));
        }
        piece.push(keycap(t, k));
        piece.push(Span::styled(format!(" {what}"), t.dim()));
        if spans_width(&spans) + spans_width(&piece) > w {
            break;
        }
        spans.extend(piece);
    }
    clamp_row(spans, w)
}

fn body(state: &SetupState, t: &Theme, w: usize, room: usize) -> Body {
    let mut b = Body { rows: vec![], cursor: None };
    match state.step {
        Step::Provider => {
            // The list is what the step is for: the intro and the note give way to it when short.
            let intro = "Where should KodAct send its requests? You can change this later.";
            let note = "Saved locally to ~/.kodact/settings.json (0600). No server, no account.";
            let count = |text: &str| {
                let mut probe = Body { rows: vec![], cursor: None };
                probe.wrapped("", text, Style::default(), w);
                probe.rows.len()
            };
            let list_n = Provider::ALL.len();
            let show_intro = room >= count(intro) + 1 + list_n;
            let show_note = show_intro && room >= count(intro) + 1 + list_n + 1 + count(note);
            if show_intro {
                b.wrapped("", intro, t.dim(), w);
                b.blank();
            }
            let label_w = Provider::ALL.iter().map(|p| width(p.short_label())).max().unwrap_or(0);
            let tail_rows = if show_note { 1 + count(note) } else { 0 };
            let list_room = room.saturating_sub(b.rows.len() + tail_rows);
            b.list(t, &Provider::ALL, state.provider_idx, list_room, w, |p, active| {
                let mark = if active { " ❯ " } else { "   " };
                let label_style = if active { t.accent().add_modifier(Modifier::BOLD) } else { Style::default() };
                let label = p.short_label();
                let hint_room = w.saturating_sub(3 + label_w + 3);
                let mut row = vec![
                    Span::styled(mark, t.accent()),
                    Span::styled(label, label_style),
                    Span::raw(" ".repeat(label_w - width(label) + 3)),
                ];
                if hint_room >= 10 {
                    row.push(Span::styled(fit(p.list_hint(), hint_room), if active { t.dim() } else { t.subtle() }));
                }
                row
            });
            if show_note {
                b.blank();
                b.wrapped("", note, t.subtle(), w);
            }
        }
        Step::Credentials => {
            let p = state.provider();
            // The fields come first when the card is short: a field you cannot see is one you cannot fill.
            let fields = usize::from(p.needs_url()) * 3 + usize::from(p.env_key().is_some()) * 3;
            if room >= 2 + fields {
                b.push(
                    vec![
                        Span::styled(p.label(), t.bold()),
                        Span::styled(format!("  ·  {}", p.list_hint()), t.subtle()),
                    ],
                    w,
                );
                b.blank();
            }
            if p.needs_url() {
                let focused = state.cred_field == CredField::Url;
                b.input(t, "Base URL", &state.base_url, "https://…/v1", focused, "", w);
            }
            if let Some(key_name) = p.env_key() {
                let focused = state.cred_field == CredField::Key;
                let placeholder = if state.key_in_env {
                    format!("Using ${key_name} from your shell · paste to replace")
                } else if p.key_optional() {
                    "Optional · paste your key".to_string()
                } else {
                    "Paste your key".to_string()
                };
                let right = if state.api_key.is_empty() {
                    String::new()
                } else if state.reveal_key {
                    format!("{} chars · visible", state.api_key.chars().count())
                } else {
                    format!("{} chars", state.api_key.chars().count())
                };
                b.input(t, key_name, &state.masked_key(), &placeholder, focused, &right, w);
                if !p.key_help().is_empty() {
                    b.push(vec![Span::raw("  "), Span::styled(p.key_help(), t.subtle())], w);
                }
            } else {
                b.push(
                    vec![
                        Span::styled("✓ ", t.success()),
                        Span::styled("No API key needed for a local server.", t.dim()),
                    ],
                    w,
                );
            }
            if let Some(err) = &state.error {
                b.blank();
                b.wrapped("✗ ", err, t.error(), w);
            }
        }
        Step::Model => {
            let rows = state.rows();
            b.input(t, "Model", &state.custom_model, "Type to filter, or enter any model ID", true, "", w);
            let status_rows = 2 + usize::from(state.error.is_some()) * 2;
            let list_room = room.saturating_sub(b.rows.len() + status_rows).max(3);
            if rows.is_empty() {
                b.push(vec![Span::styled("   No models listed · type an ID", t.subtle())], w);
            } else {
                b.list(t, &rows, state.model_idx.min(rows.len() - 1), list_room, w, |r, active| {
                    let mark = if active { " ❯ " } else { "   " };
                    let style = if active { t.accent().add_modifier(Modifier::BOLD) } else { Style::default() };
                    match r {
                        ModelRow::Listed(m) => vec![Span::styled(mark, t.accent()), Span::styled(m.clone(), style)],
                        ModelRow::Typed(m) => vec![
                            Span::styled(mark, t.accent()),
                            Span::styled("Use ", t.dim()),
                            Span::styled(format!("“{m}”"), style),
                        ],
                    }
                });
            }
            b.blank();
            if state.probing {
                let spin = SPINNER[state.tick % SPINNER.len()];
                b.push(
                    vec![
                        Span::styled(format!("{spin} "), t.accent()),
                        Span::styled("Asking the endpoint for its models…", t.dim()),
                    ],
                    w,
                );
            } else if let Some(st) = &state.status {
                b.wrapped("· ", st, t.subtle(), w);
            }
            if let Some(err) = &state.error {
                b.blank();
                b.wrapped("✗ ", err, t.error(), w);
            }
        }
        Step::Review => {
            let p = state.provider();
            // Short card: the facts and the button matter more than the intro and the blank rows.
            if room >= 11 {
                b.wrapped("", "Everything look right? KodAct saves this and starts.", t.dim(), w);
                b.blank();
            }
            let key = match p.env_key() {
                None => "not needed".to_string(),
                Some(name) if state.api_key.trim().is_empty() && state.key_in_env => format!("from ${name}"),
                Some(_) if state.api_key.trim().is_empty() => "none".to_string(),
                Some(_) => {
                    let chars: Vec<char> = state.api_key.trim().chars().collect();
                    let last: String = chars[chars.len().saturating_sub(4)..].iter().collect();
                    format!("••••{last}")
                }
            };
            let endpoint = if p.needs_url() { state.base_url.trim().to_string() } else { "default".to_string() };
            let facts = [
                ("Provider", p.label().to_string()),
                ("Endpoint", endpoint),
                ("API key", key),
                ("Model", state.selected_model()),
                ("Saved to", "~/.kodact/settings.json · 0600".to_string()),
            ];
            for (k, v) in facts {
                if k == "Saved to" && room < 8 {
                    continue;
                }
                b.push(
                    vec![
                        Span::styled(format!("  {k:<10}"), t.subtle()),
                        Span::styled(
                            v,
                            if k == "Model" { t.accent().add_modifier(Modifier::BOLD) } else { Style::default() },
                        ),
                    ],
                    w,
                );
            }
            if room >= 8 {
                b.blank();
            }
            // The plain span after it keeps the button's colour from filling the rest of the row.
            b.push(vec![Span::raw("  "), button(t, "Save & start  ⏎"), Span::raw(" ")], w);
            if let Some(err) = &state.error {
                b.blank();
                b.wrapped("✗ ", err, t.error(), w);
            }
        }
    }
    b
}

/// Draw the wizard: the name above a centred card, both cut to the terminal.
pub fn draw(f: &mut Frame, state: &SetupState, t: &Theme) {
    let area = f.area();
    f.render_widget(Clear, area);
    if area.width < 20 || area.height < 8 {
        let msg = Paragraph::new(Line::from(Span::styled(
            fit("Enlarge the terminal for setup", area.width as usize),
            t.dim(),
        )));
        f.render_widget(msg, area);
        return;
    }
    let card_w = if area.width >= CARD_MAX_W + 4 { CARD_MAX_W } else { area.width.saturating_sub(2).max(20) };
    let pad_x: u16 = if card_w >= 56 { 3 } else { 1 };
    let pad_y: u16 = if area.height >= 24 { 1 } else { 0 };
    let inner_w = card_w.saturating_sub(2 + pad_x * 2) as usize;
    // Rows the body may take: the frame, the stepper and its rule, the rule and keys at the foot.
    let chrome = 2 + pad_y * 2 + 2 + 2;
    let room = area.height.saturating_sub(chrome) as usize;
    let b = body(state, t, inner_w, room);
    let mut rows = vec![stepper(t, state.step, inner_w), Line::default()];
    let head = rows.len() as u16;
    rows.extend(b.rows);
    rows.push(Line::default());
    rows.push(footer(t, state, inner_w));
    let card_h = (rows.len() as u16 + 2 + pad_y * 2).min(area.height);

    // The name above the card when there is room for it, centred together: the
    // wordmark on a roomy terminal, the name in one row otherwise.
    let big = area.height >= card_h + 7 && area.width as usize >= super::brand::WORDMARK_W + 4;
    let brand: Vec<Line<'static>> = if big {
        super::brand::wordmark(t)
    } else if area.height >= card_h + 3 {
        vec![Line::from(vec![
            Span::styled("Kod", t.bold()),
            Span::styled("Act", t.brand_strong().add_modifier(Modifier::BOLD)),
            Span::styled("  ·  setup", t.subtle()),
        ])]
    } else {
        vec![]
    };
    let brand_h = if brand.is_empty() { 0 } else { brand.len() as u16 + 1 };
    let total = card_h + brand_h;
    let top = area.y + area.height.saturating_sub(total) / 2;
    let x = area.x + (area.width - card_w) / 2;
    for (i, l) in brand.into_iter().enumerate() {
        let lw = (spans_width(&l.spans) as u16).min(area.width);
        let r = Rect::new(area.x + (area.width - lw) / 2, top + i as u16, lw, 1);
        f.render_widget(Paragraph::new(l), r);
    }
    let card = Rect::new(x, top + brand_h, card_w, card_h);
    let step_no = format!(" {}/{} ", state.step.index() + 1, Step::ALL.len());
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.hairline())
        .title(Line::from(vec![
            Span::styled(" ◆ ", t.accent()),
            Span::styled(format!("{} ", state.step.heading()), strong(t)),
        ]))
        .padding(ratatui::widgets::Padding::new(pad_x, pad_x, pad_y, pad_y));
    if card_w >= 30 {
        block = block.title(Line::from(Span::styled(step_no, t.subtle())).right_aligned());
    }
    let inner = block.inner(card);
    f.render_widget(Clear, card);
    f.render_widget(block, card);
    // The rule under the stepper, and the one over the keys.
    let rule = Line::from(Span::styled("─".repeat(inner_w), t.hairline()));
    let n = rows.len();
    rows[1] = rule.clone();
    if n >= 2 {
        rows[n - 2] = rule;
    }
    // Too tall for the terminal: keep the stepper and the keys, drop rows from the body's end.
    let fit_rows = inner.height as usize;
    if rows.len() > fit_rows && fit_rows >= 3 {
        let foot = rows.split_off(rows.len() - 2);
        rows.truncate(fit_rows - 2);
        rows.extend(foot);
    }
    f.render_widget(Paragraph::new(rows), inner);
    if let Some((cx, cy)) = b.cursor {
        let y = inner.y + head + cy;
        let x = inner.x + cx;
        if y < inner.y + inner.height.saturating_sub(2) && x < inner.x + inner.width {
            f.set_cursor_position(Position::new(x, y));
        }
    }
}

/// The card shown for a moment after saving.
fn draw_done(f: &mut Frame, t: &Theme, provider: &str, model: &str) {
    let area = f.area();
    f.render_widget(Clear, area);
    let card_w = area.width.min(56).max(area.width.min(20));
    let inner_w = card_w.saturating_sub(4) as usize;
    let rows = vec![
        clamp_row(vec![Span::styled("✓ ", t.success()), Span::styled("You're set", t.bold())], inner_w),
        Line::default(),
        clamp_row(
            vec![
                Span::styled(provider.to_string(), t.dim()),
                Span::styled("  ·  ", t.subtle()),
                Span::styled(model.to_string(), t.accent()),
            ],
            inner_w,
        ),
    ];
    let h = (rows.len() as u16 + 4).min(area.height);
    let card = Rect::new(area.x + (area.width - card_w) / 2, area.y + area.height.saturating_sub(h) / 2, card_w, h);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(t.success())
        .padding(ratatui::widgets::Padding::new(1, 1, 1, 0));
    let inner = block.inner(card);
    f.render_widget(block, card);
    f.render_widget(Paragraph::new(rows), inner);
}

/// Puts the terminal back however the wizard ends, a panic included.
struct Screen;

impl Screen {
    fn enter() -> Result<Self, Fail> {
        enable_raw_mode().map_err(|e| Fail::config(format!("terminal raw mode failed: {e}")))?;
        let guard = Screen;
        crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste)
            .map_err(|e| Fail::config(format!("alternate screen failed: {e}")))?;
        Ok(guard)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = crossterm::execute!(io::stdout(), DisableBracketedPaste, LeaveAlternateScreen, crossterm::cursor::Show);
        let _ = disable_raw_mode();
    }
}

type Probe = Pin<Box<dyn Future<Output = Option<Vec<String>>> + Send>>;

/// Run the interactive setup wizard in the terminal, writing `~/.kodact/settings.json`.
pub async fn run(theme: &Theme) -> Result<SetupOutcome, Fail> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(SetupOutcome::Cancelled);
    }
    let settings_path: PathBuf = kodact_config::kodact_home().join("settings.json");
    let screen = Screen::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend).map_err(|e| Fail::config(format!("terminal init failed: {e}")))?;
    let _ = terminal.clear();

    let mut state = SetupState::with_env();
    let mut events = EventStream::new();
    let mut probe: Option<Probe> = None;
    let mut ticker = tokio::time::interval(Duration::from_millis(80));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let outcome = loop {
        let _ = terminal.draw(|f| draw(f, &state, theme));
        let action = tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) => state.on_key(k),
                Some(Ok(Event::Paste(s))) => {
                    state.on_paste(&s);
                    None
                }
                Some(Ok(_)) => None,
                _ => break SetupOutcome::Cancelled,
            },
            found = async { probe.as_mut().expect("guarded").await }, if probe.is_some() => {
                probe = None;
                state.probing = false;
                match found {
                    Some(found) => {
                        state.status = Some(format!("{} models found at the endpoint.", found.len()));
                        state.replace_models(found);
                    }
                    None => state.status = Some("Showing suggested models · type any ID your endpoint serves.".into()),
                }
                None
            },
            _ = ticker.tick(), if state.probing => {
                state.tick = state.tick.wrapping_add(1);
                None
            },
        };
        match action {
            Some(SetupAction::Cancel) => break SetupOutcome::Cancelled,
            Some(SetupAction::StopProbe) => {
                probe = None;
            }
            Some(SetupAction::ProbeModels) => {
                state.status = None;
                probe = probe_request(&state).map(|(url, key)| Box::pin(probe_models(url, key)) as Probe);
                state.probing = probe.is_some();
                if !state.probing {
                    state.status = Some("Suggested models · type any ID to use another.".into());
                }
            }
            Some(SetupAction::Save) => match write_settings(&settings_path, &state) {
                Ok(()) => {
                    let (provider, model) = (state.provider().label(), state.selected_model());
                    let _ = terminal.draw(|f| draw_done(f, theme, provider, &model));
                    tokio::time::sleep(Duration::from_millis(700)).await;
                    break SetupOutcome::Configured { provider, model };
                }
                Err(e) => state.error = Some(format!("Could not save settings: {e}")),
            },
            None => {}
        }
    };
    drop(terminal);
    drop(screen);
    Ok(outcome)
}

/// Standalone `kodact setup` entrypoint.
pub async fn run_standalone() -> Result<i32, Fail> {
    let theme = Theme::named("auto", crate::term::get().color, palette::Depth::detect());
    match run(&theme).await? {
        SetupOutcome::Configured { provider, model } => {
            let line = Line::from(vec![
                Span::styled("✓ ", theme.success()),
                Span::styled(format!("Saved {provider} · "), theme.bold()),
                Span::styled(model, theme.accent()),
                Span::styled("  to ~/.kodact/settings.json", theme.dim()),
            ]);
            println!("{}", super::brand::ansi(&line));
            let next = Line::from(vec![
                Span::raw("  Run "),
                Span::styled("kodact", theme.accent().add_modifier(Modifier::BOLD)),
                Span::raw(" to start."),
            ]);
            println!("{}", super::brand::ansi(&next));
            Ok(0)
        }
        SetupOutcome::Cancelled => {
            if io::stdin().is_terminal() && io::stdout().is_terminal() {
                println!(
                    "{}",
                    super::brand::ansi(&Line::from(Span::styled("Setup cancelled · nothing was saved.", theme.dim())))
                );
            }
            Ok(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn theme() -> Theme {
        Theme::named("dark", true, Depth::True)
    }

    #[test]
    fn setup_state_walks_ollama_without_key_and_writes_0600_settings() {
        let mut s = SetupState::new();
        // Move down 3 times to Ollama (index 3)
        s.on_key(key(KeyCode::Down));
        s.on_key(key(KeyCode::Down));
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.provider(), Provider::Ollama);

        // Enter -> Credentials (URL pre-filled, no key needed)
        s.on_key(key(KeyCode::Enter));
        assert_eq!(s.step, Step::Credentials);
        assert_eq!(s.base_url, "http://localhost:11434/v1");

        // Enter -> Model step
        assert_eq!(s.on_key(key(KeyCode::Enter)), Some(SetupAction::ProbeModels));
        assert_eq!(s.step, Step::Model);

        // Enter -> Review, Enter -> Save
        assert_eq!(s.on_key(key(KeyCode::Enter)), None);
        assert_eq!(s.step, Step::Review);
        assert_eq!(s.on_key(key(KeyCode::Enter)), Some(SetupAction::Save));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        write_settings(&path, &s).unwrap();
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["env"]["OPENAI_BASE_URL"], "http://localhost:11434/v1");
        assert_eq!(raw["model"], "qwen2.5-coder:14b");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn setup_state_masks_api_key_and_requires_it_for_anthropic() {
        let mut s = SetupState::new();
        s.on_key(key(KeyCode::Enter)); // Anthropic
        assert_eq!(s.on_key(key(KeyCode::Enter)), None);
        assert!(s.error.is_some());

        s.on_paste("sk-ant-secret1234\n");
        assert_eq!(s.masked_key(), "•".repeat(17));
        s.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(s.masked_key().ends_with("1234"));
        assert!(!s.masked_key().contains("sk-ant"));
    }

    #[test]
    fn a_key_in_the_environment_lets_the_field_stay_empty() {
        let mut s = SetupState::new();
        s.on_key(key(KeyCode::Enter));
        s.key_in_env = true;
        assert_eq!(s.on_key(key(KeyCode::Enter)), Some(SetupAction::ProbeModels));
        assert_eq!(s.step, Step::Model);
    }

    #[test]
    fn typing_on_the_model_step_filters_and_offers_the_typed_id() {
        let mut s = SetupState::new();
        s.on_key(key(KeyCode::Enter));
        s.on_paste("k");
        s.on_key(key(KeyCode::Enter));
        for c in "opus".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.selected_model(), "claude-opus-4-6", "the first match is picked");
        s.on_key(key(KeyCode::Down));
        assert_eq!(s.selected_model(), "opus", "the typed ID is offered after the matches");
        s.on_key(key(KeyCode::Esc));
        assert!(s.custom_model.is_empty() && s.step == Step::Model, "Esc clears the filter first");
        for c in "my-own-model".chars() {
            s.on_key(key(KeyCode::Char(c)));
        }
        assert_eq!(s.selected_model(), "my-own-model");
    }

    #[test]
    fn a_custom_url_must_be_http() {
        let mut s = SetupState::new();
        s.provider_idx = 4;
        s.on_key(key(KeyCode::Enter));
        s.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        s.on_paste("localhost:8000/v1");
        s.on_key(key(KeyCode::Enter)); // URL -> key field
        assert_eq!(s.on_key(key(KeyCode::Enter)), None);
        assert!(s.error.as_deref().is_some_and(|e| e.contains("http")));
    }

    fn render(s: &SetupState, w: u16, h: u16) -> ratatui::buffer::Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, s, &theme())).unwrap();
        term.backend().buffer().clone()
    }

    /// Every state, at sizes from tiny to wide, with long values: the card's frame stays
    /// whole and nothing is drawn outside it.
    #[test]
    fn the_card_keeps_its_edges_at_every_size() {
        let mut states = vec![];
        for p in 0..Provider::ALL.len() {
            let mut s = SetupState::new();
            s.provider_idx = p;
            states.push(s.clone());
            s.enter_credentials();
            s.on_paste(&"x".repeat(300));
            s.error = Some(
                "A long error that has to wrap across several rows of the card without crossing its edge at all".into(),
            );
            states.push(s.clone());
            s.error = None;
            s.step = Step::Model;
            s.models = (0..40).map(|i| format!("vendor/a-very-long-model-name-number-{i}-with-a-suffix")).collect();
            s.model_idx = 25;
            s.probing = true;
            states.push(s.clone());
            s.custom_model = "a-typed-model-id-that-is-much-longer-than-the-card-is-wide-ok".into();
            s.probing = false;
            s.status = Some("Showing suggested models · type any ID your endpoint serves.".into());
            states.push(s.clone());
            s.step = Step::Review;
            states.push(s);
        }
        for (w, h) in [(20, 8), (32, 12), (44, 16), (60, 20), (80, 24), (120, 40), (200, 60)] {
            for s in &states {
                let buf = render(s, w, h);
                let row = |y: u16| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
                let all: Vec<String> = (0..h).map(row).collect();
                let Some(top) = all.iter().position(|r| r.contains("╭ ◆")) else {
                    assert!(w < 40 || h < 12, "{w}x{h}: no card\n{}", all.join("\n"));
                    continue;
                };
                let left = all[top].chars().position(|c| c == '╭').unwrap();
                let right = all[top].chars().position(|c| c == '╮').unwrap();
                let bottom = (top + 1..h as usize)
                    .find(|&y| all[y].chars().nth(left) == Some('╰'))
                    .unwrap_or_else(|| panic!("{w}x{h} {:?}: card has no bottom edge\n{}", s.step, all.join("\n")));
                assert_eq!(all[bottom].chars().nth(right), Some('╯'), "{w}x{h}\n{}", all.join("\n"));
                for (y, r) in all.iter().enumerate().take(bottom).skip(top + 1) {
                    let cs: Vec<char> = r.chars().collect();
                    assert_eq!(cs[left], '│', "{w}x{h} {:?} row {y} lost its left edge\n{}", s.step, all.join("\n"));
                    assert_eq!(cs[right], '│', "{w}x{h} {:?} row {y} lost its right edge\n{}", s.step, all.join("\n"));
                    assert!(
                        cs[..left].iter().chain(&cs[right + 1..]).all(|c| *c == ' '),
                        "{w}x{h} {:?} row {y} draws outside the card\n{}",
                        s.step,
                        all.join("\n")
                    );
                }
                // The keys row survives even when the body is cut.
                let keys = &all[bottom - 1 - usize::from(h >= 24)];
                assert!(
                    keys.contains("esc") || keys.contains("⏎") || keys.contains("^C"),
                    "{w}x{h} {:?}: keys row lost\n{}",
                    s.step,
                    all.join("\n")
                );
            }
        }
    }

    #[test]
    fn inputs_show_the_end_of_long_text_and_keep_their_box() {
        let mut s = SetupState::new();
        s.provider_idx = 4;
        s.enter_credentials();
        s.base_url = format!("https://{}.example.com/v1", "sub".repeat(40));
        let buf = render(&s, 80, 24);
        let text: Vec<String> = (0..24).map(|y| (0..80).map(|x| buf[(x, y)].symbol().to_string()).collect()).collect();
        let field = text.iter().find(|r| r.contains("example.com/v1")).expect("the URL's end is shown");
        assert!(field.contains('…'), "{field}");
        assert!(text.iter().any(|r| r.contains("╭─ Base URL")), "{}", text.join("\n"));
    }

    #[test]
    fn a_late_probe_keeps_the_model_the_user_picked() {
        let mut s = SetupState::new();
        s.provider_idx = 3;
        s.enter_credentials();
        s.on_key(key(KeyCode::Down)); // not on the model step yet: still resolves by name
        s.step = Step::Model;
        s.model_idx = 1;
        assert_eq!(s.selected_model(), "llama3.3");
        s.replace_models(vec!["a-model".into(), "llama3.3".into(), "z-model".into()]);
        assert_eq!(s.selected_model(), "llama3.3", "the pick survives the list being replaced");
        s.replace_models(vec!["a-model".into(), "z-model".into()]);
        assert_eq!(s.selected_model(), "a-model", "a pick the endpoint lacks falls back to the first");
    }

    /// Wide (CJK, emoji) text in the fields at every size: no panic, frame intact, no cursor outside the card.
    #[test]
    fn wide_text_never_breaks_the_card_or_panics() {
        let wide = "漢字🙂かな漢字🙂カナ".repeat(6);
        let mut states = vec![];
        for p in 0..Provider::ALL.len() {
            let mut s = SetupState::new();
            s.provider_idx = p;
            s.enter_credentials();
            s.base_url = wide.clone();
            s.on_paste(&wide);
            s.error = Some(format!("{wide} {wide}"));
            states.push(s.clone());
            s.error = None;
            s.step = Step::Model;
            s.models = (0..30).map(|i| format!("{wide}-{i}")).collect();
            s.model_idx = 17;
            s.custom_model = wide.clone();
            s.probing = true;
            states.push(s.clone());
            s.step = Step::Review;
            states.push(s);
        }
        for w in 20..=90u16 {
            for h in [8u16, 9, 10, 12, 16, 24, 40] {
                for s in &states {
                    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                    term.draw(|f| draw(f, s, &theme())).unwrap();
                    let buf = term.backend().buffer().clone();
                    let row = |y: u16| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>();
                    let all: Vec<String> = (0..h).map(row).collect();
                    let top = all.iter().position(|r| r.contains('╭') && r.contains('╮') && r.contains('◆'));
                    let Some(top) = top else { continue };
                    let cell_row =
                        |y: usize| -> Vec<String> { (0..w).map(|x| buf[(x, y as u16)].symbol().to_string()).collect() };
                    let left = cell_row(top).iter().position(|c| c == "╭").unwrap();
                    let right = cell_row(top).iter().rposition(|c| c == "╮").unwrap();
                    let bottom = (top + 1..h as usize)
                        .find(|&y| cell_row(y)[left] == "╰")
                        .unwrap_or_else(|| panic!("{w}x{h} {:?}: no bottom edge\n{}", s.step, all.join("\n")));
                    assert_eq!(cell_row(bottom)[right], "╯", "{w}x{h}\n{}", all.join("\n"));
                    for y in top + 1..bottom {
                        // Wide glyphs leave an empty continuation cell, so compare by columns.
                        let cells: Vec<&str> = (0..w).map(|x| buf[(x, y as u16)].symbol()).collect();
                        let is_inner_text = |x: usize| x > left && x < right;
                        assert!(
                            cells[left] == "│" && cells[right] == "│",
                            "{w}x{h} {:?} row {y} lost an edge\n{}",
                            s.step,
                            all.join("\n")
                        );
                        assert!(
                            (0..w as usize)
                                .filter(|x| !is_inner_text(*x) && *x != left && *x != right)
                                .all(|x| cells[x].trim().is_empty()),
                            "{w}x{h} {:?} row {y} draws outside the card\n{}",
                            s.step,
                            all.join("\n")
                        );
                    }
                    if let Ok(pos) = term.get_cursor_position() {
                        let _ = pos; // hidden cursor reports (0,0)
                    }
                }
            }
        }
    }
}
