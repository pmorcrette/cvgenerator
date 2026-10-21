//! HTML rendering with MiniJinja. Templates are compiled into the binary so
//! the Docker image only needs a single executable.

use minijinja::value::Serde;
use minijinja::{Environment, Value, context};
use serde::Serialize;

use crate::error::Result;
use crate::model::CvDocument;
use crate::theme::{Layout, Theme};

pub struct Renderer {
    env: Environment<'static>,
}

/// Options controlling how a CV page is rendered.
#[derive(Debug, Default, Clone, Serialize)]
pub struct CvPageOptions {
    /// Hide the floating toolbar (used for previews and embedding).
    pub embed: bool,
    /// Themes offered in the toolbar's theme picker.
    pub themes: Vec<ThemeChoice>,
    /// Layout forced for this rendering (e.g. `?layout=` preview), taking
    /// precedence over the CV's and the theme's layout.
    pub layout: Option<Layout>,
}

/// Layout actually used to render `doc` with `theme`.
pub fn effective_layout(doc: &CvDocument, theme: &Theme, forced: Option<Layout>) -> Layout {
    forced.or(doc.layout).unwrap_or(theme.layout)
}

#[derive(Debug, Clone, Serialize)]
pub struct ThemeChoice {
    pub id: String,
    pub name: String,
}

impl Renderer {
    pub fn new() -> Self {
        let mut env = Environment::new();
        let templates = [
            ("cv.html", include_str!("../templates/cv.html")),
            (
                "admin_base.html",
                include_str!("../templates/admin_base.html"),
            ),
            (
                "admin_index.html",
                include_str!("../templates/admin_index.html"),
            ),
            (
                "admin_editor.html",
                include_str!("../templates/admin_editor.html"),
            ),
            (
                "admin_themes.html",
                include_str!("../templates/admin_themes.html"),
            ),
        ];
        for (name, source) in templates {
            env.add_template(name, source)
                .expect("built-in template is valid");
        }
        env.add_filter("date", format_date);
        env.add_filter("paragraphs", paragraphs);
        env.add_filter("pretty_url", pretty_url);
        env.add_filter("initials", initials);
        env.add_filter("level_percent", level_percent);
        env.add_filter("safe_url", safe_url);
        Self { env }
    }

    pub fn render_cv(
        &self,
        doc: &CvDocument,
        theme: &Theme,
        options: &CvPageOptions,
    ) -> Result<String> {
        let template = self.env.get_template("cv.html")?;
        Ok(template.render(context! {
            id => doc.id.clone(),
            cv => Value::from(Serde(&doc.resume)),
            updated_at => doc.updated_at.format("%B %Y").to_string(),
            theme => Value::from(Serde(theme)),
            theme_css => theme.to_css(),
            layout => effective_layout(doc, theme, options.layout).id(),
            default_layout => effective_layout(doc, theme, None).name(),
            forced_layout => options.layout.map(Layout::id),
            layouts => Layout::ALL
                .iter()
                .map(|l| context! { id => l.id(), name => l.name() })
                .collect::<Vec<_>>(),
            options => Value::from(Serde(options)),
        })?)
    }

    pub fn render_admin(&self, page: &str, ctx: Value) -> Result<String> {
        Ok(self.env.get_template(page)?.render(ctx)?)
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Format ISO-8601 (partial) dates for humans: `2021-03-14` and `2021-03`
/// become `Mar 2021`, a bare year is kept and anything else is returned as is.
fn format_date(value: Option<String>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    let parts: Vec<&str> = value.trim().split('-').collect();
    match parts.as_slice() {
        [year, month, ..] if year.len() == 4 && year.parse::<u32>().is_ok() => {
            match month.parse::<usize>() {
                Ok(m @ 1..=12) => format!("{} {year}", MONTHS[m - 1]),
                _ => value,
            }
        }
        _ => value,
    }
}

/// Split free text on blank lines so templates can emit `<p>` elements.
fn paragraphs(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(String::from)
        .collect()
}

/// `https://www.example.com/me/` -> `example.com/me`
fn pretty_url(value: Option<String>) -> String {
    let value = value.unwrap_or_default();
    let trimmed = value
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("mailto:")
        .trim_start_matches("www.");
    trimmed.trim_end_matches('/').to_string()
}

/// Neutralize `javascript:` and other unexpected URL schemes in links.
fn safe_url(value: Option<String>) -> String {
    let value = value.unwrap_or_default();
    let lower = value.trim().to_ascii_lowercase();
    let allowed = ["https://", "http://", "mailto:", "tel:", "/", "#"];
    if allowed.iter().any(|prefix| lower.starts_with(prefix)) {
        value
    } else if !lower.contains(':') && !lower.is_empty() {
        // Bare domains such as `example.com/me`.
        format!("https://{}", value.trim())
    } else {
        "#".to_string()
    }
}

fn initials(value: Option<String>) -> String {
    value
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|w| w.chars().next())
        .filter(|c| c.is_alphanumeric())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect()
}

/// Map a free text skill level to a percentage used by the level meter.
fn level_percent(value: Option<String>) -> Option<u8> {
    let level = value?.to_lowercase();
    let pct = match level.trim() {
        "master" | "expert" | "native" | "fluent" => 100,
        "advanced" | "proficient" | "c2" | "c1" => 82,
        "intermediate" | "b2" | "b1" => 62,
        "beginner" | "basic" | "novice" | "elementary" | "a2" | "a1" => 38,
        other => other.trim_end_matches('%').parse::<u8>().ok()?.min(100),
    };
    Some(pct)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Resume;
    use crate::theme::builtin_themes;

    #[test]
    fn formats_dates() {
        assert_eq!(format_date(Some("2021-03-14".into())), "Mar 2021");
        assert_eq!(format_date(Some("2021-12".into())), "Dec 2021");
        assert_eq!(format_date(Some("2021".into())), "2021");
        assert_eq!(format_date(Some("Summer 2020".into())), "Summer 2020");
        assert_eq!(format_date(None), "");
    }

    #[test]
    fn helpers() {
        assert_eq!(
            pretty_url(Some("https://www.example.com/me/".into())),
            "example.com/me"
        );
        assert_eq!(safe_url(Some("javascript:alert(1)".into())), "#");
        assert_eq!(safe_url(Some("example.com".into())), "https://example.com");
        assert_eq!(safe_url(Some("https://a.b".into())), "https://a.b");
        assert_eq!(initials(Some("ada king lovelace".into())), "AK");
        assert_eq!(paragraphs(Some("a\n\n\n b ".into())), vec!["a", "b"]);
        assert_eq!(level_percent(Some("Expert".into())), Some(100));
        assert_eq!(level_percent(Some("75%".into())), Some(75));
        assert_eq!(level_percent(Some("Wizard".into())), None);
    }

    #[test]
    fn renders_every_theme_and_escapes_content() {
        let renderer = Renderer::new();
        let mut resume: Resume =
            serde_json::from_str(include_str!("../examples/jane-doe.json")).unwrap();
        resume.basics.summary = Some("<script>alert(1)</script>".into());
        let now = chrono::Utc::now();
        let doc = CvDocument {
            id: "jane".into(),
            theme: "aurora".into(),
            layout: None,
            created_at: now,
            updated_at: now,
            resume,
        };
        for theme in builtin_themes() {
            for layout in Layout::ALL {
                let options = CvPageOptions {
                    layout: Some(layout),
                    ..Default::default()
                };
                let html = renderer.render_cv(&doc, &theme, &options).unwrap();
                assert!(html.contains("Jane Doe"));
                assert!(html.contains(&format!("class=\"layout-{}", layout.id())));
                assert!(html.contains("&lt;script&gt;"));
                assert!(!html.contains("<script>alert(1)"));
            }
        }
    }

    #[test]
    fn layout_precedence() {
        let theme = builtin_themes().remove(0);
        let now = chrono::Utc::now();
        let mut doc = CvDocument {
            id: "x".into(),
            theme: theme.id.clone(),
            layout: None,
            created_at: now,
            updated_at: now,
            resume: Resume::default(),
        };
        assert_eq!(effective_layout(&doc, &theme, None), theme.layout);
        doc.layout = Some(Layout::Cards);
        assert_eq!(effective_layout(&doc, &theme, None), Layout::Cards);
        assert_eq!(
            effective_layout(&doc, &theme, Some(Layout::Compact)),
            Layout::Compact
        );
    }
}
