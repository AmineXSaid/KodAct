//! What a failed command's exit code means, when it says more than the output does. Each hint
//! is added only to the failure it explains; a command that succeeds, or fails another way,
//! gets nothing.
//!
//! - 127, a program that isn't installed: which installed programs do the same job, so the
//!   next call uses one instead of probing for them one by one (minimal containers often lack
//!   `xxd`, `file`, `python3` or `rg`).
//! - 137 (128 + SIGKILL): the system killed the command, which, when the person didn't
//!   stop it, is almost always the kernel ending a process out of memory. Run the same way, it
//!   fails the same way.

/// Programs that do a job, in the order worth trying. A missing program is matched to the
/// first group it belongs to; the hint lists that group's installed members.
const GROUPS: &[(&str, &[&str])] = &[
    ("hex dumps", &["xxd", "hexdump", "od"]),
    ("scripts", &["python3", "python", "perl", "ruby", "node", "php"]),
    ("search", &["rg", "ag", "grep"]),
    ("finding files", &["fd", "fdfind", "find"]),
    ("JSON", &["jq", "python3", "node"]),
    ("downloads", &["curl", "wget", "python3"]),
    ("C compilers", &["gcc", "cc", "clang", "tcc"]),
    ("C++ compilers", &["g++", "c++", "clang++"]),
    ("file types", &["file", "xxd", "od"]),
    ("archives", &["tar", "unzip", "7z", "python3"]),
    ("tests", &["pytest", "python3"]),
    ("package installs", &["pip3", "pip", "uv", "apt-get", "apk", "dnf"]),
];

/// The program names a shell says it couldn't find: `bash: line 1: xxd: command not found`,
/// `sh: 1: xxd: not found`, `zsh: command not found: xxd`.
pub fn missing_commands(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        let name = if let Some(rest) = l.strip_suffix(": command not found").or_else(|| l.strip_suffix(": not found")) {
            rest.rsplit(": ").next().unwrap_or(rest)
        } else if let Some(i) = l.find("command not found: ") {
            &l[i + "command not found: ".len()..]
        } else {
            continue;
        };
        let name = name.trim();
        let plain = !name.is_empty() && name.len() <= 40 && !name.contains(char::is_whitespace);
        if plain && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// For each missing program with a group, the installed programs of that group (by `found`).
pub fn alternatives(missing: &[String], found: impl Fn(&str) -> bool) -> Option<String> {
    let mut parts = Vec::new();
    for name in missing {
        let Some((job, members)) = GROUPS.iter().find(|(_, m)| m.contains(&name.as_str())) else { continue };
        let have: Vec<&str> = members.iter().copied().filter(|m| *m != name && found(m)).collect();
        parts.push(if have.is_empty() {
            format!("{name} ({job}): none of {} is installed either", members.join(", "))
        } else {
            format!("instead of {name} ({job}): {}", have.join(", "))
        });
    }
    (!parts.is_empty()).then(|| format!("Installed here, {}.", parts.join("; ")))
}

/// The hint for a failed command: `code` is its exit code (`None`: ended by a signal, not explained), `text`
/// its output, `found` says whether a program is on PATH.
pub fn explain(code: Option<i32>, text: &str, found: impl Fn(&str) -> bool) -> Option<String> {
    match code {
        Some(127) => alternatives(&missing_commands(text), found),
        Some(137) => Some(
            "The command was killed (SIGKILL, exit 137). Unless you stopped it, that is almost always the \
             system ending a process that ran out of memory: the same command will fail the same way. Work \
             on the data in parts, stream it instead of loading it whole, or use less memory."
                .to_string(),
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_programs_are_read_from_each_shell() {
        let text = "/bin/bash: line 1: xxd: command not found\nsh: 1: file: not found\nzsh: command not found: rg\n\
                    /bin/bash: line 2: xxd: command not found";
        assert_eq!(missing_commands(text), ["xxd", "file", "rg"]);
        assert!(missing_commands("error: not found in the index\nNo such file or directory").is_empty());
    }

    /// The case seen on a minimal container: xxd, file and python3 missing; od, perl and gcc there.
    #[test]
    fn a_missing_program_gets_the_installed_ones_that_do_its_job() {
        let installed = |p: &str| ["od", "perl", "gcc", "grep"].contains(&p);
        let missing = vec!["xxd".to_string(), "python3".to_string(), "jq".to_string(), "frobnicate".to_string()];
        let hint = alternatives(&missing, installed).unwrap();
        assert!(hint.contains("instead of xxd (hex dumps): od"), "{hint}");
        assert!(hint.contains("instead of python3 (scripts): perl"), "{hint}");
        assert!(hint.contains("jq (JSON): none of jq, python3, node is installed either"), "{hint}");
        assert!(!hint.contains("frobnicate"), "a program of no known job is left out: {hint}");
        assert_eq!(alternatives(&["frobnicate".into()], installed), None);
    }

    #[test]
    fn only_the_failures_it_explains_get_a_hint() {
        let none = |_: &str| false;
        assert!(explain(Some(137), "Killed", none).unwrap().contains("ran out of memory"));
        assert_eq!(explain(None, "", none), None, "a signal of unknown kind isn't guessed at");
        assert_eq!(explain(Some(1), "assertion failed", none), None);
        assert_eq!(explain(Some(2), "bash: xxd: command not found", none), None);
        assert!(explain(Some(127), "bash: xxd: command not found", |p| p == "od").unwrap().contains("od"));
    }
}
