//! agent-sync — render an agent-config checkout (the single authored source of agent
//! instructions, `--dir` / `$AGENT_CONFIG_DIR`, default `~/git/agent-config`) into Claude Code
//! (`~/.claude`, `~/.claude.json`), Antigravity (`~/.gemini`), and Codex (`~/.codex`), and gate
//! drift.
//!
//! `--overlay` / `$AGENT_SYNC_OVERLAY` layers a private checkout over that base (see `overlay`).
//!
//! Exit codes: 0 clean · 2 usage · 4 sources missing · 5 refused · 6 drift (check) or an apply
//! that could not converge · 7 the secret-surface configuration is invalid.

#[macro_use]
mod out;
mod fsx;
mod import;
mod model;
mod models;
mod overlay;
mod render;
mod selftest;
mod session;
mod sync;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use sync::{Finding, Level, Paths};

#[derive(Parser)]
#[command(
    name = "agent-sync",
    version,
    about = "Keep Claude Code, Antigravity, and Codex aligned to shared instructions"
)]
struct Cli {
    /// The agent-config source checkout (default ~/git/agent-config).
    #[arg(long, env = "AGENT_CONFIG_DIR", global = true)]
    dir: Option<std::path::PathBuf>,
    /// A private overlay checkout layered over the source: its files win path by path, and its
    /// TOML sources merge into the source's by key.
    #[arg(long, env = "AGENT_SYNC_OVERLAY", global = true)]
    overlay: Option<std::path::PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read-only: check managed agent surfaces against the source. Exit 6 if out of sync.
    Check {
        /// Print all checked items, including those already in sync.
        #[arg(long)]
        verbose: bool,
    },
    /// Write every projection (atomic, .agent-sync.bak kept). Refuses AGY's settings while agy runs.
    Apply {
        /// Write AGY settings.json even while an agy process is running.
        #[arg(long)]
        force: bool,
        /// Also unregister Claude MCP servers that are not in mcp.toml.
        #[arg(long)]
        prune: bool,
    },
    /// Every surface and its state.
    Status,
    /// Seed sync/ from the current Claude configuration.
    Import {
        #[arg(long)]
        force: bool,
    },
    /// Review tool-appended approvals (AGY settings.json tail, Claude settings.local.json).
    Promote {
        /// List unpromoted approvals.
        #[arg(long)]
        list: bool,
        /// Move these native entries into permissions.toml, then apply.
        #[arg(long, num_args = 1..)]
        add: Vec<String>,
        /// Remove these entries from the tool files that hold them.
        #[arg(long, num_args = 1..)]
        drop: Vec<String>,
        #[arg(long)]
        force: bool,
    },
    /// Inspect recent Claude Code or Antigravity sessions and transcripts.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Validate agent frontmatter against schemas.
    Validate,
    /// End-to-end selftest across agent configs, components, hooks, loader symlinks, and MCP.
    Selftest {
        /// Also run live CLI probes (claude doctor, agy agents).
        #[arg(long)]
        probe_cli: bool,
        /// Run deep probes including doctor checkups.
        #[arg(long)]
        deep: bool,
        /// A pre-tool guard hook script to probe with synthetic payloads (skipped when unset).
        #[arg(long, env = "AGENT_SYNC_GUARD")]
        guard: Option<std::path::PathBuf>,
    },
}

#[derive(Subcommand)]
enum SessionCmd {
    /// Show the last session summary, prompt, response, and rate-limit status.
    Last {
        /// Filter by tool ('claude' or 'agy'). Defaults to whichever was active most recently.
        #[arg(long)]
        tool: Option<String>,
    },
    /// Tail the last N events of a session.
    Tail {
        /// Session ID, filename, or direct path. Defaults to the most recent session.
        session: Option<String>,
        /// Number of events to show (default: 5).
        #[arg(short = 'n', long, default_value = "5")]
        lines: usize,
        /// Filter event types: 'dialogue', 'tools', or 'errors'.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Show detailed status and metrics for a session.
    Status {
        /// Session ID, filename, or direct path. Defaults to the most recent session.
        session: Option<String>,
    },
}

/// A tool-appended approval not in permissions.toml: (tool, file holding it, entry).
type Unpromoted = (model::Tool, std::path::PathBuf, String);

fn collect(p: &Paths) -> Result<(Vec<Finding>, Vec<Unpromoted>)> {
    let src = p.sources()?;
    let mut all = Vec::new();
    all.extend(sync::check_resolved(p));
    all.extend(sync::check_links(p));
    all.extend(sync::check_skills(p));
    for tool in [model::Tool::Claude, model::Tool::Agy] {
        all.extend(sync::check_agents(p, &src, tool)?);
    }
    all.extend(sync::check_mcp(p, &src)?);
    let perms = sync::check_perms(p, &src)?;
    all.extend(perms.findings);
    all.extend(sync::check_hooks(p, &src)?);
    all.extend(sync::check_model_tags(p, &src)?);
    all.extend(sync::check_overlays(p));
    Ok((all, perms.unpromoted))
}

fn print(findings: &[Finding], verbose: bool) {
    for x in findings {
        let tag = match x.level {
            Level::Ok if !verbose => continue,
            Level::Ok => "ok          ",
            Level::Unpromoted => "WARN        ",
            Level::Drift => "OUT-OF-SYNC ",
        };
        if x.detail.is_empty() {
            say!("{tag} {}", x.surface);
        } else {
            say!("{tag} {}: {}", x.surface, x.detail);
        }
    }
}

/// An error that ends the run with a specific exit code.
#[derive(Debug)]
struct Exit {
    code: i32,
    msg: String,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Exit {}

fn exit(code: i32, msg: impl Into<String>) -> anyhow::Error {
    Exit {
        code,
        msg: msg.into(),
    }
    .into()
}

/// Exit codes.
mod code {
    pub const OK: i32 = 0;
    pub const USAGE: i32 = 2;
    pub const MISSING: i32 = 4;
    pub const REFUSED: i32 = 5;
    pub const DRIFT: i32 = 6;
    pub const SURFACES: i32 = 7;
}

/// The one mapping from a typed error to an exit code; anything untyped is 6.
fn exit_code(e: &anyhow::Error) -> i32 {
    if let Some(x) = e.downcast_ref::<Exit>() {
        x.code
    } else if e.downcast_ref::<sync::AgyRunning>().is_some() {
        code::REFUSED
    } else {
        code::DRIFT
    }
}

fn drift_code(f: &[Finding]) -> i32 {
    if f.iter().any(|x| x.level == Level::Drift) {
        code::DRIFT
    } else {
        code::OK
    }
}

fn run(cli: Cli) -> Result<i32> {
    let home = nuvek_core::env::home_dir();
    let mut p = Paths::from_home(&home, cli.dir)
        .with_xdg_config(nuvek_core::env::env_var("XDG_CONFIG_HOME"));
    if !p.sync().is_dir() && !matches!(cli.cmd, Cmd::Import { .. }) {
        return Err(exit(
            code::MISSING,
            format!(
                "{} not found (run `agent-sync import` to seed it)",
                p.sync().display()
            ),
        ));
    }
    let log = out::Log::get("agent-sync");
    let label = |path: &std::path::Path| sync::display_home(&home, path);
    // Kept alive until the command finishes: `check` reads its sources from here.
    let mut _scratch = None;
    let mut layering = None;
    p.state = overlay::state_dir(&home, nuvek_core::env::env_var("XDG_STATE_HOME"));
    // A blank value (`AGENT_SYNC_OVERLAY=` in a unit file) means no overlay.
    if let Some(ov) = cli.overlay.filter(|o| !o.as_os_str().is_empty()) {
        let writes_source = match &cli.cmd {
            Cmd::Import { .. } => true,
            Cmd::Promote { add, .. } => !add.is_empty(),
            _ => false,
        };
        if writes_source {
            return Err(exit(
                code::USAGE,
                "with --overlay this would write into the generated tree; edit the overlay \
                 checkout's sync/ files instead",
            ));
        }
        let ov = std::path::absolute(&ov)?;
        if !ov.is_dir() {
            return Err(exit(
                code::MISSING,
                format!("overlay {} not found", ov.display()),
            ));
        }
        let layers = overlay::Layers {
            base: std::path::absolute(&p.ac)?,
            overlay: ov,
        };
        p.ac = overlay::resolved_path(&p.state);
        p.src = p.ac.clone();
        p.layers = vec![layers.base.clone(), layers.overlay.clone()];
        if !matches!(cli.cmd, Cmd::Apply { .. }) {
            let s = overlay::build_scratch(&layers, &label)?;
            p.src = s.0.clone();
            _scratch = Some(s);
        }
        layering = Some(layers);
    }
    p.register_write_roots();
    // One writer at a time, for the whole run of every command that writes.
    let writes = match &cli.cmd {
        Cmd::Apply { .. } | Cmd::Import { .. } => true,
        Cmd::Promote { add, drop, .. } => !add.is_empty() || !drop.is_empty(),
        _ => false,
    };
    let _lock = if writes {
        Some(fsx::Lock::acquire(&p.state)?)
    } else {
        None
    };
    match cli.cmd {
        Cmd::Check { verbose } => {
            let (f, _) = collect(&p)?;
            print(&f, verbose);
            let drift = f.iter().filter(|x| x.level == Level::Drift).count();
            let warn = f.iter().filter(|x| x.level == Level::Unpromoted).count();
            if drift == 0 {
                say!(
                    "agent-sync: {} items checked, all in sync (0 out of sync, {warn} warnings)",
                    f.len()
                );
            } else {
                say!(
                    "agent-sync: {} items checked, {drift} out of sync, {warn} warnings",
                    f.len()
                );
            }
            log.info(&format!("check: {drift} out of sync, {warn} warnings"));
            Ok(drift_code(&f))
        }
        Cmd::Status => {
            let (f, _) = collect(&p)?;
            print(&f, true);
            Ok(code::OK)
        }
        Cmd::Apply { force, prune } => {
            // The manifest decides what may be deleted: an unusable one stops the run before
            // anything changes, the resolved tree included.
            sync::Manifest::load(&p)?;
            if let Some(layers) = &layering {
                // The candidate is loaded and rendered in full before it can go live.
                let validate = |fresh: &std::path::Path| {
                    let mut cand = Paths::from_home(&home, Some(fresh.to_path_buf()));
                    cand.layers = p.layers.clone();
                    sync::validate_sources(&cand)
                };
                overlay::install(layers, &p.state, &label, &validate, &log)?;
            }
            let src = p.sources()?;
            sync::apply_links(&p, &log)?;
            sync::record_managed(&p, &src)?;
            for tool in [model::Tool::Claude, model::Tool::Agy] {
                sync::apply_agents(&p, &src, tool, &log)?;
            }
            // Recorded now, so a later stage that fails cannot orphan what was just created.
            sync::record_managed(&p, &src)?;
            sync::apply_mcp(&p, &src, prune, &log)?;
            if let Err(e) = sync::apply_perms(&p, &src, force, &log) {
                log.error(&out::scrub(&format!("permissions: {e:#}")));
                return Err(e);
            }
            sync::apply_hooks(&p, &src, &log)?;
            sync::apply_model_tags(&p, &src, &log)?;
            sync::apply_overlays(&p, &log)?;
            sync::record_managed(&p, &src)?;
            let (f, _) = collect(&p)?;
            print(&f, false);
            let drift = f.iter().filter(|x| x.level == Level::Drift).count();
            if drift == 0 {
                log.info("apply complete: all items in sync (0 out of sync)");
                say!("agent-sync: applied successfully; all items in sync (0 out of sync)");
            } else {
                log.info(&format!("apply complete: {drift} items still out of sync"));
                say!("agent-sync: applied; {drift} items still out of sync");
            }
            Ok(drift_code(&f))
        }
        Cmd::Import { force } => {
            import::run(&p, force)?;
            log.info("import: seeded sync/ from the Claude configuration");
            say!("agent-sync: seeded {}", p.sync().display());
            Ok(0)
        }
        Cmd::Promote {
            list,
            add,
            drop,
            force,
        } => {
            if !add.is_empty() {
                sync::promote_add(&p, &add)?;
                log.info(&format!(
                    "promote: added {} entries to permissions.toml",
                    add.len()
                ));
                let src = p.sources()?;
                sync::apply_perms(&p, &src, force, &log)?;
            }
            if !drop.is_empty() {
                let n = sync::promote_drop(&p, &drop, force)?;
                log.info(&format!("promote: dropped {n} tool-appended entries"));
            }
            if list || (add.is_empty() && drop.is_empty()) {
                let (_, un) = collect(&p)?;
                for (tool, file, e) in &un {
                    say!("{:6} {}  {e}", tool.key(), file.display());
                }
                say!("agent-sync: {} unpromoted approvals", un.len());
            }
            Ok(0)
        }
        Cmd::Session { cmd } => {
            match cmd {
                SessionCmd::Last { tool } => {
                    session::cmd_last(&p.home, tool.as_deref())?;
                }
                SessionCmd::Tail {
                    session,
                    lines,
                    filter,
                } => {
                    session::cmd_tail(&p.home, session.as_deref(), lines, filter.as_deref())?;
                }
                SessionCmd::Status { session } => {
                    session::cmd_status(&p.home, session.as_deref())?;
                }
            }
            Ok(0)
        }
        Cmd::Validate => {
            let src = p.sources()?;
            let mut failed = 0;
            for a in &src.agents {
                for tool in [model::Tool::Claude, model::Tool::Agy] {
                    if a.applies_to(tool) {
                        match render::render_agent(&src.map, a, tool) {
                            Ok(_) => say!("ok    {} agent {}", tool.key(), a.front.name),
                            Err(e) => {
                                say_err!("ERROR {} agent {}: {e:#}", tool.key(), a.front.name);
                                failed += 1;
                            }
                        }
                    }
                }
            }
            if failed > 0 {
                bail!("{failed} agent(s) failed schema validation");
            }
            say!("agent-sync: all agent frontmatter schemas valid");
            Ok(0)
        }
        Cmd::Selftest {
            probe_cli,
            deep,
            guard,
        } => {
            let opts = selftest::SelftestOptions {
                probe_cli,
                deep,
                guard,
            };
            selftest::run_selftest(&p, &opts)
        }
    }
}

fn main() {
    // Load the secret surfaces before any text is scanned, and refuse to run without them.
    if let Err(e) = nuvek_core::scrub::init() {
        say_err!("agent-sync: secret surfaces: {e}");
        std::process::exit(code::SURFACES);
    }
    let cli = Cli::parse();
    let rc = match run(cli) {
        Ok(c) => c,
        Err(e) => {
            // The whole chain is scrubbed: it can quote a source line or a CLI's stderr.
            say_err!("agent-sync: {e:#}");
            exit_code(&e)
        }
    };
    std::process::exit(rc);
}
