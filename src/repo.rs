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

    /// Whether it lives on `host`. Host names ignore case, so a remote
    /// written `GitHub.com` is on `github.com`.
    pub fn on(&self, host: &str) -> bool {
        self.host.eq_ignore_ascii_case(host)
    }

    /// Whether it is `host`/`path`. GitHub and GitLab ignore case in both,
    /// so a clone made from `Acme/App` is `acme/app`; only the whole path
    /// counts, so it is never `acme/app2`.
    pub fn is(&self, host: &str, path: &str) -> bool {
        self.on(host) && self.path.eq_ignore_ascii_case(path)
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

/// Every remote in a git config, as (name, url), in the order they appear.
fn remotes(config: &str) -> Vec<(String, String)> {
    let mut section = String::new();
    let mut found = Vec::new();
    for raw in config.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        let Some(name) = section
            .strip_prefix("[remote \"")
            .and_then(|s| s.strip_suffix("\"]"))
        else {
            continue;
        };
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() == "url" {
            found.push((name.to_string(), value.trim().to_string()));
        }
    }
    found
}

fn remote_url(config: &str) -> Option<String> {
    let all = remotes(config);
    all.iter()
        .find(|(name, _)| name == "origin")
        .or_else(|| all.first())
        .map(|(_, url)| url.clone())
}

/// A remote of the clone and the project it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// Its name in the clone, such as `origin` or `upstream`.
    pub name: String,
    pub project: Project,
}

impl Remote {
    /// Whether it points at `host`/`path`, ignoring case as [`Project::is`].
    pub fn points_at(&self, host: &str, path: &str) -> bool {
        self.project.is(host, path)
    }
}

/// Every remote of the repository that contains `dir` whose URL names a
/// project.
pub fn remotes_of(dir: &Path) -> Vec<Remote> {
    git_common_dir(dir)
        .and_then(|c| std::fs::read_to_string(c.join("config")).ok())
        .map(|text| project_remotes(&text))
        .unwrap_or_default()
}

fn project_remotes(config: &str) -> Vec<Remote> {
    remotes(config)
        .into_iter()
        .filter_map(|(name, url)| {
            Some(Remote {
                name,
                project: parse_remote(&url)?,
            })
        })
        .collect()
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
    // `.git` in any case, as the forges accept `App.GIT` too.
    let path = match path.len().checked_sub(4) {
        Some(cut) if path.is_char_boundary(cut) && path[cut..].eq_ignore_ascii_case(".git") => {
            &path[..cut]
        }
        _ => path,
    }
    .to_string();
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
    fn remotes_with_their_projects() {
        let cfg = "[remote \"origin\"]\n\turl = git@github.com:alex/app.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n[branch \"main\"]\n\tremote = origin\n[remote \"upstream\"]\n\turl = https://github.com/Acme/App.git\n[remote \"local\"]\n\turl = /srv/app.git\n";
        let all = project_remotes(cfg);
        let names: Vec<&str> = all.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["origin", "upstream"], "a path names no project");
        assert!(
            all[1].points_at("github.com", "acme/app"),
            "case does not count"
        );
        assert!(all[0].points_at("GitHub.com", "alex/app"));
        assert!(!all[0].points_at("gitlab.com", "alex/app"));
        assert!(!all[0].points_at("github.com", "alex/app2"));
    }

    #[test]
    fn a_project_matches_ignoring_case_but_only_whole() {
        let p = parse_remote("git@GitHub.com:Acme/App.git").unwrap();
        assert!(p.on("github.com"));
        assert!(p.is("github.com", "acme/app"));
        assert!(
            !p.is("github.com", "acme/app2"),
            "a longer name is another project"
        );
        assert!(!p.is("github.com", "acme/ap"));
        assert!(!p.is("gitlab.com", "acme/app"));
        assert!(!p.on("github.co"));
        // The `.git` ending counts in any case too.
        let p = parse_remote("https://GitHub.com/Acme/App.GIT").unwrap();
        assert!(p.is("github.com", "acme/app"), "{}", p.path);
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
