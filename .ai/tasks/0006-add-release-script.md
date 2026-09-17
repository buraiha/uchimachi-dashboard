# リリーススクリプト追加

- Status: completed
- Depends on: none

## 目的と要求

ユーザーの依頼に基づき、main ブランチ専用の `release.sh` を作成する。
`git pull origin`、`podman compose down`、`podman compose up -d --build` を順に実行する。

## 変更対象

- `release.sh`（新規、実行権限付き）
- `README.md`（更新手順）
- `.ai/tasks/0006-add-release-script.md`（本タスクの記録）

## 対象外

実際の pull・リリース・コンテナ操作、アプリケーションや Compose 設定の変更。

## 守るべき仕様と制約

- スクリプト配置ディレクトリで実行する。
- main 以外および detached HEAD では更新・コンテナ操作を実行しない。
- コマンドが失敗した場合は後続処理を行わず異常終了する。

## 受け入れ条件

main でのみ要求の3コマンドが順に実行され、各失敗時に停止すること。

## 想定する検証

`bash -n release.sh`、一時環境のコマンドスタブによる正常系・ブランチ制限・失敗時停止・作業ディレクトリの検証、差分と実行権限の確認。

## 検証結果

- `bash -n release.sh`: 成功。
- コマンドスタブで正常系、main 以外、detached HEAD、ブランチ取得失敗、pull 失敗、down 失敗、up 失敗の7ケースを確認し、すべて成功。
- 別ディレクトリからの実行でもスクリプト配置ディレクトリで処理されること、および実行権限を確認。
- `git diff --check` と最終変更内容の確認: 成功。
- 実際の Git 更新と Podman による停止・ビルド・起動は、スクリプト作成のみの依頼のため未実行・未確認。
