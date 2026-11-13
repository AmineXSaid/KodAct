//! OS sandbox for shell commands (pattern from OpenAI Codex CLI; see
//! docs/REFERENCES.md). The sandbox is separate from permission rules: a
//! command confined to the workspace can run without a prompt, and a command
//! that needs more asks to run outside it (`dangerouslyDisableSandbox`).
//!
//! Linux uses bubblewrap: `/` is mounted read-only, the writable roots are
//! bound read-write, protected paths are re-bound read-only, and the network
//! namespace is unshared unless network access is allowed. macOS uses
//! `sandbox-exec` with a generated Seatbelt profile.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Nothing is writable except temp directories.
    ReadOnly,
    /// The working directories (and temp) are writable.
    WorkspaceWrite,
}

impl SandboxMode {
    pub fn parse(s: &str) -> Option<Option<Self>> {
        match s {
            "off" | "none" | "danger-full-access" => Some(None),
            "read-only" => Some(Some(SandboxMode::ReadOnly)),
            "workspace-write" => Some(Some(SandboxMode::WorkspaceWrite)),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::WorkspaceWrite => "workspace-write",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SandboxPolicy {
    pub mode: SandboxMode,
    pub network: bool,
    /// Writable in workspace-write mode (normally the working directories).
    pub writable_roots: Vec<PathBuf>,
    /// Extra writable paths from settings (`sandbox.writableRoots`).
    pub extra_writable: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Bubblewrap,
    Seatbelt,
}

/// The sandbox backend this machine supports, probed once.
pub fn backend() -> Option<Backend> {
    static B: OnceLock<Option<Backend>> = OnceLock::new();
    *B.get_or_init(|| {
        if cfg!(target_os = "macos") && Path::new("/usr/bin/sandbox-exec").exists() {
            return Some(Backend::Seatbelt);
        }
        if cfg!(target_os = "linux") {
            let ok = std::process::Command::new("bwrap")
                .args(["--ro-bind", "/", "/", "--dev", "/dev", "--unshare-net", "--die-with-parent", "true"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return Some(Backend::Bubblewrap);
            }
        }
        None
    })
}

/// Why no sandbox backend is available here, and what would give one.
pub fn unavailable_reason() -> &'static str {
    unavailable_reason_for(std::env::consts::OS)
}

fn unavailable_reason_for(os: &str) -> &'static str {
    match os {
        "linux" => "install bubblewrap (bwrap) to confine shell commands",
        "macos" => "/usr/bin/sandbox-exec was not found",
        "windows" => "KodAct has no sandbox on Windows; run KodAct inside WSL 2 to confine shell commands",
        _ => "no sandbox backend exists for this system",
    }
}

/// Paths under a writable root that sandboxed commands must not change:
/// KodAct's own settings and hooks, and git hooks (which would run unsandboxed later).
fn protected_under(root: &Path) -> Vec<PathBuf> {
    // `.forge` too: ForgeCLI's settings, from before the rename, still apply.
    [".kodact", ".forge", ".git/hooks", ".git/config"].iter().map(|p| root.join(p)).filter(|p| p.exists()).collect()
}

fn temp_dirs() -> Vec<PathBuf> {
    let mut v = vec![std::env::temp_dir()];
    for t in ["/tmp", "/var/tmp"] {
        let p = PathBuf::from(t);
        if p.is_dir() && !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

impl SandboxPolicy {
    fn writable(&self) -> Vec<PathBuf> {
        let mut out = temp_dirs();
        if self.mode == SandboxMode::WorkspaceWrite {
            out.extend(self.writable_roots.iter().cloned());
        }
        out.extend(self.extra_writable.iter().cloned());
        out.retain(|p| p.exists());
        out.dedup();
        out
    }

    /// The program and arguments that run `shell -c script` inside the sandbox.
    pub fn wrap(&self, backend: Backend, shell: &str, script: &str, cwd: &Path) -> (String, Vec<String>) {
        match backend {
            Backend::Bubblewrap => {
                let mut a: Vec<String> =
                    ["--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc"].iter().map(|s| s.to_string()).collect();
                for w in self.writable() {
                    let w = w.display().to_string();
                    a.extend(["--bind".into(), w.clone(), w]);
                }
                for w in self.writable() {
                    for p in protected_under(&w) {
                        let p = p.display().to_string();
                        a.extend(["--ro-bind".into(), p.clone(), p]);
                    }
                }
                if !self.network {
                    a.push("--unshare-net".into());
                }
                a.extend([
                    "--die-with-parent".into(),
                    "--new-session".into(),
                    "--chdir".into(),
                    cwd.display().to_string(),
                ]);
                a.extend(["--".into(), shell.into(), "-c".into(), script.into()]);
                ("bwrap".into(), a)
            }
            Backend::Seatbelt => {
                let mut profile = String::from("(version 1)\n(allow default)\n(deny file-write*)\n(allow file-write* (literal \"/dev/null\") (regex #\"^/dev/tty\")");
                for w in self.writable() {
                    profile.push_str(&format!(" (subpath \"{}\")", w.display().to_string().replace('"', "")));
                }
                profile.push_str(")\n");
                for w in self.writable() {
                    for p in protected_under(&w) {
                        profile.push_str(&format!(
                            "(deny file-write* (subpath \"{}\"))\n",
                            p.display().to_string().replace('"', "")
                        ));
                    }
                }
                if !self.network {
                    profile.push_str("(deny network-outbound (remote ip))\n");
                }
                ("/usr/bin/sandbox-exec".into(), vec!["-p".into(), profile, shell.into(), "-c".into(), script.into()])
            }
        }
    }

    /// Whether a port a sandboxed command opens can be reached only from inside that command.
    /// Bubblewrap without network gives each command a network of its own, loopback included:
    /// a server started in one is invisible to the next command, to the person's browser and to
    /// anything that runs after the session. Seatbelt only refuses outbound connections.
    pub fn isolates_ports(&self, backend: Backend) -> bool {
        backend == Backend::Bubblewrap && !self.network
    }

    /// Text appended to a failed command's output when the sandbox may be the cause.
    pub fn explain_failure(&self, output: &str) -> Option<String> {
        let o = output.to_ascii_lowercase();
        let fs = o.contains("read-only file system")
            || o.contains("operation not permitted")
            || o.contains("permission denied");
        let net = !self.network
            && (o.contains("could not resolve")
                || o.contains("network is unreachable")
                || o.contains("temporary failure in name resolution")
                || o.contains("connection refused")
                || o.contains("failed to connect"));
        (fs || net).then(|| {
            format!(
                "This command ran in the {} sandbox ({}). If it needs to write elsewhere or use the network, retry it with \
                 `dangerouslyDisableSandbox: true`; that asks the user for approval.",
                self.mode.as_str(),
                if self.network { "network allowed" } else { "no network" }
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(mode: SandboxMode, root: &Path) -> SandboxPolicy {
        SandboxPolicy { mode, network: false, writable_roots: vec![root.to_path_buf()], extra_writable: vec![] }
    }

    #[test]
    fn only_a_network_of_its_own_hides_a_port() {
        let d = tempfile::tempdir().unwrap();
        let mut p = policy(SandboxMode::WorkspaceWrite, d.path());
        assert!(p.isolates_ports(Backend::Bubblewrap));
        assert!(!p.isolates_ports(Backend::Seatbelt), "Seatbelt refuses outbound connections only");
        p.network = true;
        assert!(!p.isolates_ports(Backend::Bubblewrap));
    }

    #[cfg(unix)]
    #[test]
    fn bwrap_arguments() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".kodact")).unwrap();
        let (prog, args) =
            policy(SandboxMode::WorkspaceWrite, d.path()).wrap(Backend::Bubblewrap, "/bin/sh", "ls", d.path());
        assert_eq!(prog, "bwrap");
        let joined = args.join(" ");
        assert!(joined.starts_with("--ro-bind / /"));
        assert!(joined.contains(&format!("--bind {0} {0}", d.path().display())));
        assert!(joined.contains(&format!("--ro-bind {0}/.kodact {0}/.kodact", d.path().display())), "{joined}");
        assert!(joined.contains("--unshare-net"));
        assert!(joined.ends_with("-- /bin/sh -c ls"));
        let (_, ro) = policy(SandboxMode::ReadOnly, d.path()).wrap(Backend::Bubblewrap, "/bin/sh", "ls", d.path());
        assert!(!ro.join(" ").contains(&format!("--bind {0} {0}", d.path().display())));
    }

    #[test]
    fn failure_hints() {
        let p = policy(SandboxMode::WorkspaceWrite, Path::new("/w"));
        assert!(p.explain_failure("touch: cannot touch '/usr/x': Read-only file system").is_some());
        assert!(p.explain_failure("curl: (6) Could not resolve host: example.com").is_some());
        assert!(p.explain_failure("all good").is_none());
        assert_eq!(SandboxMode::parse("workspace-write"), Some(Some(SandboxMode::WorkspaceWrite)));
        assert_eq!(SandboxMode::parse("off"), Some(None));
        assert_eq!(SandboxMode::parse("bogus"), None);
    }

    #[test]
    fn unavailable_reason_names_the_fix_per_os() {
        assert!(unavailable_reason_for("linux").contains("bubblewrap"));
        assert!(unavailable_reason_for("windows").contains("WSL 2"));
        assert!(unavailable_reason_for("macos").contains("sandbox-exec"));
    }
}
