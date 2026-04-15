# うちまちダッシュボード

Google Calendar API から特定のカレンダーの今後のイベントを取得する Rust 製の ダッシュボード です。Docker Compose で HTTP サービスとして起動し、OAuth 2.0 のユーザー認可で Google にログインして利用します。

## できること

- `GET /health` でヘルスチェック
- `GET /auth/login` で Google ログイン開始
- `GET /auth/callback` で OAuth コールバック受信
- `GET /auth/status` で認可状態確認
- `GET /user/login` と `POST /user/login` で利用者ログイン
- `GET /user/logout` と `POST /user/logout` で利用者ログアウト
- `GET /` と `GET /dashboard` で表示用ダッシュボード
- `GET /messages` で有効な伝言一覧を取得
- `POST /messages` で1-24時間有効な伝言を登録
- `GET /calendar` で対象カレンダーの今後のイベントを取得
- refresh token をローカルファイルに保存して再利用

## 前提

- Docker と Docker Compose が使えること
- Google Cloud で Calendar API を有効化していること
- Google Cloud で OAuth 同意画面を設定できること
- OAuth 2.0 Client ID を発行できること
- 認可に使う Google アカウントが対象カレンダーを閲覧できること

## Google 側の準備

1. Google Cloud でプロジェクトを作成
2. `Google Calendar API` を有効化
3. `API とサービス` -> `OAuth 同意画面` でアプリ情報を設定
4. テスト公開のまま使うなら、認可に使う Google アカウントを `テストユーザー` に追加
5. `認証情報` -> `認証情報を作成` -> `OAuth クライアント ID`
6. アプリケーションの種類は `ウェブアプリケーション`
7. 承認済みのリダイレクト URI に `http://localhost:8080/auth/callback` を追加
8. 発行された Client ID と Client Secret を控える

認可に使う Google アカウント自体が対象カレンダーを見られないと、認証が成功してもカレンダー取得で 404 または権限エラーになります。

## 起動手順

1. 設定ファイルを作成

```bash
cp .env.example .env
mkdir -p data
```

1. `.env` を編集

```dotenv
GOOGLE_CALENDAR_ID=your-calendar-id@group.calendar.google.com
DASHBOARD_TITLE=うちまちダッシュボード
GOOGLE_OAUTH_CLIENT_ID=your-google-oauth-client-id.apps.googleusercontent.com
GOOGLE_OAUTH_CLIENT_SECRET=your-google-oauth-client-secret
GOOGLE_OAUTH_REDIRECT_URL=http://localhost:8080/auth/callback
GOOGLE_TOKEN_STORE_PATH=/data/google-oauth-token.json
MESSAGE_DB_PATH=/data/dashboard.sqlite3
DASHBOARD_AUTH_USERNAME=dashboard-user
DASHBOARD_AUTH_PASSWORD=change-this-password
DASHBOARD_AUTH_COOKIE_SECURE=false
GOOGLE_MAX_RESULTS=10
PORT=8080
```

`DASHBOARD_AUTH_USERNAME` と `DASHBOARD_AUTH_PASSWORD` を両方設定すると、ダッシュボード利用者向けのログインが有効になります。両方未設定なら従来どおり公開動作です。HTTPS 配下で運用する場合は `DASHBOARD_AUTH_COOKIE_SECURE=true` を指定してください。

1. サービスを起動

```bash
docker compose up -d --build
```

`docker-compose.yml` では `restart: unless-stopped` を指定しているため、Docker デーモン再起動やサーバー再起動後も自動復帰します。`docker compose stop` で明示的に停止した場合は、その停止状態が維持されます。

1. ブラウザで認可を実行

利用者認証を有効にした場合は、先に `http://localhost:8080/user/login` でログインします。その後 `http://localhost:8080/auth/login` にアクセスして Google ログインと同意を完了します。

1. 別ターミナルから確認

```bash
open http://localhost:8080/
curl http://localhost:8080/health
curl -c cookies.txt -d 'username=dashboard-user&password=change-this-password' -X POST http://localhost:8080/user/login -i
curl http://localhost:8080/auth/status | jq
curl http://localhost:8080/messages | jq
curl http://localhost:8080/calendar | jq
curl -X POST http://localhost:8080/messages \
  -H 'content-type: application/json; charset=utf-8' \
  -d '{"message":"19時から配信準備です","ttl_hours":6}' | jq
```

初回認可が終わると refresh token が `./data/google-oauth-token.json` としてホスト側に保存され、以後の再起動でも再利用されます。

## 本番サーバー設置

基本的には、リポジトリを clone して `.env` を設定すれば足ります。ただし、実際に必要な手順は次の4点です。

1. リポジトリを clone する
2. `.env` を用意する
3. `data` ディレクトリを作る
4. `docker compose up -d --build` を実行する

```bash
git clone https://github.com/buraiha/uchimachi-dashboard.git
cd uchimachi-dashboard
cp .env.example .env
mkdir -p data
docker compose up -d --build
```

注意点:

- `GOOGLE_OAUTH_REDIRECT_URL` は本番サーバーの公開URLに合わせて変更が必要です
- Google Cloud 側の OAuth クライアントにも、その本番URLのコールバック先を承認済みリダイレクト URI として登録する必要があります
- 利用者認証を使うなら `DASHBOARD_AUTH_USERNAME` と `DASHBOARD_AUTH_PASSWORD` を必ず設定してください
- HTTPS 経由で公開するなら `DASHBOARD_AUTH_COOKIE_SECURE=true` にしてください
- 初回だけブラウザで `/auth/login` を開いて認可を完了してください

## ローカル実行

Rust で直接動かす場合は、同じ環境変数をセットして起動できます。

```bash
export GOOGLE_CALENDAR_ID=your-calendar-id@group.calendar.google.com
export DASHBOARD_TITLE=うちまちダッシュボード
export GOOGLE_OAUTH_CLIENT_ID=your-google-oauth-client-id.apps.googleusercontent.com
export GOOGLE_OAUTH_CLIENT_SECRET=your-google-oauth-client-secret
export GOOGLE_OAUTH_REDIRECT_URL=http://localhost:8080/auth/callback
export GOOGLE_TOKEN_STORE_PATH=./data/google-oauth-token.json
export MESSAGE_DB_PATH=./data/dashboard.sqlite3
export DASHBOARD_AUTH_USERNAME=dashboard-user
export DASHBOARD_AUTH_PASSWORD=change-this-password
export DASHBOARD_AUTH_COOKIE_SECURE=false
export GOOGLE_MAX_RESULTS=10
export PORT=8080

cargo run
```

ローカル実行でも、最初に `http://localhost:8080/auth/login` をブラウザで開いて認可してください。

## レスポンス例

```json
{
  "summary": "Team Calendar",
  "timeZone": "Asia/Tokyo",
  "items": [
    {
      "id": "example-event-id",
      "status": "confirmed",
      "summary": "Weekly Sync",
      "htmlLink": "https://www.google.com/calendar/event?eid=...",
      "start": {
        "dateTime": "2026-04-15T10:00:00+09:00",
        "timeZone": "Asia/Tokyo"
      },
      "end": {
        "dateTime": "2026-04-15T11:00:00+09:00",
        "timeZone": "Asia/Tokyo"
      }
    }
  ]
}
```

## 補足

- この sandbox は読み取り専用スコープ `calendar.readonly` を使います
- access token は refresh token から必要時に再取得します
- 利用者認証はメモリ上のセッション Cookie で管理されるため、再起動後は再ログインが必要です
- ダッシュボードタイトルは `DASHBOARD_TITLE` で変更できます
- 伝言は登録時に1-24時間の有効時間を選べて、SQLite3 ファイルとしてホスト側の data 配下に保存されます
- refresh token を再発行したい場合は `data/google-oauth-token.json` を削除して `/auth/login` をやり直してください
- 本番向けにするならトークンキャッシュ、リトライ、メトリクス、エラーハンドリング強化を入れてください
