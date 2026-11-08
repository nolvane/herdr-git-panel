# Changelog

All notable changes to this plugin are documented in this file. The format is
based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- The "Review" and "CI" dots and their headings start at the left edge of
  their column, like every other column, instead of sitting centred.

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

[Unreleased]: https://github.com/nolvane/herdr-git-panel/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/nolvane/herdr-git-panel/releases/tag/v1.0.0
