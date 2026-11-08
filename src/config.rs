//! Settings, read from `config.toml` in the plugin's config directory. Only
//! flat `key = value` lines are understood; anything else is ignored, so a typo
//! never stops the plugin.

use std::path::Path;

/// Which merge requests the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Only the project of the directory the list was opened from.
    Current,
    /// Every project.
    All,
}

/// Where the list opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// Over the active pane, until the list is closed.
    Overlay,
    /// Next to the active pane, on the right.
    Split,
    /// In a tab of its own.
    Tab,
}

impl Placement {
    pub fn as_arg(self) -> &'static str {
        match self {
            Placement::Overlay => "overlay",
            Placement::Split => "split",
            Placement::Tab => "tab",
        }
    }
}

/// Which side of the focused pane a split list goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Down,
    Right,
}

impl Direction {
    pub fn as_arg(self) -> &'static str {
        match self {
            Direction::Down => "down",
            Direction::Right => "right",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub scope: Scope,
    /// Seconds between automatic refreshes.
    pub refresh_secs: u64,
    pub placement: Placement,
    /// Where a split goes: below (default) or to the right.
    pub split_direction: Direction,
    /// Nerd Font icons; plain Unicode symbols when false.
    pub nerd_font: bool,
    /// GitLab host the list asks (glab must be logged in there).
    pub gitlab_host: String,
    /// No blank line between the rows of the table.
    pub compact: bool,
    /// Whether `icons` is set in the file; when not, the list says how to
    /// switch to plain symbols, in case the terminal has no Nerd Font.
    pub icons_chosen: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            scope: Scope::Current,
            refresh_secs: 60,
            placement: Placement::Overlay,
            split_direction: Direction::Down,
            nerd_font: true,
            gitlab_host: "gitlab.com".to_string(),
            icons_chosen: false,
            compact: false,
        }
    }
}

impl Config {
    pub fn load(dir: Option<&Path>) -> Config {
        let Some(dir) = dir else {
            return Config::default();
        };
        match std::fs::read_to_string(dir.join("config.toml")) {
            Ok(text) => Config::parse(&text),
            Err(_) => Config::default(),
        }
    }

    pub fn parse(text: &str) -> Config {
        let mut config = Config::default();
        for raw in text.lines() {
            let line = raw.split('#').next().unwrap_or("").trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = value.trim().trim_matches('"').trim();
            match key.trim() {
                "scope" => match value {
                    "current" => config.scope = Scope::Current,
                    "all" => config.scope = Scope::All,
                    _ => {}
                },
                "refresh_secs" => {
                    if let Ok(n) = value.parse::<u64>() {
                        if (10..=3600).contains(&n) {
                            config.refresh_secs = n;
                        }
                    }
                }
                "placement" => match value {
                    "overlay" => config.placement = Placement::Overlay,
                    "split" => config.placement = Placement::Split,
                    "tab" => config.placement = Placement::Tab,
                    _ => {}
                },
                "split_direction" => match value {
                    "down" => config.split_direction = Direction::Down,
                    "right" => config.split_direction = Direction::Right,
                    _ => {}
                },
                "icons" => match value {
                    "nerd" => {
                        config.nerd_font = true;
                        config.icons_chosen = true;
                    }
                    "plain" => {
                        config.nerd_font = false;
                        config.icons_chosen = true;
                    }
                    _ => {}
                },
                "compact" => match value {
                    "true" => config.compact = true,
                    "false" => config.compact = false,
                    _ => {}
                },
                "gitlab_host" if !value.is_empty() => config.gitlab_host = value.to_string(),
                _ => {}
            }
        }
        config
    }
}

/// Writes `icons = "nerd"` or `"plain"` into `config.toml`, keeping every
/// other line.
pub fn save_icons(dir: &Path, nerd: bool) -> std::io::Result<()> {
    let file = dir.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    std::fs::create_dir_all(dir)?;
    std::fs::write(&file, with_icons(&text, nerd))
}

fn with_icons(text: &str, nerd: bool) -> String {
    let line = format!("icons = \"{}\"", if nerd { "nerd" } else { "plain" });
    let mut found = false;
    let mut out: Vec<String> = text
        .lines()
        .map(|l| {
            let key = l
                .split('#')
                .next()
                .unwrap_or("")
                .split('=')
                .next()
                .unwrap_or("");
            if l.contains('=') && key.trim() == "icons" {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if !found {
        out.push(line);
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::parse("");
        assert_eq!(c.scope, Scope::Current);
        assert_eq!(c.refresh_secs, 60);
        assert_eq!(c.placement, Placement::Overlay);
        assert_eq!(c.split_direction, Direction::Down);
        assert!(c.nerd_font);
    }

    #[test]
    fn icons_setting_is_written() {
        assert_eq!(with_icons("", false), "icons = \"plain\"\n");
        assert_eq!(
            with_icons("scope = \"all\"\nicons = \"nerd\" # x\n", false),
            "scope = \"all\"\nicons = \"plain\"\n"
        );
        assert!(Config::parse(&with_icons("", false)).icons_chosen);
        assert!(!Config::parse("").icons_chosen);
    }

    #[test]
    fn values_and_bad_lines() {
        let c = Config::parse(
            "scope = \"all\"\nrefresh_secs = 5\nplacement = split # comment\nsplit_direction = \"right\"\nicons = plain\n[x]\nnonsense\n",
        );
        assert_eq!(c.scope, Scope::All);
        assert_eq!(c.refresh_secs, 60, "below the minimum is ignored");
        assert_eq!(c.placement, Placement::Split);
        assert_eq!(c.split_direction, Direction::Right);
        assert!(!c.nerd_font);
    }
}
