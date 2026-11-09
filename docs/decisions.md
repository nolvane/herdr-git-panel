# Decisions

## 2026-11-08: Rust with ratatui

The plugin is written in Rust, with `ratatui` for the
terminal interface and `serde_json` for GitLab's answers. Every crate in the
dependency tree has a permissive licence (MIT, Apache-2.0, BSD, Zlib,
Unicode, Unlicense, WTFPL or MPL-2.0 as one choice of a dual licence), which
allows use and redistribution in source and binary form.

## 2026-11-08: GitLab through glab, not a token of our own

GitLab is asked through the official `glab` CLI (MIT licence) with
`glab api graphql`. glab holds the login, so the plugin never reads, stores or
asks for a token. The use stays within GitLab's terms: the official API, for
the user's own merge requests, at one request per minute. One GraphQL request
returns both lists with threads, approvals, pipeline and labels; the REST API
would need several requests per merge request.

## 2026-11-08: Approvals shown as approvers plus waiting rules

GitLab's `approvalsRequired` adds up the approval rules, so one person who
satisfies two rules is counted twice, and "0/3" would read as three people
missing. The list shows how many people approved, a check mark once the merge
request is approved, and names the rules still waiting in the detail bar.

## 2026-11-08: A plugin pane, opened by an action

The list is a Herdr plugin pane (`[[panes]]`, overlay by default), opened by
the `open` action so it can start in the directory of the focused pane and
work out the current project from there. The same action closes the list when
it is open; the open pane's id is kept per Herdr server in the state
directory. The list only runs while it is open, so there is no background
process.

Update 2026-11-08: one list per tab instead of one per server. The state file
keeps every list pane the plugin opened; the action closes only the list of
the current tab (or the focused list) and opens a new one otherwise, so lists
can stay open in several tabs and workspaces. Splits go below the focused pane
by default (`split_direction`).

## 2026-11-08: A dashboard look, own code

The list looks like a terminal dashboard: section tabs, a table with Nerd Font
state icons and a highlighted row. All code and assets are our own. Icons are
Nerd Font Octicons, looked up by name in Nerd Fonts' `glyphnames.json` (MIT);
`icons = "plain"` falls back to ordinary symbols.

## 2026-11-08: Nerd Font by default, with a way out in the list

The icons and the rounded label ends (Powerline half circles) need a Nerd Font,
and a terminal cannot tell a program which font it uses. So the list starts
with Nerd Font icons and, until `icons` is set, says for 15 seconds how to
switch: `i` toggles plain symbols and writes the choice to `config.toml`.
Someone without a Nerd Font sees the hint on the first open instead of having
to find the setting in the README.

## 2026-11-08: A second action for the split

`open-split` opens the list next to the focused pane, whatever `placement`
says, so one key can give the overlay and another the split. `open` takes an
optional placement argument for this.

## 2026-11-08: glab runs detached from the terminal, with a time limit

glab once hung inside the list pane while the same request took two seconds
from a shell: the list's terminal was glab's stdin. glab now gets no stdin,
`NO_PROMPT=1`, and is stopped after 30 seconds with a clear message, so the
list can never stay on "Loading" for good.

## 2026-11-08: Approving from the list, only after confirming

`v` approves (first `A`; changed so no letter is used both small and capital) the selected merge request (or revokes your approval) with
`glab mr approve` / `glab mr revoke`, the official API under the user's own
login. It is the only write the plugin makes, and it always asks for `y`
first, because an approval can let a merge request through. Own merge
requests are refused in the list.

## 2026-11-08: One GraphQL request per list

Adding the description and dates pushed the combined query over GitLab's
complexity limit of 250. The two lists are now fetched by two requests that
run at the same time, each well below the limit.

## 2026-11-08: Selected row colour as row style

ratatui's highlight style is painted over the cells and turned the middle of
a label pill into the selection colour. The selected row now gets its colour
as the row style, under the cells, so labels keep their colour.

## 2026-11-08: Filters, sorting and mouse in the list

Filtering by text, quick filters and sorting live in the list (not in
settings), because they change from minute to minute. They are not saved.
Mouse support uses the terminal's mouse reporting; the areas of the last frame
are recorded while drawing, so a click hits what is on screen.

## 2026-11-08: Changes in the terminal, not in an editor

`d` shows a merge request's diff with `glab mr diff --color always`, paged by
`less`, in the list's own terminal; the list steps aside and comes back when
`less` ends. An editor would need a local checkout of the branch for every
merge request, which the list does not have. `less` is on every Mac and Linux
machine.

## 2026-11-08: Markdown and wrapping done in the plugin

The details panel wraps text itself with a hanging indent, so long branch
names and links stay in the value column, and renders the description's
Markdown (headings, bullets, check boxes, quotes, code, bold, links) instead
of showing the raw text. ratatui's own wrapping cannot indent continuation
lines.

## 2026-11-08: Merging from the list

`m` merges with `glab mr merge --yes --auto-merge=false --sha <head>`, after
a `y`. It is offered only when GitLab's own merge status says the merge
request can be merged, so GitLab's rules (approvals, pipeline, threads)
decide, not the plugin. `--sha` makes the merge fail if someone pushed after
the list loaded, so only what was shown gets merged. No squash or branch
options are passed: the merge request's own settings apply.

## 2026-11-08: Comments loaded on demand

The activity tab loads a merge request's discussions only when it is shown,
one GraphQL request, and keeps them until the merge request's update time
changes. Loading every merge request's comments with the list would multiply
the requests per minute for something rarely looked at.

## 2026-11-08: Few keys in the footer, all in ?

The footer lists the seven most used keys; `?` opens an overlay with every
key and mouse action, grouped. The state line under the tabs shows what is
active and has no keys, so each key is explained in one place.

## 2026-11-08: Renamed to Herdr Git Panel

The plugin is now `herdr-git-panel`, plugin id `git-panel`, so the name
leaves room for more than the merge request list (GitHub pull requests,
changes). The actions keep their ids (`open`, `open-split`).

## 2026-11-08: Changes in a viewer of the plugin's own

The first changes view handed the terminal to `less`, which shows a file and
waits for keys most people do not know. The diff is now loaded with
`glab mr diff --raw` and shown inside the list: file headers, line numbers,
colours, `n`/`N` between files, mouse wheel, and `q` or `Esc` back to the
list.

## 2026-11-08: Two-line rows

Label pills in rows directly below each other touched and read as one block,
and a blank line between rows only looked empty. Now each merge request takes
two lines (project, number, author and branch dim above the title in bold),
the table has no box around it, the headings and the footer are bars with
their own background, and labels use a darkened tint of
their colour as background with a light tint as text. `compact = true` keeps
one line per merge request.

Update 2026-11-08: no line between merge requests after all (the dim first
line marks where each starts), the headings sit on a bar instead of having a
gap below, and the dim text colours are brighter so they stay readable.

## 2026-11-08: Fewer columns, status in the second line

The list looked crowded: many columns full of zeros and dashes, long branch
names, a summary bar repeating the selected row. Now there are three columns
besides the title (people, checks, age); checks only show what needs
attention; branch and user name moved to the details panel. Each merge
request's second line says where it stands (merge conflict, waiting for
approval, …), so the summary bar under the list is gone without losing that
information. One header line instead of two; active filters show as chips.

## 2026-11-08: Resolving threads from the activity tab

`e` (or the thread's button) resolves a thread or opens it again with the
GraphQL mutation `discussionToggleResolve`, under the user's own login. No
confirmation: it is easy to undo and changes no code. The change shows at
once and is put back if GitLab refuses it.

## 2026-11-08: File tree in the changes viewer

The changed files show as a tree on the left of the diff, folders once with
their files below, each with lines added and removed; the current file is
highlighted and a click jumps to it. Hidden on narrow panes and with `f`.

## 2026-11-08: Links in the panel open with a click

The list captures the mouse, so the terminal's own link clicking does not
reach the text. The panel therefore keeps track of where each link ends up
after wrapping (Markdown links, which show only their text, and plain URLs)
and opens it in the browser on a click.

## 2026-11-08: A column per fact, labels only in the panel

The "Checks" column packed review, pipeline, approvals and threads into
unlabelled icons. Each now has its own column with a plain heading (Your
review, Pipeline, Approvals, Threads, Changes, Opened, Updated) and a word
next to the icon. Labels left the list for the details panel: they made rows
noisy and are rarely what decides what to look at next. Titles are always
white so they stay readable on the selected row.

## 2026-11-08: GitHub through `gh`

GitHub pull requests come in through the official GitHub CLI `gh` (MIT
licence), the same way GitLab comes in through `glab`: the plugin runs the
logged-in tool and never sees a token. It uses only GitHub's public GraphQL
API and the `gh pr review/merge/diff` commands under the user's own account,
for the user's own pull requests and reviews, at one refresh a minute, which
is ordinary use under GitHub's Terms of Service (API terms: no excessive or
abusive use, which three small searches a minute are not). No data is stored
or passed on.

One model (`model.rs`) serves both forges; `forge.rs` sends each request to
the forge the merge request lives on. GitHub mappings:

- "To review" is `review-requested:@me` plus `reviewed-by:@me` without your
  own, because GitHub drops the review request once you review and GitLab
  keeps you as reviewer.
- Merge state from `mergeStateStatus` (CLEAN, HAS_HOOKS and UNSTABLE count as
  ready, BEHIND as needs rebase, DIRTY as conflict).
- Pipeline from the newest commit's `statusCheckRollup`.
- Approvals from the latest review per reviewer; GitHub has no way to take an
  approval back, so `v` on an approved pull request says so instead.
- Merge uses the repository's default merge method and
  `--match-head-commit`, like `--sha` on GitLab.
- The three searches run as separate requests at the same time: in one
  request GitHub works out mergeability for up to 90 pull requests and
  answers 502 after about ten seconds.

`gh` not installed or not logged in leaves GitHub out quietly: many people
have `gh` without using GitHub for work. Both forges failing shows the error;
one failing shows it in the footer while the other's list stays.

The header says "Pull requests" when everything in view is on GitHub, else
"Merge requests"; numbers are written `#n` and `!n` like the forges do.

## 2026-11-08: Esc never closes the list

`Esc` is the key people press to step back, and closing the list by accident
loses the pane. It now only closes the panel, the search or a filter. `q` and
`Ctrl+c` ask "Close the git panel?" first and close only on `y`.

## 2026-11-08: Dots for review and CI, approvals as given of needed

The title gets the room: "Review" and "CI" show a coloured dot only (the
details panel says it in words). Approvals show given of needed (`0/2`)
instead of a dash. GitLab's own `approvalsRequired` counts a code owner rule
once per file pattern section (3 where its UI shows two rules), so the list
adds up the approval rules instead. GitHub only shows the needed count to
repository admins (branch protection); otherwise the list shows the number
given.

## 2026-11-08: Draft on and off with `w`

`w` sets your own merge request to draft or back to ready (`glab mr update
--draft/--ready`, `gh pr ready [--undo]`). Only your own: changing someone
else's state from a list is too easy to do by accident. No confirmation,
because the same key undoes it.

## 2026-11-08: Only web links open, approvals pinned to the shown head

Descriptions and comments are written by other people, and the panel shows a
Markdown link's text, not its target. A link could therefore point a click at
a local file, an app, a network share or another URL scheme, which `open` and
`xdg-open` happily follow. Only `http` and `https` targets without spaces or
control characters are clickable now; other links show as plain text, and
`open_url` refuses anything else as a second guard. Relative links (such as
`/group/project/-/jobs/1` on GitLab) no longer open; they are rare in
descriptions and would need the forge's host to be resolved safely.

Approving now names the head commit the list showed, like merging already
did: `glab mr approve --sha`, and on GitHub the REST call
`POST /repos/{owner}/{repo}/pulls/{n}/reviews` with `event=APPROVE` and
`commit_id`, because `gh pr review` cannot name a commit. A push after the
list loaded then makes GitLab refuse the approval, and on GitHub the review
belongs to the commit that was on screen.

## 2026-11-08: Review and CI dots start at the left edge

The "Review" and "CI" dots and their headings were centred, which put the
review dot two cells into its six-cell column, out of line with every other
column. They now start at the left edge like the other values and headings.
Rendering tests check that each dot sits under the first letter of its
heading, so the centring does not come back.

## 2026-11-08: "Mine" is the first section

The list shows "Mine" first and opens on it, also when it is empty and "To
review" has entries; `1` selects "Mine" and `2` "To review". Without a second
reviewer nobody asks for reviews, so opening on "To review" meant switching
on every open. The order lives in one list, `Section::ORDER` in `src/app.rs`,
which the header tabs, the starting section and the number keys read.

## 2026-11-08: `b` opens the branch through Herdr's worktrees

`b` fetches the selected merge request's branch and opens it with
`herdr worktree create --branch` (or `herdr worktree open --path` when the
branch has its own folder already), so Herdr picks the folder, groups it with
the repository in the sidebar and removes it with its own action; the plugin
keeps no state of its own (only the local branch and its upstream in the
clone) and needs no setting. No agent starts: Herdr starts one with
a key in any pane. It works only in the clone the list opened in, on rows of
any project its remotes point at. No `y` first: it writes nothing to the
forge and never touches the current checkout.

- A branch of the same repository is fetched into its remote-tracking ref and
  gets a local branch of the same name whose upstream is set through
  `branch.<name>.remote/merge` (only when it has none and no commits of its
  own), which works whatever the remote's fetch refspec. A fork's branch is
  fetched through the forge's own ref (`refs/pull/<n>/head`,
  `refs/merge-requests/<iid>/head`) into `pr-<n>` / `mr-<iid>`, so a fork's
  `main` is never mixed up with the repository's own. The fetched commit is
  read from the ref the fetch wrote (for a fork a ref of the plugin's own,
  `refs/herdr-git-panel/…`, removed again at the end), not from the shared
  `FETCH_HEAD`, which another fetch could overwrite in between.
- Remotes match the merge request's project ignoring case, as GitHub and
  GitLab do.
  Whether it is a fork comes from GitHub's `isCrossRepository` and GitLab's
  `sourceProjectId`/`targetProjectId`; unknown counts as a fork, which is
  fetched correctly either way.
- An existing local branch only moves forward on a fast-forward; one with
  commits of its own is kept and the list says so. A branch checked out in
  the main checkout or in the list's own folder is refused before anything is
  fetched. If Herdr refuses, a branch created in the same run is deleted
  again and a moved one put back, but only while no folder has it checked
  out: Herdr may have made the folder before failing.
- `--focus` takes the user to the new workspace, so the list is out of sight
  when the answer comes. A note (the branch differs, is behind, could not be
  set to track) is therefore also shown with `herdr notification show`.
- `herdr worktree create` gets two minutes, since it checks out the whole
  branch; the fetch keeps the 30 s of every other call. On a timeout the
  tool is stopped with its process group, so the ssh that git started goes
  too.
- git runs without a terminal: `GIT_TERMINAL_PROMPT=0`,
  `GCM_INTERACTIVE=never`, and ssh's askpass forced to `false`
  (`SSH_ASKPASS_REQUIRE=force`), so a passphrase or new host key fails at once
  instead of writing over the list. That keeps the user's own ssh command and
  agent, unlike overriding `GIT_SSH_COMMAND`. ssh older than 8.4 ignores
  `SSH_ASKPASS_REQUIRE`; there a prompt stops ssh in its own process group
  until the 30 s limit, the same limit `glab` and `gh` have.
- `herdr worktree create/open` with `--cwd`, `--branch`, `--path` and
  `--focus` are documented the same way in Herdr 0.9.0, so
  `min_herdr_version` stays.
- Branch names from the forge go to `git` and `herdr` as arguments without a
  shell; names git would not accept, above all ones starting with `-`, are
  refused first.

## 2026-11-08: "All" loads per scope, in at most two small steps

The third section "All" lists every open merge request of the current
project, or with "all projects" (and outside a project) those of the
projects you are a member of (the owner's decision on #6). It is loaded with
the other lists, for the scope shown only; `a` loads it again for the other
scope instead of loading both every minute. The other two sections switch at
once as before.

- This project: one request to the forge the project lives on,
  `project(fullPath) { mergeRequests }` on GitLab (complexity about 140 of
  250) and `repository { pullRequests }` on GitHub, newest first, with the
  forge's own count for "newest 30 of 54" in the footer.
- Your projects, GitLab: `projects(membership: true)`, the 100 most recently
  active, with only the update times of their open merge requests
  (complexity about 30), then the newest 100 in full through
  `projects(fullPaths) { mergeRequests(updatedAfter) }`, one second before
  the oldest one picked so ties at the cut stay (projects with a tie are
  asked too). Each project in that second
  request adds about 1.4 to the complexity, so at most 50 go in one request
  (about 205). GitLab's top-level `mergeRequests` query is an experiment
  since 19.3 and needs a filter, so it is not used.
- Your repositories, GitHub: `viewer.repositories` with the affiliations
  owner, collaborator and organization member, the 100 most recently pushed,
  with ids and update times of their newest open pull requests, then the
  newest 30 in full through `nodes(ids)`. 30 like every other GitHub list:
  more in one request makes GitHub answer 502. A search with `repo:`
  qualifiers is limited to 256 characters, a handful of repositories.
- The lists of ids and paths go into the second query as JSON strings, which
  are valid GraphQL strings, because `glab -f` and `gh -f` only send strings
  as variables; single values stay variables.
- Your own and the ones you review in the covered projects are added from
  the other two lists, so they are in "All" also when they are older than
  the cut. "newest N of M" counts only the forges' newest, not these.
- GitHub answers with errors next to the data the login may see (an
  organization behind single sign-on, a pull request gone between the two
  steps); those are left out instead of failing "All".
- A project whose forge could not be asked at all (its tool missing or not
  logged in) shows that, not "No open merge requests".
- With both forges each is cut at its own limit, so "newest 130 of 194" means
  the newest of each. Projects beyond the 100 most active are neither shown
  nor counted.
- "All" failing leaves the other lists: the footer says "All: …" and the
  section shows the error.
- One or two more small read requests per forge a minute through the
  official APIs under the user's own login is ordinary use under GitLab's
  and GitHub's terms, as for the other lists.

## 2026-11-08: Open, merged and closed with `z`, loaded per state

`z` steps the list through open, merged and closed (without merging) merge
requests, as three states (the owner's decision on #7); a click on the
title does the same. The issue left the key open: `C` would use a letter
both small and capital (see "Approving from the list"), `>`, `+`, `!` and
`Ctrl+r` are kept for replies, requested changes and new pipelines (#10 to
#12), and `u` already means "scroll up" with Ctrl. The state is not saved:
the list opens on open ones, like its other filters.

- Each merge request carries its state from the forge (`state` in both
  fragments; GitLab's `locked`, while it merges, counts as open). Rows,
  icons and the details panel say "Merged" or "Closed" instead of what
  blocks a merge, which the forges still report for them.
- Open ones load as before. Merged and closed ones are asked for only when
  shown, through the same requests with the state (GitHub `is:merged` /
  `is:closed is:unmerged` and `states: MERGED` / `CLOSED`; GitLab
  `state: merged` / `closed`), newest updated first, within the limits of
  open ones (30 on GitHub, 100 on GitLab). Without a sort GitHub's search
  would pick arbitrary old ones among hundreds.
- With "this project", merged and closed ones are asked from that project's
  forge for that project only (GitHub `repo:owner/name`, GitLab
  `projectPath`), so the newest are its own and not whatever of it happens to
  be among the newest of all projects. `a` loads them again; `Data.of`
  records the project they were asked for. When that forge's tool is missing,
  the list says so instead of showing an empty list.
- GitHub's search string now goes in as a GraphQL variable, and a repository
  name only goes into it when it is a plain `owner/name`, so a remote's path
  cannot add search terms.
- The lists of each state are kept while the list is open, with each
  section's selected row, so going back is instant and shows them as left;
  a kept state that is older than the refresh interval, or kept from
  before an approve, merge or draft change, loads again when shown. An answer
  for a state no longer on screen is kept for that state, and the shown one
  loads at once when it is missing or old.
- Approve, merge, draft and opening the branch are refused on merged and
  closed ones, and their hints left out: they act on an open merge request
  (a merged branch is often deleted). Resolving a thread stays as it is:
  the forge decides whether it is allowed there. The quick filters (`f`)
  are about what still needs doing, so they are off for merged and closed
  ones.
- `state` adds about 3 to the complexity of GitLab's "All" request over 50
  projects, measured at 209 of the 250 allowed.

## 2026-11-09: Replies and comments from the activity tab

`>` replies to the selected thread and `+` writes a comment on the merge
request (the owner's decision on #10); both keys only act in the activity
tab, like `t`, `n` and `e`. The text field takes several lines: `Enter` adds a
line break, `Ctrl+s` finishes, `Esc` throws the text away (also the owner's
decision). Finishing shows the text and where it goes; only `y` (or a click
on its button) posts, never `Enter`, so a second `Enter` can't post unread
text. `n` goes back to the text instead of throwing it away, since a reply
can be long.

- The field lives in `src/compose.rs` with no forge or drawing code, and its
  target is an enum (`Reply`, `Comment`), so requesting changes with a text
  (#11) adds a target and reuses the field and the preview.
- The field keeps a copy of the merge request and the thread's forge id,
  not their places in the lists, so a refresh while writing can't send the
  text elsewhere. While it is open every key goes into it; clicks only work
  on its own buttons and the wheel only scrolls the panel.
- GitLab gets REST through `glab api` (`…/discussions/<id>/notes` for a
  reply, `…/notes` for a comment), which needs no new field in the list's
  query; the thread id must be a GitLab discussion id before it goes into
  the path. GitHub gets `addPullRequestReviewThreadReply` (GraphQL, with the
  thread id already loaded) and the conversation's REST comments endpoint.
  A GitHub reply that lands in a review of yours that is not submitted yet
  says so instead of "Reply posted".
- The text always goes as a raw field (`-f body=…`): `-F` reads `@…` as a
  file. The GitHub GraphQL helper sends values starting with `$int:` as
  `-F`, so text never goes through it; the reply has its own argument
  builder.
- Bracketed paste is on: pasted text arrives in one piece, so a line break
  or a `y` in it can't finish or confirm anything. The search field takes a
  paste on one line.
- Replies and comments are refused on merged and closed ones, like approve,
  merge, draft and the branch, so the merged and closed lists stay for
  looking back. Both forges would accept them; this is easy to turn around.
- After a post the merge request's comments load again (answers of loads
  started before it are dropped), the list refreshes, and the same merge
  request and thread stay selected by key and id: they move to the top,
  since both lists are sorted newest first. The list now keeps each
  section's merge request selected by key on every refresh, not by its row.
- Posting runs through the official APIs with the user's own login (the
  logged-in `glab` and `gh`), one text the user wrote and confirmed at a
  time. GitHub's Acceptable Use Policies forbid spam and "excessive automated
  bulk activity", its API terms abuse of the API; GitLab's Acceptable Use
  Policy forbids sending spam. None of that covers this use (read on
  2026-11-09: docs.github.com/en/site-policy/github-terms/github-terms-of-service,
  docs.github.com/en/site-policy/acceptable-use-policies/github-acceptable-use-policies,
  handbook.gitlab.com/handbook/legal/acceptable-use-policy/).
