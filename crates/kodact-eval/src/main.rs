use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};
use kodact_eval::{load_tasks, render_compare, render_markdown, run_suite, Report, RunOptions};

/// Measure KodAct on a task suite (see docs/GOALS.md).
#[derive(Debug, Parser)]
#[command(name = "kodact-eval")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Run the suite and write report.json and report.md
    Run {
        /// Task directory
        #[arg(long, default_value = "evals/tasks")]
        tasks: PathBuf,
        /// The kodact binary under test
        #[arg(long, default_value = "target/release/kodact")]
        kodact: PathBuf,
        /// Name of this configuration (e.g. "baseline", "verify-loop")
        #[arg(long, default_value = "run")]
        label: String,
        /// Runs per task
        #[arg(long, default_value_t = 1)]
        repeat: u32,
        /// Tasks in parallel
        #[arg(short = 'j', long, default_value_t = 2)]
        jobs: usize,
        /// Output directory (default: evals/results/<label>)
        #[arg(long)]
        out: Option<PathBuf>,
        /// Only tasks whose id, tag or pillar matches
        #[arg(long)]
        only: Vec<String>,
        /// Arguments passed to every kodact run (after `--`)
        #[arg(last = true)]
        kodact_args: Vec<String>,
    },
    /// Compare two reports (B relative to A)
    Compare { a: PathBuf, b: PathBuf },
    /// Check every task: fails before its reference solution, passes after
    Validate {
        #[arg(long, default_value = "evals/tasks")]
        tasks: PathBuf,
        #[arg(long)]
        only: Vec<String>,
    },
    /// List the tasks
    List {
        #[arg(long, default_value = "evals/tasks")]
        tasks: PathBuf,
    },
}

fn read_report(p: &std::path::Path) -> anyhow::Result<Report> {
    let p = if p.is_dir() { p.join("report.json") } else { p.to_path_buf() };
    serde_json::from_str(&std::fs::read_to_string(&p).with_context(|| format!("cannot read {}", p.display()))?)
        .with_context(|| format!("invalid report {}", p.display()))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Run { tasks, kodact, label, repeat, jobs, out, only, kodact_args } => {
            let mut list = load_tasks(&tasks, &only)?;
            anyhow::ensure!(!list.is_empty(), "no tasks found in {}", tasks.display());
            list.retain(|t| match kodact_eval::unmet(t) {
                Some(why) => {
                    eprintln!("kodact-eval: skipped {}: {why}", t.spec.id);
                    false
                }
                None => true,
            });
            let kodact = kodact_platform::path::canonicalize(&kodact)
                .with_context(|| format!("kodact binary not found: {}", kodact.display()))?;
            let out_dir = out.unwrap_or_else(|| PathBuf::from("evals/results").join(&label));
            std::fs::create_dir_all(&out_dir)?;
            let opts = RunOptions {
                kodact,
                args: kodact_args,
                env: vec![],
                label,
                out_dir: kodact_platform::path::canonicalize(&out_dir)?,
                repeat,
                jobs,
            };
            eprintln!("kodact-eval: {} task(s) x {} run(s), {} in parallel", list.len(), repeat, jobs);
            let report = run_suite(&list, &opts).await;
            std::fs::write(opts.out_dir.join("report.json"), serde_json::to_string_pretty(&report)?)?;
            let md = render_markdown(&report);
            std::fs::write(opts.out_dir.join("report.md"), &md)?;
            println!("{md}");
        }
        Cmd::Compare { a, b } => println!("{}", render_compare(&read_report(&a)?, &read_report(&b)?)),
        Cmd::Validate { tasks, only } => {
            let scratch = std::env::temp_dir().join(format!("kodact-eval-validate-{}", std::process::id()));
            let mut failed = 0;
            for t in load_tasks(&tasks, &only)? {
                if let Some(why) = kodact_eval::unmet(&t) {
                    println!("skip  {}: {why}", t.spec.id);
                    continue;
                }
                match kodact_eval::validate_task(&t, &scratch).await {
                    Ok(()) => println!("ok    {}", t.spec.id),
                    Err(e) => {
                        failed += 1;
                        println!("FAIL  {}: {e}", t.spec.id);
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&scratch);
            anyhow::ensure!(failed == 0, "{failed} task(s) invalid");
        }
        Cmd::List { tasks } => {
            for t in load_tasks(&tasks, &[])? {
                println!("{:<32} {:<14} {}", t.spec.id, t.spec.pillar, t.spec.instruction.lines().next().unwrap_or(""));
            }
        }
    }
    Ok(())
}
