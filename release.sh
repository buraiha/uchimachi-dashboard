#!/usr/bin/env bash
set -euo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

branch=$(git branch --show-current)
if [[ "$branch" != "main" ]]; then
    printf 'エラー: main ブランチでのみ実行できます（現在: %s）。\n' "${branch:-detached HEAD}" >&2
    exit 1
fi

git pull origin
podman compose down
podman compose up -d --build
