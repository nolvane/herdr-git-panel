//! One door to GitLab and GitHub: the list asks here, and each request goes
//! to the forge the merge request lives on.

use crate::cli;
use crate::model::*;
use crate::repo::Project;
use crate::{github, gitlab};

/// The lists of every forge whose tool is installed and logged in.
pub struct Fetched {
    pub data: Data,
    /// A forge that failed while another one answered.
    pub warning: Option<String>,
}

/// Which forges "All" asks: only the one `project` lives on, or every forge
/// for your projects (`None`). As `(GitLab, GitHub)`.
pub fn all_forges(project: Option<&Project>, gitlab_host: &str) -> (bool, bool) {
    match project {
        Some(p) => (p.host == gitlab_host, p.host == github::HOST),
        None => (true, true),
    }
}

/// The lists of every forge, and "All" for `all_of` (`None`: your projects).
pub fn fetch_all(gitlab_host: &str, all_of: Option<&Project>) -> Result<Fetched, String> {
    let use_gitlab = cli::find("glab").is_some();
    let use_github = github::available();
    if !use_gitlab && !use_github {
        return Err(
            "Neither glab nor gh is installed: brew install glab (GitLab) or gh (GitHub)".into(),
        );
    }
    let (all_gitlab, all_github) = all_forges(all_of, gitlab_host);
    let path = all_of.map(|p| p.path.as_str());
    let (lab, hub, lab_all, hub_all) = std::thread::scope(|scope| {
        let hub = scope.spawn(|| {
            if use_github {
                github::fetch()
            } else {
                Ok(None)
            }
        });
        let lab_all = scope.spawn(|| {
            if use_gitlab && all_gitlab {
                gitlab::fetch_all_of(gitlab_host, path).map(Some)
            } else {
                Ok(None)
            }
        });
        let hub_all = scope.spawn(|| {
            if use_github && all_github {
                github::fetch_all_of(path)
            } else {
                Ok(None)
            }
        });
        let lab = if use_gitlab {
            gitlab::fetch(gitlab_host).map(Some)
        } else {
            Ok(None)
        };
        (
            lab,
            hub.join().unwrap_or_else(|_| Err("gh crashed".into())),
            lab_all
                .join()
                .unwrap_or_else(|_| Err("glab crashed".into())),
            hub_all.join().unwrap_or_else(|_| Err("gh crashed".into())),
        )
    });
    combine(all_of, [lab, hub], [lab_all, hub_all])
}

/// Puts the forges' answers together: `lists` and `all` are GitLab's and
/// GitHub's, in that order. "All" failing never fails the other lists.
fn combine(
    all_of: Option<&Project>,
    lists: [Result<Option<Data>, String>; 2],
    all: [Result<Option<AllPart>, String>; 2],
) -> Result<Fetched, String> {
    let mut data = Data::default();
    let mut errors = Vec::new();
    let mut answered = [false; 2];
    for (i, result) in lists.into_iter().enumerate() {
        match result {
            Ok(Some(d)) => {
                answered[i] = true;
                data.review.extend(d.review);
                data.mine.extend(d.mine);
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if !answered.contains(&true) {
        return Err(if errors.is_empty() {
            "gh is not logged in to github.com: run gh auth login".into()
        } else {
            errors.join(" · ")
        });
    }
    let mut list = AllList {
        of: all_of.cloned(),
        ..AllList::default()
    };
    let mut all_errors = Vec::new();
    let mut covered: Vec<String> = Vec::new();
    for (i, result) in all.into_iter().enumerate() {
        match result {
            Ok(Some(part)) => {
                list.items.extend(part.items);
                list.total += part.total;
                covered.extend(part.projects);
            }
            Ok(None) => {}
            Err(e) => {
                // A forge whose lists failed too says so once already.
                if answered[i] {
                    errors.push(format!("All: {e}"));
                }
                all_errors.push(e);
            }
        }
    }
    list.error = (!all_errors.is_empty()).then(|| all_errors.join(" · "));
    // Your own and the ones you review in those projects are in "All" too,
    // also when they are older than what one request loads.
    for mr in data.mine.iter().chain(&data.review) {
        let key = mr.key();
        if covered.contains(&key.0) && !list.items.iter().any(|m| m.key() == key) {
            list.items.push(mr.clone());
        }
    }
    list.total = list.total.max(list.items.len() as u64);
    for list in [&mut data.review, &mut data.mine, &mut list.items] {
        list.sort_by_key(|m| std::cmp::Reverse(m.updated));
    }
    data.all = Some(list);
    Ok(Fetched {
        data,
        warning: (!errors.is_empty()).then(|| errors.join(" · ")),
    })
}

pub fn fetch_activity(mr: &MergeRequest) -> Result<Vec<Discussion>, String> {
    match mr.forge {
        Forge::GitLab => gitlab::fetch_activity(mr),
        Forge::GitHub => github::fetch_activity(mr),
    }
}

pub fn set_resolved(mr: &MergeRequest, thread: &str, resolve: bool) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_resolved(&mr.host, thread, resolve),
        Forge::GitHub => github::set_resolved(thread, resolve),
    }
}

pub fn set_draft(mr: &MergeRequest, draft: bool) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_draft(mr, draft),
        Forge::GitHub => github::set_draft(mr, draft),
    }
}

pub fn approve(mr: &MergeRequest) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_approval(mr, false),
        Forge::GitHub => github::approve(mr),
    }
}

/// Whether your approval can be taken back from the list.
pub fn can_revoke(forge: Forge) -> bool {
    forge == Forge::GitLab
}

pub fn revoke(mr: &MergeRequest) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::set_approval(mr, true),
        Forge::GitHub => Err("GitHub cannot take an approval back".into()),
    }
}

pub fn merge(mr: &MergeRequest) -> Result<(), String> {
    match mr.forge {
        Forge::GitLab => gitlab::merge(mr),
        Forge::GitHub => github::merge(mr),
    }
}

pub fn fetch_diff(mr: &MergeRequest) -> Result<String, String> {
    match mr.forge {
        Forge::GitLab => gitlab::fetch_diff(mr),
        Forge::GitHub => github::fetch_diff(mr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(host: &str) -> Project {
        Project {
            host: host.into(),
            path: "acme/app".into(),
        }
    }

    #[test]
    fn all_asks_only_the_projects_forge() {
        assert_eq!(
            all_forges(Some(&project("gitlab.com")), "gitlab.com"),
            (true, false)
        );
        assert_eq!(
            all_forges(Some(&project("github.com")), "gitlab.com"),
            (false, true)
        );
        assert_eq!(
            all_forges(Some(&project("git.acme.test")), "git.acme.test"),
            (true, false)
        );
        // Your projects: every forge.
        assert_eq!(all_forges(None, "gitlab.com"), (true, true));
    }

    fn mr(forge: Forge, iid: u64, updated: i64) -> MergeRequest {
        let mut d = crate::gitlab::parse(
            &serde_json::json!({"data": {"currentUser": {"username": "me", "authoredMergeRequests": {"nodes": [
                {"iid": iid.to_string(), "project": {"fullPath": "acme/app"}, "author": {"username": "alex"}}
            ]}}}}),
            "gitlab.com",
        )
        .unwrap();
        let mut m = d.mine.remove(0);
        m.forge = forge;
        m.updated = Some(updated);
        m
    }

    fn lists() -> Data {
        Data {
            review: vec![mr(Forge::GitLab, 1, 1)],
            mine: vec![mr(Forge::GitLab, 2, 2)],
            all: None,
        }
    }

    #[test]
    fn all_failing_keeps_the_other_lists() {
        let fetched = combine(
            Some(&project("gitlab.com")),
            [Ok(Some(lists())), Ok(None)],
            [Err("GitLab: boom".into()), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.data.review.len(), 1);
        assert_eq!(fetched.data.mine.len(), 1);
        assert_eq!(fetched.warning.as_deref(), Some("All: GitLab: boom"));
        let all = fetched.data.all.unwrap();
        assert!(all.items.is_empty());
        assert_eq!(all.error.as_deref(), Some("GitLab: boom"));
        assert_eq!(all.of, Some(project("gitlab.com")));
    }

    #[test]
    fn all_has_mine_and_the_ones_i_review_past_the_cut() {
        let mut other_project = mr(Forge::GitLab, 9, 9);
        other_project.project = "acme/other".into();
        let mut data = lists();
        data.mine.push(other_project);
        let part = AllPart {
            // The project's newest, which already holds my !2.
            items: vec![mr(Forge::GitLab, 3, 30), mr(Forge::GitLab, 2, 2)],
            total: 140,
            projects: vec!["gitlab.com/acme/app".into()],
        };
        let fetched = combine(
            Some(&project("gitlab.com")),
            [Ok(Some(data)), Ok(None)],
            [Ok(Some(part)), Ok(None)],
        )
        .unwrap();
        let all = fetched.data.all.unwrap();
        let iids: Vec<u64> = all.items.iter().map(|m| m.iid).collect();
        assert_eq!(iids, [3, 2, 1], "each once, none of another project");
        assert_eq!(all.total, 140);
    }

    #[test]
    fn a_forge_that_failed_says_it_once() {
        let fetched = combine(
            None,
            [Err("glab: not logged in".into()), Ok(Some(lists()))],
            [Err("glab: not logged in".into()), Ok(None)],
        )
        .unwrap();
        assert_eq!(fetched.warning.as_deref(), Some("glab: not logged in"));
    }

    #[test]
    fn all_of_both_forges_is_one_list_newest_first() {
        let lab = AllPart {
            items: vec![mr(Forge::GitLab, 3, 30), mr(Forge::GitLab, 4, 10)],
            total: 140,
            projects: Vec::new(),
        };
        let hub = AllPart {
            items: vec![mr(Forge::GitHub, 5, 20)],
            total: 54,
            projects: Vec::new(),
        };
        let fetched = combine(
            None,
            [Ok(Some(lists())), Ok(Some(Data::default()))],
            [Ok(Some(lab)), Ok(Some(hub))],
        )
        .unwrap();
        let all = fetched.data.all.unwrap();
        let iids: Vec<u64> = all.items.iter().map(|m| m.iid).collect();
        assert_eq!(iids, [3, 5, 4]);
        assert_eq!(all.total, 194);
        assert_eq!(all.of, None);
        assert_eq!(fetched.warning, None);
    }
}
