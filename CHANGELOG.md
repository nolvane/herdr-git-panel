# Changelog

All notable changes to this plugin are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- On GitLab, a line starting with `/` that the list posted as text now shows
  as `/merge later` in the activity tab, as on GitLab's page, instead of
  `\/merge later`. Inside code the backslash still shows. Descriptions and
  comments on both forges follow the same rule (#35).

## [1.1.0] - 2026-11-09

### Added

- `Ctrl+r` starts a failed pipeline again, after `y`: on GitLab the failed
  jobs of the merge request's head pipeline, on GitHub the failed jobs of
  every failed GitHub Actions run of the newest commit. It starts nothing
  when someone pushed since the list loaded, and says why when there is
  nothing to start or the forge refuses. The details panel shows the key
  next to a failed pipeline (#12).

- The help (`?`) shows the plugin's version in its bottom border; a click
  on it opens the plugin's repository on GitHub, for example to report a
  bug (#14).

- `!` requests changes on someone else's open merge request or pull
  request, with a reason written in the same box as replies and comments
  (`Ctrl+s` shows it, only `y` sends it). On GitHub it is a review with
  "Request changes" on the commit shown; on GitLab a comment that submits
  your review as "requested changes". A line of the reason that starts
  with `/` never runs as a GitLab command (#11).

- A third section "All" lists every open merge request or pull request of
  the project, also other people's where you are not a reviewer; with "all
  projects" those of the projects you are a member of. `3` selects it, and
  `Tab` `→` `l` and `Shift+Tab` `←` `h` now step forward and back through
  the three sections (#6).

- `z` (or a click on the title) switches the list between open, merged and
  closed (without merging) merge requests and pull requests, in every
  section; the title says which. Merged and closed ones load the newest
  first, for this project only when the list shows this project, and can be
  looked at but not approved, merged, set to draft or opened as a branch
  (#7).

- `b` fetches the branch of the selected merge request or pull request and
  opens it in a new Herdr workspace (a git worktree Herdr creates), without
  touching the current checkout; forks open through the merge request's own
  ref (#9).

- In the activity tab, `>` replies to the selected thread and `+` writes a
  comment on the merge request or pull request, in a box that takes several
  lines (`Enter` line break, `Ctrl+s` shows what will be posted, `y` posts,
  `Esc` discards). The new reply or comment shows in the tab right away; a
  failed post keeps the text and says why. Pasted text goes into the box or
  the search in one piece (#10).

### Changed

- A refresh keeps the selected merge request or pull request selected when
  it moves in the list, instead of the row it was on (#10).
- The activity tab's key hints wrap instead of being cut off, and the key
  overview (`?`) uses two columns when it is taller than the screen (#10).

- "Mine" is now the first section, before "To review", and the list opens on
  it; `1` selects "Mine" and `2` "To review" (#5).

### Fixed

- On a GitLab Community Edition server, which has no approval rules, the
  list now shows the merge requests instead of
  only the error "Field 'approvalsLeft' doesn't exist on type
  'MergeRequest'". The Approvals column shows how many approved, and the
  details who did (#28).
- On a narrow pane, the footer leaves out whole key hints from the right
  end instead of cutting one in the middle of its word, and always keeps
  two blanks before "r refresh" (#18).
- The activity tab and the overview say "now" and "opened now" for
  something less than a minute old, instead of "now ago" (#23).
- A thread with more than 50 notes shows its first note and its newest
  replies, with a dim line "… 12 earlier replies not shown" in between,
  instead of only its oldest 50 notes, and sorts by its newest reply. A
  merge request or pull request with more than 100 threads loads the 100
  started last instead of the first 100 (#23).
- A `gitlab_host` with a port (such as `git.acme.test:8443`), which `glab`
  refuses, now makes the list say to set the host name without the port and
  to give `glab` the port with `--api-host`, instead of showing `glab`'s
  "invalid hostname". The README says how to set up a GitLab on another
  port (#25).
- On GitLab a line of a reply or comment from the activity tab that starts
  with `/` is posted as text instead of running as a GitLab command such as
  `/merge` or `/approve` (#31).
- With `glab` installed but not logged in, GitLab is now left out quietly,
  like GitHub without `gh`'s login, instead of showing "GitLab: Invalid
  token" in the footer on every refresh. When no forge can be used, the list
  names the tools that need `auth login`, for a self-hosted GitLab with
  `glab auth login --hostname <host>` (#19).
- An error that only mentions 401 or "unauthorized", such as one about
  merge request !401, now shows as itself instead of leaving GitHub out
  without a word or saying "run … auth login"; only a refused login counts
  as not logged in (#29).
- The "Review" and "CI" dots and their headings start at the left edge of
  their column, like every other column, instead of sitting centred (#4).
- "This project" also finds the project when the clone's remote spells it
  with other capitals than GitHub or GitLab, such as `GitHub.com:Acme/App`
  for `acme/app` (#15).
- A `glab` without a login is asked once when the list opens instead of
  every minute, so gitlab.com no longer answers "HTTP 429"; as soon as
  `glab auth login` ran, the open list loads GitLab within about a second,
  without `r` (#27).

## [1.0.0] - 2026-11-08

### Added

- A list of the GitLab merge requests and GitHub pull requests you have to
  review and your own, opened over the tab or in a split, one per tab.
- Columns for your review, CI, approvals, open threads, changes and age, and
  a second line per row that says where it stands (draft, merge conflict,
  pipeline failed, ready to merge, …).
- A details panel with the description, labels and the activity (comments
  and threads), and a viewer for the changes with a file tree.
- Search, quick filters, sorting, this project or all projects, and mouse
  support.
- Approve, merge, resolve threads and mark your own as draft or ready from
  the list; approving and merging ask first and only act on the version that
  was on screen (#1).
- Only web links in descriptions and comments can be clicked (#1).
- Optional settings in `config.toml`, and plain symbols for terminals without
  a Nerd Font.

[Unreleased]: https://github.com/nolvane/herdr-git-panel/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/nolvane/herdr-git-panel/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/nolvane/herdr-git-panel/releases/tag/v1.0.0
