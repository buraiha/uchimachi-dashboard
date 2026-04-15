use anyhow::{Context, anyhow};
use axum::{
    Json, Router,
    extract::{Form, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Redirect, Response},
    routing::get,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, SecondsFormat, Timelike, Utc};
use chrono_tz::Tz;
use reqwest::Client;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, env, net::SocketAddr, path::Path, sync::Arc};
use tokio::{fs, sync::Mutex, task};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use uuid::Uuid;

const GOOGLE_OAUTH_AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_OAUTH_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
const DEFAULT_DASHBOARD_TITLE: &str = "うちまちダッシュボード";
const DASHBOARD_RELOAD_SECONDS: i64 = 600;
const DEFAULT_DASHBOARD_MAX_RESULTS: u32 = 10;
const DASHBOARD_MAX_RESULTS_STEP: u32 = 10;
const DASHBOARD_MAX_RESULTS_LIMIT: u32 = 40;
const DEFAULT_MESSAGE_TTL_HOURS: i64 = 12;
const MIN_MESSAGE_TTL_HOURS: i64 = 1;
const MAX_MESSAGE_TTL_HOURS: i64 = 24;
const USER_LOGIN_PATH: &str = "/user/login";
const USER_LOGOUT_PATH: &str = "/user/logout";
const USER_SESSION_COOKIE_NAME: &str = "uchimachi_dashboard_session";

#[derive(Clone)]
struct AppState {
    client: Client,
    config: Arc<Config>,
    pending_states: Arc<Mutex<HashSet<String>>>,
    user_sessions: Arc<Mutex<HashSet<String>>>,
}

#[derive(Clone)]
struct Config {
    calendar_id: String,
    dashboard_title: String,
    max_results: u32,
    port: u16,
    oauth_client_id: String,
    oauth_client_secret: String,
    oauth_redirect_url: String,
    token_store_path: String,
    message_db_path: String,
    user_auth: Option<UserAuthConfig>,
}

#[derive(Clone)]
struct UserAuthConfig {
    username: String,
    password: String,
    cookie_secure: bool,
}

impl Config {
    fn from_env() -> anyhow::Result<Self> {
        let calendar_id =
            env::var("GOOGLE_CALENDAR_ID").context("GOOGLE_CALENDAR_ID is required")?;
        let dashboard_title = env::var("DASHBOARD_TITLE")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| DEFAULT_DASHBOARD_TITLE.to_string());
        let oauth_client_id =
            env::var("GOOGLE_OAUTH_CLIENT_ID").context("GOOGLE_OAUTH_CLIENT_ID is required")?;
        let oauth_client_secret = env::var("GOOGLE_OAUTH_CLIENT_SECRET")
            .context("GOOGLE_OAUTH_CLIENT_SECRET is required")?;

        let oauth_redirect_url = env::var("GOOGLE_OAUTH_REDIRECT_URL")
            .unwrap_or_else(|_| "http://localhost:8080/auth/callback".to_string());
        let token_store_path = env::var("GOOGLE_TOKEN_STORE_PATH")
            .unwrap_or_else(|_| "./data/google-oauth-token.json".to_string());
        let message_db_path = env::var("MESSAGE_DB_PATH")
            .unwrap_or_else(|_| derive_message_db_path(&token_store_path));
        let auth_username = read_optional_trimmed_env("DASHBOARD_AUTH_USERNAME");
        let auth_password = env::var("DASHBOARD_AUTH_PASSWORD")
            .ok()
            .filter(|value| !value.is_empty());
        let auth_cookie_secure = env::var("DASHBOARD_AUTH_COOKIE_SECURE")
            .ok()
            .map(|value| parse_bool_env("DASHBOARD_AUTH_COOKIE_SECURE", &value))
            .transpose()?
            .unwrap_or(false);

        let max_results = env::var("GOOGLE_MAX_RESULTS")
            .ok()
            .map(|value| value.parse::<u32>())
            .transpose()
            .context("GOOGLE_MAX_RESULTS must be a valid u32")?
            .unwrap_or(DEFAULT_DASHBOARD_MAX_RESULTS);

        let port = env::var("PORT")
            .ok()
            .map(|value| value.parse::<u16>())
            .transpose()
            .context("PORT must be a valid u16")?
            .unwrap_or(8080);

        let user_auth = match (auth_username, auth_password) {
            (Some(username), Some(password)) => Some(UserAuthConfig {
                username,
                password,
                cookie_secure: auth_cookie_secure,
            }),
            (None, None) => None,
            _ => {
                return Err(anyhow!(
                    "DASHBOARD_AUTH_USERNAME and DASHBOARD_AUTH_PASSWORD must be set together"
                ));
            }
        };

        Ok(Self {
            calendar_id,
            dashboard_title,
            max_results,
            port,
            oauth_client_id,
            oauth_client_secret,
            oauth_redirect_url,
            token_store_path,
            message_db_path,
            user_auth,
        })
    }

    fn user_auth_enabled(&self) -> bool {
        self.user_auth.is_some()
    }
}

#[derive(Serialize)]
struct ServiceInfo {
    service: &'static str,
    calendar_id: String,
    authorized: bool,
    user_auth_enabled: bool,
    endpoints: Vec<&'static str>,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize)]
struct AuthStatusResponse {
    authorized: bool,
    user_auth_enabled: bool,
    token_store_path: String,
    login_path: &'static str,
    user_login_path: &'static str,
}

#[derive(Serialize)]
struct MessageApiResponse {
    active_messages: Vec<StoredMessage>,
    ttl_hours: i64,
    min_ttl_hours: i64,
    max_ttl_hours: i64,
}

#[derive(Deserialize)]
struct CreateMessageRequest {
    message: String,
    ttl_hours: Option<i64>,
}

#[derive(Deserialize)]
struct ManageMessageForm {
    action: String,
    id: Option<String>,
    message: Option<String>,
    ttl_hours: Option<String>,
}

#[derive(Deserialize, Default)]
struct DashboardQuery {
    max_results: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct AuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct UserLoginQuery {
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserLoginForm {
    username: String,
    password: String,
    next: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    scope: Option<String>,
    token_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredToken {
    access_token: Option<String>,
    refresh_token: String,
    expires_at: Option<DateTime<Utc>>,
    scope: Option<String>,
    token_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCalendarEventsResponse {
    #[serde(default)]
    items: Vec<GoogleCalendarEvent>,
    summary: Option<String>,
    time_zone: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCalendarEvent {
    id: Option<String>,
    status: Option<String>,
    summary: Option<String>,
    html_link: Option<String>,
    start: Option<GoogleCalendarEventDateTime>,
    end: Option<GoogleCalendarEventDateTime>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleCalendarEventDateTime {
    date: Option<String>,
    date_time: Option<String>,
    time_zone: Option<String>,
}

#[derive(Debug)]
struct AppError {
    status: StatusCode,
    error: anyhow::Error,
}

struct DashboardEvent {
    time_label: String,
    date_label: Option<String>,
    title: String,
    sort_key: i64,
}

struct DashboardMessage {
    message: String,
    registered_at_label: String,
}

struct DashboardSections {
    today_label: String,
    tomorrow_label: String,
    messages: Vec<DashboardMessage>,
    today_events: Vec<DashboardEvent>,
    tomorrow_events: Vec<DashboardEvent>,
    upcoming_events: Vec<DashboardEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredMessage {
    id: String,
    message: String,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

struct Utf8Json<T>(T);

impl<T> IntoResponse for Utf8Json<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response {
        match serde_json::to_vec(&self.0) {
            Ok(body) => (
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                body,
            )
                .into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                [(header::CONTENT_TYPE, "application/json; charset=utf-8")],
                format!(r#"{{"error":"failed to serialize response: {error}"}}"#),
            )
                .into_response(),
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error,
        }
    }
}

impl AppError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            error: anyhow!(message.into()),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = Utf8Json(serde_json::json!({
            "error": self.error.to_string(),
        }));

        (self.status, body).into_response()
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "googlecal_sandbox=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Arc::new(Config::from_env()?);
    let state = AppState {
        client: Client::builder()
            .user_agent("uchimachi-dashboard/0.1.0")
            .build()
            .context("failed to build HTTP client")?,
        config: Arc::clone(&config),
        pending_states: Arc::new(Mutex::new(HashSet::new())),
        user_sessions: Arc::new(Mutex::new(HashSet::new())),
    };

    init_message_db(&config.message_db_path).await?;

    let protected_routes = Router::new()
        .route("/", get(dashboard))
        .route("/dashboard", get(dashboard))
        .route("/messages", get(list_messages).post(create_message))
        .route("/messages/manage", get(messages_manage_page).post(messages_manage_action))
        .route("/auth/login", get(auth_login))
        .route("/auth/status", get(auth_status))
        .route("/calendar", get(calendar_events))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_user_auth,
        ));

    let app = Router::new()
        .merge(protected_routes)
        .route("/api/info", get(service_info))
        .route("/health", get(health))
        .route("/auth/callback", get(auth_callback))
        .route(USER_LOGIN_PATH, get(user_login_page).post(user_login_submit))
        .route(USER_LOGOUT_PATH, get(user_logout).post(user_logout))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!(port = config.port, calendar_id = %config.calendar_id, "starting google calendar sandbox service");

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .context("failed to bind TCP listener")?;

    axum::serve(listener, app)
        .await
        .context("server exited unexpectedly")?;

    Ok(())
}

async fn dashboard(
    State(state): State<AppState>,
    Query(query): Query<DashboardQuery>,
) -> Html<String> {
    let selected_max_results = resolve_dashboard_max_results(query.max_results, state.config.max_results);
    let authorized = token_file_exists(&state.config.token_store_path).await;
    if !authorized {
        return Html(render_dashboard_message(
            &state.config.dashboard_title,
            "OAuth 認可が必要です",
            "先に Google ログインを完了してください。",
            Some("/auth/login"),
            Some("Google でログイン"),
        ));
    }

    let content = match get_access_token(&state.client, &state.config).await {
        Ok(access_token) => {
            match fetch_calendar_events(
                &state.client,
                &state.config,
                &access_token,
                selected_max_results,
            )
            .await
            {
                Ok(events) => match load_active_messages(&state).await {
                    Ok(messages) => render_dashboard_page(
                        &state.config.dashboard_title,
                        selected_max_results,
                        &events,
                        &messages,
                        state.config.user_auth_enabled(),
                    ),
                    Err(error) => render_dashboard_message(
                        &state.config.dashboard_title,
                        "伝言の読み込みに失敗しました",
                        &error.to_string(),
                        None,
                        None,
                    ),
                },
                Err(error) => render_dashboard_message(
                    &state.config.dashboard_title,
                    "予定の取得に失敗しました",
                    &error.to_string(),
                    Some("/calendar"),
                    Some("JSON を確認"),
                ),
            }
        }
        Err(error) => render_dashboard_message(
            &state.config.dashboard_title,
            "認可トークンの更新に失敗しました",
            &error.to_string(),
            Some("/auth/login"),
            Some("再認可する"),
        ),
    };

    Html(content)
}

async fn service_info(State(state): State<AppState>) -> Utf8Json<ServiceInfo> {
    Utf8Json(ServiceInfo {
        service: "uchimachi-dashboard",
        calendar_id: state.config.calendar_id.clone(),
        authorized: token_file_exists(&state.config.token_store_path).await,
        user_auth_enabled: state.config.user_auth_enabled(),
        endpoints: vec![
            "GET /",
            "GET /dashboard",
            "GET /api/info",
            "GET /messages",
            "POST /messages",
            "GET /messages/manage",
            "POST /messages/manage",
            "GET /auth/login",
            "GET /auth/callback",
            "GET /auth/status",
            "GET /health",
            "GET /calendar",
            "GET /user/login",
            "POST /user/login",
            "GET /user/logout",
            "POST /user/logout",
        ],
    })
}

async fn health() -> Utf8Json<HealthResponse> {
    Utf8Json(HealthResponse { status: "ok" })
}

async fn auth_status(State(state): State<AppState>) -> Utf8Json<AuthStatusResponse> {
    Utf8Json(AuthStatusResponse {
        authorized: token_file_exists(&state.config.token_store_path).await,
        user_auth_enabled: state.config.user_auth_enabled(),
        token_store_path: state.config.token_store_path.clone(),
        login_path: "/auth/login",
        user_login_path: USER_LOGIN_PATH,
    })
}

async fn user_login_page(
    State(state): State<AppState>,
    Query(query): Query<UserLoginQuery>,
    headers: HeaderMap,
) -> Response {
    if !state.config.user_auth_enabled() {
        return Redirect::to("/dashboard").into_response();
    }

    let next_path = sanitize_next_path(query.next.as_deref());
    if has_valid_user_session(&state, &headers).await {
        return Redirect::to(&next_path).into_response();
    }

    Html(render_user_login_page(
        &state.config.dashboard_title,
        None,
        &next_path,
    ))
    .into_response()
}

async fn user_login_submit(
    State(state): State<AppState>,
    Form(payload): Form<UserLoginForm>,
) -> Response {
    if !state.config.user_auth_enabled() {
        return Redirect::to("/dashboard").into_response();
    }

    let next_path = sanitize_next_path(payload.next.as_deref());
    let Some(auth) = state.config.user_auth.as_ref() else {
        return Redirect::to("/dashboard").into_response();
    };

    if payload.username != auth.username || payload.password != auth.password {
        return (
            StatusCode::UNAUTHORIZED,
            Html(render_user_login_page(
                &state.config.dashboard_title,
                Some("ユーザー名またはパスワードが正しくありません。"),
                &next_path,
            )),
        )
            .into_response();
    }

    let session_id = Uuid::new_v4().to_string();
    state.user_sessions.lock().await.insert(session_id.clone());

    (
        [(header::SET_COOKIE, build_user_session_cookie(&state.config, &session_id))],
        Redirect::to(&next_path),
    )
        .into_response()
}

async fn user_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(session_id) = extract_cookie_value(&headers, USER_SESSION_COOKIE_NAME) {
        state.user_sessions.lock().await.remove(&session_id);
    }

    (
        [(header::SET_COOKIE, clear_user_session_cookie())],
        Redirect::to(USER_LOGIN_PATH),
    )
        .into_response()
}

async fn require_user_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !state.config.user_auth_enabled() {
        return next.run(request).await;
    }

    if has_valid_user_session(&state, request.headers()).await {
        return next.run(request).await;
    }

    let path_and_query = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or("/");

    if is_browser_route(request.uri().path()) {
        let location = format!(
            "{path}?next={next}",
            path = USER_LOGIN_PATH,
            next = urlencoding::encode(path_and_query),
        );
        return Redirect::to(&location).into_response();
    }

    (
        StatusCode::UNAUTHORIZED,
        Utf8Json(serde_json::json!({
            "error": "authentication required",
            "login_path": USER_LOGIN_PATH,
        })),
    )
        .into_response()
}

async fn list_messages(
    State(state): State<AppState>,
) -> Result<Utf8Json<MessageApiResponse>, AppError> {
    let active_messages = load_active_messages(&state).await?;
    Ok(Utf8Json(MessageApiResponse {
        active_messages,
        ttl_hours: DEFAULT_MESSAGE_TTL_HOURS,
        min_ttl_hours: MIN_MESSAGE_TTL_HOURS,
        max_ttl_hours: MAX_MESSAGE_TTL_HOURS,
    }))
}

async fn create_message(
    State(state): State<AppState>,
    Json(payload): Json<CreateMessageRequest>,
) -> Result<(StatusCode, Utf8Json<StoredMessage>), AppError> {
    let message = payload.message.trim();
    let ttl_hours = resolve_message_ttl_hours(payload.ttl_hours)?;
    if message.is_empty() {
        return Err(AppError::bad_request("message must not be empty"));
    }
    if message.chars().count() > 280 {
        return Err(AppError::bad_request(
            "message must be 280 characters or fewer",
        ));
    }

    let now = Utc::now();

    let new_message = StoredMessage {
        id: Uuid::new_v4().to_string(),
        message: message.to_string(),
        created_at: now,
        expires_at: now + Duration::hours(ttl_hours),
    };

    insert_message(&state.config.message_db_path, &new_message).await?;

    Ok((StatusCode::CREATED, Utf8Json(new_message)))
}

async fn messages_manage_page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let active_messages = load_active_messages(&state).await?;
    Ok(Html(render_message_manage_page(
        &active_messages,
        state.config.user_auth_enabled(),
    )))
}

async fn messages_manage_action(
    State(state): State<AppState>,
    Form(payload): Form<ManageMessageForm>,
) -> Result<Redirect, AppError> {
    match payload.action.as_str() {
        "create" => {
            let message = payload.message.unwrap_or_default();
            let ttl_hours = parse_message_ttl_hours_form_value(payload.ttl_hours.clone())?;
            let trimmed = message.trim();
            if trimmed.is_empty() {
                return Err(AppError::bad_request("message must not be empty"));
            }
            if trimmed.chars().count() > 280 {
                return Err(AppError::bad_request(
                    "message must be 280 characters or fewer",
                ));
            }

            let now = Utc::now();
            let new_message = StoredMessage {
                id: Uuid::new_v4().to_string(),
                message: trimmed.to_string(),
                created_at: now,
                expires_at: now + Duration::hours(ttl_hours),
            };
            insert_message(&state.config.message_db_path, &new_message).await?;
        }
        "update" => {
            let id = payload
                .id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| AppError::bad_request("message id is required"))?;
            let message = payload.message.unwrap_or_default();
            let trimmed = message.trim();
            if trimmed.is_empty() {
                return Err(AppError::bad_request("message must not be empty"));
            }
            if trimmed.chars().count() > 280 {
                return Err(AppError::bad_request(
                    "message must be 280 characters or fewer",
                ));
            }

            let ttl_hours = parse_message_ttl_hours_form_value(payload.ttl_hours.clone())?;
            update_message(&state.config.message_db_path, &id, trimmed, ttl_hours).await?;
        }
        "delete" => {
            let id = payload
                .id
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| AppError::bad_request("message id is required"))?;
            delete_message(&state.config.message_db_path, &id).await?;
        }
        _ => return Err(AppError::bad_request("unsupported message action")),
    }

    Ok(Redirect::to("/messages/manage"))
}

async fn auth_login(State(state): State<AppState>) -> Result<Redirect, AppError> {
    let oauth_state = Uuid::new_v4().to_string();
    state
        .pending_states
        .lock()
        .await
        .insert(oauth_state.clone());

    let authorize_url = format!(
        "{base}?response_type=code&client_id={client_id}&redirect_uri={redirect_uri}&scope={scope}&access_type=offline&prompt=consent&state={state}",
        base = GOOGLE_OAUTH_AUTHORIZE_URL,
        client_id = urlencoding::encode(&state.config.oauth_client_id),
        redirect_uri = urlencoding::encode(&state.config.oauth_redirect_url),
        scope = urlencoding::encode(GOOGLE_CALENDAR_SCOPE),
        state = urlencoding::encode(&oauth_state),
    );

    Ok(Redirect::temporary(&authorize_url))
}

async fn auth_callback(
    State(state): State<AppState>,
    Query(query): Query<AuthCallbackQuery>,
) -> Result<Html<String>, AppError> {
    if let Some(error) = query.error {
        return Err(AppError::bad_request(format!(
            "google oauth authorization failed: {error}"
        )));
    }

    let code = query
        .code
        .ok_or_else(|| AppError::bad_request("missing code query parameter"))?;
    let request_state = query
        .state
        .ok_or_else(|| AppError::bad_request("missing state query parameter"))?;

    let was_valid_state = state.pending_states.lock().await.remove(&request_state);
    if !was_valid_state {
        return Err(AppError::bad_request(
            "oauth state mismatch; retry from /auth/login",
        ));
    }

    let token_response = exchange_authorization_code(&state.client, &state.config, &code).await?;
    let refresh_token = token_response
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("refresh_token was not returned by Google; retry /auth/login after deleting any previous consent"))?;

    let stored_token = StoredToken {
        access_token: Some(token_response.access_token),
        refresh_token,
        expires_at: calculate_expires_at(token_response.expires_in),
        scope: token_response.scope,
        token_type: token_response.token_type,
    };
    persist_token(&state.config.token_store_path, &stored_token).await?;

    Ok(Html(
        "OAuth authorization completed. You can close this page and call /calendar.".to_string(),
    ))
}

async fn calendar_events(
    State(state): State<AppState>,
) -> Result<Utf8Json<GoogleCalendarEventsResponse>, AppError> {
    let access_token = get_access_token(&state.client, &state.config).await?;
    let response = fetch_calendar_events(
        &state.client,
        &state.config,
        &access_token,
        state.config.max_results,
    )
    .await?;
    Ok(Utf8Json(response))
}

async fn get_access_token(client: &Client, config: &Config) -> anyhow::Result<String> {
    let stored_token = read_stored_token(&config.token_store_path).await?;

    if token_is_still_valid(&stored_token) {
        if let Some(access_token) = stored_token.access_token {
            return Ok(access_token);
        }
    }

    let token_response = refresh_access_token(client, config, &stored_token.refresh_token).await?;
    let updated_token = StoredToken {
        access_token: Some(token_response.access_token.clone()),
        refresh_token: token_response
            .refresh_token
            .unwrap_or(stored_token.refresh_token),
        expires_at: calculate_expires_at(token_response.expires_in),
        scope: token_response.scope.or(stored_token.scope),
        token_type: token_response.token_type.or(stored_token.token_type),
    };
    persist_token(&config.token_store_path, &updated_token).await?;

    Ok(token_response.access_token)
}

async fn exchange_authorization_code(
    client: &Client,
    config: &Config,
    code: &str,
) -> anyhow::Result<GoogleTokenResponse> {
    request_google_token(
        client,
        vec![
            ("code", code.to_string()),
            ("client_id", config.oauth_client_id.clone()),
            ("client_secret", config.oauth_client_secret.clone()),
            ("redirect_uri", config.oauth_redirect_url.clone()),
            ("grant_type", "authorization_code".to_string()),
        ],
    )
    .await
}

async fn refresh_access_token(
    client: &Client,
    config: &Config,
    refresh_token: &str,
) -> anyhow::Result<GoogleTokenResponse> {
    request_google_token(
        client,
        vec![
            ("client_id", config.oauth_client_id.clone()),
            ("client_secret", config.oauth_client_secret.clone()),
            ("refresh_token", refresh_token.to_string()),
            ("grant_type", "refresh_token".to_string()),
        ],
    )
    .await
}

async fn request_google_token(
    client: &Client,
    params: Vec<(&str, String)>,
) -> anyhow::Result<GoogleTokenResponse> {
    let response = client
        .post(GOOGLE_OAUTH_TOKEN_URL)
        .form(&params)
        .send()
        .await
        .context("failed to request google oauth token")?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read response body>".to_string());
        return Err(anyhow!(
            "google oauth token endpoint returned {}: {}",
            status,
            body
        ));
    }

    let response = response
        .json::<GoogleTokenResponse>()
        .await
        .context("failed to deserialize oauth token response")?;

    Ok(response)
}

async fn read_stored_token(token_store_path: &str) -> anyhow::Result<StoredToken> {
    let raw = fs::read_to_string(token_store_path)
        .await
        .with_context(|| {
            format!(
                "token file not found at {token_store_path}; open /auth/login to authorize first"
            )
        })?;

    serde_json::from_str(&raw).context("failed to parse stored oauth token json")
}

async fn persist_token(token_store_path: &str, token: &StoredToken) -> anyhow::Result<()> {
    if let Some(parent) = Path::new(token_store_path).parent() {
        fs::create_dir_all(parent).await.with_context(|| {
            format!(
                "failed to create token store directory: {}",
                parent.display()
            )
        })?;
    }

    let content = serde_json::to_vec_pretty(token).context("failed to serialize oauth token")?;
    fs::write(token_store_path, content)
        .await
        .with_context(|| format!("failed to write token file: {token_store_path}"))?;

    Ok(())
}

fn calculate_expires_at(expires_in: Option<i64>) -> Option<DateTime<Utc>> {
    expires_in.map(|seconds| Utc::now() + Duration::seconds(seconds.saturating_sub(60)))
}

fn token_is_still_valid(token: &StoredToken) -> bool {
    token.access_token.is_some()
        && token
            .expires_at
            .map(|expires_at| expires_at > Utc::now())
            .unwrap_or(false)
}

async fn token_file_exists(token_store_path: &str) -> bool {
    fs::metadata(token_store_path).await.is_ok()
}

async fn fetch_calendar_events(
    client: &Client,
    config: &Config,
    access_token: &str,
    max_results: u32,
) -> anyhow::Result<GoogleCalendarEventsResponse> {
    let encoded_calendar_id = urlencoding::encode(&config.calendar_id);
    let url =
        format!("https://www.googleapis.com/calendar/v3/calendars/{encoded_calendar_id}/events");
    let time_min = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let query_params = vec![
        ("singleEvents".to_string(), "true".to_string()),
        ("orderBy".to_string(), "startTime".to_string()),
        ("maxResults".to_string(), max_results.to_string()),
        ("timeMin".to_string(), time_min),
    ];

    let response = client
        .get(url)
        .bearer_auth(access_token)
        .query(&query_params)
        .send()
        .await
        .context("failed to request google calendar events")?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<failed to read response body>".to_string());
        return Err(anyhow!("google calendar api returned {}: {}", status, body));
    }

    let response = response
        .json::<GoogleCalendarEventsResponse>()
        .await
        .context("failed to deserialize calendar events response")?;

    Ok(response)
}

fn render_dashboard_page(
    dashboard_title: &str,
    selected_max_results: u32,
    events: &GoogleCalendarEventsResponse,
    messages: &[StoredMessage],
    user_auth_enabled: bool,
) -> String {
    let sections = build_dashboard_sections(events, messages);

    let message_cards = render_message_cards(&sections.messages, "現在有効な伝言はありません");
    let today_cards = render_primary_event_cards(&sections.today_events, "本日の予定はありません");
    let tomorrow_cards =
        render_primary_event_cards(&sections.tomorrow_events, "明日の予定はありません");
    let upcoming_rows =
        render_upcoming_event_rows(&sections.upcoming_events, "明後日以降の予定はありません");
    let max_results_options = render_dashboard_max_results_options(selected_max_results);
    let session_actions = if user_auth_enabled {
        format!(
            r#"<form method="post" action="{logout_path}" style="margin:0;"><button type="submit" class="hero-logout">ログアウト</button></form>"#,
            logout_path = USER_LOGOUT_PATH,
        )
    } else {
        String::new()
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{page_title}</title>
    <style>
        :root {{
            --bg: #f7f1e3;
            --panel: rgba(255, 252, 245, 0.84);
            --panel-strong: rgba(252, 247, 238, 0.96);
            --line: rgba(56, 42, 30, 0.14);
            --ink: #2f241b;
            --muted: #7a6757;
            --accent: #c55c3b;
            --accent-soft: #efdbc8;
            --shadow: 0 24px 70px rgba(77, 54, 33, 0.14);
        }}
        * {{ box-sizing: border-box; }}
        html, body {{ height: 100%; margin: 0; }}
        body {{
            overflow: hidden;
            font-family: "Hiragino Sans", "Noto Sans JP", "Yu Gothic", sans-serif;
            color: var(--ink);
            background:
                radial-gradient(circle at top left, rgba(255, 214, 167, 0.9), transparent 28%),
                radial-gradient(circle at bottom right, rgba(205, 116, 82, 0.24), transparent 24%),
                linear-gradient(135deg, #fbf4e6 0%, #f4eadf 45%, #efe4d7 100%);
        }}
        .shell {{
            display: grid;
            grid-template-columns: minmax(0, 1fr);
            grid-template-rows: auto minmax(0, 1fr);
            grid-template-areas:
                "hero"
                "content";
            gap: 22px;
            height: 100vh;
            padding: 22px;
        }}
        .panel {{
            min-height: 0;
            overflow: hidden;
            border: 1px solid var(--line);
            border-radius: 28px;
            background: var(--panel);
            box-shadow: var(--shadow);
            backdrop-filter: blur(18px);
        }}
        .hero-panel {{
            grid-area: hero;
            width: 100%;
            min-width: 0;
        }}
        .content {{
            grid-area: content;
            display: grid;
            grid-template-columns: repeat(2, minmax(0, 1fr));
            gap: 22px;
            min-height: 0;
            align-items: stretch;
        }}
        .column-stack {{
            display: grid;
            gap: 22px;
            min-height: 0;
        }}
        .left-stack {{ grid-template-rows: auto auto; }}
        .right-stack {{ grid-template-rows: auto auto; }}
        .hero {{ padding: 26px 28px 22px; }}
        .panel-head {{
            padding: 24px 26px 18px;
            border-bottom: 1px solid var(--line);
            background: linear-gradient(180deg, rgba(255,255,255,0.3), rgba(255,255,255,0));
        }}
        .head-inline {{
            display: flex;
            align-items: flex-start;
            justify-content: space-between;
            gap: 14px;
        }}
        .head-copy {{
            display: grid;
            gap: 8px;
            min-width: 0;
        }}
        .hero-controls {{
            display: flex;
            align-items: center;
            gap: 10px;
            margin-left: auto;
            white-space: nowrap;
        }}
        .hero-logout {{
            appearance: none;
            border: 1px solid rgba(56, 42, 30, 0.16);
            border-radius: 999px;
            padding: 10px 16px;
            background: rgba(255,255,255,0.82);
            color: var(--ink);
            font: inherit;
            font-weight: 700;
            cursor: pointer;
        }}
        .hero-control-label {{
            font-size: 13px;
            font-weight: 700;
            color: var(--muted);
        }}
        .hero-select {{
            appearance: none;
            min-width: 112px;
            padding: 10px 36px 10px 14px;
            border: 1px solid rgba(56, 42, 30, 0.16);
            border-radius: 999px;
            background: rgba(255,255,255,0.82);
            color: var(--ink);
            font: inherit;
            font-weight: 700;
        }}
        .eyebrow {{
            display: inline-flex;
            align-items: center;
            justify-content: center;
            padding: 6px 10px;
            border-radius: 999px;
            font-size: 12px;
            letter-spacing: 0.12em;
            text-transform: uppercase;
            color: var(--accent);
            background: var(--accent-soft);
            white-space: nowrap;
        }}
        .title {{ margin: 0; font-size: 32px; line-height: 1.1; font-weight: 800; }}
        .countdown {{ margin: 0; font-size: 14px; font-weight: 700; color: var(--accent); }}
        .subtitle {{ margin: 0; color: var(--muted); font-size: 15px; line-height: 1.6; }}
        .panel-body {{ height: 100%; overflow: auto; padding: 22px 24px 24px; }}
        .schedule-body {{ padding: 14px 20px 18px; }}
        .schedule-body {{ overflow: hidden; }}
        .panel-body::-webkit-scrollbar {{ width: 10px; }}
        .panel-body::-webkit-scrollbar-thumb {{ background: rgba(90, 66, 41, 0.18); border-radius: 999px; }}
        .section-title {{ margin: 0; font-size: 28px; line-height: 1.15; font-weight: 800; }}
        .section-date {{ margin: 10px 0 0; font-size: 14px; color: var(--muted); }}
        .primary-list {{
            display: grid;
            gap: var(--card-gap, 16px);
            height: auto;
            min-height: min-content;
            padding: 2px 0;
            grid-template-rows: none;
            align-content: start;
        }}
        .message-group {{ display: grid; gap: 14px; }}
        .message-head {{ display: flex; align-items: baseline; justify-content: space-between; gap: 12px; }}
        .message-title {{ margin: 0; font-size: 22px; font-weight: 800; }}
        .message-meta {{ margin: 0; font-size: 13px; color: var(--muted); }}
        .message-card {{
            display: grid;
            gap: 10px;
            padding: 16px 18px;
            border-radius: 18px;
            background: linear-gradient(135deg, rgba(197, 92, 59, 0.12), rgba(255,255,255,0.7));
            border: 1px solid rgba(197, 92, 59, 0.18);
        }}
        .message-body {{ font-size: 18px; line-height: 1.5; font-weight: 700; white-space: pre-wrap; word-break: break-word; }}
        .message-registered {{ font-size: 13px; color: var(--muted); }}
        .primary-card {{
            display: grid;
            gap: 4px;
            padding: var(--card-padding-y, 8px) var(--card-padding-x, 6px);
            height: auto;
            min-height: unset;
            align-content: start;
            border-bottom: 1px solid rgba(106, 73, 49, 0.1);
            overflow: hidden;
        }}
        .primary-card:last-child {{ border-bottom: none; }}
        .primary-time {{
            font-size: var(--time-size, 34px);
            line-height: 1.05;
            font-weight: 800;
            letter-spacing: -0.03em;
            white-space: nowrap;
        }}
        .primary-title {{
            font-size: var(--title-size, 28px);
            line-height: 1.25;
            font-weight: 700;
            word-break: break-word;
            display: -webkit-box;
            -webkit-line-clamp: var(--title-lines, 2);
            -webkit-box-orient: vertical;
            overflow: hidden;
        }}
        .empty {{
            display: grid;
            place-items: center;
            min-height: 180px;
            padding: 24px;
            border-radius: 22px;
            border: 1px dashed rgba(101, 76, 53, 0.2);
            color: var(--muted);
            background: rgba(255,255,255,0.25);
            text-align: center;
        }}
        .upcoming-list {{ display: grid; gap: 12px; }}
        .upcoming-row {{
            display: grid;
            grid-template-columns: 112px 112px minmax(0, 1fr);
            gap: var(--upcoming-gap, 14px);
            align-items: start;
            padding: var(--upcoming-padding-y, 8px) 2px;
            border-bottom: 1px solid rgba(86, 65, 45, 0.08);
        }}
        .upcoming-row:last-child {{ border-bottom: none; }}
        .upcoming-date, .upcoming-time {{
            font-size: var(--upcoming-meta-size, 14px);
            line-height: 1.25;
            font-weight: 700;
            color: var(--muted);
            white-space: nowrap;
        }}
        .upcoming-title {{
            font-size: var(--upcoming-title-size, 20px);
            line-height: 1.3;
            font-weight: 700;
            word-break: break-word;
            display: -webkit-box;
            -webkit-line-clamp: var(--upcoming-title-lines, 2);
            -webkit-box-orient: vertical;
            overflow: hidden;
        }}
        .upcoming-list.fit-list {{
            gap: var(--upcoming-row-gap, 10px);
            height: auto;
            min-height: min-content;
            padding: 2px 0;
            align-content: start;
        }}
        @media (max-width: 1024px) {{
            body {{ overflow: auto; }}
            .shell {{
                grid-template-columns: 1fr;
                grid-template-rows: auto auto;
                grid-template-areas:
                    "hero"
                    "content";
                gap: 16px;
                height: auto;
                min-height: 100vh;
                padding: 16px;
            }}
            .hero-panel {{
                width: 100%;
                min-width: 0;
            }}
            .content {{ gap: 16px; }}
            .column-stack {{ gap: 16px; }}
            .left-stack {{ grid-template-rows: auto auto; }}
            .right-stack {{ grid-template-rows: auto auto; }}
            .hero {{ padding: 18px 20px; }}
            .panel-head {{ padding: 16px 18px 14px; }}
            .panel-body {{ padding: 16px 18px 18px; }}
            .schedule-body {{ padding: 10px 14px 14px; }}
            .schedule-body {{ max-height: 42vh; overflow: auto; }}
            .head-inline {{ gap: 10px; }}
            .head-copy {{ gap: 6px; }}
            .hero-controls {{ gap: 8px; }}
            .hero-control-label {{ font-size: 12px; }}
            .hero-select {{ min-width: 96px; padding: 8px 30px 8px 12px; }}
            .eyebrow {{ font-size: 11px; padding: 5px 8px; }}
            .title {{ font-size: 28px; }}
            .countdown {{ font-size: 13px; }}
            .section-title {{ font-size: 24px; }}
            .section-date, .subtitle {{ font-size: 13px; }}
            .message-body {{ font-size: 16px; }}
            .message-registered {{ font-size: 12px; }}
            .empty {{ min-height: 120px; padding: 18px; }}
            .panel-body {{ max-height: none; }}
            .primary-list {{
                height: auto;
                min-height: unset;
                grid-template-rows: none;
                gap: 10px;
            }}
            .primary-card {{
                height: auto;
                min-height: unset;
                padding: 8px 0;
            }}
            .primary-time {{ font-size: 24px; }}
            .primary-title {{
                font-size: 18px;
                -webkit-line-clamp: unset;
                display: block;
                overflow: visible;
            }}
            .upcoming-list.fit-list {{
                height: auto;
                min-height: unset;
                gap: 8px;
            }}
            .upcoming-title {{
                font-size: 16px;
                -webkit-line-clamp: unset;
                display: block;
                overflow: visible;
            }}
        }}
        @media (max-width: 768px) {{
            .content {{
                grid-template-columns: 1fr;
            }}
            .head-inline {{
                flex-direction: column;
                align-items: flex-start;
            }}
            .hero-controls {{
                margin-left: 0;
            }}
        }}
        @media (max-width: 1100px) {{
            body {{ overflow: auto; }}
            .shell {{
                grid-template-columns: 1fr;
                grid-template-rows: auto auto;
                grid-template-areas:
                    "hero"
                    "content";
                height: auto;
                min-height: 100vh;
            }}
            .hero-panel {{ width: 100%; min-width: 0; }}
            .column-stack {{ grid-template-rows: auto auto; }}
            .panel-body {{ max-height: 46vh; }}
            .schedule-body {{ overflow: auto; }}
        }}
    </style>
</head>
<body>
    <main class="shell">
        <section class="panel hero hero-panel">
            <div class="head-inline">
                <div class="head-inline" style="justify-content:flex-start;">
                    <span class="eyebrow">Dashboard</span>
                    <div class="head-copy">
                        <h1 class="title">{display_title}</h1>
                        <p class="countdown">更新まで <span id="countdown">{reload_seconds}</span> 秒</p>
                    </div>
                </div>
                <form class="hero-controls" action="/dashboard" method="get">
                    <label class="hero-control-label" for="max-results-select">取得件数</label>
                    <select id="max-results-select" class="hero-select" name="max_results">
                        {max_results_options}
                    </select>
                </form>
                {session_actions}
            </div>
        </section>
        <section class="content">
            <div class="column-stack left-stack">
                <section class="panel">
                    <div class="panel-head">
                        <div class="head-inline">
                            <span class="eyebrow">Message</span>
                            <div class="head-copy">
                                <h2 class="section-title">伝言</h2>
                                <p class="section-date">1-24時間で設定した期限を過ぎると自動的に消えます ・ <a href="/messages/manage" style="color:inherit;">編集</a></p>
                            </div>
                        </div>
                    </div>
                    <div class="panel-body">
                        <div class="message-group">{message_cards}</div>
                    </div>
                </section>
                <section class="panel">
                    <div class="panel-head">
                        <div class="head-inline">
                            <span class="eyebrow">Today</span>
                            <div class="head-copy">
                                <h2 class="section-title">今日の予定</h2>
                                <p class="section-date">{today_label}</p>
                            </div>
                        </div>
                    </div>
                    <div class="panel-body schedule-body">
                        {today_cards}
                    </div>
                </section>
            </div>
            <div class="column-stack right-stack">
                <section class="panel">
                    <div class="panel-head">
                        <div class="head-inline">
                            <span class="eyebrow">Tomorrow</span>
                            <div class="head-copy">
                                <h2 class="section-title">明日の予定</h2>
                                <p class="section-date">{tomorrow_label}</p>
                            </div>
                        </div>
                    </div>
                    <div class="panel-body schedule-body">
                        {tomorrow_cards}
                    </div>
                </section>
                <aside class="panel column-upcoming">
                    <div class="panel-head">
                        <div class="head-inline">
                            <span class="eyebrow">Upcoming</span>
                            <div class="head-copy">
                                <h2 class="section-title">明後日以降の予定</h2>
                                <p class="subtitle">日付、時間、タイトルを時系列で確認できます。</p>
                            </div>
                        </div>
                    </div>
                    <div class="panel-body schedule-body">
                        {upcoming_rows}
                    </div>
                </aside>
            </div>
        </section>
    </main>
    <script>
        const reloadSeconds = {reload_seconds};
        const countdownElement = document.getElementById("countdown");
        const maxResultsSelect = document.getElementById("max-results-select");
        const tabletMediaQuery = window.matchMedia("(max-width: 1024px)");
        const maxResultsStorageKey = "dashboard:max_results";
        const allowedMaxResults = new Set(["10", "20", "30", "40"]);
        let remainingSeconds = reloadSeconds;

        const persistMaxResults = (value) => {{
            if (!allowedMaxResults.has(value)) {{
                return;
            }}

            try {{
                window.localStorage.setItem(maxResultsStorageKey, value);
            }} catch (_error) {{
            }}
        }};

        const restoreMaxResults = () => {{
            const url = new URL(window.location.href);
            const currentValue = url.searchParams.get("max_results");

            if (currentValue && allowedMaxResults.has(currentValue)) {{
                persistMaxResults(currentValue);
                return;
            }}

            let storedValue = null;
            try {{
                storedValue = window.localStorage.getItem(maxResultsStorageKey);
            }} catch (_error) {{
            }}

            if (!storedValue || !allowedMaxResults.has(storedValue)) {{
                return;
            }}

            url.searchParams.set("max_results", storedValue);
            window.location.replace(url.toString());
        }};

        const handleMaxResultsChange = (value) => {{
            if (!allowedMaxResults.has(value)) {{
                return;
            }}

            persistMaxResults(value);

            const url = new URL(window.location.href);
            url.searchParams.set("max_results", value);
            window.location.assign(url.toString());
        }};

        restoreMaxResults();
        if (maxResultsSelect) {{
            maxResultsSelect.addEventListener("change", (event) => {{
                handleMaxResultsChange(event.target.value);
            }});
        }}

        const renderCountdown = () => {{
            if (countdownElement) {{
                countdownElement.textContent = String(remainingSeconds);
            }}
        }};

        renderCountdown();
        const fitPrimaryLists = () => {{
            if (tabletMediaQuery.matches) {{
                document.querySelectorAll(".js-fit-primary-list").forEach((list) => {{
                    list.style.removeProperty("--time-size");
                    list.style.removeProperty("--title-size");
                    list.style.removeProperty("--card-padding-y");
                    list.style.removeProperty("--card-padding-x");
                    list.style.removeProperty("--card-gap");
                    list.style.removeProperty("--title-lines");
                }});
                document.querySelectorAll(".js-fit-upcoming-list").forEach((list) => {{
                    list.style.removeProperty("--upcoming-meta-size");
                    list.style.removeProperty("--upcoming-title-size");
                    list.style.removeProperty("--upcoming-padding-y");
                    list.style.removeProperty("--upcoming-row-gap");
                    list.style.removeProperty("--upcoming-gap");
                    list.style.removeProperty("--upcoming-title-lines");
                }});
                return;
            }}

            document.querySelectorAll(".js-fit-primary-list").forEach((list) => {{
                const readVar = (name, fallback) => {{
                    const raw = getComputedStyle(list).getPropertyValue(name).trim();
                    const value = Number.parseFloat(raw);
                    return Number.isFinite(value) ? value : fallback;
                }};

                let timeSize = readVar("--time-size", 34);
                let titleSize = readVar("--title-size", 28);
                let paddingY = readVar("--card-padding-y", 10);
                let paddingX = readVar("--card-padding-x", 8);
                let gap = readVar("--card-gap", 16);
                let titleLines = readVar("--title-lines", 2);

                while (list.scrollHeight > list.clientHeight + 1) {{
                    const nextTimeSize = Math.max(16, timeSize - 1);
                    const nextTitleSize = Math.max(14, titleSize - 1);
                    const nextPaddingY = Math.max(4, paddingY - 1);
                    const nextPaddingX = Math.max(2, paddingX - 1);
                    const nextGap = Math.max(2, gap - 1);
                    const nextTitleLines = nextTitleSize <= 18 ? 1 : titleLines;

                    if (
                        nextTimeSize === timeSize &&
                        nextTitleSize === titleSize &&
                        nextPaddingY === paddingY &&
                        nextPaddingX === paddingX &&
                        nextGap === gap &&
                        nextTitleLines === titleLines
                    ) {{
                        break;
                    }}

                    timeSize = nextTimeSize;
                    titleSize = nextTitleSize;
                    paddingY = nextPaddingY;
                    paddingX = nextPaddingX;
                    gap = nextGap;
                    titleLines = nextTitleLines;

                    list.style.setProperty("--time-size", `${{timeSize}}px`);
                    list.style.setProperty("--title-size", `${{titleSize}}px`);
                    list.style.setProperty("--card-padding-y", `${{paddingY}}px`);
                    list.style.setProperty("--card-padding-x", `${{paddingX}}px`);
                    list.style.setProperty("--card-gap", `${{gap}}px`);
                    list.style.setProperty("--title-lines", String(titleLines));
                }}
            }});

            document.querySelectorAll(".js-fit-upcoming-list").forEach((list) => {{
                const readVar = (name, fallback) => {{
                    const raw = getComputedStyle(list).getPropertyValue(name).trim();
                    const value = Number.parseFloat(raw);
                    return Number.isFinite(value) ? value : fallback;
                }};

                let metaSize = readVar("--upcoming-meta-size", 14);
                let titleSize = readVar("--upcoming-title-size", 18);
                let paddingY = readVar("--upcoming-padding-y", 8);
                let rowGap = readVar("--upcoming-row-gap", 10);
                let columnGap = readVar("--upcoming-gap", 14);
                let titleLines = readVar("--upcoming-title-lines", 2);

                while (list.scrollHeight > list.clientHeight + 1) {{
                    const nextMetaSize = Math.max(10, metaSize - 1);
                    const nextTitleSize = Math.max(12, titleSize - 1);
                    const nextPaddingY = Math.max(2, paddingY - 1);
                    const nextRowGap = Math.max(2, rowGap - 1);
                    const nextColumnGap = Math.max(6, columnGap - 1);
                    const nextTitleLines = titleSize <= 15 ? 1 : titleLines;

                    if (
                        nextMetaSize === metaSize &&
                        nextTitleSize === titleSize &&
                        nextPaddingY === paddingY &&
                        nextRowGap === rowGap &&
                        nextColumnGap === columnGap &&
                        nextTitleLines === titleLines
                    ) {{
                        break;
                    }}

                    metaSize = nextMetaSize;
                    titleSize = nextTitleSize;
                    paddingY = nextPaddingY;
                    rowGap = nextRowGap;
                    columnGap = nextColumnGap;
                    titleLines = nextTitleLines;

                    list.style.setProperty("--upcoming-meta-size", `${{metaSize}}px`);
                    list.style.setProperty("--upcoming-title-size", `${{titleSize}}px`);
                    list.style.setProperty("--upcoming-padding-y", `${{paddingY}}px`);
                    list.style.setProperty("--upcoming-row-gap", `${{rowGap}}px`);
                    list.style.setProperty("--upcoming-gap", `${{columnGap}}px`);
                    list.style.setProperty("--upcoming-title-lines", String(titleLines));
                }}
            }});
        }};

        fitPrimaryLists();
        window.addEventListener("resize", fitPrimaryLists);
        window.setInterval(() => {{
            remainingSeconds -= 1;
            if (remainingSeconds <= 0) {{
                window.location.reload();
                return;
            }}
            renderCountdown();
        }}, 1000);
    </script>
</body>
</html>"#,
        page_title = escape_html(dashboard_title),
        display_title = escape_html(dashboard_title),
        reload_seconds = DASHBOARD_RELOAD_SECONDS,
        message_cards = message_cards,
        max_results_options = max_results_options,
        session_actions = session_actions,
        today_label = escape_html(&sections.today_label),
        tomorrow_label = escape_html(&sections.tomorrow_label),
        today_cards = today_cards,
        tomorrow_cards = tomorrow_cards,
        upcoming_rows = upcoming_rows,
    )
}

fn render_message_manage_page(messages: &[StoredMessage], user_auth_enabled: bool) -> String {
    let create_ttl_options = render_ttl_option_tags(DEFAULT_MESSAGE_TTL_HOURS);
    let session_actions = if user_auth_enabled {
        format!(
            r#"<form method="post" action="{logout_path}" style="margin:0;"><button type="submit" class="secondary-button">ログアウト</button></form>"#,
            logout_path = USER_LOGOUT_PATH,
        )
    } else {
        String::new()
    };
    let items = if messages.is_empty() {
        "<div class=\"manage-empty\">現在有効な伝言はありません</div>".to_string()
    } else {
        messages
            .iter()
            .map(|message| {
                let ttl_options = render_ttl_option_tags(message_ttl_hours(message));
                format!(
                    r#"<article class="manage-item"><form method="post" action="/messages/manage" class="manage-form"><input type="hidden" name="id" value="{id}"><input type="hidden" name="action" value="update"><label class="manage-label">伝言<textarea name="message" maxlength="280" required>{message}</textarea></label><label class="manage-label">有効時間<select name="ttl_hours" required>{ttl_options}</select></label><div class="manage-meta">登録日時: {registered_at} / 失効日時: {expires_at}</div><div class="manage-actions"><button type="submit" class="primary-button">更新する</button></div></form><form method="post" action="/messages/manage" class="delete-form"><input type="hidden" name="id" value="{id}"><input type="hidden" name="action" value="delete"><button type="submit" class="secondary-button">削除</button></form></article>"#,
                    id = escape_html(&message.id),
                    message = escape_html(&message.message),
                    ttl_options = ttl_options,
                    registered_at = escape_html(&format_registered_at(
                        message.created_at.with_timezone(&chrono_tz::Asia::Tokyo)
                    )),
                    expires_at = escape_html(&format_registered_at(
                        message.expires_at.with_timezone(&chrono_tz::Asia::Tokyo)
                    )),
                )
            })
            .collect::<Vec<_>>()
            .join("")
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>伝言の編集</title>
    <style>
        :root {{
            --bg: #f7f1e3;
            --panel: rgba(255, 252, 245, 0.92);
            --line: rgba(56, 42, 30, 0.14);
            --ink: #2f241b;
            --muted: #7a6757;
            --accent: #c55c3b;
            --accent-soft: #efdbc8;
            --shadow: 0 24px 70px rgba(77, 54, 33, 0.14);
        }}
        * {{ box-sizing: border-box; }}
        body {{
            margin: 0;
            min-height: 100vh;
            padding: 24px;
            font-family: "Hiragino Sans", "Noto Sans JP", "Yu Gothic", sans-serif;
            color: var(--ink);
            background:
                radial-gradient(circle at top left, rgba(255, 214, 167, 0.9), transparent 28%),
                radial-gradient(circle at bottom right, rgba(205, 116, 82, 0.24), transparent 24%),
                linear-gradient(135deg, #fbf4e6 0%, #f4eadf 45%, #efe4d7 100%);
        }}
        main {{ max-width: 980px; margin: 0 auto; display: grid; gap: 20px; }}
        .panel {{
            border: 1px solid var(--line);
            border-radius: 28px;
            background: var(--panel);
            box-shadow: var(--shadow);
            overflow: hidden;
        }}
        .panel-head {{ padding: 24px 26px 18px; border-bottom: 1px solid var(--line); }}
        .panel-body {{ padding: 22px 24px 24px; }}
        .eyebrow {{
            display: inline-flex;
            align-items: center;
            justify-content: center;
            padding: 6px 10px;
            border-radius: 999px;
            font-size: 12px;
            letter-spacing: 0.12em;
            text-transform: uppercase;
            color: var(--accent);
            background: var(--accent-soft);
        }}
        .title {{ margin: 12px 0 8px; font-size: 30px; line-height: 1.1; }}
        .subtitle {{ margin: 0; color: var(--muted); line-height: 1.6; }}
        .manage-list {{ display: grid; gap: 16px; }}
        .manage-item {{
            display: grid;
            gap: 12px;
            padding: 18px;
            border-radius: 20px;
            border: 1px solid rgba(56, 42, 30, 0.1);
            background: rgba(255,255,255,0.54);
        }}
        .manage-form {{ display: grid; gap: 12px; }}
        .manage-label {{ display: grid; gap: 8px; font-weight: 700; }}
        select {{
            width: 100%;
            padding: 12px 14px;
            border-radius: 16px;
            border: 1px solid rgba(56, 42, 30, 0.16);
            background: rgba(255,255,255,0.88);
            font: inherit;
            color: inherit;
        }}
        textarea {{
            width: 100%;
            min-height: 112px;
            resize: vertical;
            padding: 12px 14px;
            border-radius: 16px;
            border: 1px solid rgba(56, 42, 30, 0.16);
            background: rgba(255,255,255,0.88);
            font: inherit;
            color: inherit;
        }}
        .manage-meta {{ color: var(--muted); font-size: 13px; }}
        .manage-actions {{ display: flex; gap: 10px; flex-wrap: wrap; }}
        .primary-button, .secondary-button, .link-button {{
            appearance: none;
            border: none;
            border-radius: 999px;
            padding: 12px 18px;
            font: inherit;
            font-weight: 700;
            cursor: pointer;
            text-decoration: none;
            display: inline-flex;
            align-items: center;
            justify-content: center;
        }}
        .primary-button {{ background: var(--accent); color: #fff; }}
        .secondary-button {{ background: rgba(47, 36, 27, 0.08); color: var(--ink); }}
        .link-button {{ background: rgba(197, 92, 59, 0.12); color: var(--accent); }}
        .toolbar {{ display: flex; gap: 10px; flex-wrap: wrap; margin-top: 14px; }}
        .manage-empty {{ padding: 18px; border-radius: 18px; background: rgba(255,255,255,0.42); color: var(--muted); text-align: center; }}
        @media (max-width: 768px) {{
            body {{ padding: 16px; }}
            .panel-head {{ padding: 18px 18px 14px; }}
            .panel-body {{ padding: 16px 18px 18px; }}
            .title {{ font-size: 26px; }}
        }}
    </style>
</head>
<body>
    <main>
        <section class="panel">
            <div class="panel-head">
                <span class="eyebrow">Message</span>
                <h1 class="title">伝言の編集</h1>
                <p class="subtitle">追加・更新・削除ができます。伝言ごとに1-24時間の有効時間を選べて、更新時は現在時刻から再計算します。</p>
                <div class="toolbar">
                    <a class="link-button" href="/dashboard">ダッシュボードに戻る</a>
                    {session_actions}
                </div>
            </div>
            <div class="panel-body">
                <section class="manage-item">
                    <form method="post" action="/messages/manage" class="manage-form">
                        <input type="hidden" name="action" value="create">
                        <label class="manage-label">新しい伝言
                            <textarea name="message" maxlength="280" placeholder="伝言を入力してください" required></textarea>
                        </label>
                        <label class="manage-label">有効時間
                            <select name="ttl_hours" required>{create_ttl_options}</select>
                        </label>
                        <div class="manage-actions">
                            <button type="submit" class="primary-button">追加する</button>
                        </div>
                    </form>
                </section>
            </div>
        </section>
        <section class="panel">
            <div class="panel-head">
                <span class="eyebrow">Active</span>
                <h2 class="title" style="font-size:26px;">有効な伝言</h2>
            </div>
            <div class="panel-body">
                <div class="manage-list">{items}</div>
            </div>
        </section>
    </main>
</body>
</html>"#,
        create_ttl_options = create_ttl_options,
        session_actions = session_actions,
        items = items,
    )
}

fn render_user_login_page(
    dashboard_title: &str,
    error_message: Option<&str>,
    next_path: &str,
) -> String {
    let error_markup = error_message
        .map(|message| {
            format!(
                r#"<div class="error-banner">{message}</div>"#,
                message = escape_html(message),
            )
        })
        .unwrap_or_default();

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title}</title>
    <style>
        :root {{
            --bg: #f7f1e3;
            --panel: rgba(255, 252, 245, 0.94);
            --line: rgba(56, 42, 30, 0.14);
            --ink: #2f241b;
            --muted: #7a6757;
            --accent: #c55c3b;
            --accent-soft: #efdbc8;
            --error-bg: rgba(197, 92, 59, 0.12);
            --error-line: rgba(197, 92, 59, 0.24);
            --shadow: 0 24px 70px rgba(77, 54, 33, 0.14);
        }}
        * {{ box-sizing: border-box; }}
        body {{
            margin: 0;
            min-height: 100vh;
            display: grid;
            place-items: center;
            padding: 24px;
            font-family: "Hiragino Sans", "Noto Sans JP", "Yu Gothic", sans-serif;
            color: var(--ink);
            background:
                radial-gradient(circle at top left, rgba(255, 214, 167, 0.9), transparent 28%),
                radial-gradient(circle at bottom right, rgba(205, 116, 82, 0.24), transparent 24%),
                linear-gradient(135deg, #fbf4e6 0%, #f4eadf 45%, #efe4d7 100%);
        }}
        .panel {{
            width: min(460px, 100%);
            border: 1px solid var(--line);
            border-radius: 28px;
            background: var(--panel);
            box-shadow: var(--shadow);
            overflow: hidden;
        }}
        .panel-head {{ padding: 24px 26px 18px; border-bottom: 1px solid var(--line); }}
        .panel-body {{ padding: 22px 24px 24px; }}
        .eyebrow {{
            display: inline-flex;
            align-items: center;
            justify-content: center;
            padding: 6px 10px;
            border-radius: 999px;
            font-size: 12px;
            letter-spacing: 0.12em;
            text-transform: uppercase;
            color: var(--accent);
            background: var(--accent-soft);
        }}
        .title {{ margin: 12px 0 8px; font-size: 30px; line-height: 1.1; }}
        .subtitle {{ margin: 0; color: var(--muted); line-height: 1.6; }}
        .login-form {{ display: grid; gap: 14px; }}
        .login-label {{ display: grid; gap: 8px; font-weight: 700; }}
        .login-input {{
            width: 100%;
            padding: 12px 14px;
            border-radius: 16px;
            border: 1px solid rgba(56, 42, 30, 0.16);
            background: rgba(255,255,255,0.88);
            font: inherit;
            color: inherit;
        }}
        .primary-button {{
            appearance: none;
            border: none;
            border-radius: 999px;
            padding: 12px 18px;
            background: var(--accent);
            color: #fff;
            font: inherit;
            font-weight: 700;
            cursor: pointer;
        }}
        .error-banner {{
            margin-bottom: 14px;
            padding: 12px 14px;
            border-radius: 16px;
            border: 1px solid var(--error-line);
            background: var(--error-bg);
            color: var(--accent);
            font-weight: 700;
        }}
    </style>
</head>
<body>
    <main class="panel">
        <section class="panel-head">
            <span class="eyebrow">User</span>
            <h1 class="title">利用者ログイン</h1>
            <p class="subtitle">{dashboard_title} を開くには、利用者アカウントで認証してください。</p>
        </section>
        <section class="panel-body">
            {error_markup}
            <form class="login-form" method="post" action="{login_path}">
                <input type="hidden" name="next" value="{next_path}">
                <label class="login-label">ユーザー名
                    <input class="login-input" type="text" name="username" autocomplete="username" required>
                </label>
                <label class="login-label">パスワード
                    <input class="login-input" type="password" name="password" autocomplete="current-password" required>
                </label>
                <button class="primary-button" type="submit">ログイン</button>
            </form>
        </section>
    </main>
</body>
</html>"#,
        title = escape_html(dashboard_title),
        dashboard_title = escape_html(dashboard_title),
        error_markup = error_markup,
        login_path = USER_LOGIN_PATH,
        next_path = escape_html(next_path),
    )
}

fn render_ttl_option_tags(selected_hours: i64) -> String {
    (MIN_MESSAGE_TTL_HOURS..=MAX_MESSAGE_TTL_HOURS)
        .map(|hours| {
            let selected = if hours == selected_hours { " selected" } else { "" };
            format!(r#"<option value="{hours}"{selected}>{hours}時間</option>"#)
        })
        .collect::<Vec<_>>()
        .join("")
}

fn message_ttl_hours(message: &StoredMessage) -> i64 {
    let ttl_hours = (message.expires_at - message.created_at).num_hours();
    ttl_hours.clamp(MIN_MESSAGE_TTL_HOURS, MAX_MESSAGE_TTL_HOURS)
}

fn render_dashboard_message(
    title: &str,
    heading: &str,
    description: &str,
    action_href: Option<&str>,
    action_label: Option<&str>,
) -> String {
    let action = match (action_href, action_label) {
        (Some(href), Some(label)) => format!(
            r#"<a href="{href}" style="display:inline-flex;padding:14px 18px;border-radius:999px;background:#c55c3b;color:#fff;text-decoration:none;font-weight:700;">{label}</a>"#,
            href = escape_html(href),
            label = escape_html(label),
        ),
        _ => String::new(),
    };

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>{title}</title>
</head>
<body style="margin:0;min-height:100vh;display:grid;place-items:center;background:linear-gradient(135deg,#fbf4e6,#efe4d7);font-family:'Hiragino Sans','Noto Sans JP','Yu Gothic',sans-serif;color:#2f241b;">
    <section style="width:min(720px,92vw);padding:32px;border-radius:28px;background:rgba(255,252,245,0.92);border:1px solid rgba(56,42,30,0.14);box-shadow:0 24px 70px rgba(77,54,33,0.14);">
        <div style="display:inline-block;padding:6px 10px;border-radius:999px;font-size:12px;letter-spacing:0.12em;text-transform:uppercase;color:#c55c3b;background:#efdbc8;">Status</div>
        <h1 style="margin:14px 0 12px;font-size:34px;line-height:1.15;">{heading}</h1>
        <p style="margin:0 0 24px;color:#7a6757;line-height:1.7;white-space:pre-wrap;">{description}</p>
        {action}
    </section>
</body>
</html>"#,
        title = escape_html(title),
        heading = escape_html(heading),
        description = escape_html(description),
        action = action,
    )
}

fn build_dashboard_sections(
    events: &GoogleCalendarEventsResponse,
    messages: &[StoredMessage],
) -> DashboardSections {
    let timezone = parse_calendar_timezone(events.time_zone.as_deref());
    let now = Utc::now().with_timezone(&timezone);
    let today = now.date_naive();
    let tomorrow = today.succ_opt().unwrap_or(today);

    let mut today_events = Vec::new();
    let mut tomorrow_events = Vec::new();
    let mut upcoming_events = Vec::new();

    for event in &events.items {
        if let Some(rendered) = render_event(event, timezone) {
            if rendered.date_label.is_none() && rendered.sort_key == i64::MIN {
                continue;
            }

            let day = rendered_event_day(event, timezone);
            match day {
                Some(day) if day == today => today_events.push(rendered),
                Some(day) if day == tomorrow => tomorrow_events.push(rendered),
                Some(day) if day > tomorrow => upcoming_events.push(rendered),
                _ => {}
            }
        }
    }

    today_events.sort_by_key(|event| event.sort_key);
    tomorrow_events.sort_by_key(|event| event.sort_key);
    upcoming_events.sort_by_key(|event| event.sort_key);

    DashboardSections {
        today_label: format_date_header(today),
        tomorrow_label: format_date_header(tomorrow),
        messages: messages
            .iter()
            .map(|message| DashboardMessage {
                message: message.message.clone(),
                registered_at_label: format_registered_at(
                    message.created_at.with_timezone(&timezone),
                ),
            })
            .collect(),
        today_events,
        tomorrow_events,
        upcoming_events,
    }
}

fn rendered_event_day(event: &GoogleCalendarEvent, timezone: Tz) -> Option<NaiveDate> {
    if let Some(start) = &event.start {
        if let Some(date_time) = &start.date_time {
            let parsed = DateTime::parse_from_rfc3339(date_time).ok()?;
            return Some(parsed.with_timezone(&timezone).date_naive());
        }

        if let Some(date) = &start.date {
            return NaiveDate::parse_from_str(date, "%Y-%m-%d").ok();
        }
    }

    None
}

fn render_event(event: &GoogleCalendarEvent, timezone: Tz) -> Option<DashboardEvent> {
    let title = event
        .summary
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "無題の予定".to_string());

    if let Some(start) = &event.start {
        if let Some(date_time) = &start.date_time {
            let start_dt = DateTime::parse_from_rfc3339(date_time)
                .ok()?
                .with_timezone(&timezone);
            let (time_label, sort_key) = match event
                .end
                .as_ref()
                .and_then(|end| end.date_time.as_ref())
                .and_then(|end| DateTime::parse_from_rfc3339(end).ok())
            {
                Some(end_dt) => (
                    format!(
                        "{:02}:{:02} - {:02}:{:02}",
                        start_dt.hour(),
                        start_dt.minute(),
                        end_dt.with_timezone(&timezone).hour(),
                        end_dt.with_timezone(&timezone).minute()
                    ),
                    start_dt.timestamp(),
                ),
                None => (
                    format!("{:02}:{:02}", start_dt.hour(), start_dt.minute()),
                    start_dt.timestamp(),
                ),
            };

            return Some(DashboardEvent {
                time_label,
                date_label: Some(format_compact_date(start_dt.date_naive())),
                title,
                sort_key,
            });
        }

        if let Some(date) = &start.date {
            let parsed_date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
            let sort_key = parsed_date
                .and_hms_opt(0, 0, 0)
                .map(|value| value.and_utc().timestamp())
                .unwrap_or(i64::MIN + i64::from(parsed_date.num_days_from_ce()));

            return Some(DashboardEvent {
                time_label: "終日".to_string(),
                date_label: Some(format_compact_date(parsed_date)),
                title,
                sort_key,
            });
        }
    }

    None
}

fn render_primary_event_cards(events: &[DashboardEvent], empty_message: &str) -> String {
    if events.is_empty() {
        return format!(r#"<div class="empty">{}</div>"#, escape_html(empty_message));
    }

    let count = events.len();
    let cards = events
        .iter()
        .map(|event| {
            let title = escape_html(&event.title);
            let time = escape_html(&event.time_label);

            format!(
                r#"<article class="primary-card"><div class="primary-time">{time}</div><div class="primary-title">{title}</div></article>"#,
                time = time,
                title = title,
            )
        })
        .collect::<Vec<_>>()
        .join("");

    let count_offset = count.saturating_sub(1);
    let time_size = (34_i32 - (count_offset as i32 * 2)).max(18);
    let title_size = (28_i32 - (count_offset as i32 * 2)).max(16);
    let padding_y = (10_i32 - (count_offset as i32)).max(4);
    let padding_x = (8_i32 - (count_offset as i32 / 2)).max(2);
    let gap = (16_i32 - (count_offset as i32 * 2)).max(4);
    let title_lines = if count >= 4 { 1 } else { 2 };

    format!(
        r#"<div class="primary-list js-fit-primary-list" style="--item-count:{count};--time-size:{time_size}px;--title-size:{title_size}px;--card-padding-y:{padding_y}px;--card-padding-x:{padding_x}px;--card-gap:{gap}px;--title-lines:{title_lines};">{cards}</div>"#,
        count = count,
        time_size = time_size,
        title_size = title_size,
        padding_y = padding_y,
        padding_x = padding_x,
        gap = gap,
        title_lines = title_lines,
        cards = cards,
    )
}

fn render_message_cards(messages: &[DashboardMessage], empty_message: &str) -> String {
    if messages.is_empty() {
        return format!(r#"<div class="empty">{}</div>"#, escape_html(empty_message));
    }

    messages
        .iter()
        .map(|message| {
            format!(
                r#"<article class="message-card"><div class="message-body">{body}</div><div class="message-registered">登録日時: {registered_at}</div></article>"#,
                body = escape_html(&message.message),
                registered_at = escape_html(&message.registered_at_label),
            )
        })
        .collect::<Vec<_>>()
        .join("")
}

fn render_upcoming_event_rows(events: &[DashboardEvent], empty_message: &str) -> String {
    if events.is_empty() {
        return format!(r#"<div class="empty">{}</div>"#, escape_html(empty_message));
    }

    let count = events.len();
    let count_offset = count.saturating_sub(1);
    let meta_size = (14_i32 - (count_offset as i32 / 2)).max(10);
    let title_size = (18_i32 - (count_offset as i32 / 2)).max(12);
    let padding_y = (8_i32 - (count_offset as i32 / 2)).max(2);
    let row_gap = (10_i32 - (count_offset as i32 / 2)).max(2);
    let column_gap = (14_i32 - (count_offset as i32 / 2)).max(6);
    let title_lines = if count >= 7 { 1 } else { 2 };

    let rows = events
        .iter()
        .map(|event| {
            let date_label = escape_html(event.date_label.as_deref().unwrap_or("-"));
            let time_label = escape_html(&event.time_label);
            let title = escape_html(&event.title);

            format!(
            r#"<article class="upcoming-row"><div class="upcoming-date">{date}</div><div class="upcoming-time">{time}</div><div class="upcoming-title">{title}</div></article>"#,
            date = date_label,
            time = time_label,
            title = title,
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!(
        r#"<div class="upcoming-list fit-list js-fit-upcoming-list" style="--upcoming-meta-size:{meta_size}px;--upcoming-title-size:{title_size}px;--upcoming-padding-y:{padding_y}px;--upcoming-row-gap:{row_gap}px;--upcoming-gap:{column_gap}px;--upcoming-title-lines:{title_lines};">{rows}</div>"#,
        meta_size = meta_size,
        title_size = title_size,
        padding_y = padding_y,
        row_gap = row_gap,
        column_gap = column_gap,
        title_lines = title_lines,
        rows = rows,
    )
}

fn parse_calendar_timezone(value: Option<&str>) -> Tz {
    value
        .and_then(|time_zone| time_zone.parse::<Tz>().ok())
        .unwrap_or(chrono_tz::Asia::Tokyo)
}

fn read_optional_trimmed_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_bool_env(name: &str, value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(anyhow!("{name} must be one of true/false/1/0/yes/no/on/off")),
    }
}

async fn has_valid_user_session(state: &AppState, headers: &HeaderMap) -> bool {
    let Some(session_id) = extract_cookie_value(headers, USER_SESSION_COOKIE_NAME) else {
        return false;
    };

    state.user_sessions.lock().await.contains(&session_id)
}

fn extract_cookie_value(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    let cookie_header = headers.get(header::COOKIE)?.to_str().ok()?;
    cookie_header.split(';').find_map(|entry| {
        let (name, value) = entry.trim().split_once('=')?;
        if name == cookie_name {
            Some(value.to_string())
        } else {
            None
        }
    })
}

fn build_user_session_cookie(config: &Config, session_id: &str) -> String {
    let secure = config
        .user_auth
        .as_ref()
        .filter(|auth| auth.cookie_secure)
        .map(|_| "; Secure")
        .unwrap_or("");

    format!(
        "{name}={value}; Path=/; HttpOnly; SameSite=Lax{secure}",
        name = USER_SESSION_COOKIE_NAME,
        value = session_id,
        secure = secure,
    )
}

fn clear_user_session_cookie() -> String {
    format!(
        "{name}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT",
        name = USER_SESSION_COOKIE_NAME,
    )
}

fn is_browser_route(path: &str) -> bool {
    matches!(path, "/" | "/dashboard" | "/messages/manage" | "/auth/login")
}

fn sanitize_next_path(candidate: Option<&str>) -> String {
    let Some(candidate) = candidate.map(str::trim).filter(|value| !value.is_empty()) else {
        return "/dashboard".to_string();
    };

    if !candidate.starts_with('/')
        || candidate.starts_with("//")
        || candidate.starts_with(USER_LOGIN_PATH)
    {
        return "/dashboard".to_string();
    }

    candidate.to_string()
}

fn format_date_header(date: NaiveDate) -> String {
    format!(
        "{}年{}月{}日 ({})",
        date.year(),
        date.month(),
        date.day(),
        format_weekday_ja(date)
    )
}

fn format_compact_date(date: NaiveDate) -> String {
    format!(
        "{}/{}({})",
        date.month(),
        date.day(),
        format_weekday_ja(date)
    )
}

fn format_registered_at(value: DateTime<Tz>) -> String {
    format!(
        "{}年{}月{}日 {:02}:{:02}",
        value.year(),
        value.month(),
        value.day(),
        value.hour(),
        value.minute()
    )
}

fn derive_message_db_path(token_store_path: &str) -> String {
    let token_path = Path::new(token_store_path);
    match token_path.parent() {
        Some(parent) => parent
            .join("dashboard.sqlite3")
            .to_string_lossy()
            .into_owned(),
        None => "./data/dashboard.sqlite3".to_string(),
    }
}

fn resolve_dashboard_max_results(candidate: Option<u32>, default_value: u32) -> u32 {
    let default_value = normalize_dashboard_max_results(default_value);
    candidate
        .filter(|value| is_allowed_dashboard_max_results(*value))
        .unwrap_or(default_value)
}

fn normalize_dashboard_max_results(value: u32) -> u32 {
    if is_allowed_dashboard_max_results(value) {
        value
    } else {
        DEFAULT_DASHBOARD_MAX_RESULTS
    }
}

fn is_allowed_dashboard_max_results(value: u32) -> bool {
    (DEFAULT_DASHBOARD_MAX_RESULTS..=DASHBOARD_MAX_RESULTS_LIMIT)
        .step_by(DASHBOARD_MAX_RESULTS_STEP as usize)
        .any(|allowed| allowed == value)
}

fn render_dashboard_max_results_options(selected_value: u32) -> String {
    let selected_value = normalize_dashboard_max_results(selected_value);
    (DEFAULT_DASHBOARD_MAX_RESULTS..=DASHBOARD_MAX_RESULTS_LIMIT)
        .step_by(DASHBOARD_MAX_RESULTS_STEP as usize)
        .map(|value| {
            let selected = if value == selected_value { " selected" } else { "" };
            format!(r#"<option value="{value}"{selected}>{value}件</option>"#)
        })
        .collect::<Vec<_>>()
        .join("")
}

fn resolve_message_ttl_hours(ttl_hours: Option<i64>) -> Result<i64, AppError> {
    let ttl_hours = ttl_hours.unwrap_or(DEFAULT_MESSAGE_TTL_HOURS);
    if !(MIN_MESSAGE_TTL_HOURS..=MAX_MESSAGE_TTL_HOURS).contains(&ttl_hours) {
        return Err(AppError::bad_request(format!(
            "ttl_hours must be between {} and {}",
            MIN_MESSAGE_TTL_HOURS, MAX_MESSAGE_TTL_HOURS
        )));
    }

    Ok(ttl_hours)
}

fn parse_message_ttl_hours_form_value(ttl_hours: Option<String>) -> Result<i64, AppError> {
    let parsed = ttl_hours
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                AppError::bad_request(format!(
                    "ttl_hours must be between {} and {}",
                    MIN_MESSAGE_TTL_HOURS, MAX_MESSAGE_TTL_HOURS
                ))
            })
        })
        .transpose()?;

    resolve_message_ttl_hours(parsed)
}

async fn load_active_messages(state: &AppState) -> anyhow::Result<Vec<StoredMessage>> {
    load_active_messages_from_db(&state.config.message_db_path).await
}

async fn init_message_db(message_db_path: &str) -> anyhow::Result<()> {
    let db_path = message_db_path.to_string();
    task::spawn_blocking(move || -> anyhow::Result<()> {
        let connection = open_message_db(&db_path)?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS messages (
                id TEXT PRIMARY KEY,
                message TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                expires_at_ms INTEGER NOT NULL
            );",
        )?;
        Ok(())
    })
    .await
    .context("failed to join message db initialization task")??;

    Ok(())
}

async fn insert_message(message_db_path: &str, message: &StoredMessage) -> anyhow::Result<()> {
    let db_path = message_db_path.to_string();
    let message = message.clone();
    task::spawn_blocking(move || -> anyhow::Result<()> {
        let connection = open_message_db(&db_path)?;
        prune_expired_messages_in_db(&connection, Utc::now().timestamp_millis())?;
        connection.execute(
            "INSERT INTO messages (id, message, created_at_ms, expires_at_ms) VALUES (?1, ?2, ?3, ?4)",
            params![
                message.id,
                message.message,
                message.created_at.timestamp_millis(),
                message.expires_at.timestamp_millis(),
            ],
        )?;
        Ok(())
    })
    .await
    .context("failed to join message insert task")??;

    Ok(())
}

async fn update_message(
    message_db_path: &str,
    id: &str,
    message: &str,
    ttl_hours: i64,
) -> anyhow::Result<()> {
    let db_path = message_db_path.to_string();
    let message_id = id.to_string();
    let updated_message = message.to_string();
    task::spawn_blocking(move || -> anyhow::Result<()> {
        let connection = open_message_db(&db_path)?;
        let now = Utc::now();
        prune_expired_messages_in_db(&connection, now.timestamp_millis())?;
        let updated_rows = connection.execute(
            "UPDATE messages SET message = ?1, created_at_ms = ?2, expires_at_ms = ?3 WHERE id = ?4",
            params![
                updated_message,
                now.timestamp_millis(),
                (now + Duration::hours(ttl_hours)).timestamp_millis(),
                message_id,
            ],
        )?;

        if updated_rows == 0 {
            return Err(anyhow!("message not found: {}", message_id));
        }

        Ok(())
    })
    .await
    .context("failed to join message update task")??;

    Ok(())
}

async fn delete_message(message_db_path: &str, id: &str) -> anyhow::Result<()> {
    let db_path = message_db_path.to_string();
    let message_id = id.to_string();
    task::spawn_blocking(move || -> anyhow::Result<()> {
        let connection = open_message_db(&db_path)?;
        let deleted_rows = connection.execute(
            "DELETE FROM messages WHERE id = ?1",
            params![message_id],
        )?;

        if deleted_rows == 0 {
            return Err(anyhow!("message not found: {}", message_id));
        }

        Ok(())
    })
    .await
    .context("failed to join message delete task")??;

    Ok(())
}

async fn load_active_messages_from_db(message_db_path: &str) -> anyhow::Result<Vec<StoredMessage>> {
    let db_path = message_db_path.to_string();
    task::spawn_blocking(move || -> anyhow::Result<Vec<StoredMessage>> {
        let connection = open_message_db(&db_path)?;
        let now_ms = Utc::now().timestamp_millis();
        prune_expired_messages_in_db(&connection, now_ms)?;

        let mut statement = connection.prepare(
            "SELECT id, message, created_at_ms, expires_at_ms
             FROM messages
             WHERE expires_at_ms > ?1
             ORDER BY created_at_ms DESC",
        )?;
        let mut rows = statement.query(params![now_ms])?;
        let mut messages = Vec::new();

        while let Some(row) = rows.next()? {
            messages.push(StoredMessage {
                id: row.get(0)?,
                message: row.get(1)?,
                created_at: millis_to_utc(row.get::<_, i64>(2)?)?,
                expires_at: millis_to_utc(row.get::<_, i64>(3)?)?,
            });
        }

        Ok(messages)
    })
    .await
    .context("failed to join message load task")?
}

fn open_message_db(message_db_path: &str) -> anyhow::Result<Connection> {
    if let Some(parent) = Path::new(message_db_path).parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("failed to create message db directory: {}", parent.display())
        })?;
    }

    Connection::open(message_db_path)
        .with_context(|| format!("failed to open sqlite database: {message_db_path}"))
}

fn prune_expired_messages_in_db(connection: &Connection, now_ms: i64) -> anyhow::Result<()> {
    connection.execute(
        "DELETE FROM messages WHERE expires_at_ms <= ?1",
        params![now_ms],
    )?;
    Ok(())
}

fn millis_to_utc(value: i64) -> anyhow::Result<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp_millis(value)
        .ok_or_else(|| anyhow!("invalid sqlite timestamp millis: {value}"))
}

fn format_weekday_ja(date: NaiveDate) -> &'static str {
    match date.weekday().num_days_from_monday() {
        0 => "月",
        1 => "火",
        2 => "水",
        3 => "木",
        4 => "金",
        5 => "土",
        _ => "日",
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
