//! Herdr Git Panel: a list of the GitLab merge requests and GitHub pull
//! requests you have to review and your own open ones, in a Herdr pane.
//!
//! `herdr-git-panel` (or `... list`) shows the list in the current
//! terminal. `... open [overlay|split|tab]` is the plugin action: it opens the list as a Herdr
//! plugin pane, or closes it when it is already open.

mod app;
mod cli;
mod config;
mod diff;
mod forge;
mod github;
mod gitlab;
mod model;
mod pane;
mod repo;
mod text;
mod theme;
mod time;
mod ui;
mod view;

use config::Config;
use std::path::PathBuf;

fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("HERDR_PLUGIN_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/herdr/plugins/config/git-panel"))
}

fn list() -> std::io::Result<()> {
    let dir = config_dir();
    let config = Config::load(dir.as_deref());
    let project = std::env::current_dir()
        .ok()
        .and_then(|d| repo::detect(&d))
        .filter(|p| p.host == config.gitlab_host || p.host == github::HOST);
    let app = app::App::new(config, project, dir);
    let mut terminal = ratatui::init();
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::EnableMouseCapture
    );
    let result = app.run(&mut terminal);
    let _ = ratatui::crossterm::execute!(
        std::io::stdout(),
        ratatui::crossterm::event::DisableMouseCapture
    );
    ratatui::restore();
    pane::forget_self();
    result
}

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "list".into());
    let code = match command.as_str() {
        "list" => match list() {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("herdr-git-panel: {e}");
                1
            }
        },
        "open" => {
            // `open split`, `open tab` or `open overlay` override the setting.
            let config = Config::load(config_dir().as_deref());
            let placement = match std::env::args().nth(2).as_deref() {
                Some("split") => config::Placement::Split,
                Some("tab") => config::Placement::Tab,
                Some("overlay") => config::Placement::Overlay,
                _ => config.placement,
            };
            match pane::toggle(placement, config.split_direction) {
                Ok(msg) => {
                    println!("{msg}");
                    0
                }
                Err(e) => {
                    eprintln!("{e}");
                    1
                }
            }
        }
        "--version" | "-V" => {
            println!("herdr-git-panel {}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprintln!("usage: herdr-git-panel [list|open [overlay|split|tab]|--version]");
            2
        }
    };
    std::process::exit(code);
}
