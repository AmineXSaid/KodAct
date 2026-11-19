//! Interactive local BYOK onboarding wizard (`kodact setup` and automatic first-run).
//!
//! Writes provider settings (`env.<KEY>`, `model`, and optional `env.OPENAI_BASE_URL` /
//! `env.ANTHROPIC_BASE_URL`) directly to `~/.kodact/settings.json` with `0o600`
//! permissions on Unix—zero servers, zero accounts.

use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Terminal;
use serde_json::{json, Map, Value};

use super::palette;
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

    pub fn hint(self) -> &'static str {
        match self {
            Provider::Anthropic => "Claude models · ANTHROPIC_API_KEY",
            Provider::OpenAi => "GPT / o-series · OPENAI_API_KEY",
            Provider::Gemini => "Gemini models · GEMINI_API_KEY",
            Provider::Ollama => "Local server at http://localhost:11434/v1 · no key needed",
            Provider::Custom => "OpenRouter, Groq, vLLM, LM Studio · OPENAI_BASE_URL",
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredField {
    Url,
    Key,
}

/// Pure state machine for the 3-step onboarding wizard.
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
    pub custom_model: String,
    pub probing: bool,
    pub status: Option<String>,
    pub error: Option<String>,
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
        }
    }

    pub fn provider(&self) -> Provider {
        Provider::ALL[self.provider_idx.min(Provider::ALL.len() - 1)]
    }

    pub fn selected_model(&self) -> String {
        let trimmed = self.custom_model.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        self.models.get(self.model_idx).cloned().unwrap_or_else(|| self.provider().default_models()[0].to_string())
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

    /// Advance from Step 1 to Step 2 with provider defaults.
    pub fn enter_credentials(&mut self) {
        let p = self.provider();
        self.base_url = p.default_base_url().unwrap_or("").to_string();
        self.cred_field = if p.needs_url() { CredField::Url } else { CredField::Key };
        self.models = p.default_models().iter().map(|s| (*s).to_string()).collect();
        self.model_idx = 0;
        self.custom_model.clear();
        self.error = None;
        self.step = Step::Credentials;
    }

    /// Validate Step 2 and advance to Step 3.
    pub fn enter_model(&mut self) -> Result<(), String> {
        let p = self.provider();
        if p.needs_url() && self.base_url.trim().is_empty() {
            return Err("Enter a base URL (for example http://localhost:11434/v1).".to_string());
        }
        if p != Provider::Ollama && p != Provider::Custom && self.api_key.trim().is_empty() {
            return Err(format!("Paste your {} to continue.", p.env_key().unwrap_or("API key")));
        }
        self.error = None;
        self.step = Step::Model;
        Ok(())
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
                    self.provider_idx = self.provider_idx.saturating_sub(1);
                    None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if self.provider_idx + 1 < Provider::ALL.len() {
                        self.provider_idx += 1;
                    }
                    None
                }
                KeyCode::Enter => {
                    self.enter_credentials();
                    None
                }
                _ => None,
            },
            Step::Credentials => match k.code {
                KeyCode::Esc => {
                    self.error = None;
                    self.step = Step::Provider;
                    None
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    if self.provider().needs_url() && self.provider() != Provider::Ollama {
                        self.cred_field = match self.cred_field {
                            CredField::Url => CredField::Key,
                            CredField::Key => CredField::Url,
                        };
                    }
                    None
                }
                KeyCode::Char('r') if ctrl => {
                    self.reveal_key = !self.reveal_key;
                    None
                }
                KeyCode::Enter => {
                    if self.provider() == Provider::Custom && self.cred_field == CredField::Url {
                        self.cred_field = CredField::Key;
                        return None;
                    }
                    match self.enter_model() {
                        Ok(()) => Some(SetupAction::ProbeModels),
                        Err(e) => {
                            self.error = Some(e);
                            None
                        }
                    }
                }
                KeyCode::Backspace => {
                    match self.cred_field {
                        CredField::Url => {
                            self.base_url.pop();
                        }
                        CredField::Key => {
                            self.api_key.pop();
                        }
                    }
                    self.error = None;
                    None
                }
                KeyCode::Char(c) if !ctrl => {
                    match self.cred_field {
                        CredField::Url => self.base_url.push(c),
                        CredField::Key => self.api_key.push(c),
                    }
                    self.error = None;
                    None
                }
                _ => None,
            },
            Step::Model => match k.code {
                KeyCode::Esc => {
                    self.error = None;
                    self.probing = false;
                    self.step = Step::Credentials;
                    None
                }
                KeyCode::Up => {
                    self.model_idx = self.model_idx.saturating_sub(1);
                    None
                }
                KeyCode::Down => {
                    if self.model_idx + 1 < self.models.len() {
                        self.model_idx += 1;
                    }
                    None
                }
                KeyCode::Backspace => {
                    self.custom_model.pop();
                    None
                }
                KeyCode::Char(c) if !ctrl => {
                    self.custom_model.push(c);
                    None
                }
                KeyCode::Enter => Some(SetupAction::Save),
                _ => None,
            },
        }
    }

    pub fn on_paste(&mut self, text: &str) {
        let clean = text.trim();
        match self.step {
            Step::Credentials => match self.cred_field {
                CredField::Url => self.base_url.push_str(clean),
                CredField::Key => self.api_key.push_str(clean),
            },
            Step::Model => self.custom_model.push_str(clean),
            Step::Provider => {}
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupAction {
    ProbeModels,
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

    let env_obj =
        root.entry("env").or_insert_with(|| Value::Object(Map::new())).as_object_mut().cloned().unwrap_or_default();
    let mut env = env_obj;

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

/// Best-effort live `/models` probe for OpenAI-compatible or Ollama endpoints.
async fn probe_models(state: &SetupState) -> Option<Vec<String>> {
    let p = state.provider();
    let url = match p {
        Provider::Ollama | Provider::Custom => {
            format!("{}/models", state.base_url.trim().trim_end_matches('/'))
        }
        Provider::OpenAi => "https://api.openai.com/v1/models".to_string(),
        _ => return None,
    };
    let client = reqwest::Client::builder().timeout(Duration::from_secs(4)).build().ok()?;
    let mut req = client.get(&url);
    if !state.api_key.trim().is_empty() {
        req = req.bearer_auth(state.api_key.trim());
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
    ids.truncate(12);
    if ids.is_empty() {
        None
    } else {
        Some(ids)
    }
}

/// Render the setup wizard card.
pub fn render_lines(state: &SetupState, t: &Theme, width: usize) -> Vec<Line<'static>> {
    let bw = width.clamp(48, 76).min(width);
    let inner = bw.saturating_sub(4).max(1);
    let rule = "─".repeat(bw.saturating_sub(4));
    let step_num = match state.step {
        Step::Provider => 1,
        Step::Credentials => 2,
        Step::Model => 3,
    };
    let step_title = match state.step {
        Step::Provider => "Choose Provider",
        Step::Credentials => "Endpoint & API Key",
        Step::Model => "Default Model",
    };

    let mut out = vec![
        Line::from(vec![
            Span::styled("┌─", t.accent()),
            Span::styled(rule.clone(), t.hairline()),
            Span::styled("─┐", t.accent()),
        ]),
        Line::from(vec![
            Span::raw("  "),
            Span::styled("KodAct Setup", t.brand_strong().add_modifier(Modifier::BOLD)),
            Span::styled(format!("  ·  Step {step_num} of 3: {step_title}"), t.dim()),
        ]),
        Line::from(""),
    ];

    match state.step {
        Step::Provider => {
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("Saved locally to ~/.kodact/settings.json (0600). No server or account.", t.subtle()),
            ]));
            out.push(Line::from(""));
            for (i, p) in Provider::ALL.iter().enumerate() {
                let active = i == state.provider_idx;
                let mark = if active { "❯ " } else { "  " };
                let label_style = if active { t.accent().add_modifier(Modifier::BOLD) } else { Style::default() };
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(mark, t.accent()),
                    Span::styled(p.label(), label_style),
                    Span::styled(format!("  {}", p.hint()), t.dim()),
                ]));
            }
            out.push(Line::from(""));
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("↑/↓ choose  ·  Enter continue  ·  Esc cancel", t.dim()),
            ]));
        }
        Step::Credentials => {
            let p = state.provider();
            out.push(Line::from(vec![Span::raw("  "), Span::styled(format!("Provider: {}", p.label()), t.bold())]));
            out.push(Line::from(""));
            if p.needs_url() {
                let active = state.cred_field == CredField::Url;
                let mark = if active { "❯ " } else { "  " };
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(mark, t.accent()),
                    Span::styled("Base URL: ", t.dim()),
                    Span::styled(state.base_url.clone(), if active { t.bold() } else { Style::default() }),
                ]));
            }
            if let Some(key_name) = p.env_key() {
                let active = state.cred_field == CredField::Key;
                let mark = if active { "❯ " } else { "  " };
                let shown = if state.api_key.is_empty() { "(paste key here)".to_string() } else { state.masked_key() };
                let val_style = if state.api_key.is_empty() {
                    t.subtle()
                } else if active {
                    t.bold()
                } else {
                    Style::default()
                };
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(mark, t.accent()),
                    Span::styled(format!("{key_name}: "), t.dim()),
                    Span::styled(shown, val_style),
                ]));
            } else {
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled("No API key required for local Ollama.", t.subtle()),
                ]));
            }
            if let Some(err) = &state.error {
                out.push(Line::from(""));
                out.push(Line::from(vec![Span::raw("  "), Span::styled(err.clone(), t.error())]));
            }
            out.push(Line::from(""));
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("Enter continue  ·  Tab switch field  ·  Ctrl+R peek key  ·  Esc back", t.dim()),
            ]));
        }
        Step::Model => {
            if let Some(st) = &state.status {
                out.push(Line::from(vec![Span::raw("  "), Span::styled(st.clone(), t.subtle())]));
            }
            out.push(Line::from(""));
            for (i, m) in state.models.iter().enumerate() {
                let active = i == state.model_idx && state.custom_model.trim().is_empty();
                let mark = if active { "❯ " } else { "  " };
                out.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(mark, t.accent()),
                    Span::styled(
                        m.clone(),
                        if active { t.accent().add_modifier(Modifier::BOLD) } else { Style::default() },
                    ),
                ]));
            }
            out.push(Line::from(""));
            let custom_shown = if state.custom_model.is_empty() {
                "(or type any model ID)".to_string()
            } else {
                state.custom_model.clone()
            };
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("Custom ID: ", t.dim()),
                Span::styled(custom_shown, if state.custom_model.is_empty() { t.subtle() } else { t.bold() }),
            ]));
            if let Some(err) = &state.error {
                out.push(Line::from(vec![Span::raw("  "), Span::styled(err.clone(), t.error())]));
            }
            out.push(Line::from(""));
            out.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("↑/↓ pick  ·  Enter save & launch  ·  Esc back", t.dim()),
            ]));
        }
    }

    out.push(Line::from(vec![
        Span::styled("└─", t.accent()),
        Span::styled(rule, t.hairline()),
        Span::styled("─┘", t.accent()),
    ]));
    let _ = inner;
    out
}

/// Run the interactive setup wizard in the terminal, writing `~/.kodact/settings.json`.
pub async fn run(theme: &Theme) -> Result<SetupOutcome, Fail> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(SetupOutcome::Cancelled);
    }
    let settings_path: PathBuf = kodact_config::kodact_home().join("settings.json");
    enable_raw_mode().map_err(|e| Fail::config(format!("terminal raw mode failed: {e}")))?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)
        .map_err(|e| Fail::config(format!("alternate screen failed: {e}")))?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).map_err(|e| Fail::config(format!("terminal init failed: {e}")))?;

    let mut state = SetupState::new();
    let mut events = EventStream::new();
    let outcome = loop {
        let _ = terminal.draw(|f| {
            let area = f.area();
            let lines = render_lines(&state, theme, area.width as usize);
            f.render_widget(Paragraph::new(lines), area);
        });
        let Some(Ok(ev)) = events.next().await else {
            break SetupOutcome::Cancelled;
        };
        let action = match ev {
            Event::Key(k) => state.on_key(k),
            Event::Paste(s) => {
                state.on_paste(&s);
                None
            }
            _ => None,
        };
        match action {
            Some(SetupAction::Cancel) => break SetupOutcome::Cancelled,
            Some(SetupAction::ProbeModels) => {
                state.probing = true;
                state.status = Some("Checking endpoint /models...".to_string());
                let _ = terminal.draw(|f| {
                    let area = f.area();
                    let lines = render_lines(&state, theme, area.width as usize);
                    f.render_widget(Paragraph::new(lines), area);
                });
                if let Some(found) = probe_models(&state).await {
                    state.models = found;
                    state.model_idx = 0;
                    state.status = Some(format!("Found {} models from endpoint.", state.models.len()));
                } else {
                    state.status = Some("Using default model list (or type any model ID).".to_string());
                }
                state.probing = false;
            }
            Some(SetupAction::Save) => match write_settings(&settings_path, &state) {
                Ok(()) => {
                    break SetupOutcome::Configured {
                        provider: state.provider().label(),
                        model: state.selected_model(),
                    };
                }
                Err(e) => state.error = Some(format!("Could not save settings: {e}")),
            },
            None => {}
        }
    };

    let _ = disable_raw_mode();
    let _ = crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen);
    Ok(outcome)
}

/// Standalone `kodact setup` entrypoint.
pub async fn run_standalone() -> Result<i32, Fail> {
    let theme = Theme::named("auto", crate::term::get().color, palette::Depth::detect());
    match run(&theme).await? {
        SetupOutcome::Configured { provider, model } => {
            println!("Saved {provider} ({model}) to ~/.kodact/settings.json");
            Ok(0)
        }
        SetupOutcome::Cancelled => Ok(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
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

        // Enter -> Save
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

        s.on_paste("sk-ant-secret1234");
        assert_eq!(s.masked_key(), "•".repeat(17));
        s.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(s.masked_key().ends_with("1234"));
        assert!(!s.masked_key().contains("sk-ant"));
    }
}
