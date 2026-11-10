//! Which endpoint this session talks to, with which key, and where each came
//! from. One resolver for the provider, `kodact doctor` and `/status`.
//!
//! Rules:
//! - Keys are per provider. `KODACT_API_KEY` / `KODACT_AUTH_TOKEN` (and
//!   `apiKeyHelper`) go only to the Messages endpoint; `KODACT_OPENAI_API_KEY`
//!   (and `openai.apiKeyHelper`) only to the OpenAI-compatible one;
//!   `KODACT_GEMINI_API_KEY` (and `gemini.apiKeyHelper`) only to the Gemini
//!   one. A key is never sent to another host: a mix-up is named instead.
//! - The Gemini endpoint is chosen by `KODACT_GEMINI_BASE_URL` or
//!   `gemini.baseUrl`, or by `KODACT_GEMINI_API_KEY` alone (Google AI Studio)
//!   when no Messages key is set.
//! - A project's checked-in `.kodact/settings.json` can't choose where the key
//!   goes (`baseUrl`, `openai.baseUrl`) or run a key helper; the user's own
//!   settings (user, local, `--settings`, managed) can.
//! - Without a key, the Messages path refuses to start (it never accepts
//!   keyless requests); the OpenAI path starts, silently for a local server,
//!   with a warning for a remote one.

use std::path::PathBuf;

use kodact_api::auth::{is_local_url, parse_base_url, Backend};
use kodact_config::{LoadedSettings, SettingSource};

/// The layers that may pick the endpoint or run a key helper.
const TRUSTED: [SettingSource; 4] =
    [SettingSource::User, SettingSource::Local, SettingSource::Flag, SettingSource::Managed];

/// Settings that send the key somewhere or run a command for it.
const SENSITIVE: [(&str, &str); 6] = [
    ("/baseUrl", "baseUrl"),
    ("/openai/baseUrl", "openai.baseUrl"),
    ("/gemini/baseUrl", "gemini.baseUrl"),
    ("/apiKeyHelper", "apiKeyHelper"),
    ("/openai/apiKeyHelper", "openai.apiKeyHelper"),
    ("/gemini/apiKeyHelper", "gemini.apiKeyHelper"),
];

/// Google AI Studio's Gemini API, when no `KODACT_GEMINI_BASE_URL` is set.
pub const GEMINI_DEFAULT_URL: &str = "https://generativelanguage.googleapis.com/v1beta";

#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub backend: Option<Backend>,
    /// The base URL (cleaned), or `None` for the backend's default host.
    pub url: Option<String>,
    /// The variable or setting that chose the URL.
    pub url_from: Option<&'static str>,
    /// The settings file, when a setting chose the URL.
    pub url_file: Option<PathBuf>,
    /// The URL is on this machine or a private network.
    pub local: bool,
    /// The key: `x-api-key` on Messages, the bearer token on OpenAI.
    pub key: Option<String>,
    pub key_var: Option<&'static str>,
    /// `KODACT_AUTH_TOKEN` (Messages only).
    pub token: Option<String>,
    /// A key helper to run when no key is set: (command, setting, file).
    pub helper: Option<(String, &'static str, Option<PathBuf>)>,
    /// Variables that were wrapped in quotes (removed).
    pub quoted: Vec<&'static str>,
    /// Keys for the other provider that are set but never sent here.
    pub others: Vec<&'static str>,
    /// Shown once at startup.
    pub warnings: Vec<String>,
    /// Configurations that can never work: refuse to start.
    pub problems: Vec<String>,
}

impl Resolution {
    pub fn backend(&self) -> Backend {
        self.backend.unwrap_or(Backend::Messages)
    }

    /// Where the base URL came from: `"KODACT_OPENAI_BASE_URL"` or `"openai.baseUrl in <file>"`.
    pub fn url_origin(&self) -> String {
        match (self.url_from, &self.url_file) {
            (Some(s), Some(f)) => format!("{s} in {}", f.display()),
            (Some(s), None) => s.to_string(),
            (None, _) => "default".into(),
        }
    }
}

/// Trim, and drop one pair of surrounding quotes (what `set X="..."` in cmd.exe
/// leaves). `Ok(None)` when nothing is left; `Err` for a value that can't be a key.
fn clean(var: &str, raw: &str, quoted: &mut bool) -> Result<Option<String>, String> {
    let mut v = raw.trim();
    for q in ['"', '\''] {
        if v.len() >= 2 && v.starts_with(q) && v.ends_with(q) {
            v = v[1..v.len() - 1].trim();
            *quoted = true;
        }
    }
    if v.is_empty() {
        return Ok(None);
    }
    if !v.chars().all(|c| c.is_ascii_graphic()) {
        return Err(format!(
            "{var} can't be used: it contains a space, a control character or a non-ASCII character. A key is one \
             word of plain characters; check how it was set."
        ));
    }
    Ok(Some(v.to_string()))
}

/// The highest-precedence trusted layer with a string at `pointer`.
fn trusted_str(settings: &LoadedSettings, pointer: &str) -> Option<(String, Option<PathBuf>)> {
    settings
        .layers
        .iter()
        .filter(|l| TRUSTED.contains(&l.source))
        .filter_map(|l| l.value.pointer(pointer).and_then(|v| v.as_str()).map(|s| (l.source.rank(), s, &l.path)))
        .max_by_key(|(rank, _, _)| *rank)
        .map(|(_, s, p)| (s.to_string(), p.clone()))
}

/// Resolve the endpoint and key. Pure: reads `env` and `settings` only.
pub fn resolve(settings: &LoadedSettings, env: &dyn Fn(&str) -> Option<String>) -> Resolution {
    let mut r = Resolution::default();
    let var = |name: &'static str, r: &mut Resolution| -> Option<String> {
        let raw = env(name)?;
        let mut quoted = false;
        match clean(name, &raw, &mut quoted) {
            Ok(v) => {
                if quoted && v.is_some() {
                    r.quoted.push(name);
                }
                v
            }
            Err(p) => {
                r.problems.push(p);
                None
            }
        }
    };

    // Project settings that would choose the endpoint or run a command are ignored.
    for layer in settings.layers.iter().filter(|l| l.source == SettingSource::Project) {
        for (pointer, key) in SENSITIVE {
            if layer.value.pointer(pointer).is_some() {
                let file =
                    layer.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "project settings".into());
                let why = if key.ends_with("Helper") {
                    "a project's shared settings can't run a command to fetch your key"
                } else {
                    "a project's shared settings can't choose where your key is sent"
                };
                r.warnings.push(format!(
                    "Ignored {key} in {file}: {why}. Put it in {} or .kodact/settings.local.json.",
                    kodact_config::config_dir().join("settings.json").display()
                ));
            }
        }
    }

    // 1. The URL, and so the backend.
    let (url, from, file) = if let Some(u) = var("KODACT_OPENAI_BASE_URL", &mut r) {
        r.backend = Some(Backend::OpenAi);
        (Some(u), Some("KODACT_OPENAI_BASE_URL"), None)
    } else if let Some((u, f)) = trusted_str(settings, "/openai/baseUrl") {
        r.backend = Some(Backend::OpenAi);
        (Some(u.trim().to_string()), Some("openai.baseUrl"), f)
    } else if let Some(u) = var("KODACT_GEMINI_BASE_URL", &mut r) {
        r.backend = Some(Backend::Gemini);
        (Some(u), Some("KODACT_GEMINI_BASE_URL"), None)
    } else if let Some((u, f)) = trusted_str(settings, "/gemini/baseUrl") {
        r.backend = Some(Backend::Gemini);
        (Some(u.trim().to_string()), Some("gemini.baseUrl"), f)
    } else if let Some(u) = var("KODACT_BASE_URL", &mut r) {
        r.backend = Some(Backend::Messages);
        (Some(u), Some("KODACT_BASE_URL"), None)
    } else if let Some((u, f)) = trusted_str(settings, "/baseUrl") {
        r.backend = Some(Backend::Messages);
        (Some(u.trim().to_string()), Some("baseUrl"), f)
    } else {
        // A Gemini key (or helper) and no Messages key: Google AI Studio.
        let set = |k: &str| env(k).is_some_and(|v| !v.trim().is_empty());
        let gemini = (set("KODACT_GEMINI_API_KEY") || trusted_str(settings, "/gemini/apiKeyHelper").is_some())
            && !set("KODACT_API_KEY")
            && !set("KODACT_AUTH_TOKEN")
            && trusted_str(settings, "/apiKeyHelper").is_none();
        r.backend = Some(if gemini { Backend::Gemini } else { Backend::Messages });
        (None, None, None)
    };
    r.url_from = from;
    r.url_file = file;
    if let Some(u) = &url {
        match parse_base_url(u) {
            Ok(parsed) => {
                r.local = is_local_url(&parsed);
                let path = parsed.path().trim_end_matches('/');
                if r.backend == Some(Backend::Gemini) && (path.ends_with("/models") || path.contains(':')) {
                    r.warnings.push(format!(
                        "{} should end before /models: KodAct adds /models/<model>:streamGenerateContent itself. \
                         For Google AI Studio it is {GEMINI_DEFAULT_URL}.",
                        r.url_from.unwrap_or("The URL"),
                    ));
                } else if r.backend == Some(Backend::Gemini) && path.ends_with("/openai") {
                    r.warnings.push(format!(
                        "{} is Google's OpenAI-compatible path; the Gemini wire takes the part before /openai. To \
                         use that path, set KODACT_OPENAI_BASE_URL to it instead.",
                        r.url_from.unwrap_or("The URL"),
                    ));
                }
                if path.ends_with("/chat/completions") {
                    r.warnings.push(format!(
                        "{} ends in /chat/completions, which KodAct adds itself, so requests would go to \
                         {}/chat/completions. Set it to the part before /chat/completions.",
                        r.url_from.unwrap_or("The URL"),
                        u.trim_end_matches('/')
                    ));
                }
            }
            Err(_) => r.problems.push(format!(
                "{} is not a valid http:// or https:// URL: \"{}\"",
                r.url_from.unwrap_or("The URL"),
                kodact_config::redact_text(u)
            )),
        }
        r.url = Some(u.trim_end_matches('/').to_string());
    }

    // 2. The key, for this backend only.
    match r.backend() {
        Backend::OpenAi => {
            r.key = var("KODACT_OPENAI_API_KEY", &mut r);
            if r.key.is_some() {
                r.key_var = Some("KODACT_OPENAI_API_KEY");
            } else if let Some((cmd, f)) = trusted_str(settings, "/openai/apiKeyHelper") {
                r.helper = Some((cmd, "openai.apiKeyHelper", f));
            }
            r.others = ["KODACT_API_KEY", "KODACT_AUTH_TOKEN", "OPENAI_API_KEY"]
                .into_iter()
                .filter(|k| env(k).is_some_and(|v| !v.trim().is_empty()))
                .collect();
            if r.key.is_none() && r.helper.is_none() && !r.local && r.problems.is_empty() {
                let url = r.url.clone().unwrap_or_default();
                let base = format!("No key for {url}: KODACT_OPENAI_API_KEY is not set, so requests carry none.");
                r.warnings.push(match r.others.as_slice() {
                    [] => format!("{base} Set it if this endpoint needs a key."),
                    [one] => format!(
                        "{base} {one} is set, but it is never sent to an OpenAI-compatible endpoint; if it holds \
                         this endpoint's key, set KODACT_OPENAI_API_KEY to it."
                    ),
                    many => format!(
                        "{base} {} are set, but they are never sent to an OpenAI-compatible endpoint; if one holds \
                         this endpoint's key, set KODACT_OPENAI_API_KEY to it.",
                        and_list(many)
                    ),
                });
            }
        }
        Backend::Gemini => {
            r.key = var("KODACT_GEMINI_API_KEY", &mut r);
            if r.key.is_some() {
                r.key_var = Some("KODACT_GEMINI_API_KEY");
            } else if let Some((cmd, f)) = trusted_str(settings, "/gemini/apiKeyHelper") {
                r.helper = Some((cmd, "gemini.apiKeyHelper", f));
            }
            r.others =
                ["KODACT_API_KEY", "KODACT_AUTH_TOKEN", "KODACT_OPENAI_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY"]
                    .into_iter()
                    .filter(|k| env(k).is_some_and(|v| !v.trim().is_empty()))
                    .collect();
            let google = r.url.as_deref().is_none_or(|u| {
                parse_base_url(u).ok().and_then(|p| p.host_str().map(|h| h.ends_with("googleapis.com"))) == Some(true)
            });
            if r.key.is_none() && r.helper.is_none() && !r.local && r.problems.is_empty() {
                let url = r.url.clone().unwrap_or_else(|| GEMINI_DEFAULT_URL.to_string());
                let others = match r.others.as_slice() {
                    [] => String::new(),
                    [one] => format!(" {one} is set, but KodAct never sends it to a Gemini endpoint."),
                    many => format!(" {} are set, but KodAct never sends them to a Gemini endpoint.", and_list(many)),
                };
                if google {
                    r.problems.push(format!(
                        "no key for {url}: set KODACT_GEMINI_API_KEY (an API key from \
                         https://aistudio.google.com/apikey), or gemini.apiKeyHelper for Vertex AI.{others}"
                    ));
                } else {
                    r.warnings.push(format!(
                        "No key for {url}: KODACT_GEMINI_API_KEY is not set, so requests carry none.{others}"
                    ));
                }
            }
        }
        Backend::Messages => {
            r.key = var("KODACT_API_KEY", &mut r);
            if r.key.is_some() {
                r.key_var = Some("KODACT_API_KEY");
            }
            r.token = var("KODACT_AUTH_TOKEN", &mut r);
            if r.key.is_none() && r.token.is_none() {
                if let Some((cmd, f)) = trusted_str(settings, "/apiKeyHelper") {
                    r.helper = Some((cmd, "apiKeyHelper", f));
                }
            }
            let openai_key = env("KODACT_OPENAI_API_KEY").is_some_and(|v| !v.trim().is_empty());
            if r.key.is_none() && r.token.is_none() && r.helper.is_none() {
                r.problems.push(if openai_key {
                    "no API key for the Messages API. KODACT_OPENAI_API_KEY is set, but it is only used with an \
                     OpenAI-compatible endpoint: set KODACT_OPENAI_BASE_URL to that endpoint's URL (for example \
                     https://gateway.example.com/v1)."
                        .into()
                } else {
                    "no API key for the Messages API: set KODACT_API_KEY (or KODACT_AUTH_TOKEN for a gateway that \
                     expects a bearer token). To use an OpenAI-compatible endpoint instead, set \
                     KODACT_OPENAI_BASE_URL and KODACT_OPENAI_API_KEY."
                        .into()
                });
            } else if openai_key {
                r.warnings.push(
                    "KODACT_OPENAI_API_KEY is set but unused: it applies only with KODACT_OPENAI_BASE_URL (or \
                     openai.baseUrl)."
                        .into(),
                );
            }
            if env("KODACT_GEMINI_API_KEY").is_some_and(|v| !v.trim().is_empty()) && r.problems.is_empty() {
                r.warnings.push(
                    "KODACT_GEMINI_API_KEY is set but unused: a Messages key or URL is set too, so the Messages API \
                     is used. Set KODACT_GEMINI_BASE_URL (for Google AI Studio, \
                     https://generativelanguage.googleapis.com/v1beta) to use Gemini."
                        .into(),
                );
            }
        }
    }

    // 3. Plain http to a remote host would expose the key.
    if let (Some(u), false) = (&r.url, r.local) {
        if u.starts_with("http://") && (r.key.is_some() || r.token.is_some()) {
            let host = parse_base_url(u).ok().and_then(|p| p.host_str().map(str::to_string)).unwrap_or_default();
            let key_var = r.key_var.unwrap_or("KODACT_AUTH_TOKEN");
            r.warnings.push(format!(
                "{} uses http:// for {host}, so {key_var} would travel unencrypted. Use https:// if the endpoint \
                 supports it.",
                r.url_from.unwrap_or("The URL")
            ));
        }
    }

    // 4. Quotes, and credentials placed where KodAct doesn't read them.
    for q in r.quoted.clone() {
        r.warnings.push(format!(
            "{q} was wrapped in quotes; KodAct removed them. In cmd.exe, write set {q}=value without quotes."
        ));
    }
    for name in [
        "KODACT_API_KEY",
        "KODACT_AUTH_TOKEN",
        "KODACT_OPENAI_API_KEY",
        "KODACT_GEMINI_API_KEY",
        "KODACT_BASE_URL",
        "KODACT_OPENAI_BASE_URL",
        "KODACT_GEMINI_BASE_URL",
    ] {
        if env(name).is_some() {
            continue;
        }
        if let Some(layer) = settings.layers.iter().find(|l| l.value.pointer(&format!("/env/{name}")).is_some()) {
            let file = layer.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "settings".into());
            r.warnings.push(format!(
                "{name} in the env block of {file} only reaches tools; KodAct reads its own endpoint and key from \
                 its environment. Set it in your shell or system environment instead."
            ));
        }
    }
    r
}

/// "a, b and c".
pub(crate) fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => one.to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A key as it may be shown: the last four characters and the length.
pub fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n >= 16 {
        let tail: String = s.chars().skip(n - 4).collect();
        format!("...{tail}, {n} characters")
    } else {
        format!("{n} characters")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodact_config::SettingsLayer;
    use serde_json::json;
    use std::collections::HashMap;

    fn run(vars: &[(&str, &str)], layers: Vec<SettingsLayer>) -> Resolution {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let settings = LoadedSettings { layers, ..Default::default() };
        resolve(&settings, &|k| vars.get(k).cloned())
    }

    fn layer(source: SettingSource, value: serde_json::Value) -> SettingsLayer {
        SettingsLayer { source, path: Some(PathBuf::from(format!("/{source:?}/settings.json"))), value }
    }

    #[test]
    fn kodact_api_key_is_never_used_for_the_openai_endpoint() {
        let r = run(
            &[("KODACT_OPENAI_BASE_URL", "https://gw.example.com/v1"), ("KODACT_API_KEY", "sk-messages-0123456789")],
            vec![],
        );
        assert_eq!(r.backend(), Backend::OpenAi);
        assert!(r.key.is_none() && r.problems.is_empty());
        assert_eq!(
            r.warnings,
            ["No key for https://gw.example.com/v1: KODACT_OPENAI_API_KEY is not set, so requests carry none. \
              KODACT_API_KEY is set, but it is never sent to an OpenAI-compatible endpoint; if it holds this \
              endpoint's key, set KODACT_OPENAI_API_KEY to it."]
        );
        assert!(!format!("{r:?}").contains("sk-messages") || r.key.is_none());
    }

    #[test]
    fn local_openai_server_needs_no_key_and_no_warning() {
        let r = run(&[("KODACT_OPENAI_BASE_URL", "http://localhost:11434/v1"), ("KODACT_API_KEY", "x")], vec![]);
        assert!(r.warnings.is_empty() && r.problems.is_empty(), "{r:?}");
        assert!(r.local);
    }

    #[test]
    fn quoted_and_padded_values_are_cleaned() {
        let r = run(
            &[
                ("KODACT_OPENAI_BASE_URL", "\"https://gw.example.com/v1\""),
                ("KODACT_OPENAI_API_KEY", "  \"sk-abc0123456789\" "),
            ],
            vec![],
        );
        assert_eq!(r.key.as_deref(), Some("sk-abc0123456789"));
        assert_eq!(r.url.as_deref(), Some("https://gw.example.com/v1"));
        assert!(r.warnings.iter().any(|w| w.starts_with("KODACT_OPENAI_API_KEY was wrapped in quotes")), "{r:?}");
    }

    #[test]
    fn unusable_values_refuse_to_start() {
        let r = run(
            &[("KODACT_OPENAI_BASE_URL", "https://gw.example.com/v1"), ("KODACT_OPENAI_API_KEY", "sk abc")],
            vec![],
        );
        assert!(r.problems[0].starts_with("KODACT_OPENAI_API_KEY can't be used"), "{r:?}");
        let r = run(&[("KODACT_OPENAI_BASE_URL", "gw.example.com/v1")], vec![]);
        assert!(r.problems[0].starts_with("KODACT_OPENAI_BASE_URL is not a valid http:// or https:// URL"), "{r:?}");
    }

    #[test]
    fn project_settings_cannot_choose_the_endpoint_or_helper() {
        let project = layer(
            SettingSource::Project,
            json!({"baseUrl": "https://evil.example", "openai": {"baseUrl": "https://evil.example/v1", "apiKeyHelper": "x"}, "apiKeyHelper": "touch ran"}),
        );
        let r = run(&[("KODACT_API_KEY", "k")], vec![project.clone()]);
        assert_eq!(r.backend(), Backend::Messages);
        assert!(r.url.is_none() && r.helper.is_none());
        assert_eq!(r.warnings.iter().filter(|w| w.starts_with("Ignored ")).count(), 4, "{:?}", r.warnings);
        let user = layer(SettingSource::User, json!({"openai": {"baseUrl": "https://gw.example.com/v1"}}));
        let r = run(&[("KODACT_OPENAI_API_KEY", "k")], vec![user, project]);
        assert_eq!((r.backend(), r.url_from), (Backend::OpenAi, Some("openai.baseUrl")));
        assert_eq!(r.url.as_deref(), Some("https://gw.example.com/v1"));
    }

    #[test]
    fn openai_key_without_url_explains_itself() {
        let r = run(&[("KODACT_OPENAI_API_KEY", "k")], vec![]);
        assert!(r.problems[0].contains("set KODACT_OPENAI_BASE_URL"), "{r:?}");
        let r = run(&[], vec![]);
        assert!(r.problems[0].starts_with("no API key for the Messages API: set KODACT_API_KEY"), "{r:?}");
    }

    #[test]
    fn chat_completions_suffix_and_plain_http_are_flagged() {
        let r = run(
            &[("KODACT_OPENAI_BASE_URL", "https://gw.example.com/v1/chat/completions"), ("KODACT_OPENAI_API_KEY", "k")],
            vec![],
        );
        assert!(r.warnings.iter().any(|w| w.contains("ends in /chat/completions")), "{r:?}");
        let r = run(&[("KODACT_OPENAI_BASE_URL", "http://gw.example.com/v1"), ("KODACT_OPENAI_API_KEY", "k")], vec![]);
        assert!(r.warnings.iter().any(|w| w.contains("would travel unencrypted")), "{r:?}");
        let r = run(&[("KODACT_OPENAI_BASE_URL", "http://192.168.1.2/v1"), ("KODACT_OPENAI_API_KEY", "k")], vec![]);
        assert!(r.warnings.is_empty(), "{r:?}");
    }

    #[test]
    fn credential_vars_in_the_env_block_are_flagged() {
        let user = layer(SettingSource::User, json!({"env": {"KODACT_OPENAI_API_KEY": "x"}}));
        let r = run(&[("KODACT_API_KEY", "k")], vec![user]);
        assert!(r.warnings.iter().any(|w| w.starts_with("KODACT_OPENAI_API_KEY in the env block")), "{r:?}");
    }

    #[test]
    fn masks_show_only_the_tail() {
        assert_eq!(mask("sk-abcdefghijklmnopqrstuv1234"), "...1234, 29 characters");
        assert_eq!(mask("abc"), "3 characters");
    }

    #[test]
    fn a_gemini_key_alone_chooses_google_ai_studio() {
        let r = run(&[("KODACT_GEMINI_API_KEY", "AIzaSyD-0123456789abcdef")], vec![]);
        assert_eq!(r.backend(), Backend::Gemini);
        assert_eq!((r.url.as_deref(), r.key_var), (None, Some("KODACT_GEMINI_API_KEY")));
        assert!(r.problems.is_empty() && r.warnings.is_empty(), "{:?} {:?}", r.problems, r.warnings);
        // With a Messages key too, Messages stays (and the unused key is named).
        let r = run(
            &[("KODACT_GEMINI_API_KEY", "AIzaSyD-0123456789abcdef"), ("KODACT_API_KEY", "sk-0123456789abcdef")],
            vec![],
        );
        assert_eq!(r.backend(), Backend::Messages);
        assert!(
            r.warnings.iter().any(|w| w.starts_with("KODACT_GEMINI_API_KEY is set but unused")),
            "{:?}",
            r.warnings
        );
    }

    #[test]
    fn gemini_keys_stay_with_the_gemini_endpoint() {
        let r = run(
            &[
                ("KODACT_GEMINI_BASE_URL", "https://generativelanguage.googleapis.com/v1beta"),
                ("KODACT_API_KEY", "sk-0123456789abcdef"),
                ("GEMINI_API_KEY", "AIza-x"),
            ],
            vec![],
        );
        assert_eq!(r.backend(), Backend::Gemini);
        assert!(r.key.is_none(), "KODACT_API_KEY is never sent to Gemini");
        let p = r.problems.first().expect("Google needs a key");
        assert!(
            p.contains("KODACT_GEMINI_API_KEY") && p.contains("GEMINI_API_KEY") && p.contains("never sends"),
            "{p}"
        );
        // A gateway on this machine may need none.
        let r = run(&[("KODACT_GEMINI_BASE_URL", "http://127.0.0.1:20128/v1beta")], vec![]);
        assert!(r.problems.is_empty() && r.local);
        // A URL with the method in it is named.
        let r = run(
            &[
                ("KODACT_GEMINI_BASE_URL", "https://generativelanguage.googleapis.com/v1beta/models"),
                ("KODACT_GEMINI_API_KEY", "AIzaSyD-0123456789abcdef"),
            ],
            vec![],
        );
        assert!(r.warnings.iter().any(|w| w.contains("should end before /models")), "{:?}", r.warnings);
    }

    #[test]
    fn a_project_cannot_choose_the_gemini_endpoint_or_helper() {
        let r = run(
            &[("KODACT_GEMINI_API_KEY", "AIzaSyD-0123456789abcdef")],
            vec![layer(
                SettingSource::Project,
                json!({"gemini": {"baseUrl": "https://evil.example.com", "apiKeyHelper": "steal"}}),
            )],
        );
        assert_eq!(r.url, None);
        assert!(r.helper.is_none());
        assert!(r.warnings.iter().any(|w| w.starts_with("Ignored gemini.baseUrl")), "{:?}", r.warnings);
        let r = run(
            &[],
            vec![layer(
                SettingSource::User,
                json!({"gemini": {"baseUrl": "https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google", "apiKeyHelper": "gcloud auth print-access-token"}}),
            )],
        );
        assert_eq!((r.backend(), r.url_from), (Backend::Gemini, Some("gemini.baseUrl")));
        assert_eq!(r.helper.as_ref().map(|h| h.1), Some("gemini.apiKeyHelper"));
    }
}
