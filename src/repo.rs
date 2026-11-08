//! Finds the GitLab project of a directory from its git remote, by reading
//! `.git/config` directly (no git command runs).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub host: String,
    /// Full path such as `group/subgroup/project`.
    pub path: String,
}

impl Project {
    /// The last part of the path, used as a short name.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

/// The project of the repository that contains `dir`, taken from the `origin`
/// remote (or the first remote when there is no `origin`).
pub fn detect(dir: &Path) -> Option<Project> {
    let common = git_common_dir(dir)?;
    let text = std::fs::read_to_string(common.join("config")).ok()?;
    let url = remote_url(&text)?;
    parse_remote(&url)
}

/// The directory holding the repository's shared config, also for worktrees
/// and submodules where `.git` is a file.
fn git_common_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let dot = d.join(".git");
        if dot.is_dir() {
            return Some(dot);
        }
        if dot.is_file() {
            let text = std::fs::read_to_string(&dot).ok()?;
            let gitdir = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
            let gitdir = if Path::new(gitdir).is_absolute() {
                PathBuf::from(gitdir)
            } else {
                d.join(gitdir)
            };
            if let Ok(common) = std::fs::read_to_string(gitdir.join("commondir")) {
                let common = common.trim();
                return Some(if Path::new(common).is_absolute() {
                    PathBuf::from(common)
                } else {
                    gitdir.join(common)
                });
            }
            return Some(gitdir);
        }
        dir = d.parent();
    }
    None
}

fn remote_url(config: &str) -> Option<String> {
    let mut section = String::new();
    let mut first = None;
    for raw in config.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        if !section.starts_with("[remote ") {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "url" {
            continue;
        }
        let url = value.trim().to_string();
        if section == "[remote \"origin\"]" {
            return Some(url);
        }
        first.get_or_insert(url);
    }
    first
}

/// Understands `git@host:group/project.git`, `ssh://git@host:22/group/project`
/// and `https://host/group/project.git`.
pub fn parse_remote(url: &str) -> Option<Project> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, r)| r) {
        let rest = rest.rsplit_once('@').map(|(_, r)| r).unwrap_or(rest);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?.to_string(), path.to_string())
    } else {
        let rest = url.rsplit_once('@').map(|(_, r)| r).unwrap_or(url);
        let (host, path) = rest.split_once(':')?;
        (host.to_string(), path.to_string())
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path).to_string();
    if host.is_empty() || !path.contains('/') {
        return None;
    }
    Some(Project { host, path })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_forms() {
        let want = Project {
            host: "gitlab.com".into(),
            path: "acme/tools/app".into(),
        };
        assert_eq!(
            parse_remote("git@gitlab.com:acme/tools/app.git"),
            Some(want.clone())
        );
        assert_eq!(
            parse_remote("https://gitlab.com/acme/tools/app.git"),
            Some(want.clone())
        );
        assert_eq!(
            parse_remote("ssh://git@gitlab.com:22/acme/tools/app"),
            Some(want.clone())
        );
        assert_eq!(
            parse_remote("https://user@gitlab.com/acme/tools/app/"),
            Some(want)
        );
        assert_eq!(parse_remote("/local/path"), None);
    }

    #[test]
    fn origin_wins() {
        let cfg = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = git@a.com:x/y.git\n[remote \"origin\"]\n\turl = git@b.com:p/q.git\n";
        assert_eq!(remote_url(cfg).as_deref(), Some("git@b.com:p/q.git"));
    }

    #[test]
    fn worktree_file() {
        let root = std::env::temp_dir().join(format!("mr-repo-test-{}", std::process::id()));
        let main_git = root.join("main/.git");
        let wt_git = main_git.join("worktrees/wt");
        let wt = root.join("wt/sub");
        std::fs::create_dir_all(&wt_git).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(
            main_git.join("config"),
            "[remote \"origin\"]\n\turl = git@gitlab.com:acme/app.git\n",
        )
        .unwrap();
        std::fs::write(wt_git.join("commondir"), "../..\n").unwrap();
        std::fs::write(
            root.join("wt/.git"),
            format!("gitdir: {}\n", wt_git.display()),
        )
        .unwrap();
        let p = detect(&wt).unwrap();
        assert_eq!(p.path, "acme/app");
        assert_eq!(p.name(), "app");
        std::fs::remove_dir_all(&root).ok();
    }
}
