mod app;
mod ui;

use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::{Parser, Subcommand, ValueEnum};
use crossterm::event::{self, Event, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::cursor;
use keytle_core::config::{Config, Mode};
use keytle_core::emulate::Emulator;
use keytle_core::paths::{self, Paths};
use keytle_core::stats::{self, Filter};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;

use crate::app::{Action, App};

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
    /// Start the trainer (the default when no subcommand is given)
    Run,
    /// Print the progress dashboard as plain text
    Stats {
        /// Only count tests on this layout
        #[arg(long)]
        layout: Option<String>,
        /// Only count tests in this language
        #[arg(long)]
        lang: Option<String>,
        /// Only count tests in this mode: pseudo, words or sentences
        #[arg(long)]
        mode: Option<String>,
        /// Summarise the newest N tests
        #[arg(long, default_value_t = 20)]
        last: usize,
    },
    /// Write the raw records out as JSONL or TSV
    Export {
        /// Which shape to write
        #[arg(long, value_enum, default_value_t = ExportFormat::Jsonl)]
        format: ExportFormat,
        /// Write to a file instead of standard output
        #[arg(short, long, value_name = "PATH")]
        output: Option<PathBuf>,
    },
    /// Print resolved paths, config, version, terminal type and reachability
    Doctor,
    /// Inspect the available layouts
    Layouts {
        #[command(subcommand)]
        command: LayoutsCommand,
    },
}

/// The shapes `keytle export` can write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ExportFormat {
    /// One JSON record per line, exactly as stored
    Jsonl,
    /// A header row and one row per record, for spreadsheets
    Tsv,
}

#[derive(Subcommand)]
enum LayoutsCommand {
    /// Print every layout id with its name
    List,
}

fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Doctor) => {
            doctor();
            Ok(())
        }
        Some(Command::Layouts {
            command: LayoutsCommand::List,
        }) => {
            layouts_list();
            Ok(())
        }
        Some(Command::Stats {
            layout,
            lang,
            mode,
            last,
        }) => stats_command(layout, lang, mode.as_deref(), last),
        Some(Command::Export { format, output }) => export_command(format, output.as_deref()),
        None | Some(Command::Run) => run_tui(),
    };
    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

/// The current wall clock in milliseconds. The only clock read in the program:
/// everything below it takes `now_ms` as an argument.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Put the terminal back the way we found it. Idempotent and silent: it is
/// called from a guard, from the panic hook, and on the normal path.
fn restore_terminal() {
    let _ = disable_raw_mode();
    // Drawn frames hide the cursor; leaving it hidden would strand the shell.
    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
}

/// Restores the terminal when dropped — on success, on error, and on unwind.
struct TerminalGuard {
    restored: bool,
}

impl TerminalGuard {
    fn new() -> TerminalGuard {
        TerminalGuard { restored: false }
    }

    fn disarm(&mut self) {
        self.restored = true;
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if !self.restored {
            restore_terminal();
        }
    }
}

/// Start the TUI: raw mode, alternate screen, and a loop that survives resizes.
fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    let dirs = paths::paths();
    // A hand-edited file may hold a value the drill cannot honour; loading says
    // which ones it had to clamp, and the settings screen reports them once.
    let (config, adjusted) = Config::load_reporting(&dirs.config_file())?;
    let started = now_ms();
    let mut app = App::new(config, &dirs, started)?;
    app.note_adjustments(&adjusted);

    install_panic_hook();
    enable_raw_mode()?;
    let mut guard = TerminalGuard::new();
    execute!(io::stdout(), EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = event_loop(&mut terminal, &mut app);
    drop(terminal);
    guard.disarm();
    restore_terminal();
    result
}

/// Wait for input, handle it, redraw. Redrawing on every tick keeps the elapsed
/// seconds and speed live while the typist is thinking.
fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &mut App,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;
        if !event::poll(Duration::from_millis(16))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) => {
                if handle(terminal, app, key)? {
                    return Ok(());
                }
            }
            // Bracketed paste is not typing: it must never reach the scorer.
            Event::Paste(_) => app.ignore_input(),
            // A resize needs nothing but the next frame, which is already drawn
            // from the new area.
            Event::Resize(_, _) => {}
            _ => {}
        }
    }
}

/// Apply one key event; `Ok(true)` means quit. Generic over the backend so the
/// loop's decision — quit, or record and redraw — can be tested off a TTY.
fn handle<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    key: KeyEvent,
) -> Result<bool, Box<dyn std::error::Error>> {
    match app.handle_key(key, now_ms()) {
        Action::Quit => Ok(true),
        Action::Finish => {
            app.finish_lesson(now_ms());
            terminal.draw(|frame| ui::draw(frame, app))?;
            Ok(false)
        }
        Action::Redraw | Action::StartLesson => Ok(false),
    }
}

/// A panic in the UI must not leave the shell in raw mode with no cursor.
fn install_panic_hook() {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        hook(info);
    }));
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
    let reach = reachability(&config, &dirs.layouts_dir());
    print!(
        "{}",
        render(
            &dirs,
            &config,
            env!("CARGO_PKG_VERSION"),
            term.as_deref(),
            &reach
        )
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

/// `keytle stats`: the dashboard as plain text, for a shell that is not running
/// the TUI.
///
/// Every number comes from `keytle_core::stats` — the same folds the dashboard
/// draws — so the two views cannot disagree about a personal best.
fn stats_command(
    layout: Option<String>,
    lang: Option<String>,
    mode: Option<&str>,
    last: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mode = match mode {
        Some(text) => Some(parse_mode(text).ok_or_else(|| {
            format!("unknown mode \"{text}\": try pseudo, words or sentences")
        })?),
        None => None,
    };
    let dirs = paths::paths();
    let records = stats::read_all(&dirs.tests_file())?;
    let filter = Filter {
        layout,
        lang,
        mode,
    };
    if !records.iter().any(|rec| filter.matches(rec)) {
        println!("{}", empty_history_line(&filter, records.len()));
        return Ok(());
    }
    let window = last.max(1);
    let points = stats::series(&records, &filter);
    let speed: Vec<f32> = points.iter().map(|point| point.wpm).collect();
    let recent = &points[points.len().saturating_sub(window)..];
    let accuracy = recent.iter().map(|point| point.accuracy).sum::<f32>() / recent.len() as f32;
    let best = stats::personal_best(&records, &filter).unwrap_or(0.0);
    let config = Config::load(&dirs.config_file())?;
    let keys = stats::key_rows(&records, &filter, window, config.target_interval_ms());

    println!("{:<12} {}", "tests", points.len());
    println!("{:<12} {:.0}", "latest wpm", speed.last().copied().unwrap_or(0.0));
    println!("{:<12} {best:.0}", "best wpm");
    println!("{:<12} {:.1}%", "accuracy", accuracy * 100.0);
    println!("{:<12} {:.1}/5", "consistency", stats::consistency(&speed));
    println!("weakest keys");
    if keys.is_empty() {
        println!("  (no per-key data)");
    }
    for row in keys.iter().take(5) {
        println!(
            "  {:<3} {:>5.1}% accuracy  conf {:.2}",
            row.ch,
            row.accuracy * 100.0,
            row.confidence
        );
    }
    Ok(())
}

/// The line `stats` prints when nothing matched. An empty store is genuinely
/// "no tests yet"; a filter that hides existing history is a different fact,
/// so it names the filter and says how much history it is hiding — otherwise
/// the same `no tests yet` would contradict the dashboard, which shows the
/// records the filter merely skipped.
fn empty_history_line(filter: &Filter, total: usize) -> String {
    if total == 0 {
        return "no tests yet".to_string();
    }
    let mut active: Vec<String> = Vec::new();
    if let Some(layout) = &filter.layout {
        active.push(format!("layout {layout}"));
    }
    if let Some(lang) = &filter.lang {
        active.push(format!("language {lang}"));
    }
    if let Some(mode) = filter.mode {
        active.push(format!("mode {mode}"));
    }
    let mut line = String::from("no tests match this filter");
    if !active.is_empty() {
        line.push_str(&format!(" ({})", active.join(", ")));
    }
    line.push_str(&format!(
        "; {total} test{} recorded for other layouts/languages/modes",
        if total == 1 { "" } else { "s" }
    ));
    line
}

/// The config file's spelling of a mode, or `None` for anything else.
fn parse_mode(text: &str) -> Option<Mode> {
    match text {
        "pseudo" => Some(Mode::Pseudo),
        "words" => Some(Mode::Words),
        "sentences" => Some(Mode::Sentences),
        _ => None,
    }
}

/// `keytle export`: the raw history, to stdout or to a file.
fn export_command(
    format: ExportFormat,
    output: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let dirs = paths::paths();
    let records = stats::read_all(&dirs.tests_file())?;
    let text = match format {
        ExportFormat::Jsonl => stats::export_jsonl(&records)?,
        ExportFormat::Tsv => stats::export_tsv(&records),
    };
    match output {
        Some(path) => {
            if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, text)?;
        }
        None => {
            let mut stdout = io::stdout().lock();
            stdout.write_all(text.as_bytes())?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// For the configured (host, target) pair: how many target positions the host
/// board cannot press, and the characters sitting on them.
fn reachability(config: &Config, layout_dir: &Path) -> String {
    let load = |id: &str| keytle_core::layouts::load(id, layout_dir);
    let (Ok(host), Ok(target)) = (load(&config.host_layout), load(&config.layout)) else {
        return format!(
            "reachability: {} -> {}: cannot load both layouts",
            config.host_layout, config.layout
        );
    };
    let emulator = Emulator::new(host, target);
    let missing = emulator.target().keys.len() - emulator.reachable().len();
    if missing == 0 {
        format!(
            "reachability: {} -> {}: all positions reachable",
            config.host_layout, config.layout
        )
    } else {
        let chars: String = emulator
            .unreachable_chars()
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "reachability: {} -> {}: {missing} unreachable position{} ({chars})",
            config.host_layout,
            config.layout,
            if missing == 1 { "" } else { "s" }
        )
    }
}

/// The `doctor` report, kept pure so its format can be checked by tests.
fn render(
    dirs: &Paths,
    config: &Config,
    version: &str,
    term: Option<&str>,
    reachability: &str,
) -> String {
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
    out.push('\n');
    out.push_str(reachability);
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Screen;
    use crossterm::event::{KeyCode, KeyModifiers};
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn dirs() -> Paths {
        Paths {
            config_dir: PathBuf::from("/cfg/keytle"),
            data_dir: PathBuf::from("/data/keytle"),
        }
    }

    #[test]
    fn doctor_report_names_version_paths_and_term() {
        let out = render(&dirs(), &Config::default(), "0.1.0", Some("xterm-kitty"), "reachability: test line");
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
        let out = render(&dirs(), &Config::default(), "0.1.0", None, "reachability: test line");
        assert!(out.contains("TERM:        (unset)"), "{out}");
    }

    #[test]
    fn reachability_names_the_unreachable_characters() {
        // azerty has no key at (3, 6), where colemak-dh puts 'h'.
        let config = Config {
            host_layout: "azerty".to_string(),
            layout: "colemak-dh".to_string(),
            ..Config::default()
        };
        let line = reachability(&config, &empty_layout_dir());
        assert!(line.contains("reachability:"), "{line}");
        assert!(line.contains("1 unreachable position"), "{line}");
        assert!(line.contains('h'), "the characters are named: {line}");

        let identical = Config {
            host_layout: "qwerty".to_string(),
            layout: "colemak-dh".to_string(),
            ..Config::default()
        };
        assert!(
            reachability(&identical, &empty_layout_dir()).contains("all positions reachable"),
            "a shared board shape reaches everything"
        );
    }

    /// A layout directory that does not exist, so only the bundled layouts load.
    fn empty_layout_dir() -> PathBuf {
        PathBuf::from("/nonexistent/keytle/layouts")
    }

    #[test]
    fn doctor_report_carries_the_reachability_line() {
        let out = render(
            &dirs(),
            &Config::default(),
            "0.1.0",
            None,
            "reachability: qwerty -> colemak-dh: all positions reachable",
        );
        assert!(out.contains("reachability: qwerty -> colemak-dh"), "{out}");
    }

    #[test]
    fn doctor_report_prints_the_resolved_config() {
        let config = Config {
            layout: "workman".to_string(),
            mode: keytle_core::config::Mode::Words,
            target_wpm: 60.0,
            ..Config::default()
        };
        let out = render(&dirs(), &config, "0.1.0", Some("xterm-kitty"), "reachability: test line");
        assert!(out.contains("config:\n  layout = \"workman\"\n"), "{out}");
        assert!(out.contains("  mode = \"words\"\n"), "{out}");
        assert!(out.contains("  target_wpm = 60\n"), "{out}");
        assert!(out.contains("  emulate = true\n"), "{out}");
    }

    /// A throwaway config/data root, so `App::new` never touches the real ones.
    fn scratch(tag: &str) -> (PathBuf, Paths) {
        let base = std::env::temp_dir().join(format!(
            "keytle-main-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let paths = Paths {
            config_dir: base.join("config"),
            data_dir: base.join("data"),
        };
        (base, paths)
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buf = terminal.backend().buffer();
        let width = buf.area().width as usize;
        buf.content()
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(terminal: &mut Terminal<TestBackend>, app: &mut App, code: KeyCode) -> bool {
        handle(terminal, app, KeyEvent::new(code, KeyModifiers::NONE)).unwrap()
    }

    #[test]
    fn q_on_the_idle_typing_screen_tells_the_loop_to_quit() {
        let (base, paths) = scratch("quit");
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        assert!(app.is_idle());
        assert!(press(&mut terminal, &mut app, KeyCode::Char('q')));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn finishing_a_lesson_records_it_and_draws_the_results() {
        let (base, paths) = scratch("finish");
        let mut app = App::new(Config::default(), &paths, 0).unwrap();
        app.config.emulate = false;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        app.start_lesson(0);
        let text = app.lesson.as_ref().unwrap().text.clone();
        let mut quit = false;
        for ch in text.chars() {
            quit = press(&mut terminal, &mut app, KeyCode::Char(ch));
        }
        assert!(!quit, "the last character finishes, it does not quit");
        assert_eq!(app.screen, Screen::Results);
        assert_eq!(app.profile.tests_completed, 1, "the lesson was recorded");
        let drawn = screen(&terminal);
        assert!(drawn.contains("results"), "the loop redrew as results:\n{drawn}");
        assert!(drawn.contains("accuracy"), "{drawn}");
        let _ = std::fs::remove_dir_all(base);
    }
}
