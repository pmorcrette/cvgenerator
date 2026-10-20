//! Model Context Protocol server.
//!
//! Exposes CV and theme management as MCP tools so AI agents can create,
//! tailor and restyle CVs. The protocol core is transport agnostic and is
//! served both over Streamable HTTP (`POST /mcp`) and over stdio
//! (`cvgenerator mcp`).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::error::Error;
use crate::model::{CvDocument, CvSummary, SECTIONS};
use crate::service::{SectionEdit, edit_section, parse_resume, patch_resume};
use crate::theme::Theme;
use crate::web::AppState;

const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Manage curriculum vitae (CVs) hosted by cvgenerator. \
CV content follows the JSON Resume schema (call get_resume_schema once if unsure). \
Prefer targeted edits (patch_cv, add/update/remove/move_section_item) over full rewrites, \
use duplicate_cv to tailor a CV for a specific job without losing the original, \
and share the returned `url` so humans can see the rendered result.";

// ----- JSON-RPC plumbing ------------------------------------------------

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

struct RpcError {
    code: i64,
    message: String,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub struct McpServer {
    state: AppState,
}

impl McpServer {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    /// Handle a raw payload (single message or batch). Returns the
    /// serialized response, or `None` when only notifications were received.
    pub fn handle_payload(&self, payload: &[u8]) -> Option<Value> {
        let message: Value = match serde_json::from_slice(payload) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(
                    Value::Null,
                    RpcError::new(PARSE_ERROR, e.to_string()),
                ));
            }
        };
        match message {
            Value::Array(batch) => {
                let responses: Vec<Value> = batch
                    .into_iter()
                    .filter_map(|m| self.handle_message(m))
                    .collect();
                (!responses.is_empty()).then_some(Value::Array(responses))
            }
            single => self.handle_message(single),
        }
    }

    pub fn handle_message(&self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // Responses from the client (we never send requests) are ignored.
            if message.get("result").is_some() || message.get("error").is_some() {
                return None;
            }
            return Some(error_response(
                id.unwrap_or(Value::Null),
                RpcError::new(INVALID_REQUEST, "missing method"),
            ));
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let Some(id) = id else {
            // Notification (`notifications/initialized`, `notifications/cancelled`...).
            tracing::debug!("MCP notification: {method}");
            return None;
        };
        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_definitions() })),
            "tools/call" => self.call_tool(&params),
            "resources/list" => self.list_resources(),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [{
                "uriTemplate": "cv://{id}",
                "name": "CV",
                "description": "A stored CV document (metadata + JSON Resume content)",
                "mimeType": "application/json"
            }] })),
            "resources/read" => self.read_resource(&params),
            "prompts/list" => Ok(json!({ "prompts": prompt_definitions() })),
            "prompts/get" => self.get_prompt(&params),
            "logging/setLevel" => Ok(json!({})),
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(err) => error_response(id, err),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let version = SUPPORTED_VERSIONS
            .iter()
            .find(|v| **v == requested)
            .unwrap_or(&SUPPORTED_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "listChanged": false, "subscribe": false },
                "prompts": { "listChanged": false },
                "logging": {}
            },
            "serverInfo": {
                "name": "cvgenerator",
                "title": "CV Generator",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": INSTRUCTIONS
        })
    }

    // ----- Tools ------------------------------------------------------------

    fn call_tool(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "missing tool name"))?;
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !tool_definitions().iter().any(|t| t["name"] == name) {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown tool: {name}"),
            ));
        }
        // Tool failures are reported in the result so the model can react.
        Ok(match self.run_tool(name, args) {
            Ok(value) => {
                let text = serde_json::to_string_pretty(&value).unwrap_or_default();
                let mut result =
                    json!({ "content": [{ "type": "text", "text": text }], "isError": false });
                if value.is_object() {
                    result["structuredContent"] = value;
                }
                result
            }
            Err(err) => {
                json!({ "content": [{ "type": "text", "text": err.to_string() }], "isError": true })
            }
        })
    }

    fn run_tool(&self, name: &str, args: Value) -> Result<Value, Error> {
        let store = &self.state.store;
        match name {
            "list_cvs" => {
                let cvs: Vec<Value> = store
                    .list_cvs()?
                    .iter()
                    .map(|s| self.summary_json(s))
                    .collect();
                Ok(json!({ "cvs": cvs }))
            }
            "get_cv" => {
                let a: IdArgs = parse_args(args)?;
                self.doc_json(&store.get_cv(&a.id)?)
            }
            "create_cv" => {
                #[derive(Deserialize)]
                struct A {
                    id: Option<String>,
                    theme: Option<String>,
                    resume: Option<Value>,
                }
                let a: A = parse_args(args)?;
                let resume = parse_resume(a.resume.unwrap_or_else(|| json!({})))?;
                self.doc_json(&store.create_cv(a.id, resume, a.theme)?)
            }
            "update_cv" => {
                #[derive(Deserialize)]
                struct A {
                    id: String,
                    theme: Option<String>,
                    resume: Option<Value>,
                }
                let a: A = parse_args(args)?;
                let resume = a.resume.map(parse_resume).transpose()?;
                self.doc_json(&store.update_cv(&a.id, |doc| {
                    if let Some(resume) = resume {
                        doc.resume = resume;
                    }
                    if let Some(theme) = a.theme {
                        doc.theme = theme;
                    }
                    Ok(())
                })?)
            }
            "patch_cv" => {
                #[derive(Deserialize)]
                struct A {
                    id: String,
                    patch: Value,
                }
                let a: A = parse_args(args)?;
                self.doc_json(
                    &store.update_cv(&a.id, |doc| patch_resume(&mut doc.resume, &a.patch))?,
                )
            }
            "add_section_item"
            | "update_section_item"
            | "remove_section_item"
            | "move_section_item" => {
                #[derive(Deserialize)]
                struct A {
                    id: String,
                    section: String,
                    index: Option<usize>,
                    item: Option<Value>,
                    patch: Option<Value>,
                    from: Option<usize>,
                    to: Option<usize>,
                }
                let a: A = parse_args(args)?;
                let required = |v: Option<usize>, field: &str| {
                    v.ok_or_else(|| Error::Invalid(format!("'{field}' is required")))
                };
                let edit = match name {
                    "add_section_item" => SectionEdit::Add {
                        index: a.index,
                        item: a
                            .item
                            .ok_or_else(|| Error::Invalid("'item' is required".into()))?,
                    },
                    "update_section_item" => SectionEdit::Update {
                        index: required(a.index, "index")?,
                        patch: a
                            .patch
                            .ok_or_else(|| Error::Invalid("'patch' is required".into()))?,
                    },
                    "remove_section_item" => SectionEdit::Remove {
                        index: required(a.index, "index")?,
                    },
                    _ => SectionEdit::Move {
                        from: required(a.from, "from")?,
                        to: required(a.to, "to")?,
                    },
                };
                let section = a.section;
                self.doc_json(
                    &store.update_cv(&a.id, |doc| edit_section(&mut doc.resume, &section, edit))?,
                )
            }
            "duplicate_cv" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct A {
                    id: String,
                    new_id: Option<String>,
                }
                let a: A = parse_args(args)?;
                let source = store.get_cv(&a.id)?;
                let new_id = a.new_id.unwrap_or_else(|| format!("{}-copy", a.id));
                self.doc_json(&store.create_cv(Some(new_id), source.resume, Some(source.theme))?)
            }
            "delete_cv" => {
                let a: IdArgs = parse_args(args)?;
                store.delete_cv(&a.id)?;
                Ok(json!({ "deleted": a.id }))
            }
            "set_cv_theme" => {
                #[derive(Deserialize)]
                struct A {
                    id: String,
                    theme: String,
                }
                let a: A = parse_args(args)?;
                self.doc_json(&store.update_cv(&a.id, |doc| {
                    doc.theme = a.theme;
                    Ok(())
                })?)
            }
            "list_themes" => {
                let themes: Vec<Value> = store
                    .list_themes()?
                    .into_iter()
                    .map(|t| {
                        json!({
                            "id": t.id, "name": t.name, "description": t.description,
                            "layout": t.layout, "builtin": t.builtin, "accent": t.colors.accent
                        })
                    })
                    .collect();
                Ok(json!({ "themes": themes }))
            }
            "get_theme" => {
                let a: IdArgs = parse_args(args)?;
                Ok(serde_json::to_value(store.get_theme(&a.id)?)?)
            }
            "save_theme" => {
                #[derive(Deserialize)]
                struct A {
                    theme: Value,
                    base: Option<String>,
                }
                let a: A = parse_args(args)?;
                // Start from a base theme so agents only need to send what changes.
                let mut value =
                    serde_json::to_value(store.get_theme(a.base.as_deref().unwrap_or("aurora"))?)?;
                crate::model::merge_patch(&mut value, &a.theme);
                let theme: Theme = serde_json::from_value(value)
                    .map_err(|e| Error::Invalid(format!("theme: {e}")))?;
                let id = theme.id.clone();
                let created = store.save_theme(theme)?;
                Ok(json!({ "created": created, "theme": store.get_theme(&id)? }))
            }
            "delete_theme" => {
                let a: IdArgs = parse_args(args)?;
                store.delete_theme(&a.id)?;
                Ok(json!({ "deleted": a.id }))
            }
            "get_resume_schema" => Ok(json!({
                "documentation": RESUME_SCHEMA_DOC,
                "sections": SECTIONS,
                "example": crate::web::example_resume()
            })),
            other => Err(Error::Invalid(format!("unknown tool: {other}"))),
        }
    }

    fn summary_json(&self, summary: &CvSummary) -> Value {
        let mut value = serde_json::to_value(summary).unwrap_or_default();
        value["url"] = json!(self.state.config.cv_url(&summary.id));
        value
    }

    fn doc_json(&self, doc: &CvDocument) -> Result<Value, Error> {
        let mut value = serde_json::to_value(doc)?;
        value["url"] = json!(self.state.config.cv_url(&doc.id));
        Ok(value)
    }

    // ----- Resources --------------------------------------------------------

    fn list_resources(&self) -> Result<Value, RpcError> {
        let cvs = self.state.store.list_cvs().map_err(internal)?;
        let mut resources: Vec<Value> = cvs
            .iter()
            .map(|cv| {
                json!({
                    "uri": format!("cv://{}", cv.id),
                    "name": cv.id,
                    "title": if cv.name.is_empty() { cv.id.clone() } else { format!("{} — {}", cv.name, cv.label) },
                    "mimeType": "application/json"
                })
            })
            .collect();
        resources.push(json!({
            "uri": "cv://schema",
            "name": "schema",
            "title": "CV format (JSON Resume) documentation",
            "mimeType": "text/markdown"
        }));
        Ok(json!({ "resources": resources }))
    }

    fn read_resource(&self, params: &Value) -> Result<Value, RpcError> {
        let uri = params
            .get("uri")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "missing uri"))?;
        let id = uri
            .strip_prefix("cv://")
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, format!("unknown resource: {uri}")))?;
        let (mime, text) = if id == "schema" {
            ("text/markdown", RESUME_SCHEMA_DOC.to_string())
        } else {
            let doc = self
                .state
                .store
                .get_cv(id)
                .map_err(|e| RpcError::new(INVALID_PARAMS, e.to_string()))?;
            (
                "application/json",
                serde_json::to_string_pretty(&self.doc_json(&doc).map_err(internal)?)
                    .unwrap_or_default(),
            )
        };
        Ok(json!({ "contents": [{ "uri": uri, "mimeType": mime, "text": text }] }))
    }

    // ----- Prompts ----------------------------------------------------------

    fn get_prompt(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let arg = |key: &str| {
            args.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let text = match name {
            "tailor_cv" => format!(
                "Tailor the CV '{id}' for the job offer below.\n\n\
                 1. Read it with get_cv.\n\
                 2. Create a copy with duplicate_cv (newId based on the company name) so the original stays intact.\n\
                 3. On the copy, rewrite basics.label and basics.summary for the role, reorder and rephrase work \
                 highlights to emphasize relevant achievements (keep facts truthful, never invent experience), \
                 and put the most relevant skills first.\n\
                 4. Reply with a short summary of the changes and the URL of the new CV.\n\n\
                 Job offer:\n{job}",
                id = arg("id"),
                job = arg("job_description"),
            ),
            "review_cv" => format!(
                "Review the CV '{}' (use get_cv). Check clarity, impact (quantified achievements, action verbs), \
                 consistency of dates and tense, typos and missing sections. Give a prioritized list of \
                 suggestions, then ask whether to apply them.",
                arg("id")
            ),
            other => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    format!("unknown prompt: {other}"),
                ));
            }
        };
        Ok(json!({ "messages": [{ "role": "user", "content": { "type": "text", "text": text } }] }))
    }
}

#[derive(Deserialize)]
struct IdArgs {
    id: String,
}

fn parse_args<T: DeserializeOwned>(args: Value) -> Result<T, Error> {
    serde_json::from_value(args).map_err(|e| Error::Invalid(format!("arguments: {e}")))
}

fn internal(err: Error) -> RpcError {
    RpcError::new(-32603, err.to_string())
}

fn error_response(id: Value, err: RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": err.code, "message": err.message } })
}

// ----- Definitions --------------------------------------------------------

fn tool(
    name: &str,
    title: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    annotations: Value,
) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        "annotations": annotations,
    })
}

fn tool_definitions() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "openWorldHint": false });
    let write = json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false });
    let destructive =
        json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false });
    let id = json!({ "type": "string", "description": "CV identifier (lowercase letters, digits, dashes)" });
    let section =
        json!({ "type": "string", "enum": SECTIONS, "description": "List section of the resume" });
    let resume = json!({ "type": "object", "description": "Resume content in JSON Resume format (see get_resume_schema)" });
    vec![
        tool(
            "list_cvs",
            "List CVs",
            "List all stored CVs with their name, headline, theme, last update and public URL.",
            json!({}),
            &[],
            read_only.clone(),
        ),
        tool(
            "get_cv",
            "Get CV",
            "Get a CV: metadata, theme, public URL and full JSON Resume content.",
            json!({ "id": id }),
            &["id"],
            read_only.clone(),
        ),
        tool(
            "create_cv",
            "Create CV",
            "Create a new CV. The id is derived from basics.name when omitted. Returns the stored document and its URL.",
            json!({ "id": id, "theme": { "type": "string", "description": "Theme id (see list_themes), default 'aurora'" }, "resume": resume }),
            &[],
            write.clone(),
        ),
        tool(
            "update_cv",
            "Replace CV content",
            "Replace the whole resume content and/or change the theme of a CV. Prefer patch_cv or section tools for small edits.",
            json!({ "id": id, "resume": resume, "theme": { "type": "string" } }),
            &["id"],
            destructive.clone(),
        ),
        tool(
            "patch_cv",
            "Patch CV",
            "Apply a JSON Merge Patch (RFC 7396) to the resume: objects are merged, null deletes a field, arrays are replaced. \
             Example: {\"basics\": {\"label\": \"Staff Engineer\", \"phone\": null}}.",
            json!({ "id": id, "patch": { "type": "object", "description": "Merge patch applied to the resume" } }),
            &["id", "patch"],
            write.clone(),
        ),
        tool(
            "add_section_item",
            "Add section entry",
            "Add an entry (job, degree, skill, project...) to a list section. Appended unless an index is given (0 = first).",
            json!({ "id": id, "section": section, "item": { "type": "object", "description": "The entry, e.g. {\"name\": \"ACME\", \"position\": \"CTO\", \"startDate\": \"2024-01\"}" }, "index": { "type": "integer", "minimum": 0 } }),
            &["id", "section", "item"],
            write.clone(),
        ),
        tool(
            "update_section_item",
            "Update section entry",
            "Merge-patch the entry at a 0-based index of a list section.",
            json!({ "id": id, "section": section, "index": { "type": "integer", "minimum": 0 }, "patch": { "type": "object" } }),
            &["id", "section", "index", "patch"],
            write.clone(),
        ),
        tool(
            "remove_section_item",
            "Remove section entry",
            "Remove the entry at a 0-based index of a list section.",
            json!({ "id": id, "section": section, "index": { "type": "integer", "minimum": 0 } }),
            &["id", "section", "index"],
            destructive.clone(),
        ),
        tool(
            "move_section_item",
            "Reorder section entry",
            "Move an entry of a list section from one 0-based index to another.",
            json!({ "id": id, "section": section, "from": { "type": "integer", "minimum": 0 }, "to": { "type": "integer", "minimum": 0 } }),
            &["id", "section", "from", "to"],
            write.clone(),
        ),
        tool(
            "duplicate_cv",
            "Duplicate CV",
            "Copy a CV under a new id, e.g. to tailor it for a specific job offer without touching the original.",
            json!({ "id": id, "newId": { "type": "string", "description": "Id of the copy, default '<id>-copy'" } }),
            &["id"],
            write.clone(),
        ),
        tool(
            "delete_cv",
            "Delete CV",
            "Permanently delete a CV.",
            json!({ "id": id }),
            &["id"],
            destructive.clone(),
        ),
        tool(
            "set_cv_theme",
            "Set CV theme",
            "Change the theme used to render a CV.",
            json!({ "id": id, "theme": { "type": "string", "description": "Theme id" } }),
            &["id", "theme"],
            write.clone(),
        ),
        tool(
            "list_themes",
            "List themes",
            "List available themes (built-in and custom) with their layout and accent color.",
            json!({}),
            &[],
            read_only.clone(),
        ),
        tool(
            "get_theme",
            "Get theme",
            "Get the full definition of a theme (colors, fonts, layout, custom CSS).",
            json!({ "id": { "type": "string" } }),
            &["id"],
            read_only.clone(),
        ),
        tool(
            "save_theme",
            "Create or update theme",
            "Create or replace a custom theme. Fields not provided are taken from the `base` theme (default 'aurora'). \
             Theme fields: id, name, description, layout ('sidebar' | 'classic' | 'banner'), \
             colors {background, surface, sidebar, sidebarText, text, muted, accent, accentSoft, border} (CSS colors), \
             fonts {heading, body (CSS font-family stacks), importUrl (https stylesheet, e.g. Google Fonts)}, \
             radius (CSS length), customCss (extra CSS). Built-in themes cannot be overwritten.",
            json!({
                "theme": { "type": "object", "description": "Theme definition; must contain at least id and name", "required": ["id", "name"] },
                "base": { "type": "string", "description": "Theme to start from" }
            }),
            &["theme"],
            write.clone(),
        ),
        tool(
            "delete_theme",
            "Delete theme",
            "Delete a custom theme that no CV uses anymore.",
            json!({ "id": { "type": "string" } }),
            &["id"],
            destructive,
        ),
        tool(
            "get_resume_schema",
            "Get CV format",
            "Documentation of the CV content format (JSON Resume) with a complete example.",
            json!({}),
            &[],
            read_only,
        ),
    ]
}

fn prompt_definitions() -> Value {
    json!([
        {
            "name": "tailor_cv",
            "title": "Tailor a CV for a job offer",
            "description": "Duplicate a CV and adapt it to a specific job description.",
            "arguments": [
                { "name": "id", "description": "CV to start from", "required": true },
                { "name": "job_description", "description": "The job offer text", "required": true }
            ]
        },
        {
            "name": "review_cv",
            "title": "Review a CV",
            "description": "Get prioritized suggestions to improve a CV.",
            "arguments": [{ "name": "id", "description": "CV to review", "required": true }]
        }
    ])
}

const RESUME_SCHEMA_DOC: &str = r#"# CV format

CVs use the JSON Resume schema (https://jsonresume.org/schema). Every field is optional.
Dates are ISO 8601 strings: "2024", "2024-03" or "2024-03-15". Omit `endDate` for a current position.
Text fields support paragraphs separated by a blank line ("\n\n").

- basics: { name, label (headline), image (URL), email, phone, url, summary,
            location: { address, postalCode, city, countryCode, region },
            profiles: [{ network ("GitHub", "LinkedIn"...), username, url }] }
- work: [{ name (company), position, location, url, startDate, endDate, summary, highlights: [string] }]
- volunteer: [{ organization, position, url, startDate, endDate, summary, highlights: [string] }]
- education: [{ institution, url, area, studyType, startDate, endDate, score, courses: [string] }]
- awards: [{ title, date, awarder, summary }]
- certificates: [{ name, date, issuer, url }]
- publications: [{ name, publisher, releaseDate, url, summary }]
- skills: [{ name, level ("Beginner" | "Intermediate" | "Advanced" | "Expert" or "0-100%"), keywords: [string] }]
- languages: [{ language, fluency ("Native", "Fluent", "C1", "Intermediate"...) }]
- interests: [{ name, keywords: [string] }]
- references: [{ name, reference }]
- projects: [{ name, description, highlights: [string], keywords: [string], startDate, endDate, url, roles: [string] }]

Entries are displayed in the order of the arrays: put the most recent / relevant first.
"#;

// ----- Transports ---------------------------------------------------------

/// Streamable HTTP transport: every POST gets a single JSON response.
pub async fn http_handler(State(state): State<AppState>, body: Bytes) -> Response {
    let server = McpServer::new(state);
    match server.handle_payload(&body) {
        Some(response) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            response.to_string(),
        )
            .into_response(),
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// stdio transport: newline delimited JSON-RPC messages on stdin/stdout.
pub async fn run_stdio(state: AppState) -> anyhow::Result<()> {
    let server = McpServer::new(state);
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle_payload(line.as_bytes()) {
            let mut out = response.to_string();
            out.push('\n');
            stdout.write_all(out.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::storage::Store;
    use clap::Parser;

    fn server() -> (McpServer, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cli = crate::config::Cli::parse_from([
            "cvgenerator",
            "--data-dir",
            dir.path().to_str().unwrap(),
        ]);
        let config: Config = cli.config;
        let store = Store::open(&config.data_dir).unwrap();
        (McpServer::new(AppState::new(store, config)), dir)
    }

    fn call(server: &McpServer, name: &str, arguments: Value) -> Value {
        let response = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } }))
            .unwrap();
        response["result"].clone()
    }

    #[test]
    fn initialize_and_list_tools() {
        let (server, _dir) = server();
        let init = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } } }))
            .unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        assert!(
            server
                .handle_message(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
                .is_none()
        );

        let tools = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
            .unwrap();
        let tools = tools["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 15);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        }

        let unknown = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" }))
            .unwrap();
        assert_eq!(unknown["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(
            server.handle_payload(b"{oops").unwrap()["error"]["code"],
            PARSE_ERROR
        );
    }

    #[test]
    fn manage_cv_through_tools() {
        let (server, _dir) = server();
        let created = call(
            &server,
            "create_cv",
            json!({ "resume": { "basics": { "name": "Grace Hopper" } } }),
        );
        assert_eq!(created["isError"], false);
        assert_eq!(created["structuredContent"]["id"], "grace-hopper");
        assert_eq!(created["structuredContent"]["url"], "/cv/grace-hopper");

        call(
            &server,
            "patch_cv",
            json!({ "id": "grace-hopper", "patch": { "basics": { "label": "Rear Admiral" } } }),
        );
        call(
            &server,
            "add_section_item",
            json!({ "id": "grace-hopper", "section": "work", "item": { "name": "US Navy" } }),
        );
        call(
            &server,
            "add_section_item",
            json!({ "id": "grace-hopper", "section": "work", "item": { "name": "Harvard" }, "index": 0 }),
        );
        call(
            &server,
            "move_section_item",
            json!({ "id": "grace-hopper", "section": "work", "from": 0, "to": 1 }),
        );
        let cv =
            call(&server, "get_cv", json!({ "id": "grace-hopper" }))["structuredContent"].clone();
        assert_eq!(cv["resume"]["basics"]["label"], "Rear Admiral");
        assert_eq!(cv["resume"]["work"][0]["name"], "US Navy");

        let copy = call(
            &server,
            "duplicate_cv",
            json!({ "id": "grace-hopper", "newId": "grace-navy" }),
        );
        assert_eq!(
            copy["structuredContent"]["resume"]["work"][1]["name"],
            "Harvard"
        );

        let theme = call(
            &server,
            "save_theme",
            json!({ "theme": { "id": "navy", "name": "Navy", "colors": { "accent": "#1e3a8a" } }, "base": "classic" }),
        );
        assert_eq!(theme["structuredContent"]["theme"]["layout"], "classic");
        call(
            &server,
            "set_cv_theme",
            json!({ "id": "grace-navy", "theme": "navy" }),
        );
        assert_eq!(
            call(&server, "get_cv", json!({ "id": "grace-navy" }))["structuredContent"]["theme"],
            "navy"
        );

        let err = call(
            &server,
            "set_cv_theme",
            json!({ "id": "grace-navy", "theme": "missing" }),
        );
        assert_eq!(err["isError"], true);
        let err = call(
            &server,
            "remove_section_item",
            json!({ "id": "grace-hopper", "section": "work", "index": 7 }),
        );
        assert_eq!(err["isError"], true);

        let list = call(&server, "list_cvs", json!({}));
        assert_eq!(
            list["structuredContent"]["cvs"].as_array().unwrap().len(),
            2
        );
        call(&server, "delete_cv", json!({ "id": "grace-navy" }));
        assert_eq!(
            call(&server, "list_cvs", json!({}))["structuredContent"]["cvs"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn resources_and_prompts() {
        let (server, _dir) = server();
        call(
            &server,
            "create_cv",
            json!({ "id": "ada", "resume": { "basics": { "name": "Ada" } } }),
        );
        let list = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }))
            .unwrap();
        assert_eq!(list["result"]["resources"][0]["uri"], "cv://ada");
        let read = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 2, "method": "resources/read", "params": { "uri": "cv://ada" } }))
            .unwrap();
        assert!(
            read["result"]["contents"][0]["text"]
                .as_str()
                .unwrap()
                .contains("\"Ada\"")
        );
        let prompt = server
            .handle_message(json!({ "jsonrpc": "2.0", "id": 3, "method": "prompts/get", "params": { "name": "tailor_cv", "arguments": { "id": "ada", "job_description": "Rustacean" } } }))
            .unwrap();
        assert!(
            prompt["result"]["messages"][0]["content"]["text"]
                .as_str()
                .unwrap()
                .contains("Rustacean")
        );
    }
}
