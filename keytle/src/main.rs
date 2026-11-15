use clap::{CommandFactory, Parser, Subcommand};
use keytle_core::paths::{self, Paths};

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
    let term = std::env::var_os("TERM").map(|term| term.to_string_lossy().into_owned());
    print!(
        "{}",
        render(&dirs, env!("CARGO_PKG_VERSION"), term.as_deref())
    );
}

/// The `doctor` report, kept pure so its format can be checked by tests.
fn render(dirs: &Paths, version: &str, term: Option<&str>) -> String {
    format!(
        "keytle {version}\nconfig dir: {}\ndata dir:   {}\nTERM:       {}\n",
        dirs.config_dir.display(),
        dirs.data_dir.display(),
        term.unwrap_or("(unset)")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn dirs() -> Paths {
        Paths {
            config_dir: PathBuf::from("/cfg/keytle"),
            data_dir: PathBuf::from("/data/keytle"),
        }
    }

    #[test]
    fn doctor_report_names_version_paths_and_term() {
        let out = render(&dirs(), "0.1.0", Some("xterm-kitty"));
        assert!(out.contains("keytle 0.1.0"), "{out}");
        assert!(out.contains("/cfg/keytle"), "{out}");
        assert!(out.contains("/data/keytle"), "{out}");
        assert!(out.contains("TERM:       xterm-kitty"), "{out}");
    }

    #[test]
    fn doctor_report_marks_a_missing_term() {
        assert!(render(&dirs(), "0.1.0", None).contains("TERM:       (unset)"));
    }
}
