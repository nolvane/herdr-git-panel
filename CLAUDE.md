# Herdr Git Panel

A [Herdr](https://herdr.dev) plugin that lists the GitLab merge requests and
GitHub pull requests to review, your own and all of a project. What it does
and how users install it: [README.md](README.md). Why it is built this way:
[docs/decisions.md](docs/decisions.md).

## Stack

- Rust (edition 2021), one binary built by `scripts/build.sh` into the plugin
  root, where `herdr-plugin.toml` starts it.
- `ratatui` for the interface, `serde_json` for the forges' answers,
  `unicode-width` for column widths.
- GitLab through `glab`, GitHub through `gh`: the plugin runs the logged-in
  tools and never reads or stores a token.
- Needs Herdr 0.9.0 or newer (`min_herdr_version` in `herdr-plugin.toml`).

## Conventions

- Code, comments and docs are in English.
- Notable technical decisions go into `docs/decisions.md` (decision, why,
  date).
- The repository is public: no personal data, no secrets, no private setup.
  Examples and tests use made-up projects and people (`acme/app`, `@alex`).
- Every write to a forge (approve, merge, reply, comment) asks for `y` first,
  except ones the same key undoes (resolve a thread, draft on or off).

## Checks

```sh
./scripts/check.sh            # default stage: merge
./scripts/check.sh --stage quick
```

Every stage (`quick`, `merge`, `release`) runs the same steps: gitleaks secret
scan, `cargo fmt --check`, `cargo clippy` with warnings as errors, the unit
tests and the release build. There are no integration tests, since they would
need a running Herdr server and logged-in forge accounts.

The checks run locally, not in GitHub Actions: they take about a minute, need
no secrets, and the merge stage adds nothing to the quick stage, so a
workflow would add a second place to maintain without catching more. Revisit
this when outside contributors send pull requests.

## Non-goals

- No token handling of its own; logins stay with `glab` and `gh`.
- No background process; the list only runs while it is open.
- No other forges than GitLab and GitHub for now.

## Releasing

**Version.** It lives in two places that must always match: `version` in
`herdr-plugin.toml` and `[package] version` in `Cargo.toml`. After changing
it, run `cargo build --release` once (without `--locked`) so `Cargo.lock`
picks up the new version, then `./scripts/check.sh --stage release`, which
runs with `--locked`.

**What ships.** There is no store and no separate test channel. Users install
from GitHub, and `herdr plugin update` fetches the newest commit of the ref
recorded at install (the default branch, unless they installed with `--ref`)
and builds it. A merge to `main` is therefore what reaches users, at their
next update. An open list keeps the old binary until it is closed and opened
again.

**Try a release before it reaches users.**

1. A local checkout:

   ```sh
   sh scripts/build.sh
   herdr plugin link "$PWD"
   ```

   `herdr plugin list` shows `local:<path>`.
2. A pushed branch:

   ```sh
   herdr plugin install nolvane/herdr-git-panel --ref <branch> --yes
   ```

Back to the normal install (after a link, run `herdr plugin unlink git-panel`
first, since Herdr refuses to install over a linked plugin):

```sh
herdr plugin install nolvane/herdr-git-panel --yes
```

**Changelog and GitHub release.** `CHANGELOG.md` follows the
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) format: an
`[Unreleased]` section, then one section per version (`[X.Y.Z] - YYYY-MM-DD`,
newest first) with Added, Changed, Fixed and Removed. The first release
creates the file. A release ends with an annotated tag `vX.Y.Z` on the
released commit and a GitHub release of that tag whose notes are the
version's section of `CHANGELOG.md`.
