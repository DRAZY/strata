#!/usr/bin/env bash
# Tidy the local checkout after a release has been uploaded to GitHub.
#
#   scripts/clean-local.sh            dist/: keep only the current version's files
#   scripts/clean-local.sh --builds   also delete build caches (target/), ~40 GB;
#                                     the next build starts from scratch
#
# Published files live on the GitHub release pages, so nothing here is the
# only copy. Never touches source files, settings or anything outside the repo.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
before=$(du -sk . | cut -f1)

if [ -d dist ]; then
  echo "==> dist/: keeping Strata-$VERSION-* and latest.json"
  find dist -mindepth 1 -maxdepth 1 \
    ! -name "Strata-$VERSION-*" ! -name latest.json ! -name Strata.app \
    -exec rm -rf {} +
  # The unpacked app is rebuilt by every package run.
  rm -rf dist/Strata.app dist/.DS_Store
fi

echo "==> worktrees left by finished agents"
git worktree prune
for w in $(git worktree list --porcelain | awk '/^worktree .*\/\.claude\/worktrees\//{print $2}'); do
  b=$(git -C "$w" branch --show-current 2>/dev/null || true)
  if [ -n "$b" ] && git merge-base --is-ancestor "$b" HEAD; then
    git worktree remove --force "$w" && git branch -D "$b" >/dev/null && echo "removed $w"
  else
    echo "kept $w (unmerged work)"
  fi
done

if [ "${1:-}" = "--builds" ]; then
  echo "==> build caches (target/)"
  cargo clean
fi

echo "==> git housekeeping"
git gc --quiet --prune=now

after=$(du -sk . | cut -f1)
echo "Freed $(( (before - after) / 1024 )) MB"
