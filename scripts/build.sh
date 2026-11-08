#!/bin/sh
# Builds the release binary and places it in the plugin root, where the
# manifest's startup and action commands expect it.
set -eu

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
  for dir in "$HOME/.cargo/bin" "$HOME/.homebrew/bin" /opt/homebrew/bin /usr/local/bin; do
    if [ -x "$dir/cargo" ]; then
      PATH="$dir:$PATH"
      export PATH
      break
    fi
  done
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found: install Rust (https://rustup.rs or 'brew install rust')" >&2
  exit 1
fi

cargo build --release --locked 2>&1 || cargo build --release
cp target/release/herdr-git-panel ./herdr-git-panel.new
mv -f ./herdr-git-panel.new ./herdr-git-panel
echo "built $(pwd)/herdr-git-panel"
