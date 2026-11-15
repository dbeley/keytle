use clap::{CommandFactory, Parser, Subcommand};
use keytle_core::config::Config;
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
    /// Print resolved paths, config, version and terminal type
    Doctor,
    /// Inspect the available layouts
    Layouts {
        #[command(subcommand)]
        command: LayoutsCommand,
    },
}

#[derive(Subcommand)]
enum LayoutsCommand {
    /// Print every layout id with its name
    List,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Doctor) => doctor(),
        Some(Command::Layouts {
            command: LayoutsCommand::List,
        }) => layouts_list(),
        None => {
            let mut out = Cli::command();
            let _ = out.print_help();
        }
    }
}

fn doctor() {
    let dirs = paths::paths();
    let config = match Config::load(&dirs.config_file()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    };
    let term = std::env::var_os("TERM").map(|term| term.to_string_lossy().into_owned());
    print!(
        "{}",
        render(&dirs, &config, env!("CARGO_PKG_VERSION"), term.as_deref())
    );
}

/// One line per layout: the id, then its name, then where it came from.
fn layouts_list() {
    let dir = paths::paths().layouts_dir();
    for id in keytle_core::layouts::available(&dir) {
        match keytle_core::layouts::load(&id, &dir) {
            Ok(layout) => {
                let source = if dir.join(format!("{id}.toml")).is_file() {
                    "user"
                } else {
                    "bundled"
                };
                println!("{:<12} {} ({source})", layout.id, layout.name);
            }
            Err(err) => println!("{:<12} unreadable: {err}", id),
        }
    }
}

/// The `doctor` report, kept pure so its format can be checked by tests.
fn render(dirs: &Paths, config: &Config, version: &str, term: Option<&str>) -> String {
    let mut out = format!(
        "keytle {version}\n\
         config dir:  {}\n\
         data dir:    {}\n\
         TERM:        {}\n\
         config file: {}\n\
         config:\n",
        dirs.config_dir.display(),
        dirs.data_dir.display(),
        term.unwrap_or("(unset)"),
        dirs.config_file().display(),
    );
    out.push_str(&format!("  layout = \"{}\"\n", config.layout));
    out.push_str(&format!("  host_layout = \"{}\"\n", config.host_layout));
    out.push_str(&format!("  language = \"{}\"\n", config.language));
    out.push_str(&format!("  mode = \"{}\"\n", config.mode));
    out.push_str(&format!("  emulate = {}\n", config.emulate));
    out.push_str(&format!("  strict = {}\n", config.strict));
    out.push_str(&format!("  lesson_words = {}\n", config.lesson_words));
    out.push_str(&format!("  target_wpm = {}\n", config.target_wpm));
    out
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
        let out = render(&dirs(), &Config::default(), "0.1.0", Some("xterm-kitty"));
        assert!(out.contains("keytle 0.1.0"), "{out}");
        assert!(out.contains("/cfg/keytle"), "{out}");
        assert!(out.contains("/data/keytle"), "{out}");
        assert!(
            out.contains("config file: /cfg/keytle/config.toml"),
            "{out}"
        );
        assert!(out.contains("TERM:        xterm-kitty"), "{out}");
    }

    #[test]
    fn doctor_report_marks_a_missing_term() {
        let out = render(&dirs(), &Config::default(), "0.1.0", None);
        assert!(out.contains("TERM:        (unset)"), "{out}");
    }

    #[test]
    fn doctor_report_prints_the_resolved_config() {
        let config = Config {
            layout: "workman".to_string(),
            mode: keytle_core::config::Mode::Words,
            target_wpm: 60.0,
            ..Config::default()
        };
        let out = render(&dirs(), &config, "0.1.0", Some("xterm-kitty"));
        assert!(out.contains("config:\n  layout = \"workman\"\n"), "{out}");
        assert!(out.contains("  mode = \"words\"\n"), "{out}");
        assert!(out.contains("  target_wpm = 60\n"), "{out}");
        assert!(out.contains("  emulate = true\n"), "{out}");
    }
}
