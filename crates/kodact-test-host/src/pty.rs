//! The real `kodact` TUI in a pseudo-terminal, as a person runs it: keys and pastes go in, the
//! screen comes back as text. For tests of what only the terminal UI does (input, paste,
//! rendering), which `-p` runs and in-memory render tests can't reach.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// A `kodact` TUI running in a pty of `cols` x `rows`, its screen kept by a terminal emulator.
pub struct Pty {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    screen: Arc<Mutex<vt100::Parser>>,
    _master: Box<dyn MasterPty + Send>,
}

impl Pty {
    /// Start `bin` in `cwd`, with the same isolated environment as [`crate::command`]: its own
    /// home, the scripted API at `api`, no retries.
    pub fn spawn(bin: &Path, cwd: &Path, home: &Path, api: &str, args: &[&str], cols: u16, rows: u16) -> Pty {
        let pair =
            native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).expect("open a pty");
        let mut cmd = CommandBuilder::new(bin);
        cmd.args(args);
        cmd.cwd(cwd);
        cmd.env_clear();
        for (k, v) in [
            ("PATH", std::env::var("PATH").unwrap_or_default()),
            ("HOME", home.display().to_string()),
            ("KODACT_HOME", home.join(".kodact").display().to_string()),
            ("KODACT_BASE_URL", api.to_string()),
            ("KODACT_API_KEY", "test-key".to_string()),
            ("KODACT_MAX_RETRIES", "0".to_string()),
            ("TERM", "xterm-256color".to_string()),
        ] {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd).expect("spawn kodact in the pty");
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().expect("pty reader");
        let writer = pair.master.take_writer().expect("pty writer");
        let screen = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 0)));
        let feed = screen.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                feed.lock().unwrap().process(&buf[..n]);
            }
        });
        Pty { child, writer, screen, _master: pair.master }
    }

    /// Type `text` as keys (a `\r` in it is Enter).
    pub fn keys(&mut self, text: &str) {
        self.writer.write_all(text.as_bytes()).expect("write to the pty");
        self.writer.flush().ok();
    }

    /// Paste `text` the way a terminal does when the application asked for bracketed paste.
    pub fn paste(&mut self, text: &str) {
        self.keys(&format!("\x1b[200~{text}\x1b[201~"));
    }

    /// What the screen shows now, one line per row.
    pub fn screen(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    /// Wait until the screen shows `needle`, at most `timeout`; the screen as it then is, or
    /// as it was when the wait ran out.
    pub fn wait_for(&self, needle: &str, timeout: Duration) -> Result<String, String> {
        let end = Instant::now() + timeout;
        loop {
            let s = self.screen();
            if s.contains(needle) {
                return Ok(s);
            }
            if Instant::now() >= end {
                return Err(s);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Whether the process has exited.
    pub fn exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
