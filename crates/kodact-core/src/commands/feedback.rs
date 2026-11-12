//! `/feedback [description]` (`/bug`, `/share`): a bug-report bundle written
//! on this machine. Nothing is uploaded: the person decides what to share.
//!
//! The bundle, in `<state>/feedback/<time>-<session>/`:
//! - `report.txt`: the description and the setup (version, OS, model, mode);
//! - `doctor.txt`: the `/doctor` checks;
//! - `transcript.jsonl`: this session's transcript, secrets masked;
//! - `settings.json`: the effective settings, secrets masked.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use super::run::{doctor_checks, err, ok};
use super::Exec;
use crate::driver::Driver;

fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The transcript with secrets masked, one JSON object per line.
fn redacted_transcript(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else { return String::new() };
    let mut out = String::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => out.push_str(&kodact_config::redact_deep(&v).to_string()),
            Err(_) => out.push_str(&kodact_config::redact_text(line)),
        }
        out.push('\n');
    }
    out
}

fn report(d: &Driver, description: &str) -> String {
    let h = d.engine.handle();
    let rt = h.runtime();
    let mode = h.permissions.read().unwrap().mode.as_str();
    let mut s = String::new();
    let _ = writeln!(s, "Description:\n{}\n", if description.is_empty() { "(none given)" } else { description });
    let _ = writeln!(s, "KodAct:     {}", crate::VERSION);
    let _ = writeln!(s, "System:       {} {}", std::env::consts::OS, std::env::consts::ARCH);
    let _ = writeln!(s, "Session:      {}", d.info.session_id);
    let _ = writeln!(s, "Directory:    {}", d.info.cwd.display());
    let _ = writeln!(
        s,
        "Model:        {} (effort {}, fast mode {})",
        rt.model,
        rt.effort.as_deref().unwrap_or("default"),
        if rt.fast { "on" } else { "off" }
    );
    let _ = writeln!(s, "Provider:     {}", d.engine.provider_name());
    let _ = writeln!(s, "Permissions:  {mode}");
    let _ = writeln!(s, "Front end:    {:?}", d.surface);
    let st = &d.engine.state;
    let _ = writeln!(
        s,
        "Conversation: {} messages, about {} tokens of context, ${:.4} so far",
        st.messages.len(),
        st.context_tokens,
        st.total_cost_usd
    );
    if !d.info.warnings.is_empty() {
        let _ = writeln!(s, "Warnings:     {}", d.info.warnings.join("; "));
    }
    s
}

pub(super) fn run(d: &Driver, args: &str) -> Exec {
    let description = kodact_config::redact_text(args.trim());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let short = &d.info.session_id[..8.min(d.info.session_id.len())];
    let _ = kodact_config::private_state_dir();
    let dir: PathBuf = kodact_config::state_dir().join("feedback").join(format!("{stamp}-{short}"));
    let write = || -> std::io::Result<Vec<&'static str>> {
        private_dir(&dir)?;
        let mut files = vec!["report.txt", "doctor.txt", "settings.json"];
        // Warnings and checks can quote command lines and URLs: mask those too.
        std::fs::write(dir.join("report.txt"), kodact_config::redact_text(&report(d, &description)))?;
        let doctor = crate::doctor::render(&doctor_checks(d)) + "\n";
        std::fs::write(dir.join("doctor.txt"), kodact_config::redact_text(&doctor))?;
        let settings = kodact_config::redact_deep(&d.info.settings.merged);
        std::fs::write(dir.join("settings.json"), serde_json::to_string_pretty(&settings).unwrap_or_default() + "\n")?;
        if let Some(t) = d.engine.transcript().path() {
            std::fs::write(dir.join("transcript.jsonl"), redacted_transcript(t))?;
            files.push("transcript.jsonl");
        }
        Ok(files)
    };
    match write() {
        Ok(files) => {
            let mut text = format!(
                "Saved a feedback bundle in {}\n({}). Nothing was sent anywhere. Secrets KodAct recognizes are masked, \
                 but read the files before you share them.",
                dir.display(),
                files.join(", ")
            );
            if description.is_empty() {
                text.push_str("\nTip: /feedback <what went wrong> adds a description.");
            }
            ok(text)
        }
        Err(e) => err(format!("Could not write the feedback bundle in {}: {e}", dir.display())),
    }
}
