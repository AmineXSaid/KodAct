//! Known models: aliases, limits, pricing and how each takes thinking.

/// How a model accepts the `thinking` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingStyle {
    /// Thinking is always on; omit the parameter (or send adaptive). Depth via effort.
    AlwaysOn,
    /// Adaptive thinking; may be switched off (`disabled`, or `between_tools`).
    Adaptive { off: OffMode },
    /// Legacy `{type: "enabled", budget_tokens}`.
    Budget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OffMode {
    Disabled,
    BetweenTools,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: &'static str,
    pub display_name: &'static str,
    pub context_window: u64,
    pub max_output: u32,
    /// USD per million tokens.
    pub input_price: f64,
    pub output_price: f64,
    pub cache_read_price: f64,
    pub thinking: ThinkingStyle,
    /// Effort levels accepted in `output_config.effort`; empty = unsupported.
    pub effort_levels: &'static [&'static str],
    pub default_effort: Option<&'static str>,
    pub supports_fast_mode: bool,
}

impl ModelInfo {
    /// Prompt-cache write price (5-minute TTL): 1.25x input.
    pub fn cache_write_price(&self) -> f64 {
        self.input_price * 1.25
    }

    pub fn supports_effort(&self) -> bool {
        !self.effort_levels.is_empty()
    }

    /// Cost in USD of a request's usage.
    pub fn cost(&self, u: &kodact_types::Usage) -> f64 {
        (u.input_tokens as f64 * self.input_price
            + u.output_tokens as f64 * self.output_price
            + u.cache_read_input_tokens as f64 * self.cache_read_price
            + u.cache_creation_input_tokens as f64 * self.cache_write_price())
            / 1_000_000.0
    }
}

const ALL_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const NO_XHIGH: &[&str] = &["low", "medium", "high", "max"];
/// Gemini: KodAct's levels, sent as Gemini's own (`gemini_rules::thinking_level`).
const GEMINI_EFFORT: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// A Gemini model: always thinking, effort as its thinking level (3 and
/// later) or budget (2.5). List prices for prompts up to 200k tokens.
const fn gemini(id: &'static str, display_name: &'static str, max_output: u32, prices: (f64, f64, f64)) -> ModelInfo {
    ModelInfo {
        id,
        display_name,
        context_window: 1_048_576,
        max_output,
        input_price: prices.0,
        output_price: prices.1,
        cache_read_price: prices.2,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: GEMINI_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    }
}

pub const MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "claude-fable-5-1",
        display_name: "Fable 5.1",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 10.0,
        output_price: 50.0,
        cache_read_price: 0.25,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-fable-5",
        display_name: "Fable 5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 10.0,
        output_price: 50.0,
        cache_read_price: 1.0,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-opus-5-5",
        display_name: "Opus 5.5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 4.0,
        output_price: 20.0,
        cache_read_price: 0.20,
        thinking: ThinkingStyle::AlwaysOn,
        effort_levels: ALL_EFFORT,
        default_effort: Some("medium"),
        supports_fast_mode: true,
    },
    ModelInfo {
        id: "claude-opus-5",
        display_name: "Opus 5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: true,
    },
    ModelInfo {
        id: "claude-opus-4-8",
        display_name: "Opus 4.8",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: true,
    },
    ModelInfo {
        id: "claude-opus-4-7",
        display_name: "Opus 4.7",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-opus-4-6",
        display_name: "Opus 4.6",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 5.0,
        output_price: 25.0,
        cache_read_price: 0.50,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: NO_XHIGH,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-sonnet-5-5",
        display_name: "Sonnet 5.5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 2.0,
        output_price: 10.0,
        cache_read_price: 0.20,
        thinking: ThinkingStyle::Adaptive { off: OffMode::BetweenTools },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-sonnet-5",
        display_name: "Sonnet 5",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 2.0,
        output_price: 10.0,
        cache_read_price: 0.20,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: ALL_EFFORT,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-sonnet-4-6",
        display_name: "Sonnet 4.6",
        context_window: 1_000_000,
        max_output: 128_000,
        input_price: 3.0,
        output_price: 15.0,
        cache_read_price: 0.30,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: NO_XHIGH,
        default_effort: Some("high"),
        supports_fast_mode: false,
    },
    ModelInfo {
        id: "claude-haiku-4-5",
        display_name: "Haiku 4.5",
        context_window: 200_000,
        max_output: 64_000,
        input_price: 1.0,
        output_price: 5.0,
        cache_read_price: 0.10,
        thinking: ThinkingStyle::Budget,
        effort_levels: &[],
        default_effort: None,
        supports_fast_mode: false,
    },
    gemini("gemini-3.1-pro", "Gemini 3.1 Pro", 65_536, (2.0, 12.0, 0.20)),
    gemini("gemini-3-pro", "Gemini 3 Pro", 65_536, (2.0, 12.0, 0.20)),
    gemini("gemini-3-flash", "Gemini 3 Flash", 65_536, (0.50, 3.0, 0.05)),
    gemini("gemini-2.5-pro", "Gemini 2.5 Pro", 65_536, (1.25, 10.0, 0.125)),
    gemini("gemini-2.5-flash", "Gemini 2.5 Flash", 65_536, (0.30, 2.50, 0.03)),
    gemini("gemini-2.5-flash-lite", "Gemini 2.5 Flash-Lite", 65_536, (0.10, 0.40, 0.01)),
];

pub const DEFAULT_MODEL: &str = "claude-opus-5-5";
/// Model used for cheap background work (titles, summaries of web pages).
pub const SMALL_FAST_MODEL: &str = "claude-haiku-4-5";

/// Map an alias (`opus`, `sonnet`, `haiku`, `fable`, `default`) to a model id.
/// Unknown names pass through unchanged so new ids work without a release.
pub fn resolve_model(name: &str) -> String {
    let n = name.trim();
    let lower = n.to_ascii_lowercase();
    let (base, suffix) = match lower.strip_suffix("[1m]") {
        Some(b) => (b.to_string(), ""),
        None => (lower.clone(), ""),
    };
    let id = match base.as_str() {
        "" | "default" | "opus" | "best" => DEFAULT_MODEL,
        "sonnet" => "claude-sonnet-5-5",
        "haiku" => "claude-haiku-4-5",
        "fable" => "claude-fable-5-1",
        "opusplan" => DEFAULT_MODEL,
        _ => return n.to_string(),
    };
    format!("{id}{suffix}")
}

/// Look up a model by id (or a dated / prefixed variant of one).
pub fn model_info(id: &str) -> Option<&'static ModelInfo> {
    MODELS
        .iter()
        .find(|m| m.id == id)
        .or_else(|| MODELS.iter().filter(|m| id.starts_with(m.id)).max_by_key(|m| m.id.len()))
}

/// Limits set for a model from outside the built-in table: the `modelLimits`
/// setting, `KODACT_CONTEXT_WINDOW`, or what the endpoint's model list reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub context_window: Option<u64>,
    pub max_output: Option<u32>,
}

/// Where a model's limits come from (shown by `/model`, `/context` and the startup notice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LimitsSource {
    /// The built-in model table.
    Known,
    /// Set by the user (`modelLimits`, `KODACT_CONTEXT_WINDOW`).
    Configured,
    /// Reported by the endpoint's model list.
    Listed,
    /// Nothing known: KodAct's guesses.
    Guessed,
}

type LimitMap = std::collections::HashMap<String, (Limits, LimitsSource)>;

fn limit_map() -> &'static std::sync::RwLock<LimitMap> {
    static M: std::sync::OnceLock<std::sync::RwLock<LimitMap>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// The context window assumed for any model without a known one (`KODACT_CONTEXT_WINDOW`).
fn default_window() -> Option<u64> {
    std::env::var("KODACT_CONTEXT_WINDOW").ok().and_then(|v| v.trim().replace('_', "").parse().ok())
}

/// Set limits the user configured for `id` (they win over everything else).
pub fn configure_limits(id: &str, l: Limits) {
    limit_map().write().unwrap().insert(id.to_string(), (l, LimitsSource::Configured));
}

/// Record a context window the endpoint reports for `id`, unless the user set one.
pub fn listed_limits(id: &str, l: Limits) {
    let mut m = limit_map().write().unwrap();
    if !matches!(m.get(id), Some((_, LimitsSource::Configured))) {
        m.insert(id.to_string(), (l, LimitsSource::Listed));
    }
}

/// The window `id`'s endpoint stated when it refused a request as too long. It counts as the
/// endpoint's own word, like its model list: it wins over `KODACT_CONTEXT_WINDOW`, the table
/// and the guess, never over `modelLimits`. True when it changes the window KodAct uses.
pub fn reported_window(id: &str, window: u64) -> bool {
    let before = model_info_or_default(id).context_window;
    let max_output = limit_map().read().unwrap().get(id).and_then(|(l, _)| l.max_output);
    listed_limits(id, Limits { context_window: Some(window), max_output });
    model_info_or_default(id).context_window != before
}

/// Nothing about `id` itself gives its window yet: it is in no table, the user set no
/// limits for it, and its endpoint has listed none. Until the endpoint's model list says
/// otherwise, its window is `KODACT_CONTEXT_WINDOW` or the 200,000-token guess.
pub fn window_unknown(id: &str) -> bool {
    model_info(id).is_none() && !limit_map().read().unwrap().contains_key(id)
}

/// Where `id`'s limits come from.
pub fn limits_source(id: &str) -> LimitsSource {
    if let Some((_, src)) = limit_map().read().unwrap().get(id) {
        return *src;
    }
    if model_info(id).is_some() {
        LimitsSource::Known
    } else if default_window().is_some() {
        LimitsSource::Configured
    } else {
        LimitsSource::Guessed
    }
}

/// Info for a model, falling back to conservative defaults for unknown ids;
/// configured or listed limits win over both.
pub fn model_info_or_default(id: &str) -> ModelInfo {
    info_with_fallback_window(id, default_window())
}

/// [`model_info_or_default`], with `KODACT_CONTEXT_WINDOW` given as `fallback`. A window
/// that belongs to the model wins, in this order: the `modelLimits` setting for it, the
/// window its endpoint lists for it, the built-in table. `fallback`, one value for every
/// model, only fills in for a model with none of these, before the 200,000-token guess:
/// set for a small model, it never shrinks a large one whose endpoint reports its window.
fn info_with_fallback_window(id: &str, fallback: Option<u64>) -> ModelInfo {
    let mut info = base_info(id);
    if model_info(id).is_none() {
        if let Some(w) = fallback {
            info.context_window = w;
        }
    }
    if let Some((l, _)) = limit_map().read().unwrap().get(id) {
        if let Some(w) = l.context_window {
            info.context_window = w;
        }
        if let Some(o) = l.max_output {
            info.max_output = o;
        }
    }
    info
}

fn base_info(id: &str) -> ModelInfo {
    model_info(id).cloned().unwrap_or(ModelInfo {
        id: "unknown",
        display_name: "Custom model",
        context_window: 200_000,
        max_output: 32_000,
        input_price: 0.0,
        output_price: 0.0,
        cache_read_price: 0.0,
        thinking: ThinkingStyle::Adaptive { off: OffMode::Disabled },
        effort_levels: &[],
        default_effort: None,
        supports_fast_mode: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodact_types::Usage;

    #[test]
    fn configured_limits_win_over_listed_and_defaults() {
        let id = "test-unknown-model-limits";
        assert_eq!(limits_source(id), LimitsSource::Guessed);
        assert_eq!(model_info_or_default(id).context_window, 200_000);
        listed_limits(id, Limits { context_window: Some(64_000), max_output: None });
        assert_eq!((model_info_or_default(id).context_window, limits_source(id)), (64_000, LimitsSource::Listed));
        configure_limits(id, Limits { context_window: Some(128_000), max_output: Some(8_192) });
        listed_limits(id, Limits { context_window: Some(32_000), max_output: None });
        let info = model_info_or_default(id);
        assert_eq!((info.context_window, info.max_output), (128_000, 8_192));
        assert_eq!(limits_source(id), LimitsSource::Configured);
    }

    /// `KODACT_CONTEXT_WINDOW` is one value for every model. Set for a small model, it must not
    /// shrink a large one whose endpoint reports its window, nor stretch a small one (docs/goal.md,
    /// "Fix, don't fit"); it still fills in for a model with no window of its own.
    #[test]
    fn a_models_own_window_wins_over_the_global_one_at_any_size() {
        // Only a model with nothing of its own waits for its endpoint's list.
        assert!(window_unknown("test-listed-later"));
        listed_limits("test-listed-later", Limits { context_window: Some(1_000_000), max_output: None });
        assert!(!window_unknown("test-listed-later"));
        assert!(!window_unknown("claude-haiku-4-5"));
        let set_for_a_small_model = Some(100_000);
        listed_limits("test-listed-large", Limits { context_window: Some(1_000_000), max_output: None });
        assert_eq!(info_with_fallback_window("test-listed-large", set_for_a_small_model).context_window, 1_000_000);
        assert_eq!(limits_source("test-listed-large"), LimitsSource::Listed);
        listed_limits("test-listed-small", Limits { context_window: Some(32_768), max_output: None });
        assert_eq!(info_with_fallback_window("test-listed-small", Some(1_000_000)).context_window, 32_768);
        // Nothing of its own: the global value, else the guess.
        assert_eq!(info_with_fallback_window("test-unlisted", set_for_a_small_model).context_window, 100_000);
        assert_eq!(info_with_fallback_window("test-unlisted", None).context_window, 200_000);
        // A listing with only a reply limit: the global value still gives the window.
        listed_limits("test-listed-reply-only", Limits { context_window: None, max_output: Some(8_192) });
        let info = info_with_fallback_window("test-listed-reply-only", set_for_a_small_model);
        assert_eq!((info.context_window, info.max_output), (100_000, 8_192));
        // A known model keeps its table window.
        let known = model_info("claude-haiku-4-5").expect("in the table").context_window;
        assert_eq!(info_with_fallback_window("claude-haiku-4-5", set_for_a_small_model).context_window, known);
        // modelLimits, the user's setting for this one model, wins over its listing.
        configure_limits("test-configured-and-listed", Limits { context_window: Some(64_000), max_output: None });
        listed_limits("test-configured-and-listed", Limits { context_window: Some(1_000_000), max_output: None });
        assert_eq!(
            info_with_fallback_window("test-configured-and-listed", set_for_a_small_model).context_window,
            64_000
        );
    }

    #[test]
    fn aliases_resolve() {
        assert_eq!(resolve_model("opus"), "claude-opus-5-5");
        assert_eq!(resolve_model("Sonnet"), "claude-sonnet-5-5");
        assert_eq!(resolve_model("claude-opus-4-8"), "claude-opus-4-8");
        assert_eq!(resolve_model("my-local-model"), "my-local-model");
    }

    #[test]
    fn dated_ids_match_their_family() {
        assert_eq!(model_info("claude-haiku-4-5-20251001").unwrap().id, "claude-haiku-4-5");
        assert_eq!(model_info("claude-opus-5").unwrap().id, "claude-opus-5");
        assert!(model_info("gpt-x").is_none());
    }

    #[test]
    fn cost_is_per_million() {
        let m = model_info("claude-opus-5-5").unwrap();
        let u = Usage { input_tokens: 1_000_000, output_tokens: 1_000_000, ..Default::default() };
        assert!((m.cost(&u) - 24.0).abs() < 1e-9);
    }
}
