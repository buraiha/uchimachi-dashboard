# Project Contract

このファイルには、AIの調査、変更、検証へ直接影響するプロジェクト固有の契約だけを記載する。

AIの共通行動規範は `AGENTS.md` と `.ai/common.md`、詳しい仕様と運用手順は `README.md` を参照すること。

## 1. Project Overview

- 概要: Google Calendar APIから複数カレンダーの予定を取得し、伝言、予定メモ、スケジュールチャートとともに表示する単一バイナリのWebダッシュボード
- 主な技術: Rust 2024、Axum、Tokio、SQLite（rusqlite）、Google Calendar API、OAuth 2.0
- 仕様・設計の参照先: `README.md`

## 2. Repository Boundaries

- `src/main.rs`: ルーティング、HTML生成、Google Calendar API呼び出し、SQLite永続化、認証・セッション管理、設定読み込みを含むアプリケーション本体
- `Dockerfile`: リリースバイナリのビルドと実行イメージ
- `docker-compose.yml`: ローカルのコンテナ起動、環境変数、`./data` のマウント
- `.env.example`: 環境変数の公開用サンプル。ローカルの `.env` は管理対象外
- `data/.gitkeep` 以外の `data/` 配下: ローカル実行データであり変更対象外

`src/main.rs` は複数の責務を含むため、リファクタリングは責務単位で小さく進める。ビューを分離する場合は、HTML生成と表示用ヘルパーだけを対象とし、明示的な要求なしにルーティング、認証、Google Calendar API処理、SQLite処理、設定読み込み、アプリケーション全体のモジュール構成を同時に再編成しない。

手書きのソース以外に、直接編集を避けるべき生成ソースは現在確認されていない。`Cargo.lock` は依存関係を変更した場合だけCargoの標準手順で更新する。

## 3. Development Environment and Commands

- 前提環境: Rust 1.89相当のツールチェーン、Cargo。コンテナ検証にはPodman ComposeまたはDocker Composeを使用する
- ローカル実行: READMEに記載された環境変数を設定して `cargo run`
- フォーマット: `cargo fmt --check`
- コンパイル確認: `cargo check`
- テスト: `cargo test`
- ビルド: `cargo build`。コンテナ変更時は `podman compose build` または `docker compose build`
- 差分の空白エラー確認: `git diff --check`

環境変数を追加、変更、削除する場合は、`src/main.rs`、`.env.example`、`docker-compose.yml`、`README.md` の変数名、説明、許可値、デフォルト値を必要な範囲で同期する。

## 4. Invariants and Compatibility

### データとSQLite

- SQLiteの変更は既存データを保持できる後方互換な方法を優先する。
- 明示的な依頼と事前確認なしに、テーブル・列の削除、SQLite DBの再作成、実データの更新・削除、後方互換性のないスキーマ変更を行わない。
- スキーマ変更時は、既存データへの影響と移行方法を確認する。

### 認証、外部連携、入力

- Google CalendarのOAuth scopeは `calendar.readonly` を維持し、明示的な要求なしに権限を追加・拡大しない。
- 認証、Cookie保護、セッション管理、リダイレクト先、OAuth stateなどの既存検証を弱めない。
- Google Calendar API、SQLite、フォーム、URLパラメーター、環境変数、エラーなど外部由来の値をHTMLへ埋め込む場合は、`escape_html` または同等の処理を通す。
- URLとリダイレクト先には既存の検証・正規化処理を使用する。

### 実行時の仕様

- 日時表示とGoogle Calendarの取得範囲は `Asia/Tokyo` を前提とする。日時処理の変更時は日付境界、終日予定、タイムゾーン変換を確認する。
- `GOOGLE_MAX_RESULTS` の許可値は `10`、`20`、`30`、`40`。カレンダーごとのAPI取得上限と、複数カレンダー統合・時刻順整列後の最終表示上限の両方へ適用する。
- ダッシュボードの自動リロードは意図して削除されている。明示的な要求なしに、更新タイマー、カウントダウン、JavaScriptによる自動更新、`window.location.reload()`、関連設定やテンプレート引数を再導入しない。

## 5. Validation Contract

| 変更種別 | 必須の検証 | 追加の確認 |
| --- | --- | --- |
| Rustコード | `cargo fmt --check`、`cargo check`、`cargo test`、`git diff --check` | 変更した経路の動作確認。既存の未整形箇所がある場合は無関係な一括整形をしない |
| HTML・フォーム | Rustコードの検証 | 主要要素、リンク、フォーム、外部値のエスケープ、URL検証、認証分岐、エラー表示。必要に応じてブラウザ幅も確認 |
| 環境変数・設定 | `cargo check`、`cargo test`、`git diff --check` | `src/main.rs`、`.env.example`、`docker-compose.yml`、`README.md` の整合確認 |
| `Dockerfile`・Compose | Compose設定の解決、イメージビルド、コンテナ起動、起動ログ、`GET /health` | 検証用ローカル環境だけを使用し、終了後の状態を確認 |
| SQLite・保存形式 | `cargo test` と関連するコード検証 | 既存データ互換性、移行方法、復旧可能性を確認 |
| AI文書・その他の文書のみ | `git diff --check` と最終差分確認 | コード、設定、READMEとの整合確認 |

Google OAuth、認証情報、実運用環境が必要で確認できない経路は、確認済みとして扱わず未確認事項として報告する。実際の認証情報を外部環境へ送信して検証しない。

## 6. Protected Data and Files

- `.env`、OAuthトークン、SQLite DB、セッション情報、Cookie、実運用データ、その他のローカル設定や認証情報を変更・出力・コミットしない。
- `data/.gitkeep` 以外の `data/` 配下を変更対象にしない。
- テストには既存の単体テスト内データ、明示的なfixture、匿名化されたサンプルだけを使用する。

## 7. External and Production Boundaries

- 明示的な依頼なしに、本番環境、共有環境、実運用中のサービスやコンテナを起動、停止、再起動、更新しない。
- Google Calendar APIなど外部サービスへの書き込み、通知、権限変更を行わない。
- デプロイ、リリース、コミット、プッシュはユーザーが明示的に依頼した場合だけ行う。
