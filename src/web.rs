//! HTTP server: public CV pages, the admin UI, the REST API and the MCP endpoint.

use std::sync::Arc;

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use minijinja::context;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::model::{CvDocument, CvSummary, Resume, double_option};
use crate::render::{CvPageOptions, Renderer, ThemeChoice};
use crate::service::{parse_resume, patch_resume};
use crate::storage::Store;
use crate::theme::{Layout, Theme};

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub renderer: Arc<Renderer>,
    pub config: Arc<Config>,
}

impl AppState {
    pub fn new(store: Store, config: Config) -> Self {
        Self {
            store: Arc::new(store),
            renderer: Arc::new(Renderer::new()),
            config: Arc::new(config),
        }
    }

    /// Render a CV, optionally with another theme than its own.
    pub fn render_cv(
        &self,
        doc: &CvDocument,
        theme: Option<&str>,
        layout: Option<Layout>,
        embed: bool,
    ) -> Result<String> {
        let theme = match theme {
            Some(id) => self.store.get_theme(id)?,
            None => self
                .store
                .get_theme(&doc.theme)
                .or_else(|_| self.store.get_theme(&crate::model::default_theme()))?,
        };
        let themes = self
            .store
            .list_themes()?
            .into_iter()
            .map(|t| ThemeChoice {
                id: t.id,
                name: t.name,
            })
            .collect();
        let options = CvPageOptions {
            embed,
            themes,
            layout,
        };
        self.renderer.render_cv(doc, &theme, &options)
    }
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/cvs", get(list_cvs).post(create_cv))
        .route(
            "/cvs/{id}",
            get(get_cv)
                .put(replace_cv)
                .patch(patch_cv)
                .delete(delete_cv),
        )
        .route("/cvs/{id}/duplicate", post(duplicate_cv))
        .route("/themes", get(list_themes).post(create_theme))
        .route(
            "/themes/{id}",
            get(get_theme).put(put_theme).delete(delete_theme),
        )
        .route("/preview", post(preview))
        .route("/auth", get(|| async { StatusCode::NO_CONTENT }));

    let protected = Router::new()
        .nest("/api", api)
        .route(
            "/mcp",
            post(crate::mcp::http_handler).get(mcp_get).delete(mcp_get),
        )
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));

    Router::new()
        .route("/", get(home))
        .route("/healthz", get(|| async { "ok" }))
        .route("/cv/{id}", get(cv_page))
        .route("/cv/{id}/resume.json", get(cv_json))
        .route("/admin", get(admin_index))
        .route("/admin/cvs/{id}", get(admin_editor))
        .route("/admin/themes", get(admin_themes))
        .route("/static/{file}", get(static_file))
        .merge(protected)
        .with_state(state)
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

// ----- Authentication ---------------------------------------------------

async fn require_token(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.config.api_token() else {
        return next.run(req).await;
    };
    let headers = req.headers();
    let provided = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()));
    match provided {
        Some(token) if constant_time_eq(token.trim().as_bytes(), expected.as_bytes()) => {
            next.run(req).await
        }
        _ => {
            let mut res = (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "missing or invalid API token" })),
            )
                .into_response();
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            res
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn mcp_get() -> Response {
    // This server answers each POST with a single JSON response and never
    // opens a server-initiated SSE stream (MCP Streamable HTTP transport).
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")]).into_response()
}

// ----- Public pages -----------------------------------------------------

#[derive(Deserialize)]
struct PageQuery {
    theme: Option<String>,
    /// Layout id; empty means the CV's default (sent by the toolbar form).
    layout: Option<String>,
    embed: Option<bool>,
}

impl PageQuery {
    fn layout(&self) -> Result<Option<Layout>> {
        match self.layout.as_deref() {
            None | Some("") => Ok(None),
            Some(id) => Layout::parse(id)
                .map(Some)
                .ok_or_else(|| Error::NotFound(format!("layout '{id}'"))),
        }
    }
}

async fn home(State(state): State<AppState>, query: Query<PageQuery>) -> Result<Response> {
    match state.config.home.clone() {
        Some(id) => cv_page(State(state), Path(id), query).await,
        None => Ok(Redirect::temporary("/admin").into_response()),
    }
}

async fn cv_page(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PageQuery>,
) -> Result<Response> {
    let doc = state.store.get_cv(&id)?;
    let html = state.render_cv(
        &doc,
        query.theme.as_deref(),
        query.layout()?,
        query.embed.unwrap_or(false),
    )?;
    Ok(Html(html).into_response())
}

async fn cv_json(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Resume>> {
    Ok(Json(state.store.get_cv(&id)?.resume))
}

// ----- Admin UI (data is loaded client side through the API) -----------

async fn admin_index(State(state): State<AppState>) -> Result<Html<String>> {
    admin_page(&state, "admin_index.html", context! { page => "cvs" })
}

async fn admin_editor(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Html<String>> {
    admin_page(
        &state,
        "admin_editor.html",
        context! { page => "editor", id },
    )
}

async fn admin_themes(State(state): State<AppState>) -> Result<Html<String>> {
    admin_page(&state, "admin_themes.html", context! { page => "themes" })
}

fn admin_page(state: &AppState, template: &str, ctx: minijinja::Value) -> Result<Html<String>> {
    let layouts: Vec<_> = Layout::ALL
        .iter()
        .map(|l| context! { id => l.id(), name => l.name(), description => l.description() })
        .collect();
    let ctx = context! {
        auth_required => state.config.api_token().is_some(),
        layouts,
        ..ctx
    };
    Ok(Html(state.renderer.render_admin(template, ctx)?))
}

async fn static_file(Path(file): Path<String>) -> Response {
    let (body, mime): (&'static str, &str) = match file.as_str() {
        "cv.css" => (include_str!("../static/cv.css"), "text/css"),
        "admin.css" => (include_str!("../static/admin.css"), "text/css"),
        "admin.js" => (include_str!("../static/admin.js"), "text/javascript"),
        "favicon.svg" => (include_str!("../static/favicon.svg"), "image/svg+xml"),
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&format!("{mime}; charset=utf-8")).unwrap(),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    (headers, body).into_response()
}

// ----- REST API: CVs ----------------------------------------------------

async fn list_cvs(State(state): State<AppState>) -> Result<Json<Vec<CvSummary>>> {
    Ok(Json(state.store.list_cvs()?))
}

#[derive(Deserialize)]
struct CreateCv {
    id: Option<String>,
    theme: Option<String>,
    layout: Option<Layout>,
    #[serde(default)]
    resume: Option<Value>,
}

async fn create_cv(
    State(state): State<AppState>,
    Json(body): Json<CreateCv>,
) -> Result<(StatusCode, Json<CvDocument>)> {
    let resume = parse_resume(body.resume.unwrap_or_else(|| json!({})))?;
    let doc = state
        .store
        .create_cv(body.id, resume, body.theme, body.layout)?;
    Ok((StatusCode::CREATED, Json(doc)))
}

async fn get_cv(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<CvDocument>> {
    Ok(Json(state.store.get_cv(&id)?))
}

#[derive(Deserialize)]
struct ReplaceCv {
    theme: Option<String>,
    /// Absent: unchanged, `null`: use the theme's layout.
    #[serde(default, deserialize_with = "double_option")]
    layout: Option<Option<Layout>>,
    resume: Value,
}

async fn replace_cv(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ReplaceCv>,
) -> Result<Json<CvDocument>> {
    let resume = parse_resume(body.resume)?;
    let doc = state.store.update_cv(&id, |doc| {
        doc.resume = resume;
        if let Some(theme) = body.theme {
            doc.theme = theme;
        }
        if let Some(layout) = body.layout {
            doc.layout = layout;
        }
        Ok(())
    })?;
    Ok(Json(doc))
}

#[derive(Deserialize)]
struct PatchCv {
    theme: Option<String>,
    /// Absent: unchanged, `null`: use the theme's layout.
    #[serde(default, deserialize_with = "double_option")]
    layout: Option<Option<Layout>>,
    /// JSON Merge Patch (RFC 7396) applied to the resume.
    resume: Option<Value>,
}

async fn patch_cv(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<PatchCv>,
) -> Result<Json<CvDocument>> {
    let doc = state.store.update_cv(&id, |doc| {
        if let Some(patch) = &body.resume {
            patch_resume(&mut doc.resume, patch)?;
        }
        if let Some(theme) = body.theme {
            doc.theme = theme;
        }
        if let Some(layout) = body.layout {
            doc.layout = layout;
        }
        Ok(())
    })?;
    Ok(Json(doc))
}

async fn delete_cv(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.store.delete_cv(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize, Default)]
struct DuplicateCv {
    id: Option<String>,
}

async fn duplicate_cv(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<Json<DuplicateCv>>,
) -> Result<(StatusCode, Json<CvDocument>)> {
    let source = state.store.get_cv(&id)?;
    let new_id = body
        .and_then(|b| b.0.id)
        .or_else(|| Some(format!("{id}-copy")));
    let doc = state
        .store
        .create_cv(new_id, source.resume, Some(source.theme), source.layout)?;
    Ok((StatusCode::CREATED, Json(doc)))
}

// ----- REST API: themes -------------------------------------------------

async fn list_themes(State(state): State<AppState>) -> Result<Json<Vec<Theme>>> {
    Ok(Json(state.store.list_themes()?))
}

async fn get_theme(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<Theme>> {
    Ok(Json(state.store.get_theme(&id)?))
}

async fn create_theme(
    State(state): State<AppState>,
    Json(theme): Json<Theme>,
) -> Result<(StatusCode, Json<Theme>)> {
    if state.store.get_theme(&theme.id).is_ok() {
        return Err(Error::Conflict(format!("theme '{}'", theme.id)));
    }
    state.store.save_theme(theme.clone())?;
    Ok((StatusCode::CREATED, Json(state.store.get_theme(&theme.id)?)))
}

async fn put_theme(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut theme): Json<Theme>,
) -> Result<(StatusCode, Json<Theme>)> {
    theme.id = id.clone();
    let created = state.store.save_theme(theme)?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(state.store.get_theme(&id)?)))
}

async fn delete_theme(State(state): State<AppState>, Path(id): Path<String>) -> Result<StatusCode> {
    state.store.delete_theme(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ----- Live preview -----------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewRequest {
    /// Resume to render; defaults to the bundled example.
    resume: Option<Value>,
    /// Theme id to use.
    theme: Option<String>,
    /// Unsaved theme definition (takes precedence over `theme`).
    theme_data: Option<Theme>,
    /// Layout overriding the theme's one.
    layout: Option<Layout>,
}

async fn preview(
    State(state): State<AppState>,
    Json(req): Json<PreviewRequest>,
) -> Result<Html<String>> {
    let resume = match req.resume {
        Some(value) => parse_resume(value)?,
        None => example_resume(),
    };
    let theme = match (req.theme_data, req.theme) {
        (Some(theme), _) => {
            theme.validate().map_err(Error::Invalid)?;
            theme
        }
        (None, Some(id)) => state.store.get_theme(&id)?,
        (None, None) => state.store.get_theme(&crate::model::default_theme())?,
    };
    let now = chrono::Utc::now();
    let doc = CvDocument {
        id: "preview".into(),
        theme: theme.id.clone(),
        layout: req.layout,
        created_at: now,
        updated_at: now,
        resume,
    };
    let options = CvPageOptions {
        embed: true,
        ..Default::default()
    };
    Ok(Html(state.renderer.render_cv(&doc, &theme, &options)?))
}

pub fn example_resume() -> Resume {
    serde_json::from_str(include_str!("../examples/jane-doe.json"))
        .expect("bundled example is valid")
}
