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

/// A package manager stopped while it was changing the system: what it may have left behind and
/// the command that repairs it. `None` for anything else, and for runs that only read
/// (`apt-get update`, `apt list`, `dnf search`). Said when KodAct itself stops such a command:
/// a half-configured package database fails every later install, on the person's own machine
/// too, until it is repaired by hand.
pub fn stopped_package_manager(command: &str) -> Option<String> {
    const CHANGES: &[&str] = &[
        "install",
        "reinstall",
        "remove",
        "purge",
        "autoremove",
        "upgrade",
        "dist-upgrade",
        "full-upgrade",
        "update",
        "add",
        "del",
        "erase",
        "downgrade",
        "groupinstall",
        "build-dep",
    ];
    for part in kodact_permissions::split_compound(command) {
        let mut words = part.split_whitespace().peekable();
        // `sudo`, `env`, `VAR=value`, `timeout 600`, `nohup`, `time`, `nice` in front.
        while let Some(w) = words.peek() {
            let wrapper = matches!(*w, "sudo" | "env" | "nohup" | "time" | "nice" | "command" | "exec" | "timeout");
            let number = w.chars().all(|c| c.is_ascii_digit() || c == '.');
            if wrapper || number || w.contains('=') || w.starts_with('-') {
                words.next();
            } else {
                break;
            }
        }
        let Some(prog) = words.next().map(|w| w.rsplit('/').next().unwrap_or(w)) else { continue };
        let args: Vec<&str> = words.collect();
        let sub = args.iter().find(|a| !a.starts_with('-')).copied().unwrap_or("");
        let changes = CHANGES.contains(&sub);
        let (what, repair) = match prog {
            // `apt-get update` only refreshes the package lists: nothing to repair.
            "apt-get" | "apt" | "aptitude" if changes && sub != "update" => {
                ("apt", "run `dpkg --configure -a`, then `apt-get install -f`")
            }
            "dpkg"
                if args.iter().any(|a| {
                    matches!(*a, "-i" | "--install" | "--configure" | "-r" | "--remove" | "-P" | "--purge" | "--unpack")
                }) =>
            {
                ("dpkg", "run `dpkg --configure -a`")
            }
            "apk" if (changes && sub != "update") || sub == "fix" => ("apk", "run `apk fix`"),
            "pacman" if args.iter().any(|a| a.starts_with("-S") || a.starts_with("-R") || a.starts_with("-U")) => {
                ("pacman", "remove the stale lock `/var/lib/pacman/db.lck` if no pacman is running, then run it again")
            }
            "dnf" | "yum" | "zypper" | "microdnf" if changes => (
                "the package manager",
                "run the same command again and read what it reports before relying on what is installed",
            ),
            _ => continue,
        };
        return Some(format!(
            "It was stopped while {what} was installing or removing packages, which can leave the package database \
             half-updated so that later installs fail (for anyone using this machine, not only this session). \
             Before anything else, {repair}."
        ));
    }
    None
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

    #[test]
    fn a_stopped_package_manager_names_its_repair() {
        for (cmd, repair) in [
            ("apt-get install -y gcc g++ make", "dpkg --configure -a"),
            (
                "apt-get update -qq 2>&1 | tail -2 && apt-get install -y -qq gcc 2>&1 | tail -5; command -v gcc",
                "dpkg --configure -a",
            ),
            ("sudo DEBIAN_FRONTEND=noninteractive apt install -y curl", "dpkg --configure -a"),
            ("timeout 600 apt-get -y dist-upgrade", "dpkg --configure -a"),
            ("dpkg -i ./tool.deb", "dpkg --configure -a"),
            ("apk add --no-cache build-base", "apk fix"),
            ("pacman -Syu --noconfirm", "db.lck"),
            ("dnf install -y gcc", "run the same command again"),
        ] {
            let note = stopped_package_manager(cmd).unwrap_or_else(|| panic!("a note for: {cmd}"));
            assert!(note.contains(repair) && note.contains("half-updated"), "{cmd}: {note}");
        }
        for quiet in [
            "apt-get update",
            "apt list --installed",
            "apk update",
            "apt-cache search gcc",
            "dpkg -l | grep gcc",
            "dnf search gcc",
            "python3 -m http.server 8000",
            "npm install",
            "pip install numpy",
            "echo apt-get install gcc",
            "sleep 20",
        ] {
            assert_eq!(stopped_package_manager(quiet), None, "nothing to repair after: {quiet}");
        }
    }
}
