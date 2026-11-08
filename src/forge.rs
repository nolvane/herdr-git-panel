//! One door to GitLab and GitHub: the list asks here, and each request goes
//! to the forge the merge request lives on.

use crate::cli;
use crate::model::*;
use crate::{github, gitlab};

/// The lists of every forge whose tool is installed and logged in.
pub struct Fetched {
    pub data: Data,
    /// A forge that failed while another one answered.
    pub warning: Option<String>,
}

pub fn fetch_all(gitlab_host: &str) -> Result<Fetched, String> {
    let use_gitlab = cli::find("glab").is_some();
    let use_github = github::available();
    if !use_gitlab && !use_github {
        return Err(
            "Neither glab nor gh is installed: brew install glab (GitLab) or gh (GitHub)".into(),
        );
    }
    let (lab, hub) = std::thread::scope(|scope| {
        let hub = scope.spawn(|| {
            if use_github {
                github::fetch()
            } else {
                Ok(None)
            }
        });
        let lab = if use_gitlab {
            gitlab::fetch(gitlab_host).map(Some)
        } else {
            Ok(None)
        };
        (lab, hub.join().unwrap_or_else(|_| Err("gh crashed".into())))
    });

    let mut data = Data::default();
    let mut errors = Vec::new();
    let mut answered = false;
    for result in [lab, hub] {
        match result {
            Ok(Some(d)) => {
                answered = true;
                data.review.extend(d.review);
                data.mine.extend(d.mine);
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if !answered {
        return Err(if errors.is_empty() {
            "gh is not logged in to github.com: run gh auth login".into()
        } else {
            errors.join(" · ")
        });
    }
    for list in [&mut data.review, &mut data.mine] {
        list.sort_by_key(|m| std::cmp::Reverse(m.updated));
    }
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
