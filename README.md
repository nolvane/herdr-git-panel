# Herdr Git Panel

A [Herdr](https://herdr.dev) plugin, written in Rust, that lists your GitLab
merge requests and GitHub pull requests in a pane: your own open ones, the
ones you have to review, and all open ones of the project, and with `z` the
merged or closed ones instead. It uses whichever
of `glab` and `gh` is installed and logged in; with both, the lists mix
GitLab and GitHub, each row with its forge's logo.

```
  Merge requests    Mine 8   To review 2   All 14                     ⌕ Search   all projects

     Merge request                         Author   Review  CI  Approvals  Threads       Changes  Opened  Updated
    Add payment notices to the contract    @alex      ●     ●   1/2        –             +48 -9   2d      1h
      ledger !90  ·  Draft
    Fix the login redirect                 @sam       ●     ●   0          💬 3 open     +40 -10  8m      6m
      my-app #2903  ·  Pipeline failed
 ⏎ open  p details  / search  ? all keys  q close                          r refresh  ↻ updated 4s ago
```

Each row takes two lines: the title in white, and under it the project, the
number (`!90` on GitLab, `#2903` on GitHub) and where it stands (ready to
merge, draft, merge conflict, needs rebase, pipeline failed, open threads,
waiting for approval; merged or closed for those). The columns:

| Column | Meaning |
|---|---|
| Author / Reviewers | Who wrote it (to review, all), or who reviews it (mine) |
| Review | Your own review as a dot (to review, and in all only where you review): green approved, red changes requested, yellow commented or started, grey not yet |
| CI | The pipeline (GitLab) or the checks (GitHub) of the newest commit as a dot: green passed, red failed, yellow running, grey canceled or manual |
| Approvals | Approvals given of needed (`1/2`); only the number when the forge does not say how many are needed; a check mark once it is approved |
| Threads | Open threads, or how many were resolved |
| Changes | Lines added and removed |
| Opened / Updated | Time since it was opened and since the last update |

Narrow panes drop columns (opened, changes, people, approvals, pipeline)
before the title gets shorter than 30 characters. Labels show in the details
panel only.

`compact = true` gives one line per merge request, with the number and the
labels in columns of their own.

`p` opens the details panel (on the right on wide panes, below otherwise).
**Overview**: title, status, branches, author, every reviewer and where they
are, who approved and which approval rules still wait, threads, pipeline,
size, labels, link and the description with its Markdown rendered.
**Activity** (`c`, or a click on the tab): the comments and threads, newest
first, with replies, the file and line a thread is on and the forge's own
notes (approvals, new commits) as short lines. Show all, only open or only resolved
threads (`t`, or click the chips); `n` selects the next thread and `N` the
previous one, `e` (or the thread's "Mark resolved" / "Reopen thread" button)
resolves the selected thread or opens it again. On open ones, `>` replies to
the selected thread and `+` writes a comment on the merge request itself: a
box at the bottom of the panel takes the text, over several lines (`Enter`
adds a line break, pasted text goes in whole). `Ctrl+s` shows what will be
posted and where (`↑` `↓` scroll a long text), `y` posts it (`n` goes back to
the text, `Esc` throws it away). The new reply or comment then shows in the tab; when posting fails,
the box says why and keeps the text. On GitLab a line of a reply or
comment that starts with `/` is posted as text, never run as a GitLab
command (`\/` in front, as for change requests; inside a code block the
backslash shows). Links in both tabs (the merge
request's link, Markdown links and plain URLs in descriptions and comments)
open in the browser with a click.
The tab loads the 100 threads started last; a thread with more than
50 notes shows its first note and its newest 49, with a dim line that says
how many replies in between are not shown.

`d` shows the changes inside the list: the changed files as a tree on the
left (with lines added and removed; click one to jump to it, `f` hides the
tree), the diff on the right with line numbers and colours. `n` and `N` jump
between files, the arrows and the mouse wheel scroll, `q` or `Esc` go back.

The list refreshes every 60 seconds; `r` (or a click on it at the bottom
right) refreshes at once. The footer shows the most used keys and when the
list last refreshed; `?` shows all keys.

`q` closes the list after asking ("Close the git panel?", `y` to close, any
other key keeps it). `Esc` never closes it: it only closes the panel, the
search or a filter.

## The three sections

- **Mine**: your own open merge requests. The list opens here.
- **To review**: the ones you are asked to review (on GitHub also the ones
  you reviewed already).
- **All**: every open merge request of the project the list was opened in,
  whoever wrote it and whoever reviews it, your own and the ones you review
  included. With "all projects" (`a`), or outside a project, it lists the
  open ones of the projects you are a member of instead: on GitLab the ones
  you are a member of, on GitHub your own repositories, the ones you
  collaborate on and those of your organizations.

"All" loads the newest open ones up to what one request gives (100 on
GitLab, 30 on GitHub, the limits the other lists use); when there are more,
the footer says how many it shows ("newest 30 of 54"). With "all projects"
it looks at your 100 most recently active projects on each forge. Since
"All" lists other merge requests per scope, `a` loads it again, and it shows
"Loading" for a moment while "Mine" and "To review" switch at once.

## Open, merged and closed

The list shows open merge requests when it opens. `z` (or a click on the
title) switches to merged ones, then to ones closed without merging, then
back to open ones. The title says which: "Merge requests" for open ones,
"Merged merge requests" or "Closed merge requests" otherwise ("pull
requests" when everything shown is on GitHub; only "Merged" or "Closed" on
a narrow pane). Every section follows it, each row says "Merged" or
"Closed", and the list opens on open ones again next time.

- Merged and closed ones load when you first switch to them, the most
  recently updated first, up to the same limits as open ones (100 per list
  on GitLab, 30 on GitHub). With "this project" they are that project's own
  newest, asked from its forge only, so `a` loads them again.
- The lists of each state are kept while the list is open: switching back to
  open ones shows them at once, with the rows selected where you left them.
  `r` and the automatic refresh load the state on screen.
- Merged and closed ones can be opened, copied, looked at in the details
  panel, the activity tab and the changes viewer, searched and sorted.
  Approving (`v`), merging (`m`), draft (`w`) and opening the branch (`b`)
  are for open ones only; the list says "!42 is merged" instead.
- The quick filters (`f`) are for open ones; switching away from open ones
  turns them off. `x` and `Esc` keep the state.

## Search and sort

- `/` searches as you type: every word must appear in the number, title,
  project, author, branch or a label. `Enter` keeps the search, `Esc` clears
  it.
- `f` (open ones only) steps through what is shown: all, needs me (to
  review: not approved or answered by you yet; mine: open threads, failed
  pipeline, conflict or rebase; all: your own and the ones you review by
  those rules, nobody else's), open threads, pipeline failed, ready to merge.
- `s` steps through sort orders (updated, opened, open threads, size, project,
  title). Clicking a column heading sorts by it; a second click turns the
  order round.
- `x` clears search and filter.

Active filters and sort orders show as chips on the right of the header,
next to the search field; each can be clicked.

## Approve and merge

`v` approves the selected merge request after you confirm with `y` (or click
"yes"); on a GitLab merge request you already approved it takes your approval
back (GitHub has no way to do that). Your own cannot be approved from here.

`!` requests changes on the selected merge request, with a reason: a box at
the bottom takes the text over several lines (`Enter` adds a line break),
`Ctrl+s` shows it with the merge request it goes to, and only `y` sends it
(`n` goes back to the text, `Esc` throws it away). On GitHub it is a review
with "Request changes" on the commit the list showed; on GitLab a comment
with the text that also submits your review as "requested changes" (this
also publishes comments of yours that are still pending in a review). The
Review dot turns red at the refresh. Your own cannot get it, like
approving; when sending fails, the box says why and keeps the text. On
GitLab a line of the reason that starts with `/` is sent as text, never as
a GitLab command (`\/` in front; inside a code block the backslash shows).

`m` merges it after you confirm, only when GitLab or GitHub says it can be
merged (approved, pipeline passed, no conflict, threads resolved); otherwise
the list says what is missing. GitLab uses the merge request's own settings
(squash, delete the branch), GitHub the repository's default merge method.
Either way it only goes through if nobody pushed since the list loaded it.

Both run through `glab` or `gh` and refresh the list.

`Ctrl+r` starts a failed pipeline again after you confirm with `y`, on open
ones with a red CI dot, your own and others'. On GitLab it retries the
failed and cancelled jobs of the merge request's head pipeline; on GitHub
the failed jobs of every failed GitHub Actions run of the newest commit.
Other apps' checks and outside CI services' statuses can't be started from
here, and the list says so. On any other pipeline it asks nothing and says
why there is nothing to start. Like a merge, it only goes through if nobody
pushed since the list loaded it. The CI dot shows it running at the latest
at the next refresh.

`w` marks your own merge request as a draft, or as ready for review again,
without asking (the same key undoes it).

## Open the branch locally

`b` fetches the branch of the selected merge request or pull request and
opens it in a new Herdr workspace, through Herdr's own worktree feature:
Herdr creates a folder with the branch checked out (under its
`[worktrees].directory`, `~/.herdr/worktrees` by default) and groups it with
the repository in the sidebar. Start a coding agent there to review or
continue the work; remove the folder again with Herdr's own worktree remove
action.

- It works on rows of the repository the list was opened in (any of its
  remotes, such as `origin` and `upstream`), and only inside Herdr.
- Your current checkout, its branch and its uncommitted changes stay as they
  are.
- A branch of the same repository gets a local branch of the same name that
  tracks it, so `git push` there updates the merge request (a local branch
  that already tracks something keeps its upstream). A fork's branch is
  fetched through the merge request's own ref and opens as `pr-<n>` (GitHub)
  or `mr-<iid>` (GitLab), without upstream.
- A second `b` opens the existing folder again. A local branch that has
  commits of its own is kept as it is, and the list says it differs (for a
  fork's `pr-<n>` after a rebase: delete it to open the current version).
  Since Herdr switches to the new workspace, such a note also shows there as
  a Herdr notification. A branch checked out in the main checkout or in the
  list's own folder is not opened a second time: the list says where it is.
- When the branch can't be fetched (no network, branch deleted, no login for
  the remote), the list says why and changes nothing. git never asks for a
  password or passphrase here; it uses the logins and ssh keys you already
  have.

## Mouse

Click a row to select it, double-click to open it in the browser. Click a tab,
the title (open / merged / closed), the project name (this project / all), a
column heading (sort), a part of the second header line or a key hint in the
footer. The wheel moves through the list, or scrolls the details panel when
the pointer is over it.

The help (`?`) shows the plugin's version in its bottom border; a click on
it opens the plugin's repository on GitHub.

## Keys

| Key | What it does |
|---|---|
| `↑` `↓` or `j` `k` | Move |
| `Tab` `Shift+Tab`, `←` `→`, `h` `l` or `1` `2` `3` | Switch between "Mine", "To review" and "All" |
| `Enter` or `o` | Open the merge request in the browser |
| `p` or `Space` | Details panel on or off; `Ctrl+d` `Ctrl+u` scroll it |
| `d` | Show the changes (diff); `n` `N` next or previous file, `f` file tree, `q` back |
| `c` | Details panel: overview or activity (comments) |
| `t` | Activity: all, open or resolved threads |
| `n` `N` | Activity: select the next or previous thread |
| `e` | Activity: resolve the selected thread, or open it again |
| `>` | Activity, open ones: reply to the selected thread |
| `+` | Activity, open ones: write a comment on the merge request |
| `Enter` `Ctrl+s` `Esc` | In the text of a reply or comment: line break, show it before posting (`y` posts), discard |
| `v` | Open ones: approve (or take your approval back), after confirming |
| `!` | Open ones of others: request changes with a reason (`Ctrl+s`, then `y` sends) |
| `m` | Open ones: merge, after confirming, when GitLab or GitHub allows it |
| `Ctrl+r` | Open ones with a failed pipeline: start the failed jobs again, after confirming |
| `w` | Your own open ones: mark as draft, or as ready for review again |
| `b` | Open ones: open the branch in a new Herdr workspace (a git worktree) |
| `?` | All keys |
| `/` | Search |
| `f` | Open ones: show all, needs me, open threads, pipeline failed, ready |
| `s` | Sort by the next key |
| `x` | Clear search and filter |
| `y` | Copy the link |
| `r` | Refresh now |
| `a` | Switch between this project and all projects |
| `z` | Switch between open, merged and closed ones |
| `i` | Switch between Nerd Font icons and plain symbols (saved) |
| `Esc` | Close the panel, clear the search or filter (never closes the list) |
| `q` | Close the list, after confirming with `y` |

## Install

Needs:

- Herdr 0.9.0 or newer and Rust (`cargo`).
- For GitLab: the GitLab CLI [`glab`](https://gitlab.com/gitlab-org/cli),
  logged in with `glab auth login`.
- For GitHub: the GitHub CLI [`gh`](https://cli.github.com), logged in with
  `gh auth login`.
- One of the two is enough. The plugin asks through these tools, so it never
  sees your token. A tool that is not installed or not logged in is left
  out quietly, `glab` as well as `gh`; when neither can be used, the list
  says what to install or log in to.
- **A [Nerd Font](https://www.nerdfonts.com) as the terminal's font** for the
  icons and the rounded labels. In macOS Terminal take the "S" variant (for
  example MesloLGS Nerd Font Mono): fonts with a large line gap make box
  lines look dashed. Without one the icons show as `?` or boxes:
  press `i` in the list (it says so the first time it opens) or set
  `icons = "plain"`, and the list uses ordinary symbols and square labels.

```sh
herdr plugin install nolvane/herdr-git-panel
```

Then bind the list to keys in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+m"
type = "plugin_action"
command = "git-panel.open"
description = "Git panel"

[[keys.command]]
key = "prefix+shift+m"
type = "plugin_action"
command = "git-panel.open-split"
description = "Git panel (split)"
```

and reload with `herdr server reload-config`. `prefix+m` opens the list where
`placement` says (over the tab by default), `prefix+shift+m` in a split below
the focused pane. Either key closes the list of the current tab again, as does
`q` (after confirming). Every tab can have its own list, in as many tabs and workspaces as you
like, each starting with the project of the pane it was opened from.

To work on the code, clone the repository, run `sh scripts/build.sh` and
`herdr plugin link "$PWD"`. Run `./herdr-git-panel` in any terminal to see
the list without Herdr.

## Settings

Optional. Copy `config.toml.example` to the directory
`herdr plugin config-dir git-panel` prints and name it `config.toml`.
Changes apply the next time the list opens.

| Setting | Default | Meaning |
|---|---|---|
| `scope` | `current` | `current`: only the GitLab project or GitHub repository of the directory the list opens in; `all`: everything ("All": your projects) |
| `refresh_secs` | `60` | Seconds between automatic refreshes |
| `placement` | `overlay` | `overlay` over the tab, `split` next to the focused pane, or `tab` |
| `split_direction` | `down` | Where a split goes: `down` (below) or `right` |
| `icons` | `nerd` | `plain` for terminals without a Nerd Font (or press `i`) |
| `compact` | `false` | `true`: one line per merge request instead of two, with the number in a column |
| `gitlab_host` | `gitlab.com` | GitLab host `glab` is logged in to, without a port |

For a GitLab whose web and API run on another port (such as
`https://git.acme.test:8443`), set `gitlab_host = "git.acme.test"` and give
`glab` the port as its API host:
`glab auth login --hostname git.acme.test --api-host git.acme.test:8443`
(add `--api-protocol http` for a GitLab without HTTPS; when `glab` is already
logged in to the host, `glab config set api_host git.acme.test:8443 --host
git.acme.test` is enough).
`glab` takes no port in the host name, and a clone with a port in its remote
is still recognised as its project.

A GitLab Community Edition server works too. It has no approval rules, so
the list shows how many people approved and who, but not how many are
needed or which rule waits.

## How it works

When the list opens, it reads the git remote of its directory from
`.git/config` to know the current project. It then asks:

- GitLab's GraphQL API through `glab api graphql` for the merge requests you
  are a reviewer of and the ones you wrote, one request per list;
- GitHub's GraphQL API through `gh api graphql` for the pull requests waiting
  for your review, the ones you already reviewed (GitHub drops the review
  request once you review, GitLab keeps you as reviewer) and your own, one
  search each, at the same time (in one request GitHub takes too long to work
  out mergeability and gives up);
- for "All", at the same time: in a project, one request to the forge it
  lives on for its open merge requests; with "all projects", two small
  requests per forge, first which of your projects have the newest open
  ones (only their update times), then those in full;
- for merged and closed ones, the same requests for that state, newest
  updated first (GitHub: `is:merged` or `is:closed is:unmerged` with
  `sort:updated-desc`; GitLab: `state: merged` or `closed` with
  `sort: UPDATED_DESC`), and with "this project" only that project's forge,
  for that project only.

Comments load when a merge request's activity tab is open. The list only
writes what you asked for: an approval or taking one back, a merge, a reply
to a thread, a comment or a change request (all after confirming), and
resolving a thread or opening it again. Starting a failed pipeline again
(after confirming) asks the forge for the current head first, then sends
GitLab's GraphQL `pipelineRetry`, or GitHub's REST `rerun-failed-jobs` for
each failed GitHub Actions run, through `gh api`. Replies, comments and change
requests go through `glab api` (GitLab's REST API) and `gh api` (GitHub's
GraphQL API for replies, REST for comments and reviews), with the text as
typed, sent to them on standard input; on GitLab a change request adds the
quick action line in front, and replies, comments and change requests
get a backslash before lines starting with `/`. `b` runs
`git fetch` against the clone's remote, creates or moves forward a local
branch that is not checked out anywhere, sets its upstream when it has
none, and asks Herdr to create the folder. Nothing else is stored and
nothing else is contacted. The list only runs while it is open.

## Development

```sh
sh scripts/check.sh
```

This runs a secret scan (needs `gitleaks`), `cargo fmt --check`,
`cargo clippy`, the tests and the release build. Design decisions are in
[docs/decisions.md](docs/decisions.md).
