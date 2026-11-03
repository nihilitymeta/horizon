#!/usr/bin/env bash
# Runs step.sh until the rebase is done or a conflict needs a person (step.sh refuses files with markers).
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
[ -d "$(git rev-parse --git-path rebase-merge)" ] || { echo "no rebase in progress"; exit 1; }
here=$(dirname "$0")
while [ -d "$(git rev-parse --git-path rebase-merge)" ]; do
  "$here/step.sh"; rc=$?
  [ $rc = 2 ] && { git status | sed -n 2,4p; exit 2; }
  [ $rc != 0 ] && [ -d "$(git rev-parse --git-path rebase-merge)" ] && exit 1
done
echo "DONE $(git rev-parse --abbrev-ref HEAD) $(git rev-parse --short HEAD)"
