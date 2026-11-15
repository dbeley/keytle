use clap::{CommandFactory, Parser, Subcommand};
use keytle_core::paths;

#[derive(Parser)]
#[command(
    name = "keytle",
    version,
    about = "A fast, offline TUI that teaches keyboard layouts"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print resolved paths, version and terminal type
    Doctor,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Doctor) => doctor(),
        None => {
            let mut out = Cli::command();
            let _ = out.print_help();
        }
    }
}

fn doctor() {
    let dirs = paths::paths();
    println!("keytle {}", env!("CARGO_PKG_VERSION"));
    println!("config dir: {}", dirs.config_dir.display());
    println!("data dir:   {}", dirs.data_dir.display());
    println!(
        "TERM:       {}",
        std::env::var("TERM").unwrap_or_else(|_| "(unset)".into())
    );
}
