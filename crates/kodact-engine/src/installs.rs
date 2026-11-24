//! Python packages the model installed with no version, which nothing in the project pins
//! (`nudges.unpinnedInstalls`, GOALS pillar: verification).
//!
//! `pip install transformers` installs the latest release. Checks run afterwards show the code
//! works with that release, and no other: where the code runs next (the person's environment,
//! a CI job, a test harness) installs its own, and an API that changed between the two breaks
//! it (2026-11-24: `create_causal_mask(inputs_embeds=...)`, written and checked against
//! transformers 5.24.0, failed under the 4.55.0 the code later ran with). The model saw the
//! install's output, but nothing tells it that its evidence covers one version. Before the turn
//! ends, once a run, it is told which packages it installed that way and the versions its
//! checks ran against: a fact about the evidence, not advice.
//!
//! Read from the command's own words only, failing safe (docs/goal.md, rule 10): a package
//! with a version (`x==1.2`, `x>=1`, `x@url`), from a requirements, constraints or editable
//! file, a path or an archive, or one the project's manifests name (requirements*.txt,
//! pyproject.toml, setup.cfg, setup.py, Pipfile, environment.yml, uv.lock, poetry.lock), is
//! pinned by someone, and says nothing.

use std::path::Path;

/// One package installed with no version, and the version installed when the command's output
/// said (`Successfully installed transformers-5.24.0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unpinned {
    pub name: String,
    pub version: Option<String>,
}

/// `pip`'s options that take a value: the value is not a package.
const VALUE_OPTIONS: [&str; 22] = [
    "-i",
    "--index-url",
    "--extra-index-url",
    "-f",
    "--find-links",
    "-t",
    "--target",
    "--prefix",
    "--root",
    "--platform",
    "--python-version",
    "--implementation",
    "--abi",
    "--only-binary",
    "--no-binary",
    "--trusted-host",
    "--cache-dir",
    "--src",
    "--upgrade-strategy",
    "--progress-bar",
    "--log",
    "--python",
];

/// Options whose value names a file of requirements: what it lists is pinned there (or not)
/// by the file, which is the project's decision.
const FILE_OPTIONS: [&str; 6] = ["-r", "--requirement", "-c", "--constraint", "-e", "--editable"];

/// The packages `command` installs with pip with no version: `pip install x`, `pip3 install`,
/// `python -m pip install`, `uv pip install`. Lower case, extras dropped (`x[cli]` is `x`).
pub(crate) fn unpinned_in(command: &str) -> Vec<String> {
    let mut out = vec![];
    for part in kodact_permissions::split_compound(command) {
        let words = words(&part);
        let Some(at) = install_at(&words) else { continue };
        let mut skip_next = false;
        for w in &words[at + 1..] {
            if skip_next {
                skip_next = false;
                continue;
            }
            if FILE_OPTIONS.contains(&w.as_str()) || VALUE_OPTIONS.contains(&w.as_str()) {
                skip_next = true;
                continue;
            }
            if w.starts_with('-') {
                continue;
            }
            if let Some(name) = bare_name(w) {
                if !out.contains(&name) {
                    out.push(name);
                }
            }
        }
    }
    out
}

/// Where `install` is in a pip command's words, when it is one.
fn install_at(words: &[String]) -> Option<usize> {
    // Leading `VAR=value` assignments and `sudo` don't change the command.
    let start = words.iter().position(|w| !(w.contains('=') && !w.starts_with('-')) && w != "sudo")?;
    let w = &words[start..];
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_string();
    let first = base(w.first()?);
    let is_pip =
        |s: &str| s == "pip" || (s.starts_with("pip") && s[3..].chars().all(|c| c.is_ascii_digit() || c == '.'));
    let is_python =
        |s: &str| s == "python" || (s.starts_with("python") && s[6..].chars().all(|c| c.is_ascii_digit() || c == '.'));
    let sub = if is_pip(&first) {
        1
    } else if is_python(&first) && w.get(1).map(String::as_str) == Some("-m") && w.get(2).is_some_and(|m| is_pip(m)) {
        3
    } else if first == "uv" && w.get(1).map(String::as_str) == Some("pip") {
        2
    } else {
        return None;
    };
    (w.get(sub).map(String::as_str) == Some("install")).then_some(start + sub)
}

/// The name of a package given with no version, or None for anything pinned or not a package
/// name (a path, an archive, a URL, `.`).
fn bare_name(spec: &str) -> Option<String> {
    let s = spec.trim();
    if s.is_empty()
        || s.contains(['/', '\\', '@', '=', '<', '>', '~', '!', ';', ':', '$', '*'])
        || s.starts_with('.')
        || [".whl", ".zip", ".gz", ".tgz", ".bz2", ".tar"].iter().any(|e| s.ends_with(e))
    {
        return None;
    }
    let name = s.split('[').next()?.trim();
    let valid = !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric());
    valid.then(|| name.to_ascii_lowercase())
}

/// The versions pip's output says it installed: `Successfully installed a-1.0 b_c-2.3.4`.
pub(crate) fn installed_versions(output: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    for line in output.lines() {
        let Some(rest) = line.trim().strip_prefix("Successfully installed ") else { continue };
        for item in rest.split_whitespace() {
            // The version is what follows the last `-` that starts a digit.
            if let Some(i) = item
                .rmatch_indices('-')
                .map(|(i, _)| i)
                .find(|i| item[i + 1..].chars().next().is_some_and(|c| c.is_ascii_digit()))
            {
                out.push((normalize(&item[..i]), item[i + 1..].to_string()));
            }
        }
    }
    out
}

/// A package name as pip compares them: lower case, `-`, `_` and `.` alike.
fn normalize(name: &str) -> String {
    name.to_ascii_lowercase().replace(['_', '.'], "-")
}

/// The project's manifests: where a package it depends on is named.
const MANIFESTS: [&str; 8] = [
    "pyproject.toml",
    "setup.cfg",
    "setup.py",
    "Pipfile",
    "environment.yml",
    "environment.yaml",
    "uv.lock",
    "poetry.lock",
];

/// Whether the project at `root` names `name` in a manifest or a requirements file at its root:
/// then the project decides the version, not the model.
pub(crate) fn project_names(root: &Path, name: &str) -> bool {
    let wanted = normalize(name);
    let mut files: Vec<std::path::PathBuf> = MANIFESTS.iter().map(|m| root.join(m)).collect();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            if n.starts_with("requirements") && n.ends_with(".txt") {
                files.push(e.path());
            }
        }
    }
    files.iter().any(|f| {
        std::fs::read_to_string(f).is_ok_and(|text| {
            // Whole words only: `torch` isn't named by `torchvision`.
            normalize(&text).split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')).any(|w| w == wanted)
        })
    })
}

/// The note, and its record for the transcript.
pub(crate) fn note(list: &[Unpinned]) -> (String, serde_json::Value) {
    let shown: Vec<String> = list
        .iter()
        .map(|u| match &u.version {
            Some(v) => format!("{} {v}", u.name),
            None => format!("{} (its latest release)", u.name),
        })
        .collect();
    let (them, that) = if list.len() == 1 { ("it", "that version") } else { ("them", "those versions") };
    let text = format!(
        "<system-reminder>\nYou installed {} without a version, and nothing in the project pins {them}: \
         your checks ran against {} only. Where this code runs next, another version may be installed. \
         If the code depends on an API of {that}, make it work with the versions it may meet, or say in your \
         answer which version it was checked with.\n</system-reminder>",
        list.iter().map(|u| u.name.as_str()).collect::<Vec<_>>().join(", "),
        shown.join(", "),
    );
    let record = serde_json::json!({
        "kind": crate::nudges::Nudge::UnpinnedInstalls.as_str(),
        "packages": list.iter().map(|u| serde_json::json!({"name": u.name, "version": u.version})).collect::<Vec<_>>(),
    });
    (text, record)
}

/// A command's words, with quotes and backslashes taken off.
fn words(cmd: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = cmd.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                started = true;
            }
            (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    started = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_packages_installed_with_no_version() {
        assert_eq!(unpinned_in("pip install --break-system-packages transformers 2>&1 | tail -6"), ["transformers"]);
        assert_eq!(unpinned_in("python3 -m pip install -q numpy 'pandas[excel]'"), ["numpy", "pandas"]);
        assert_eq!(unpinned_in("uv pip install --system Requests"), ["requests"]);
        assert_eq!(unpinned_in("cd /app && sudo pip3.12 install -U torch"), ["torch"]);
        assert_eq!(unpinned_in("PIP_NO_INPUT=1 pip install rich"), ["rich"]);
    }

    #[test]
    fn a_version_a_file_or_a_path_is_pinned_by_someone() {
        for c in [
            "pip install transformers==4.55.0",
            "pip install 'numpy>=2' 'pandas<3' 'x~=1.2' 'y!=2'",
            "pip install -r requirements.txt",
            "pip install -c constraints.txt -e .",
            "pip install ./dist/pkg-1.0-py3-none-any.whl",
            "pip install git+https://example.com/pkg.git",
            "pip install pkg@https://example.com/pkg.tar.gz",
            "pip install --find-links /wheels --index-url https://pypi.example",
            "pip download torch",
            "pip show transformers",
            "npm install left-pad",
            "echo pip install x",
        ] {
            assert!(unpinned_in(c).is_empty(), "{c}: {:?}", unpinned_in(c));
        }
        // An option's value is never a package.
        assert_eq!(unpinned_in("pip install --target /opt/lib -i https://pypi.example six"), ["six"]);
    }

    #[test]
    fn reads_the_versions_pip_installed() {
        let out = "Collecting transformers\n...\nSuccessfully installed huggingface-hub-0.36.0 safetensors-0.6.2 transformers-5.24.0\n";
        let v = installed_versions(out);
        assert!(v.contains(&("transformers".into(), "5.24.0".into())));
        assert!(v.contains(&("huggingface-hub".into(), "0.36.0".into())));
        assert!(installed_versions("Requirement already satisfied: torch").is_empty());
    }

    #[test]
    fn a_package_the_project_names_is_the_projects_to_pin() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("requirements-dev.txt"), "pytest\nTorch_Vision\n").unwrap();
        std::fs::write(d.path().join("pyproject.toml"), "[project]\ndependencies = [\"numpy>=2\"]\n").unwrap();
        assert!(project_names(d.path(), "numpy"));
        assert!(project_names(d.path(), "pytest"));
        assert!(project_names(d.path(), "torch-vision"));
        // A word that only contains the name doesn't name it.
        assert!(!project_names(d.path(), "torch"));
        assert!(!project_names(d.path(), "transformers"));
    }

    #[test]
    fn the_note_names_the_versions_the_checks_covered() {
        let (text, record) = note(&[Unpinned { name: "transformers".into(), version: Some("5.24.0".into()) }]);
        assert!(text.contains("You installed transformers without a version"), "{text}");
        assert!(text.contains("your checks ran against transformers 5.24.0 only"), "{text}");
        assert_eq!(record["kind"], "unpinned_installs");
        assert_eq!(record["packages"][0]["version"], "5.24.0");
        let (two, _) = note(&[
            Unpinned { name: "a".into(), version: None },
            Unpinned { name: "b".into(), version: Some("2".into()) },
        ]);
        assert!(two.contains("a (its latest release), b 2 only"), "{two}");
        assert!(two.contains("pins them"), "{two}");
    }
}
