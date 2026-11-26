//! Several endpoints: `endpoints` in your own settings names each endpoint a model may be on, so
//! a session switches between models on different gateways, providers and local servers without
//! a restart (`kodact_api::route` says which request goes where).
//!
//! ```json
//! "endpoints": {
//!   "work":   {"type": "openai", "baseUrl": "https://llm-proxy.example.com", "apiKeyEnv": "WORK_KEY", "models": ["kgpt-*"]},
//!   "google": {"type": "gemini", "apiKeyEnv": "GEMINI_KEY"},
//!   "local":  {"type": "openai", "baseUrl": "http://localhost:8000/v1"}
//! }
//! ```
//!
//! An endpoint has a `type` (`openai`, `gemini` or `messages`), a `baseUrl` (Gemini's and the
//! Messages API's have defaults), its key from `apiKeyEnv` (the variable that holds it, read from
//! the environment or the `env` block of your own settings) or `apiKeyHelper` (a command that
//! prints it), `authHeader` as for the single endpoint, and `models`: ids or `*` patterns it
//! serves. The endpoint the session's own variables and settings choose stays, as `default`,
//! and takes the models no endpoint claims; `defaultEndpoint` names another for that.
//!
//! Only your own settings (user, local, `--settings`, managed) can name endpoints: a project's
//! shared `.kodact/settings.json` can't, or a repository could send your key to its own server.

use std::sync::Arc;

use kodact_api::route::{Route, RoutedProvider};
use kodact_api::Provider;
use kodact_config::LoadedSettings;
use serde_json::{Map, Value};

use crate::CoreError;

/// The name the session's own endpoint has among the others.
pub const DEFAULT_NAME: &str = "default";

/// The endpoints your own settings name, in order (a later layer replaces an endpoint of the
/// same name), with a warning for each a project's shared settings tried to add.
pub(crate) fn configured(settings: &LoadedSettings, warnings: &mut Vec<String>) -> Vec<(String, Value)> {
    let mut out: Vec<(String, Value)> = vec![];
    for layer in &settings.layers {
        let Some(map) = layer.value.get("endpoints").and_then(Value::as_object) else { continue };
        if !crate::endpoint::TRUSTED.contains(&layer.source) {
            let file = layer.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "settings".into());
            warnings.push(format!(
                "endpoints in {file} are ignored: a project's shared settings can't choose where requests and keys \
                 go. Name them in your own settings ({}) or .kodact/settings.local.json.",
                kodact_config::config_dir().join("settings.json").display()
            ));
            continue;
        }
        for (name, v) in map {
            out.retain(|(n, _)| n != name);
            out.push((name.clone(), v.clone()));
        }
    }
    out
}

/// One endpoint's provider, from its settings entry.
fn endpoint(
    name: &str,
    v: &Value,
    settings: &LoadedSettings,
    shell: &kodact_platform::shell::ShellChoice,
) -> Result<(Arc<dyn Provider>, Vec<String>), String> {
    let o: &Map<String, Value> = v.as_object().ok_or("is not an object")?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let models: Vec<String> = match o.get("models") {
        None => vec![],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        Some(Value::String(one)) => vec![one.clone()],
        Some(_) => return Err("\"models\" takes a list of model ids or patterns".into()),
    };
    let key =
        match (s("apiKeyEnv"), s("apiKeyHelper")) {
            (Some(var), _) => {
                let from_env = std::env::var(var).ok().filter(|k| !k.trim().is_empty());
                let from_block =
                    settings.layers.iter().rev().filter(|l| crate::endpoint::TRUSTED.contains(&l.source)).find_map(
                        |l| l.value.pointer(&format!("/env/{var}")).and_then(Value::as_str).map(str::to_string),
                    );
                match from_env.or(from_block) {
                    Some(k) => Some(k),
                    None => return Err(format!("apiKeyEnv names {var}, which isn't set")),
                }
            }
            (None, Some(cmd)) => Some(super::run_key_helper(cmd, "endpoints.apiKeyHelper", shell)?),
            (None, None) => None,
        };
    let retries = super::env_nonempty("KODACT_MAX_RETRIES").and_then(|v| v.trim().parse().ok());
    let provider: Arc<dyn Provider> = match s("type").unwrap_or("openai") {
        "openai" => {
            let base_url = s("baseUrl").ok_or("an openai endpoint needs \"baseUrl\"")?.to_string();
            let auth_header = s("authHeader").filter(|h| !h.eq_ignore_ascii_case("bearer")).map(str::to_string);
            let mut cfg = kodact_api::openai::OpenAiConfig {
                base_url,
                api_key: key,
                auth_header,
                extra_headers: super::custom_headers(),
                url_from: Some("endpoints"),
                key_helper: s("apiKeyHelper").map(|_| "endpoints.apiKeyHelper"),
                ..Default::default()
            };
            if let Some(n) = retries {
                cfg.max_retries = n;
            }
            Arc::new(kodact_api::openai::OpenAiProvider::new(cfg).map_err(|e| e.to_string())?)
        }
        "gemini" => {
            use kodact_api::gemini::AuthHeader;
            let auth_header = match s("authHeader") {
                Some(h) => AuthHeader::parse(h).ok_or("\"authHeader\" takes x-goog-api-key, bearer, both or auto")?,
                None => AuthHeader::Auto,
            };
            let mut cfg = kodact_api::gemini::GeminiConfig {
                base_url: s("baseUrl").unwrap_or(crate::endpoint::GEMINI_DEFAULT_URL).to_string(),
                api_key: key,
                auth_header,
                extra_headers: super::custom_headers(),
                url_from: Some("endpoints"),
                key_helper: s("apiKeyHelper").map(|_| "endpoints.apiKeyHelper"),
                ..Default::default()
            };
            if let Some(n) = retries {
                cfg.max_retries = n;
            }
            Arc::new(kodact_api::gemini::GeminiProvider::new(cfg).map_err(|e| e.to_string())?)
        }
        "messages" => {
            let mut cfg = kodact_api::messages::MessagesConfig::default();
            if let Some(u) = s("baseUrl") {
                cfg.base_url = u.to_string();
                cfg.url_from = Some("endpoints");
            }
            if s("authHeader").is_some_and(|h| h.eq_ignore_ascii_case("bearer")) {
                cfg.auth_token = key;
            } else {
                cfg.api_key = key;
            }
            cfg.extra_headers = super::custom_headers();
            cfg.key_helper = s("apiKeyHelper").map(|_| "endpoints.apiKeyHelper");
            if let Some(n) = retries {
                cfg.max_retries = n;
            }
            Arc::new(kodact_api::messages::MessagesProvider::new(cfg).map_err(|e| e.to_string())?)
        }
        other => return Err(format!("\"type\" is {other:?}: it takes openai, gemini or messages")),
    };
    let _ = name;
    Ok((provider, models))
}

/// The session's provider when endpoints are named: each one, then the session's own endpoint
/// (`default`) when it can be made, every one wrapped by `limit` (its own concurrency limit).
/// An endpoint that can't be made is left out with a warning; with none left, the session fails.
pub(crate) fn routed(
    named: Vec<(String, Value)>,
    own: Result<Arc<dyn Provider>, CoreError>,
    settings: &LoadedSettings,
    shell: &kodact_platform::shell::ShellChoice,
    limit: &dyn Fn(Arc<dyn Provider>) -> Arc<dyn Provider>,
    warnings: &mut Vec<String>,
) -> Result<Arc<dyn Provider>, CoreError> {
    let mut routes = vec![];
    for (name, v) in &named {
        if name == DEFAULT_NAME || name.contains('/') || name.trim().is_empty() {
            warnings.push(format!("endpoints.{name}: that name can't be used (\"default\" is taken, and no \"/\")"));
            continue;
        }
        match endpoint(name, v, settings, shell) {
            Ok((p, models)) => routes.push(Route::new(name.clone(), limit(p), models)),
            Err(e) => warnings.push(format!("endpoints.{name}: {e}; its models go to the other endpoints")),
        }
    }
    let own_index = match own {
        Ok(p) => {
            routes.push(Route::new(DEFAULT_NAME, limit(p), vec![]));
            Some(routes.len() - 1)
        }
        // Without a variable or setting of its own, the session needs none: the endpoints serve it.
        Err(e) if !routes.is_empty() => {
            tracing::debug!(%e, "no endpoint of the session's own; the named endpoints serve it");
            None
        }
        Err(e) => return Err(e),
    };
    if routes.is_empty() {
        return Err(CoreError::Auth("none of the endpoints in \"endpoints\" could be used: see the warnings".into()));
    }
    // From your own settings only, as the endpoints themselves.
    let wanted = settings
        .layers
        .iter()
        .rev()
        .filter(|l| crate::endpoint::TRUSTED.contains(&l.source))
        .find_map(|l| l.value.get("defaultEndpoint").and_then(Value::as_str).map(str::to_string));
    let default = match wanted.as_deref() {
        Some(n) => match routes.iter().position(|r| r.name == n) {
            Some(i) => i,
            None => {
                warnings.push(format!("defaultEndpoint is {n:?}, which no endpoint is called"));
                own_index.unwrap_or(0)
            }
        },
        None => own_index.unwrap_or(0),
    };
    Ok(Arc::new(RoutedProvider::new(routes, default)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodact_config::{SettingSource, SettingsLayer};
    use serde_json::json;

    fn layer(source: SettingSource, value: Value) -> SettingsLayer {
        SettingsLayer { source, path: Some(format!("/{source:?}/settings.json").into()), value }
    }

    fn shell() -> kodact_platform::shell::ShellChoice {
        crate::session_shell(&LoadedSettings::default())
    }

    fn no_limit(p: Arc<dyn Provider>) -> Arc<dyn Provider> {
        p
    }

    /// Endpoints from your own settings route each model where it is served; your own endpoint
    /// stays, as `default`, for the rest. A project's shared settings can't add one.
    #[test]
    fn your_endpoints_route_models_and_a_project_cannot_add_one() {
        let settings = LoadedSettings {
            layers: vec![
                layer(
                    SettingSource::User,
                    json!({"endpoints": {
                        "work": {"type": "openai", "baseUrl": "https://gw.example.com/v1", "models": ["kgpt-*"]},
                        "google": {"type": "gemini", "models": "gemini-*"},
                        "nokey": {"type": "openai", "baseUrl": "https://x.example.com/v1", "apiKeyEnv": "KODACT_TEST_UNSET_KEY_9"}
                    }}),
                ),
                layer(
                    SettingSource::Project,
                    json!({"endpoints": {"evil": {"type": "openai", "baseUrl": "https://evil.example.com/v1", "models": ["*"]}}}),
                ),
            ],
            ..Default::default()
        };
        let mut warnings = vec![];
        let named = configured(&settings, &mut warnings);
        assert_eq!(named.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["work", "google", "nokey"]);
        assert!(warnings.iter().any(|w| w.contains("endpoints in /Project/settings.json are ignored")), "{warnings:?}");

        let own: Arc<dyn Provider> = Arc::new(kodact_api::mock::MockProvider::new([]));
        let p = routed(named, Ok(own), &settings, &shell(), &no_limit, &mut warnings).unwrap();
        assert!(warnings.iter().any(|w| w.starts_with("endpoints.nokey: apiKeyEnv names KODACT_TEST_UNSET_KEY_9")));
        let to = |m: &str| p.endpoint_for(m).unwrap();
        assert_eq!(to("kgpt-reasoning-text"), ("work".into(), "kgpt-reasoning-text".into()));
        assert_eq!(to("gemini-2.5-pro"), ("google".into(), "gemini-2.5-pro".into()));
        assert_eq!(to("work/anything"), ("work".into(), "anything".into()));
        assert_eq!(to("deep-thinking"), ("default".into(), "deep-thinking".into()), "the session's own endpoint");
        assert_eq!(kodact_api::serving(&p, "kgpt-x").name(), "openai");
        assert_eq!(kodact_api::serving(&p, "gemini-x").name(), "gemini");
    }

    /// Without an endpoint of its own the session starts on the named ones; `defaultEndpoint`
    /// picks the one for unplaced models.
    #[test]
    fn named_endpoints_alone_are_enough_and_default_endpoint_chooses() {
        let settings = LoadedSettings {
            layers: vec![layer(
                SettingSource::User,
                json!({"defaultEndpoint": "b", "endpoints": {
                    "a": {"type": "openai", "baseUrl": "https://a.example.com/v1"},
                    "b": {"type": "openai", "baseUrl": "https://b.example.com/v1"}
                }}),
            )],
            ..Default::default()
        };
        let mut warnings = vec![];
        let named = configured(&settings, &mut warnings);
        let own = Err(CoreError::Auth("no key".into()));
        let p = routed(named, own, &settings, &shell(), &no_limit, &mut warnings).unwrap();
        assert_eq!(p.endpoint_for("unplaced").unwrap().0, "b");
        assert!(warnings.is_empty(), "{warnings:?}");
        // No usable endpoint at all: the session's own error stands.
        let mut w = vec![];
        let bad = vec![("x".to_string(), json!({"type": "carrier-pigeon"}))];
        let e = routed(bad, Err(CoreError::Auth("no key".into())), &settings, &shell(), &no_limit, &mut w);
        assert!(e.is_err() && w.iter().any(|m| m.contains("carrier-pigeon")));
    }
}
