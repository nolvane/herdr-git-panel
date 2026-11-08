//! GitLab: open merge requests, their threads, approve, merge and changes,
//! through the `glab` CLI.

use crate::cli;
use crate::model::*;
use crate::time;
use serde_json::Value;

/// One list per request: both in one query exceed GitLab's complexity limit
/// (250). `LIST` is replaced by the list's field name.
const QUERY: &str = r#"
query {
  currentUser {
    username
    LIST(state: opened, first: 100) { nodes { ...mr } }
  }
}
fragment mr on MergeRequest {
  iid title description webUrl draft createdAt updatedAt sourceBranch targetBranch diffHeadSha
  sourceProjectId targetProjectId
  project { fullPath }
  author { username }
  approved approvalsRequired approvalsLeft
  approvedBy { nodes { username } }
  approvalState { rules { name type approved approvalsRequired } }
  reviewers { nodes { username mergeRequestInteraction { reviewState } } }
  detailedMergeStatus
  resolvableDiscussionsCount resolvedDiscussionsCount
  labels { nodes { title color } }
  headPipeline { status }
  diffStatsSummary { additions deletions }
}
"#;

pub fn fetch(host: &str) -> Result<Data, String> {
    // Both lists at the same time.
    let (review, mine) = std::thread::scope(|scope| {
        let mine = scope.spawn(|| query(host, "authoredMergeRequests"));
        let review = query(host, "reviewRequestedMergeRequests");
        (
            review,
            mine.join().unwrap_or_else(|_| Err("glab crashed".into())),
        )
    });
    let mut merged = review?;
    let mine = mine?;
    if merged.get("errors").is_none() && mine.get("errors").is_some() {
        merged = mine;
    } else if let Some(list) = mine
        .pointer("/data/currentUser/authoredMergeRequests")
        .cloned()
    {
        if let Some(user) = merged
            .pointer_mut("/data/currentUser")
            .and_then(Value::as_object_mut)
        {
            user.insert("authoredMergeRequests".into(), list);
        }
    }
    parse(&merged, host)
}

/// Runs one list query through glab and returns GitLab's JSON answer.
fn query(host: &str, list: &str) -> Result<Value, String> {
    graphql(host, &QUERY.replace("LIST", list), &[])
}

/// Runs a GraphQL query through glab, with variables, and returns GitLab's
/// JSON answer.
fn graphql(host: &str, query: &str, vars: &[(&str, &str)]) -> Result<Value, String> {
    let mut cmd = cli::command("glab");
    cmd.args(["api", "graphql", "--hostname", host, "-f"])
        .arg(format!("query={query}"));
    for (k, v) in vars {
        cmd.arg("-f").arg(format!("{k}={v}"));
    }
    let out = cli::run("glab", cmd, host)?;
    // GitLab's own errors come as JSON, also when glab exits with an error.
    if let Ok(value) = serde_json::from_slice::<Value>(&out.stdout) {
        return Ok(value);
    }
    if !out.success {
        return Err(cli::explain_failure("glab", host, &out.stderr));
    }
    Err("unreadable answer from GitLab".into())
}

/// Runs a `glab mr …` command on the merge request and returns its output.
fn mr_command(mr: &MergeRequest, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = cli::command("glab");
    cmd.arg("mr")
        .args(&args[..1])
        .arg(mr.iid.to_string())
        .args(&args[1..])
        .arg("--repo")
        .arg(format!("https://{}/{}", mr.host, mr.project));
    let out = cli::run("glab", cmd, &mr.host)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(cli::explain_failure("glab", &mr.host, &out.stderr))
    }
}

const ACTIVITY: &str = r#"
query($project: ID!, $iid: String!) {
  project(fullPath: $project) {
    mergeRequest(iid: $iid) {
      discussions(first: 100) {
        nodes {
          id resolvable resolved
          notes(first: 50) {
            nodes { body system createdAt author { username } position { filePath newLine oldLine } }
          }
        }
      }
    }
  }
}
"#;

/// The discussions of a merge request, newest activity first.
pub fn fetch_activity(mr: &MergeRequest) -> Result<Vec<Discussion>, String> {
    let iid = mr.iid.to_string();
    let value = graphql(
        &mr.host,
        ACTIVITY,
        &[("project", &mr.project), ("iid", &iid)],
    )?;
    parse_activity(&value)
}

pub fn parse_activity(value: &Value) -> Result<Vec<Discussion>, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let nodes = value
        .pointer("/data/project/mergeRequest/discussions/nodes")
        .and_then(Value::as_array)
        .ok_or("GitLab returned no discussions")?;
    let mut list: Vec<Discussion> = nodes
        .iter()
        .map(|d| {
            let notes: Vec<Note> = d["notes"]["nodes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|n| Note {
                            author: str_of(&n["author"]["username"]),
                            body: str_of(&n["body"]),
                            created: n["createdAt"].as_str().and_then(time::parse_iso),
                            system: n["system"].as_bool().unwrap_or(false),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let first = &d["notes"]["nodes"][0]["position"];
            let place = first["filePath"].as_str().map(|path| {
                match first["newLine"]
                    .as_u64()
                    .or_else(|| first["oldLine"].as_u64())
                {
                    Some(line) => format!("{path}:{line}"),
                    None => path.to_string(),
                }
            });
            Discussion {
                id: str_of(&d["id"]),
                resolvable: d["resolvable"].as_bool().unwrap_or(false),
                resolved: d["resolved"].as_bool().unwrap_or(false),
                place,
                notes,
            }
        })
        .filter(|d| !d.notes.is_empty())
        .collect();
    // Newest activity first: by the last note of each discussion.
    list.sort_by_key(|d| std::cmp::Reverse(d.notes.iter().filter_map(|n| n.created).max()));
    Ok(list)
}

/// Marks a thread as resolved, or opens it again.
pub fn set_resolved(host: &str, discussion: &str, resolve: bool) -> Result<(), String> {
    let mutation = format!(
        "mutation($id: DiscussionID!) {{ discussionToggleResolve(input: {{id: $id, resolve: {resolve}}}) {{ errors }} }}"
    );
    let value = graphql(host, &mutation, &[("id", discussion)])?;
    if let Some(msg) = cli::graphql_errors(&value) {
        return Err(format!("GitLab: {msg}"));
    }
    let errors = value
        .pointer("/data/discussionToggleResolve/errors")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("; ")
        })
        .unwrap_or_default();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("GitLab: {errors}"))
    }
}

/// Merges the merge request with its own settings (squash, delete branch),
/// only if its head is still the one the list showed.
pub fn merge(mr: &MergeRequest) -> Result<(), String> {
    let mut args = vec!["merge", "--yes", "--auto-merge=false"];
    if !mr.head_sha.is_empty() {
        args.extend(["--sha", mr.head_sha.as_str()]);
    }
    mr_command(mr, &args).map(|_| ())
}

/// Marks the merge request as draft, or as ready.
pub fn set_draft(mr: &MergeRequest, draft: bool) -> Result<(), String> {
    mr_command(mr, &["update", if draft { "--draft" } else { "--ready" }]).map(|_| ())
}

/// Approves the merge request, or takes your approval back (`revoke`).
/// An approval is pinned to the head the list showed, so a push after it
/// loaded makes GitLab refuse the approval instead of approving unseen code.
pub fn set_approval(mr: &MergeRequest, revoke: bool) -> Result<(), String> {
    mr_command(mr, &approval_args(revoke, &mr.head_sha)).map(|_| ())
}

fn approval_args(revoke: bool, head_sha: &str) -> Vec<&str> {
    if revoke {
        vec!["revoke"]
    } else if head_sha.is_empty() {
        vec!["approve"]
    } else {
        vec!["approve", "--sha", head_sha]
    }
}

/// The merge request's changes as a unified diff, without colours.
pub fn fetch_diff(mr: &MergeRequest) -> Result<String, String> {
    mr_command(mr, &["diff", "--color", "never", "--raw"])
        .map(|out| String::from_utf8_lossy(&out).into_owned())
}

pub fn parse(value: &Value, host: &str) -> Result<Data, String> {
    if let Some(msg) = cli::graphql_errors(value) {
        return Err(format!("GitLab: {msg}"));
    }
    let user = &value["data"]["currentUser"];
    if user.is_null() {
        return Err("GitLab did not return a user: run glab auth login".into());
    }
    let viewer = str_of(&user["username"]);
    let list = |key: &str| -> Vec<MergeRequest> {
        let mut mrs: Vec<MergeRequest> = user[key]["nodes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| parse_mr(n, host, &viewer))
                    .collect()
            })
            .unwrap_or_default();
        mrs.sort_by_key(|m| std::cmp::Reverse(m.updated));
        mrs
    };
    Ok(Data {
        review: list("reviewRequestedMergeRequests"),
        mine: list("authoredMergeRequests"),
    })
}

fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

fn num_of(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

/// Whether the merge request's branch lives in another project (a fork).
/// An unknown source project counts as a fork: the branch is then fetched
/// through the merge request's own ref, which is right either way.
fn from_fork(n: &Value) -> bool {
    match (n["sourceProjectId"].as_u64(), n["targetProjectId"].as_u64()) {
        (Some(source), Some(target)) => source != target,
        _ => true,
    }
}

fn parse_mr(n: &Value, host: &str, viewer: &str) -> Option<MergeRequest> {
    let iid = n["iid"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| n["iid"].as_u64())?;
    let draft = n["draft"].as_bool().unwrap_or(false);
    let merge_state = match n["detailedMergeStatus"].as_str().unwrap_or("") {
        _ if draft => MergeState::Draft,
        "MERGEABLE" => MergeState::Ready,
        "DRAFT_STATUS" => MergeState::Draft,
        "CONFLICT" => MergeState::Conflict,
        "NEED_REBASE" => MergeState::NeedsRebase,
        "CHECKING" | "UNCHECKED" | "PREPARING" | "CI_STILL_RUNNING" => MergeState::Checking,
        _ => MergeState::Blocked,
    };
    let required = num_of(&n["approvalsRequired"]);
    let approver_names: Vec<String> = n["approvedBy"]["nodes"]
        .as_array()
        .map(|a| a.iter().map(|u| str_of(&u["username"])).collect())
        .unwrap_or_default();
    let approved_by = approver_names.len() as u64;
    let reviewers = n["reviewers"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|r| Reviewer {
                    username: str_of(&r["username"]),
                    state: match r["mergeRequestInteraction"]["reviewState"]
                        .as_str()
                        .unwrap_or("")
                    {
                        "APPROVED" => ReviewState::Approved,
                        "REQUESTED_CHANGES" => ReviewState::ChangesRequested,
                        "REVIEWED" => ReviewState::Reviewed,
                        "REVIEW_STARTED" => ReviewState::Started,
                        _ => ReviewState::Unreviewed,
                    },
                })
                .collect()
        })
        .unwrap_or_default();
    let mut pending_rules: Vec<String> = Vec::new();
    // Each rule once: GitLab's own total counts code owner sections twice.
    let rules_required: u64 = n["approvalState"]["rules"]
        .as_array()
        .map(|a| a.iter().map(|r| num_of(&r["approvalsRequired"])).sum())
        .unwrap_or(0);
    for rule in n["approvalState"]["rules"].as_array().into_iter().flatten() {
        if rule["approved"].as_bool().unwrap_or(true) || num_of(&rule["approvalsRequired"]) == 0 {
            continue;
        }
        let name = match rule["type"].as_str().unwrap_or("") {
            // A code owner rule is named after its path pattern.
            "CODE_OWNER" => "code owners".to_string(),
            "ANY_APPROVER" => "any approver".to_string(),
            _ => str_of(&rule["name"]),
        };
        if !name.is_empty() && !pending_rules.contains(&name) {
            pending_rules.push(name);
        }
    }
    let total = num_of(&n["resolvableDiscussionsCount"]);
    let resolved = num_of(&n["resolvedDiscussionsCount"]);
    let pipeline = match n["headPipeline"]["status"].as_str() {
        None => Pipeline::None,
        Some("SUCCESS") => Pipeline::Success,
        Some("FAILED") => Pipeline::Failed,
        Some("CANCELED") | Some("CANCELING") | Some("SKIPPED") => Pipeline::Canceled,
        Some("MANUAL") | Some("SCHEDULED") => Pipeline::Manual,
        Some(_) => Pipeline::Running,
    };
    let labels = n["labels"]["nodes"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|l| Label {
                    title: str_of(&l["title"]),
                    color: str_of(&l["color"]),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(MergeRequest {
        forge: Forge::GitLab,
        host: host.to_string(),
        viewer: viewer.to_string(),
        iid,
        title: str_of(&n["title"]),
        description: str_of(&n["description"]),
        url: str_of(&n["webUrl"]),
        project: str_of(&n["project"]["fullPath"]),
        author: str_of(&n["author"]["username"]),
        source_branch: str_of(&n["sourceBranch"]),
        from_fork: from_fork(n),
        head_sha: str_of(&n["diffHeadSha"]),
        target_branch: str_of(&n["targetBranch"]),
        created: n["createdAt"].as_str().and_then(time::parse_iso),
        updated: n["updatedAt"].as_str().and_then(time::parse_iso),
        merge_state,
        approvals_given: approved_by,
        approvals_required: Some(if rules_required > 0 {
            rules_required
        } else {
            required
        }),
        approved_by: approver_names,
        approved: n["approved"].as_bool().unwrap_or(false) && (required > 0 || approved_by > 0),
        pending_rules,
        reviewers,
        threads_total: total,
        threads_open: total.saturating_sub(resolved),
        labels,
        pipeline,
        additions: n["diffStatsSummary"]["additions"].as_u64(),
        deletions: n["diffStatsSummary"]["deletions"].as_u64(),
        merge_method: String::new(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn approval_is_pinned_to_the_shown_head() {
        assert_eq!(
            approval_args(false, "abc123"),
            ["approve", "--sha", "abc123"]
        );
        assert_eq!(approval_args(false, ""), ["approve"]);
        assert_eq!(approval_args(true, "abc123"), ["revoke"]);
    }

    use super::*;
    use serde_json::json;

    fn sample() -> Value {
        json!({"data": {"currentUser": {
            "username": "me",
            "reviewRequestedMergeRequests": {"nodes": [{
                "iid": "42", "title": "Add export", "webUrl": "https://gitlab.com/acme/app/-/merge_requests/42",
                "draft": false, "updatedAt": "2026-11-08T08:00:00Z",
                "sourceBranch": "abc-1-export", "targetBranch": "main",
                "sourceProjectId": 11, "targetProjectId": 11,
                "project": {"fullPath": "acme/app"}, "author": {"username": "alex"},
                "approved": false, "approvalsRequired": 1, "approvalsLeft": 1,
                "approvedBy": {"nodes": []},
                "approvalState": {"rules": [
                    {"name": "*, /docs/", "type": "CODE_OWNER", "approved": false, "approvalsRequired": 1},
                    {"name": "Builders", "type": "REGULAR", "approved": false, "approvalsRequired": 1},
                    {"name": "Optional", "type": "REGULAR", "approved": false, "approvalsRequired": 0}
                ]},
                "reviewers": {"nodes": [{"username": "me", "mergeRequestInteraction": {"reviewState": "UNREVIEWED"}}]},
                "detailedMergeStatus": "NOT_APPROVED",
                "resolvableDiscussionsCount": 3, "resolvedDiscussionsCount": 1,
                "labels": {"nodes": [{"title": "backend", "color": "#428BCA"}]},
                "headPipeline": {"status": "RUNNING"},
                "diffStatsSummary": {"additions": 120, "deletions": 8}
            }]},
            "authoredMergeRequests": {"nodes": [
                {"iid": "7", "title": "Old", "webUrl": "u7", "draft": true, "updatedAt": "2026-11-01T08:00:00Z",
                 "project": {"fullPath": "acme/app"}, "author": {"username": "me"},
                 "approved": true, "approvalsRequired": 0, "approvalsLeft": 0, "approvedBy": {"nodes": []},
                 "reviewers": {"nodes": []}, "detailedMergeStatus": "DRAFT_STATUS",
                 "resolvableDiscussionsCount": 0, "resolvedDiscussionsCount": 0,
                 "labels": {"nodes": []}, "headPipeline": null, "diffStatsSummary": null},
                {"iid": "9", "title": "New", "webUrl": "u9", "draft": false, "updatedAt": "2026-11-07T08:00:00Z",
                 "sourceProjectId": 12, "targetProjectId": 11,
                 "project": {"fullPath": "acme/lib"}, "author": {"username": "me"},
                 "approved": true, "approvalsRequired": 2, "approvalsLeft": 0, "approvedBy": {"nodes": [{"username": "alex"}]},
                 "reviewers": {"nodes": [{"username": "alex", "mergeRequestInteraction": {"reviewState": "APPROVED"}}]},
                 "detailedMergeStatus": "MERGEABLE",
                 "resolvableDiscussionsCount": 2, "resolvedDiscussionsCount": 2,
                 "labels": {"nodes": []}, "headPipeline": {"status": "SUCCESS"}, "diffStatsSummary": {"additions": 1, "deletions": 0}}
            ]}
        }}})
    }

    #[test]
    fn parses_both_lists() {
        let d = parse(&sample(), "gitlab.com").unwrap();
        assert_eq!(d.review[0].viewer, "me");
        assert_eq!(d.review[0].reference(), "!42");
        let r = &d.review[0];
        assert_eq!(r.iid, 42);
        assert_eq!(r.project_name(), "app");
        assert_eq!(r.threads_open, 2);
        assert_eq!(r.approvals_given, 0);
        assert_eq!(r.pipeline, Pipeline::Running);
        assert_eq!(r.merge_state, MergeState::Blocked);
        assert_eq!(r.review_of("me"), Some(ReviewState::Unreviewed));
        // Newest first.
        assert_eq!(d.mine[0].iid, 9);
        assert_eq!(d.mine[0].approvals_given, 1);
        assert_eq!(d.mine[0].approved_by, vec!["alex"]);
        assert!(d.mine[0].pending_rules.is_empty());
        assert_eq!(d.review[0].pending_rules, vec!["code owners", "Builders"]);
        assert_eq!(d.review[0].approvals_required, Some(2), "one per rule");
        assert_eq!(d.mine[0].merge_state, MergeState::Ready);
        assert!(d.mine[0].approved);
        assert_eq!(d.mine[1].merge_state, MergeState::Draft);
        assert!(!d.mine[1].approved, "no approval rule and nobody approved");
        assert_eq!(d.mine[1].pipeline, Pipeline::None);
        assert!(!d.review[0].from_fork, "same project");
        assert!(d.mine[0].from_fork, "from another project");
        assert!(d.mine[1].from_fork, "source unknown: the safe side");
    }

    #[test]
    fn activity_newest_first_with_place() {
        let v = json!({"data": {"project": {"mergeRequest": {"discussions": {"nodes": [
            {"resolvable": false, "resolved": false, "notes": {"nodes": [
                {"body": "approved this merge request", "system": true, "createdAt": "2026-11-01T08:00:00Z",
                 "author": {"username": "alex"}, "position": null}]}},
            {"id": "gid://gitlab/Discussion/abc", "resolvable": true, "resolved": false, "notes": {"nodes": [
                {"body": "Why?", "system": false, "createdAt": "2026-11-02T08:00:00Z",
                 "author": {"username": "sam"}, "position": {"filePath": "src/a.rs", "newLine": 12, "oldLine": null}},
                {"body": "Because.", "system": false, "createdAt": "2026-11-03T08:00:00Z",
                 "author": {"username": "me"}, "position": null}]}},
            {"resolvable": false, "resolved": false, "notes": {"nodes": []}}
        ]}}}}});
        let a = parse_activity(&v).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].place.as_deref(), Some("src/a.rs:12"));
        assert_eq!(a[0].id, "gid://gitlab/Discussion/abc");
        assert_eq!(a[0].notes.len(), 2);
        assert!(a[1].notes[0].system);
    }

    #[test]
    fn graphql_errors_are_reported() {
        let e = parse(&json!({"errors": [{"message": "boom"}]}), "gitlab.com").unwrap_err();
        assert!(e.contains("boom"));
    }
}
