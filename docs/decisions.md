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
