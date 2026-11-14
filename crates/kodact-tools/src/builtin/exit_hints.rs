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

/// A command that installs or removes packages: which manager, and what repairs its database
/// when it was stopped halfway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackageChange {
    /// `apt`, `dpkg`, `apk`, `pacman`, or `the package manager`.
    pub manager: &'static str,
    /// What to do after it was stopped halfway, as a clause: "run `dpkg --configure -a`".
    pub repair: &'static str,
}

impl PackageChange {
    /// Stopped at once (SIGTERM, then SIGKILL): what it may have left behind and the repair.
    pub fn forced_note(&self) -> String {
        format!(
            "It was stopped while {} was installing or removing packages, which can leave the package database \
             half-updated so that later installs fail (for anyone using this machine, not only this session). \
             Before anything else, {}.",
            self.manager, self.repair
        )
    }

    /// Asked to stop with SIGINT, and it ended.
    pub fn ended_note(&self) -> String {
        format!(
            "It was installing or removing packages, so it was stopped with SIGINT, which {} stops on without \
             breaking its package database: it finished the step it was on, then ended. Packages it hadn't reached \
             are not installed; run the install again for them.",
            self.manager
        )
    }

    /// Asked to stop with SIGINT, and still running after `waited`.
    pub fn finishing_note(&self, id: &str, waited: std::time::Duration) -> String {
        format!(
            "It is installing or removing packages, so it was asked to stop with SIGINT, which {} stops on without \
             breaking its package database: it finishes the step it is on first, and after {} it is still doing \
             that. It ends by itself, and you will be told when it does. Meanwhile don't start another install, \
             since it holds the package lock. KillShell {id} again stops it at once, which can leave the package \
             database half-updated.",
            self.manager,
            super::bash::duration(waited)
        )
    }
}

/// Each part of a compound command line as its program (no directory) and arguments, with the
/// wrappers in front left out: `sudo`, `env`, `VAR=value`, `timeout 600`, `nohup`, `time`, `nice`.
fn programs(command: &str) -> Vec<(String, Vec<String>)> {
    let mut out = vec![];
    for part in kodact_permissions::split_compound(command) {
        let mut words = part.split_whitespace().peekable();
        while let Some(w) = words.peek() {
            let wrapper = matches!(*w, "sudo" | "env" | "nohup" | "time" | "nice" | "command" | "exec" | "timeout");
            let number = w.chars().all(|c| c.is_ascii_digit() || c == '.');
            if wrapper || number || w.contains('=') || w.starts_with('-') {
                words.next();
            } else {
                break;
            }
        }
        if let Some(prog) = words.next().map(|w| w.rsplit('/').next().unwrap_or(w).to_string()) {
            out.push((prog, words.map(str::to_string).collect()));
        }
    }
    out
}

/// Whether a command line runs git.
pub fn runs_git(command: &str) -> bool {
    programs(command).iter().any(|(p, _)| p == "git")
}

/// The package change a command line makes, if any. `None` for anything else, and for runs that
/// only read. A half-configured package database fails every later install, on the person's own
/// machine too, until it is repaired by hand: KodAct stops such a command with SIGINT instead
/// of SIGTERM, and says what a stop may have left.
pub fn package_change(command: &str) -> Option<PackageChange> {
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
    for (prog, args) in programs(command) {
        let sub = args.iter().find(|a| !a.starts_with('-')).map(String::as_str).unwrap_or("");
        let changes = CHANGES.contains(&sub);
        let (manager, repair) = match prog.as_str() {
            // `apt-get update` only refreshes the package lists: nothing to repair.
            "apt-get" | "apt" | "aptitude" if changes && sub != "update" => {
                ("apt", "run `dpkg --configure -a`, then `apt-get install -f`")
            }
            "dpkg"
                if args.iter().any(|a| {
                    matches!(
                        a.as_str(),
                        "-i" | "--install" | "--configure" | "-r" | "--remove" | "-P" | "--purge" | "--unpack"
                    )
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
        return Some(PackageChange { manager, repair });
    }
    None
}

/// The lines that [`state_hint`] reads, for a cheap test before anything else is looked up.
const STATE_MARKS: &[&str] =
    &["dpkg was interrupted", "Could not get lock", "': File exists", "could not lock database"];

/// Whether an output may say something [`state_hint`] explains.
pub fn mentions_state(text: &str) -> bool {
    STATE_MARKS.iter().any(|m| text.contains(m))
}

/// What a command's output says is left broken or locked on this machine, with the fact KodAct
/// holds that the output doesn't: whether one of this session's own commands holds the lock, and
/// whether this session stopped an install halfway. Read from the text whatever the exit code:
/// `apt-get install … | tail` ends 0 with the error in it.
///
/// - `running`: this session's commands running now, as (ID, command line);
/// - `owner_of`: the ID of this session's command a process belongs to, if it is one;
/// - `forced`: installs this session stopped at once while they ran.
pub fn state_hint(
    text: &str,
    running: &[(String, String)],
    owner_of: impl Fn(u32) -> Option<String>,
    forced: &[String],
) -> Option<String> {
    if !mentions_state(text) {
        return None;
    }
    let mut hints: Vec<String> = vec![];
    let own_install = running.iter().find(|(_, c)| package_change(c).is_some());
    for line in text.lines() {
        let hint = if line.contains("dpkg was interrupted, you must manually run") {
            let who = match forced.last() {
                Some(c) => format!("this session stopped `{}` while it ran", short(c)),
                None => "an install or removal was stopped before it finished".to_string(),
            };
            format!(
                "The package database is half-updated: {who}. Every install fails until it is repaired, for anyone \
                 using this machine, even where what you need is already installed. Repair it before going on: \
                 `dpkg --configure -a`, then `apt-get install -f`."
            )
        } else if let Some(at) = line.find("Could not get lock ") {
            let held = line[at..].split("held by process ").nth(1);
            let pid = held.and_then(|h| h.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u32>().ok());
            let owner = pid.and_then(&owner_of).and_then(|id| running.iter().find(|(i, _)| *i == id));
            match owner.or(own_install) {
                Some((id, c)) => format!(
                    "The lock is held by this session's own command {id} (`{}`), which is still running: wait for it to \
                     finish (you will be told when it exits) instead of starting another install. Stopping it, or \
                     removing the lock, can leave the package database half-updated.",
                    short(c)
                ),
                None => "None of this session's commands is installing: another program on this machine holds the \
                         lock. Wait for it to finish, then run the command again; removing the lock while it runs \
                         can leave the package database half-updated."
                    .to_string(),
            }
        } else if let Some(path) = git_lock_in(line) {
            match running.iter().find(|(_, c)| runs_git(c)) {
                Some((id, c)) => format!(
                    "This session's command {id} (`{}`) is still running git and may hold it: wait for it (you will \
                     be told when it exits) or stop it with KillShell, and don't remove the lock while it runs.",
                    short(c)
                ),
                None => format!(
                    "None of this session's commands is running git. If no other git process is using this repository \
                     (an editor, an IDE, another terminal), the lock is stale, left by a git process that was killed \
                     before it could remove it: remove that one file, {path}, and run the command again."
                ),
            }
        } else if line.contains("could not lock database: File exists") {
            match running.iter().find(|(_, c)| programs(c).iter().any(|(p, _)| p == "pacman")) {
                Some((id, c)) => format!(
                    "This session's command {id} (`{}`) is still running pacman and holds the lock: wait for it.",
                    short(c)
                ),
                None => "None of this session's commands is running pacman. If no other pacman runs, the lock is \
                         stale: remove /var/lib/pacman/db.lck and run the command again."
                    .to_string(),
            }
        } else {
            continue;
        };
        if !hints.contains(&hint) {
            hints.push(hint);
        }
    }
    (!hints.is_empty()).then(|| hints.join("\n\n"))
}

/// The lock file in git's "Unable to create '<path>.lock': File exists." line.
fn git_lock_in(line: &str) -> Option<&str> {
    let rest = &line[line.find("Unable to create '")? + "Unable to create '".len()..];
    let path = &rest[..rest.find("': File exists")?];
    path.ends_with(".lock").then_some(path)
}

/// A command line for a hint: its first line, at most 80 characters.
fn short(command: &str) -> String {
    let first = command.trim().lines().next().unwrap_or_default();
    let mut s: String = first.chars().take(80).collect();
    if s.len() < first.len() || command.trim().lines().count() > 1 {
        s.push_str(" …");
    }
    s
}

/// The lock files git leaves when it is killed before it can remove them, that exist now in the
/// repository holding `dir`: `index.lock` and the other locks of the repository itself.
pub fn git_locks_left(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Some(git_dir) = git_dir_of(dir) else { return vec![] };
    ["index.lock", "HEAD.lock", "config.lock", "packed-refs.lock", "shallow.lock"]
        .iter()
        .map(|f| git_dir.join(f))
        .filter(|p| p.exists())
        .collect()
}

/// The git directory of the repository holding `dir`: `.git`, or where a `.git` file points
/// (a worktree or a submodule).
fn git_dir_of(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    for d in dir.ancestors() {
        let dot = d.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            return Some(d.join(target));
        }
    }
    None
}

/// The note for git lock files left after KodAct stopped a git command.
pub fn git_locks_note(locks: &[std::path::PathBuf]) -> Option<String> {
    let first = locks.first()?;
    Some(format!(
        "{} is still there now that it has stopped: git leaves it when it is killed before it can clean up, and \
         every git command that writes fails with \"File exists\" until it is removed. If no other git process is \
         using this repository, remove {}.",
        first.display(),
        if locks.len() == 1 {
            "that file".to_string()
        } else {
            format!("it and {} other lock(s) there", locks.len() - 1)
        }
    ))
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
            let note = package_change(cmd).map(|c| c.forced_note()).unwrap_or_else(|| panic!("a note for: {cmd}"));
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
            assert_eq!(package_change(quiet), None, "nothing to repair after: {quiet}");
        }
        assert!(runs_git("cd repo && git rebase main") && runs_git("sudo /usr/bin/git gc"));
        assert!(!runs_git("echo git push") && !runs_git("legit --help"));
    }

    #[test]
    fn broken_or_locked_states_are_explained_with_what_the_session_knows() {
        let none = |_: u32| -> Option<String> { None };
        // The run that showed it: apt's error inside a pipeline that ended 0.
        let text =
            "E: dpkg was interrupted, you must manually run 'dpkg --configure -a' to correct the problem.\nEXIT:0";
        let h = state_hint(text, &[], none, &[]).unwrap();
        assert!(
            h.contains("`dpkg --configure -a`, then `apt-get install -f`") && h.contains("before it finished"),
            "{h}"
        );
        let forced = vec!["apt-get install -y gcc g++ make".to_string()];
        let h = state_hint(text, &[], none, &forced).unwrap();
        assert!(h.contains("this session stopped `apt-get install -y gcc g++ make`"), "{h}");

        // A lock held by the session's own background install: by its process, or by its command.
        let running = vec![("bash_1".to_string(), "apt-get install -y g++".to_string())];
        let lock = "E: Could not get lock /var/lib/dpkg/lock-frontend. It is held by process 957 (apt-get)";
        let h = state_hint(lock, &running, |p| (p == 957).then(|| "bash_1".to_string()), &[]).unwrap();
        assert!(h.contains("own command bash_1 (`apt-get install -y g++`)") && h.contains("wait for it"), "{h}");
        assert!(state_hint(lock, &running, none, &[]).unwrap().contains("bash_1"));
        let h = state_hint(lock, &[], none, &[]).unwrap();
        assert!(h.contains("None of this session's commands is installing"), "{h}");

        // git's stale lock, and one a running git command of the session may hold.
        let git = "fatal: Unable to create '/w/repo/.git/index.lock': File exists.\n\nAnother git process seems to be running";
        let h = state_hint(git, &[], none, &[]).unwrap();
        assert!(h.contains("remove that one file, /w/repo/.git/index.lock,"), "{h}");
        let running_git = vec![("bash_3".to_string(), "git rebase main".to_string())];
        assert!(state_hint(git, &running_git, none, &[]).unwrap().contains("bash_3 (`git rebase main`)"));

        let pacman =
            "error: failed to init transaction (unable to lock database)\nerror: could not lock database: File exists";
        assert!(state_hint(pacman, &[], none, &[]).unwrap().contains("/var/lib/pacman/db.lck"));

        // Ordinary output, and a file named like a lock in another message, get nothing.
        for quiet in ["Reading package lists... Done", "cp: cannot create 'a.lock': File exists", "all good"] {
            assert_eq!(state_hint(quiet, &[], none, &[]), None, "{quiet}");
        }
    }

    #[test]
    fn git_locks_are_found_from_any_directory_of_the_repository() {
        let d = tempfile::tempdir().unwrap();
        let git = d.path().join(".git");
        std::fs::create_dir_all(git.join("worktrees/wt")).unwrap();
        std::fs::create_dir_all(d.path().join("src/deep")).unwrap();
        assert!(git_locks_left(&d.path().join("src/deep")).is_empty());
        std::fs::write(git.join("index.lock"), "").unwrap();
        assert_eq!(git_locks_left(&d.path().join("src/deep")), vec![git.join("index.lock")]);
        // A worktree: its `.git` is a file that points at its own git directory.
        let wt = d.path().join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", git.join("worktrees/wt").display())).unwrap();
        std::fs::write(git.join("worktrees/wt/index.lock"), "").unwrap();
        let left = git_locks_left(&wt);
        assert_eq!(left, vec![git.join("worktrees/wt/index.lock")]);
        assert!(git_locks_note(&left).unwrap().contains("remove that file"));
        assert_eq!(git_locks_note(&[]), None);
    }
}
