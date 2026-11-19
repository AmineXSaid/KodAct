//! What each built-in command does.

use std::fmt::Write as _;

use kodact_types::MessageContent;

use super::{parse, Builtin, Invocation, BUILTINS};
use crate::driver::Driver;

/// What a command asks the driver to do next.
#[derive(Debug, PartialEq)]
pub enum Exec {
    /// Send this to the model as the user's prompt.
    Submit(MessageContent),
    /// Answer locally; nothing goes to the model.
    Local { text: String, is_error: bool },
    /// End the session.
    Exit,
}

pub(super) fn ok(text: impl Into<String>) -> Exec {
    Exec::Local { text: text.into(), is_error: false }
}

pub(super) fn err(text: impl Into<String>) -> Exec {
    Exec::Local { text: text.into(), is_error: true }
}

/// Invoke each skill of `/a /b args` (`kodact_agents::invoke`) and join what
/// they give: a `context: fork` skill runs in a sub-agent first, and its
/// report is what joins.
pub(crate) async fn invoke_skills(d: &Driver, chain: &[kodact_agents::SkillDef], args: &str) -> Result<String, String> {
    use kodact_agents::invoke::{invoke, Invoked, Session};
    use kodact_tools::Tool as _;
    let handle = d.engine.handle();
    let ctx = d.engine.tool_ctx().clone();
    let session = Session { handle: Some(&handle), hooks: Some(&d.agent_rt.hooks) };
    let mut parts = vec![];
    for s in chain {
        let invoked = match invoke(s, args, &ctx, session).await {
            Ok(v) => v,
            Err(e) => {
                handle.end_turn_overrides();
                return Err(e);
            }
        };
        match invoked {
            Invoked::Text(t) => parts.push(t),
            Invoked::Fork { agent, prompt } => {
                let task = kodact_agents::TaskTool { rt: d.agent_rt.clone() };
                let input = serde_json::json!({"description": format!("/{}", s.name), "prompt": prompt, "subagent_type": agent});
                if let Err(e) = task.validate(&input, &ctx) {
                    handle.end_turn_overrides();
                    return Err(format!("/{}: {e}", s.name));
                }
                let out = task.call(input, &ctx).await;
                if out.is_error {
                    handle.end_turn_overrides();
                    return Err(format!("/{}: {}", s.name, out.text_content()));
                }
                parts.push(format!(
                    "The /{} skill ran in a sub-agent ({agent}) and reported:\n\n{}",
                    s.name,
                    out.text_content()
                ));
            }
        }
    }
    Ok(parts.join("\n\n"))
}

enum InlineOwned {
    Custom(kodact_agents::CommandDef),
    Skill(Box<kodact_agents::SkillDef>),
    McpPrompt,
}

/// Expand any mid-prompt `/skill`, `/custom-command`, or `/mcp__*` invocations in `text`
/// into a single `<system-reminder>` block while leaving the user's original prompt intact.
pub(crate) async fn expand_inline(
    d: &Driver,
    text: &str,
    skip: &[&str],
    only_model_invocable: bool,
) -> Result<Option<String>, String> {
    use super::{inline_invocations, InlineTarget};
    let (invocations, stripped) = inline_invocations(text, &d.catalog, skip);
    let items: Vec<(String, InlineOwned, String)> = invocations
        .into_iter()
        .filter_map(|inv| {
            let owned = match inv.target {
                InlineTarget::Custom(def) => InlineOwned::Custom(def.clone()),
                InlineTarget::Skill(s) => {
                    if only_model_invocable && !s.model_invocable {
                        return None;
                    }
                    InlineOwned::Skill(Box::new(s.clone()))
                }
                InlineTarget::McpPrompt => {
                    if only_model_invocable {
                        return None;
                    }
                    InlineOwned::McpPrompt
                }
            };
            Some((inv.name, owned, inv.after.to_string()))
        })
        .collect();
    if items.is_empty() {
        return Ok(None);
    }

    let cwd = d.info.cwd.clone();
    let shell = d.engine.tool_ctx().shell.clone();
    let mut sections = Vec::with_capacity(items.len());
    for (name, target, after) in items {
        let body = match target {
            InlineOwned::Custom(def) => {
                let args = if after.is_empty() && kodact_agents::commands::uses_arguments(&def.body) {
                    stripped.as_str()
                } else {
                    after.as_str()
                };
                match kodact_agents::commands::expand(&def, args, &cwd, &shell).await {
                    Ok(p) => p,
                    Err(e) => {
                        d.engine.handle().end_turn_overrides();
                        return Err(e);
                    }
                }
            }
            InlineOwned::Skill(s) => {
                let args = if !s.fork && !kodact_agents::substitute::uses_arguments(&s.body, &s.arguments) {
                    ""
                } else if after.is_empty() {
                    stripped.as_str()
                } else {
                    after.as_str()
                };
                invoke_skills(d, &[*s], args).await?
            }
            InlineOwned::McpPrompt => {
                let Some(m) = d.catalog.mcp.clone() else { continue };
                match m.get_prompt(&name, &after).await {
                    Some(Ok(p)) => p,
                    Some(Err(e)) => {
                        d.engine.handle().end_turn_overrides();
                        return Err(format!("/{name}: {e}"));
                    }
                    None => continue,
                }
            }
        };
        if !body.trim().is_empty() {
            sections.push(format!("### /{name}\n{body}"));
        }
    }
    if sections.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!("<system-reminder>\n{}\n</system-reminder>", sections.join("\n\n"))))
}

fn with_inline_note(primary: String, note: Option<String>) -> Exec {
    match note {
        Some(n) => Exec::Submit(MessageContent::Blocks(vec![
            kodact_types::ContentBlock::text(primary),
            kodact_types::ContentBlock::text(n),
        ])),
        None => Exec::Submit(MessageContent::Text(primary)),
    }
}

/// Run the slash command in `text` (a message starting with `/`).
pub async fn execute(d: &mut Driver, text: &str) -> Exec {
    let cwd = d.info.cwd.clone();
    let (id, args) = match parse(text, &d.catalog) {
        Invocation::NotACommand => return Exec::Submit(MessageContent::Text(text.to_string())),
        Invocation::Unknown(name) => return err(format!("Unknown command: /{name}")),
        Invocation::Custom { def, args } => {
            let def = def.clone();
            let args = args.to_string();
            let shell = d.engine.tool_ctx().shell.clone();
            let primary = match kodact_agents::commands::expand(&def, &args, &cwd, &shell).await {
                Ok(p) => p,
                Err(e) => return err(e),
            };
            return match expand_inline(d, &args, &[def.name.as_str()], false).await {
                Ok(note) => with_inline_note(primary, note),
                Err(e) => err(e),
            };
        }
        Invocation::Skills { chain, args } => {
            let chain: Vec<kodact_agents::SkillDef> = chain.into_iter().cloned().collect();
            let args = args.to_string();
            let primary = match invoke_skills(d, &chain, &args).await {
                Ok(text) => text,
                Err(e) => return err(e),
            };
            let skip: Vec<&str> = chain.iter().map(|s| s.name.as_str()).collect();
            return match expand_inline(d, &args, &skip, false).await {
                Ok(note) => with_inline_note(primary, note),
                Err(e) => err(e),
            };
        }
        Invocation::McpPrompt { name, args } => {
            let name = name.to_string();
            let args = args.to_string();
            let Some(m) = d.catalog.mcp.clone() else { return err(format!("Unknown command: /{name}")) };
            let primary = match m.get_prompt(&name, &args).await {
                Some(Ok(p)) => p,
                Some(Err(e)) => return err(format!("/{name}: {e}")),
                None => return err(format!("Unknown command: /{name}")),
            };
            return match expand_inline(d, &args, &[name.as_str()], false).await {
                Ok(note) => with_inline_note(primary, note),
                Err(e) => err(e),
            };
        }
        Invocation::Builtin { spec, args } => (spec.id, args.to_string()),
    };
    let args = args.trim();
    match id {
        Builtin::Help => ok(d.catalog.help(d.surface)),
        Builtin::Exit => Exec::Exit,
        Builtin::Clear => super::switching::clear(d, args).await,
        Builtin::Compact => match d.engine.compact(Some(args).filter(|a| !a.is_empty())).await {
            Ok(info) => ok(format!("Compacted the conversation (about {} tokens before).", info.pre_tokens)),
            Err(e) => err(format!("Could not compact: {e}")),
        },
        // Immediate commands read the session view, idle or mid-turn (C17): one implementation.
        Builtin::Usage | Builtin::Status | Builtin::Tasks | Builtin::Context | Builtin::Mcp | Builtin::Btw => {
            d.sync_view();
            // Ctrl-C (or a host's interrupt) stops a side question, as it stops a turn.
            let cancel = if id == Builtin::Btw { d.handle().new_token() } else { Default::default() };
            let r = super::immediate::run(&d.view(), id, args, &cancel).await;
            d.sync_view();
            r
        }
        Builtin::Skills => ok(skills(d)),
        Builtin::Agents => super::agents::run(d, args).await,
        Builtin::Hooks => super::hooks::run(d, args).await,
        Builtin::Memory => ok(memory(d)),
        Builtin::Doctor => doctor(d),
        Builtin::ReleaseNotes => ok(release_notes()),
        Builtin::Plugin => match args {
            "" | "list" => ok(plugins(d)),
            _ => err("Plugin marketplaces aren't supported. Load a plugin directory with --plugin-dir or the pluginDirs setting."),
        },
        Builtin::Subtask => subtask(d, args),
        Builtin::Model => super::settings::model(d, args).await,
        Builtin::Effort => super::settings::effort(d, args),
        Builtin::Fast => super::settings::fast(d, args),
        Builtin::Config => super::settings::config(d, args),
        Builtin::OutputStyle => super::settings::output_style(d, args),
        Builtin::Autocompact => super::settings::autocompact(d, args),
        Builtin::Sandbox => super::settings::sandbox(d, args),
        Builtin::Rename => super::session::rename(d, args).await,
        Builtin::Export => super::session::export(d, args),
        Builtin::Diff => match args {
            "" => super::session::diff(d),
            _ => err("Usage: /diff"),
        },
        Builtin::Debug => super::session::debug(d, args),
        Builtin::Permissions => super::session::permissions(d, args),
        Builtin::AddDir => super::session::add_dir(d, args),
        Builtin::Goal => super::session::goal(d, args),
        Builtin::Recap => match args {
            "" => super::session::recap(d).await,
            _ => err("Usage: /recap"),
        },
        Builtin::Plan => super::session::plan(d, args),
        Builtin::Loop => super::looping::run(d, args).await,
        Builtin::Feedback => super::feedback::run(d, args),
        Builtin::Import => super::importing::run(d, args),
        Builtin::Advisor => super::settings::advisor(d, args),
        Builtin::Rewind => super::switching::rewind(d, args).await,
        Builtin::Resume => super::switching::resume(d, args).await,
        Builtin::Branch => super::switching::branch(d, args).await,
        Builtin::Cd => super::switching::cd(d, args).await,
        Builtin::Theme => super::settings::theme(d, args),
        Builtin::Statusline => super::settings::statusline(d, args),
        // The terminal UI answers these itself: they need the terminal.
        Builtin::Copy | Builtin::Keybindings | Builtin::TerminalSetup | Builtin::Color | Builtin::Focus => {
            let name = BUILTINS.iter().find(|c| c.id == id).map(|c| c.name).unwrap_or("");
            err(format!("/{name} works only in the terminal UI."))
        }
        Builtin::ReloadSkills | Builtin::ReloadPlugins => {
            if !d.can_switch() {
                return err("Reloading isn't available here.");
            }
            let what = if id == Builtin::ReloadPlugins { "plugins" } else { "skills" };
            super::switching::reload(d, what).await
        }
    }
}

pub(super) fn thousands(n: u64) -> String {
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

pub(crate) fn duration(d: std::time::Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

pub(super) fn tokens_of(text: &str) -> usize {
    text.len() / 4
}

fn skills(d: &Driver) -> String {
    if d.catalog.skills.is_empty() {
        return "No skills found. Add one as .kodact/skills/<name>/SKILL.md (or in your config directory).".into();
    }
    let mut s = format!("{} skill(s):\n", d.catalog.skills.len());
    for k in &d.catalog.skills {
        let who = match (k.user_invocable, k.model_invocable) {
            (true, true) => "you and KodAct",
            (true, false) => "you only",
            (false, true) => "KodAct only",
            (false, false) => "hidden",
        };
        let _ = writeln!(
            s,
            "  {} - {} [{}; {}; ~{} tokens when loaded]",
            k.name,
            k.description,
            k.source,
            who,
            tokens_of(&k.body)
        );
    }
    s.trim_end().to_string()
}

fn memory(d: &Driver) -> String {
    let files = kodact_config::load_memory(&d.info.cwd);
    if files.is_empty() {
        return "No memory files. Create KODACT.md (or AGENTS.md) in the project, or KODACT.md in your config directory."
            .into();
    }
    let mut s = String::from("Memory files, loaded in this order (later ones take precedence):\n");
    for f in &files {
        let _ = writeln!(s, "  {} ({:?}, ~{} tokens)", f.path.display(), f.kind, tokens_of(&f.content));
    }
    s.trim_end().to_string()
}

fn doctor(d: &Driver) -> Exec {
    let checks = doctor_checks(d);
    let failed = checks.iter().any(|c| !c.ok);
    Exec::Local { text: crate::doctor::render(&checks), is_error: failed }
}

/// `kodact doctor`'s checks plus this session's: warnings, MCP servers, model pricing.
pub(super) fn doctor_checks(d: &Driver) -> Vec<crate::doctor::Check> {
    let mut checks = crate::doctor::checks(&d.info.cwd);
    // The model row below says what an unknown model means; don't say it twice.
    let warnings: Vec<&String> =
        d.info.warnings.iter().filter(|w| !w.starts_with(crate::UNKNOWN_MODEL_PREFIX)).collect();
    checks.push(crate::doctor::Check {
        note: false,
        ok: warnings.is_empty(),
        name: "session",
        detail: if warnings.is_empty() {
            "no warnings".into()
        } else {
            warnings.iter().map(|w| w.as_str()).collect::<Vec<_>>().join("; ")
        },
    });
    if let Some(m) = &d.catalog.mcp {
        let failed = m.warnings();
        checks.push(crate::doctor::Check {
            note: false,
            ok: failed.is_empty(),
            name: "mcp",
            detail: if failed.is_empty() { format!("{} server(s) fine", m.servers.len()) } else { failed.join("; ") },
        });
    }
    let model = d.handle().model();
    let priced = d.engine.has_price(&model);
    let guessed = kodact_api::models::limits_source(&model) == kodact_api::models::LimitsSource::Guessed;
    let mut unknown = vec![];
    if guessed {
        let info = kodact_api::models::model_info_or_default(&model);
        unknown.push(format!(
            "limits guessed ({} context, {} output; set \"modelLimits\")",
            thousands(info.context_window),
            thousands(u64::from(info.max_output))
        ));
    }
    if !priced {
        unknown.push("no price, so costs show as unknown (set \"modelPricing\")".into());
    }
    checks.push(crate::doctor::Check {
        note: !unknown.is_empty(),
        ok: true,
        name: "model",
        detail: if unknown.is_empty() {
            format!("{model} (known limits and pricing)")
        } else {
            format!("{model}: {}", unknown.join("; "))
        },
    });
    checks
}

fn release_notes() -> String {
    include_str!(env!("KODACT_CHANGELOG")).trim().to_string()
}

fn plugins(d: &Driver) -> String {
    if d.catalog.plugins.is_empty() {
        return "No plugins loaded. Load one with --plugin-dir <dir> or the pluginDirs setting.".into();
    }
    let mut s = String::from("Plugins:\n");
    for p in &d.catalog.plugins {
        let version = p.version.as_deref().map(|v| format!(" {v}")).unwrap_or_default();
        let _ = writeln!(s, "  {}{version} - {} ({})", p.name, p.description, p.dir.display());
    }
    s.trim_end().to_string()
}

fn subtask(d: &mut Driver, args: &str) -> Exec {
    if args.is_empty() {
        return err("Usage: /subtask <task>. A background agent works on it from a copy of this conversation \
                    while you go on; its report comes back when it's done.");
    }
    match d.start_subtask(args) {
        Ok(id) => ok(format!(
            "Started {id} in the background: {args}. Its report comes back to this conversation when it's done; \
             /tasks stop {id} ends it."
        )),
        Err(e) => err(format!("Could not start a subtask: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers_and_durations() {
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(duration(std::time::Duration::from_secs(75)), "1m 15s");
        assert_eq!(duration(std::time::Duration::from_secs(7322)), "2h 02m");
    }
}
