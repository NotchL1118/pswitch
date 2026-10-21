use anyhow::Result;
use clap::{Parser, Subcommand};
use pswitch::{mask, App, Paths, Provider, Tool};

#[derive(Parser)]
#[command(name = "pswitch", version, about = "Switch model providers for Claude Code and Codex")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Add a provider
    Add {
        tool: Tool,
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        key: String,
        #[arg(long)]
        model: String,
        /// Codex only: responses (default) or chat
        #[arg(long)]
        wire: Option<String>,
    },
    /// List providers
    List,
    /// Show active provider per tool
    Current,
    /// Activate a provider (`official` restores the official setup)
    Use { tool: Tool, name: String },
    /// Remove a provider
    Remove { tool: Tool, name: String },
    /// Restore original config files from backups
    Restore,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let app = App::new(Paths::from_env()?);
    match cli.cmd {
        Cmd::Add { tool, name, url, key, model, wire } => {
            if tool == Tool::Claude && wire.is_some() {
                eprintln!("note: --wire is ignored for claude");
            }
            if tool == Tool::Codex && wire.as_deref() == Some("chat") {
                eprintln!("warning: recent Codex versions removed wire_api = \"chat\"; only \"responses\" works there");
            }
            app.add(Provider { name: name.clone(), tool, base_url: url, key, model, wire_api: wire })?;
            println!("added {tool}/{name}");
        }
        Cmd::List => {
            let s = app.load_store()?;
            for tool in [Tool::Claude, Tool::Codex] {
                let cur = app.current(tool)?;
                println!("{tool}:");
                let mark = |n: &str| if n == cur { "*" } else { " " };
                println!("  {} official", mark("official"));
                for p in s.providers.iter().filter(|p| p.tool == tool) {
                    let wire = p.wire_api.as_deref().map(|w| format!("  wire={w}")).unwrap_or_default();
                    println!("  {} {}  {}  model={}  key={}{}", mark(&p.name), p.name, p.base_url, p.model, mask(&p.key), wire);
                }
            }
        }
        Cmd::Current => {
            for tool in [Tool::Claude, Tool::Codex] {
                println!("{tool}: {}", app.current(tool)?);
            }
        }
        Cmd::Use { tool, name } => {
            app.use_provider(tool, &name)?;
            println!("{tool} -> {name}");
            match tool {
                Tool::Claude => println!("Start a new Claude Code session for it to take effect."),
                Tool::Codex => println!("Restart Codex for it to take effect."),
            }
        }
        Cmd::Remove { tool, name } => {
            app.remove(tool, &name)?;
            println!("removed {tool}/{name}");
        }
        Cmd::Restore => {
            let done = app.restore()?;
            if done.is_empty() {
                println!("nothing to restore");
            }
            for p in done {
                println!("restored {}", p.display());
            }
        }
    }
    Ok(())
}
