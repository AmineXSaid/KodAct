use std::path::{Path, PathBuf};

/// Resolve a tool's path argument against the project directory.
pub fn expand_path(raw: &str, base: &Path) -> PathBuf {
    kodact_permissions::normalize(Path::new(raw.trim()), base)
}

/// Keep the head and tail of long output, eliding the middle.
pub fn truncate_middle(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let mut head_end = half;
    while !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len() - half;
    while !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let dropped = s[head_end..tail_start].lines().count();
    format!("{}\n\n... [{dropped} lines truncated] ...\n\n{}", &s[..head_end], &s[tail_start..])
}

/// Fit `text` into `max` characters (GOALS pillar 1: output budgets). Long
/// output keeps its head and tail; the full text is saved in the session's
/// spill directory and the result says where, so nothing is lost for good.
pub fn fit_output(ctx: &crate::ToolContext, text: &str, max: usize, label: &str) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let saved = ctx.spill_dir.as_ref().and_then(|dir| {
        std::fs::create_dir_all(dir).ok()?;
        let id = if ctx.tool_use_id.is_empty() { "output" } else { ctx.tool_use_id.as_str() };
        let path = dir.join(format!("{id}-{label}.txt"));
        std::fs::write(&path, text).ok()?;
        Some(path)
    });
    let body = truncate_middle(text, max);
    match saved {
        Some(p) => format!(
            "{body}\n\n[The full {label} ({} lines, {} characters) is saved at {}. Grep it, or Read it with offset \
             and limit, instead of running the command again.]",
            text.lines().count(),
            text.len(),
            p.display()
        ),
        None => body,
    }
}

/// `cat -n` style numbering used by Read and in edit snippets.
/// What to try when `path` is not there (GOALS pillar 4: an error that says the next step):
/// the nearest directory that exists, and the entries in it whose names are close to the one
/// asked for (another case, another extension, a longer or shorter name).
pub fn missing_path_hint(path: &Path) -> String {
    let Some(dir) = path.ancestors().skip(1).find(|a| a.is_dir()) else { return String::new() };
    let wanted = path.strip_prefix(dir).ok().and_then(|rest| rest.components().next());
    let Some(wanted) = wanted.map(|c| c.as_os_str().to_string_lossy().to_lowercase()) else { return String::new() };
    let stem = wanted.rsplit_once('.').map(|(s, _)| s).filter(|s| !s.is_empty()).unwrap_or(&wanted).to_string();
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.sort();
    let total = names.len();
    let close: Vec<String> = names
        .into_iter()
        .filter(|n| {
            let n = n.to_lowercase();
            let n_stem = n.rsplit_once('.').map(|(s, _)| s).filter(|s| !s.is_empty()).unwrap_or(&n);
            n == wanted
                || n_stem == stem
                || (stem.len() >= 3 && (n.contains(&stem) || (n_stem.len() >= 3 && stem.contains(n_stem))))
        })
        .take(5)
        .map(|n| dir.join(n).display().to_string())
        .collect();
    let whole = path.parent() == Some(dir);
    let place = if whole {
        format!("Its directory exists: {}.", dir.display())
    } else {
        format!("The nearest directory that exists is {}.", dir.display())
    };
    match close.is_empty() {
        false => format!(" {place} Close names there: {}.", close.join(", ")),
        true => format!(" {place} Nothing there has a similar name ({total} entries; list it with LS or Glob)."),
    }
}

pub fn number_lines(lines: &[&str], first: usize) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        out.push_str(&format!("{:>6}\t{}\n", first + i, l));
    }
    out
}
