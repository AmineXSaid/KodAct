//! Limits an endpoint stated only by refusing a request (its context window, its reply limit),
//! kept between sessions per endpoint and model in a file under the cache directory.
//!
//! Without them, every new session on a model whose endpoint lists no limits sent one request
//! too long before it learned them, and compacted late: 36 of the 89 quick-thinking trials of
//! 2026-11-17 sent a 68,001-token prompt to a 100,000-token window first. A limit learned this
//! way is the endpoint's own word, so it is loaded as one (below `modelLimits`, which still wins,
//! and replaced by what the endpoint lists in the new session). It is kept for [`KEPT_FOR`]: an
//! endpoint can be given a larger window, and a larger window is never learned from a refusal.
//! A model whose endpoint never refuses has nothing kept, so a large one is left as it was.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::models::Limits;

/// How long a learned limit is used before it has to be learned again.
pub const KEPT_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

struct Store {
    path: PathBuf,
    endpoint: String,
}

fn store() -> &'static Mutex<Option<Store>> {
    static S: OnceLock<Mutex<Option<Store>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Keep the limits learned on `endpoint` in `path`: those still fresh go into this session's
/// limits as the endpoint's own word, and what this session learns is added there. Returns the
/// models whose limits were loaded.
pub fn keep_at(path: PathBuf, endpoint: &str) -> Vec<String> {
    let now = now_secs();
    let mut loaded = vec![];
    if let Some(models) = read(&path).get(endpoint).and_then(Value::as_object) {
        for (model, v) in models {
            let learned = v.get("learned").and_then(Value::as_u64).unwrap_or(0);
            if now.saturating_sub(learned) >= KEPT_FOR.as_secs() {
                continue;
            }
            let n = |k: &str| v.get(k).and_then(Value::as_u64).filter(|n| *n > 0);
            let l = Limits {
                context_window: n("contextWindow"),
                max_output: n("maxOutputTokens").map(|n| n.min(u64::from(u32::MAX)) as u32),
            };
            if l != Limits::default() {
                crate::models::listed_limits(model, l);
                loaded.push(model.clone());
            }
        }
    }
    *store().lock().unwrap_or_else(|e| e.into_inner()) = Some(Store { path, endpoint: endpoint.to_string() });
    loaded
}

/// Keep a limit `model`'s endpoint stated by refusing a request, merged with what was kept for
/// it before. Nothing is written unless [`keep_at`] chose a file for this session.
pub(crate) fn remember(model: &str, l: Limits) {
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = guard.as_ref() else { return };
    let mut file = read(&s.path);
    let Some(root) = file.as_object_mut() else { return };
    let endpoint = root.entry(s.endpoint.clone()).or_insert_with(|| json!({}));
    if !endpoint.is_object() {
        *endpoint = json!({});
    }
    let Some(models) = endpoint.as_object_mut() else { return };
    let entry = models.entry(model.to_string()).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    let Some(e) = entry.as_object_mut() else { return };
    if let Some(w) = l.context_window {
        e.insert("contextWindow".into(), json!(w));
    }
    if let Some(m) = l.max_output {
        e.insert("maxOutputTokens".into(), json!(m));
    }
    e.insert("learned".into(), json!(now_secs()));
    if let Err(err) = write(&s.path, &file) {
        tracing::debug!(path = %s.path.display(), %err, "could not keep a learned limit");
    }
}

fn read(path: &Path) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

/// Written whole to a file of its own, then renamed over the old one: a session that reads it
/// meanwhile sees the old file or the new one, never half of one.
fn write(path: &Path, v: &Value) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(v).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{configure_limits, model_info_or_default, reported_max_output, reported_window};

    /// One test, since the file is the session's one store: what a refusal teaches is written for
    /// this endpoint and model, and the next session loads it; another endpoint's limits, stale
    /// ones and a model the user configured are left as they were.
    #[test]
    fn limits_learned_from_refusals_are_kept_for_the_next_session() {
        let dir = std::env::temp_dir().join(format!("kodact-learned-limits-test-{}", std::process::id()));
        let path = dir.join("cache").join("learned-limits.json");
        let old = now_secs() - KEPT_FOR.as_secs() - 60;
        let seed = json!({
            "openai http://gw/v1": {
                "kept-fresh": {"contextWindow": 100_000, "learned": now_secs()},
                "kept-stale": {"contextWindow": 50_000, "learned": old},
                "kept-configured": {"contextWindow": 10_000, "learned": now_secs()}
            },
            "openai http://other/v1": {"kept-elsewhere": {"contextWindow": 20_000, "learned": now_secs()}}
        });
        write(&path, &seed).unwrap();
        configure_limits("kept-configured", Limits { context_window: Some(400_000), max_output: None });
        let loaded = keep_at(path.clone(), "openai http://gw/v1");
        assert!(loaded.contains(&"kept-fresh".to_string()), "{loaded:?}");
        assert!(!loaded.contains(&"kept-stale".to_string()), "a week old: learned again");
        assert_eq!(model_info_or_default("kept-fresh").context_window, 100_000);
        assert_eq!(model_info_or_default("kept-configured").context_window, 400_000, "modelLimits wins");
        assert_ne!(model_info_or_default("kept-elsewhere").context_window, 20_000, "another endpoint's");

        // Learned in this session: written for this endpoint and model, merged with what was there.
        assert!(reported_window("kept-new", 131_072));
        assert!(reported_max_output("kept-new", 8_192));
        let file = read(&path);
        let entry = &file["openai http://gw/v1"]["kept-new"];
        assert_eq!((entry["contextWindow"].as_u64(), entry["maxOutputTokens"].as_u64()), (Some(131_072), Some(8_192)));
        assert!(entry["learned"].as_u64().unwrap() >= now_secs() - 5);
        assert_eq!(file["openai http://other/v1"]["kept-elsewhere"]["contextWindow"], 20_000, "left alone");

        // A broken file is started over rather than trusted.
        std::fs::write(&path, "not json").unwrap();
        keep_at(path.clone(), "openai http://gw/v1");
        assert!(reported_window("kept-after-broken", 65_536));
        assert_eq!(read(&path)["openai http://gw/v1"]["kept-after-broken"]["contextWindow"], 65_536);
        *store().lock().unwrap() = None;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
