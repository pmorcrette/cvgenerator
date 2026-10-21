//! End-to-end tests of the HTTP router: pages, REST API, MCP endpoint and auth.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use clap::Parser;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use cvgenerator::config::Cli;
use cvgenerator::storage::Store;
use cvgenerator::web::{AppState, router};

fn app(token: Option<&str>) -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut args = vec!["cvgenerator", "--data-dir", dir.path().to_str().unwrap()];
    if let Some(token) = token {
        args.extend(["--api-token", token]);
    }
    let config = Cli::parse_from(args).config;
    let store = Store::open(&config.data_dir).unwrap();
    (router(AppState::new(store, config)), dir)
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    token: Option<&str>,
) -> (StatusCode, String) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let req = match body {
        Some(body) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string())),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap()
}

#[tokio::test]
async fn rest_api_and_pages() {
    let (app, _dir) = app(None);

    let (status, body) = send(&app, "POST", "/api/cvs", Some(json!({ "resume": { "basics": { "name": "Linus Torvalds", "label": "Kernel hacker" } } })), None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(json(&body)["id"], "linus-torvalds");

    let (status, body) = send(
        &app,
        "PATCH",
        "/api/cvs/linus-torvalds",
        Some(
            json!({ "theme": "midnight", "resume": { "work": [{ "name": "Linux Foundation" }] } }),
        ),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["theme"], "midnight");

    let (status, html) = send(&app, "GET", "/cv/linus-torvalds", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("Linus Torvalds") && html.contains("Linux Foundation"));
    assert!(html.contains("layout-sidebar"));
    // The theme picker submits a GET form to the CV page with `?theme=`.
    assert!(
        html.contains(r#"<form class="toolbar-theme" method="get" action="/cv/linus-torvalds">"#)
    );
    assert!(html.contains(r#"<select name="theme""#));

    let (_, html) = send(&app, "GET", "/cv/linus-torvalds?theme=forest", None, None).await;
    assert!(html.contains("layout-banner"));

    let (status, body) = send(&app, "GET", "/cv/linus-torvalds/resume.json", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["basics"]["label"], "Kernel hacker");

    let (status, _) = send(
        &app,
        "POST",
        "/api/cvs/linus-torvalds/duplicate",
        Some(json!({ "id": "linus-2" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (_, body) = send(&app, "GET", "/api/cvs", None, None).await;
    assert_eq!(json(&body).as_array().unwrap().len(), 2);

    let (status, _) = send(
        &app,
        "PATCH",
        "/api/cvs/linus-2",
        Some(json!({ "theme": "does-not-exist" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = send(
        &app,
        "PUT",
        "/api/cvs/linus-2",
        Some(json!({ "resume": [] })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = send(&app, "DELETE", "/api/cvs/linus-2", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = send(&app, "GET", "/cv/linus-2", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    for page in [
        "/admin",
        "/admin/themes",
        "/admin/cvs/linus-torvalds",
        "/static/cv.css",
        "/static/admin.js",
        "/healthz",
    ] {
        let (status, _) = send(&app, "GET", page, None, None).await;
        assert_eq!(status, StatusCode::OK, "{page}");
    }
}

#[tokio::test]
async fn layouts() {
    let (app, _dir) = app(None);
    let (status, body) = send(
        &app,
        "POST",
        "/api/cvs",
        Some(json!({ "id": "ada", "theme": "aurora", "layout": "academic" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(json(&body)["layout"], "academic");

    // The CV layout overrides the theme's one, `?layout=` overrides both.
    let (_, html) = send(&app, "GET", "/cv/ada", None, None).await;
    assert!(html.contains(r#"class="layout-academic"#));
    let (_, html) = send(&app, "GET", "/cv/ada?layout=cards", None, None).await;
    assert!(html.contains(r#"class="layout-cards"#));
    // The toolbar form sends an empty value for "default".
    let (status, html) = send(&app, "GET", "/cv/ada?theme=slate&layout=", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains(r#"class="layout-academic"#));
    let (status, _) = send(&app, "GET", "/cv/ada?layout=zigzag", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Duplicates keep the layout.
    let (_, body) = send(&app, "POST", "/api/cvs/ada/duplicate", None, None).await;
    assert_eq!(json(&body)["layout"], "academic");

    // PATCH: absent keeps the layout, null resets it to the theme's.
    let (_, body) = send(
        &app,
        "PATCH",
        "/api/cvs/ada",
        Some(json!({ "theme": "slate" })),
        None,
    )
    .await;
    assert_eq!(json(&body)["layout"], "academic");
    let (_, body) = send(
        &app,
        "PATCH",
        "/api/cvs/ada",
        Some(json!({ "layout": null })),
        None,
    )
    .await;
    assert!(json(&body).get("layout").is_none());
    let (_, html) = send(&app, "GET", "/cv/ada", None, None).await;
    assert!(html.contains(r#"class="layout-profile"#));

    let (status, body) = send(
        &app,
        "PUT",
        "/api/cvs/ada",
        Some(json!({ "resume": {}, "layout": "compact" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(json(&body)["layout"], "compact");

    let (_, html) = send(
        &app,
        "POST",
        "/api/preview",
        Some(json!({ "layout": "profile" })),
        None,
    )
    .await;
    assert!(html.contains(r#"class="layout-profile"#));
}

#[tokio::test]
async fn themes_and_preview() {
    let (app, _dir) = app(None);
    let theme = json!({ "id": "sunset", "name": "Sunset", "layout": "banner", "colors": { "accent": "#f97316" } });
    let (status, body) = send(&app, "POST", "/api/themes", Some(theme.clone()), None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, _) = send(&app, "POST", "/api/themes", Some(theme), None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &app,
        "PUT",
        "/api/themes/aurora",
        Some(json!({ "id": "aurora", "name": "Hacked" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, html) = send(
        &app,
        "POST",
        "/api/preview",
        Some(json!({ "theme": "sunset" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("#f97316") && html.contains("Jane Doe") && !html.contains("toolbar-btn"));

    let (status, _) = send(
        &app,
        "POST",
        "/api/preview",
        Some(json!({ "themeData": { "id": "x", "name": "X", "customCss": "</style><script>" } })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    let (status, _) = send(&app, "DELETE", "/api/themes/sunset", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn token_protects_api_and_mcp_but_not_public_pages() {
    let (app, _dir) = app(Some("s3cret"));
    let (status, _) = send(
        &app,
        "POST",
        "/api/cvs",
        Some(json!({ "id": "me" })),
        Some("s3cret"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = send(&app, "GET", "/api/cvs", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(&app, "GET", "/api/cvs", None, Some("wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = send(
        &app,
        "POST",
        "/mcp",
        Some(json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = send(&app, "GET", "/cv/me", None, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(&app, "GET", "/admin", None, None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn mcp_over_http() {
    let (app, _dir) = app(Some("tok"));
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } } });
    let (status, body) = send(&app, "POST", "/mcp", Some(init), Some("tok")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(json(&body)["result"]["serverInfo"]["name"], "cvgenerator");

    let (status, _) = send(
        &app,
        "POST",
        "/mcp",
        Some(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })),
        Some("tok"),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    let call = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "create_cv", "arguments": { "resume": { "basics": { "name": "Agent Smith" } } } } });
    let (_, body) = send(&app, "POST", "/mcp", Some(call), Some("tok")).await;
    assert_eq!(
        json(&body)["result"]["structuredContent"]["id"],
        "agent-smith"
    );

    let (status, _) = send(&app, "GET", "/mcp", None, Some("tok")).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
}
