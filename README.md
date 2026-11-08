# Herdr Git Panel

A [Herdr](https://herdr.dev) plugin, written in Rust, that lists your GitLab
merge requests and GitHub pull requests in a pane: the ones you have to
review and your own open ones. It uses whichever of `glab` and `gh` is
installed and logged in; with both, the lists mix GitLab and GitHub, each row
with its forge's logo.

```
  Merge requests    Mine 8   To review 2                              ⌕ Search   all projects

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
waiting for approval). The columns:

| Column | Meaning |
|---|---|
| Author / Reviewers | Who wrote it (to review), or who reviews it (mine) |
| Review | Your own review as a dot (to review only): green approved, red changes requested, yellow commented or started, grey not yet |
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
resolves the selected thread or opens it again. Links in both tabs (the merge
request's link, Markdown links and plain URLs in descriptions and comments)
open in the browser with a click.

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

## Search and sort

- `/` searches as you type: every word must appear in the number, title,
  project, author, branch or a label. `Enter` keeps the search, `Esc` clears
  it.
- `f` steps through what is shown: all, needs me (to review: not approved or
  answered by you yet; mine: open threads, failed pipeline, conflict or
  rebase), open threads, pipeline failed, ready to merge.
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

`m` merges it after you confirm, only when GitLab or GitHub says it can be
merged (approved, pipeline passed, no conflict, threads resolved); otherwise
the list says what is missing. GitLab uses the merge request's own settings
(squash, delete the branch), GitHub the repository's default merge method.
Either way it only goes through if nobody pushed since the list loaded it.

Both run through `glab` or `gh` and refresh the list.

`w` marks your own merge request as a draft, or as ready for review again,
without asking (the same key undoes it).

## Mouse

Click a row to select it, double-click to open it in the browser. Click a tab,
the project name (this project / all), a column heading (sort), a part of the
second header line or a key hint in the footer. The wheel moves through the list, or
scrolls the details panel when the pointer is over it.

## Keys

| Key | What it does |
|---|---|
| `↑` `↓` or `j` `k` | Move |
| `Tab` or `1` `2` | Switch between "Mine" and "To review" |
| `Enter` or `o` | Open the merge request in the browser |
| `p` or `Space` | Details panel on or off; `Ctrl+d` `Ctrl+u` scroll it |
| `d` | Show the changes (diff); `n` `N` next or previous file, `f` file tree, `q` back |
| `c` | Details panel: overview or activity (comments) |
| `t` | Activity: all, open or resolved threads |
| `n` `N` | Activity: select the next or previous thread |
| `e` | Activity: resolve the selected thread, or open it again |
| `v` | Approve (or take your approval back), after confirming |
| `m` | Merge, after confirming, when GitLab or GitHub allows it |
| `w` | Your own: mark as draft, or as ready for review again |
| `?` | All keys |
| `/` | Search |
| `f` | Show: all, needs me, open threads, pipeline failed, ready |
| `s` | Sort by the next key |
| `x` | Clear search and filter |
| `y` | Copy the link |
| `r` | Refresh now |
| `a` | Switch between this project and all projects |
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
  sees your token. A tool that is not installed, or `gh` when it is not
  logged in, is left out quietly.
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
| `scope` | `current` | `current`: only the GitLab project or GitHub repository of the directory the list opens in; `all`: everything |
| `refresh_secs` | `60` | Seconds between automatic refreshes |
| `placement` | `overlay` | `overlay` over the tab, `split` next to the focused pane, or `tab` |
| `split_direction` | `down` | Where a split goes: `down` (below) or `right` |
| `icons` | `nerd` | `plain` for terminals without a Nerd Font (or press `i`) |
| `compact` | `false` | `true`: one line per merge request instead of two, with the number in a column |
| `gitlab_host` | `gitlab.com` | GitLab host `glab` is logged in to |

## How it works

When the list opens, it reads the git remote of its directory from
`.git/config` to know the current project. It then asks:

- GitLab's GraphQL API through `glab api graphql` for the merge requests you
  are a reviewer of and the ones you wrote, one request per list;
- GitHub's GraphQL API through `gh api graphql` for the pull requests waiting
  for your review, the ones you already reviewed (GitHub drops the review
  request once you review, GitLab keeps you as reviewer) and your own, one
  search each, at the same time (in one request GitHub takes too long to work
  out mergeability and gives up).

Comments load when a merge request's activity tab is open. The list only
writes what you asked for: an approval or taking one back, a merge (both
after confirming), and resolving a thread or opening it again. Nothing is
stored and nothing else is contacted. The list only runs while it is open.

## Development

```sh
sh scripts/check.sh
```

This runs a secret scan (needs `gitleaks`), `cargo fmt --check`,
`cargo clippy`, the tests and the release build. Design decisions are in
[docs/decisions.md](docs/decisions.md).
