//! Themes: a set of design tokens (colors, fonts, layout) turned into CSS
//! custom properties, plus optional free-form CSS for full control.

use serde::{Deserialize, Serialize};

/// Page structure used to lay out the CV.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Layout {
    /// Two columns: contact, skills and languages in a tinted sidebar.
    #[default]
    Sidebar,
    /// Single centered column with a classic header.
    Classic,
    /// Bold colored header band above two columns, sidebar on the right.
    Banner,
    /// Full-height colored sidebar holding the photo, name, contact and skills.
    Profile,
    /// Dates in a left gutter, sober typography: suited to academic CVs.
    Academic,
    /// Dense, smaller type and tight spacing to fit on a single page.
    Compact,
    /// Every section in its own card on a tinted background.
    Cards,
}

impl Layout {
    pub const ALL: [Layout; 7] = [
        Layout::Sidebar,
        Layout::Classic,
        Layout::Banner,
        Layout::Profile,
        Layout::Academic,
        Layout::Compact,
        Layout::Cards,
    ];

    /// Identifier used in JSON, URLs (`?layout=`) and CSS classes.
    pub fn id(self) -> &'static str {
        match self {
            Layout::Sidebar => "sidebar",
            Layout::Classic => "classic",
            Layout::Banner => "banner",
            Layout::Profile => "profile",
            Layout::Academic => "academic",
            Layout::Compact => "compact",
            Layout::Cards => "cards",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Layout::Sidebar => {
                "Two columns with contact, skills and languages in a tinted sidebar."
            }
            Layout::Classic => {
                "Single centered column with a classic header; side details at the bottom."
            }
            Layout::Banner => "Bold colored header band, main column with a sidebar on the right.",
            Layout::Profile => {
                "Full-height colored sidebar holding the photo, name, contact and skills."
            }
            Layout::Academic => {
                "Dates in a left gutter and sober typography, suited to academic CVs."
            }
            Layout::Compact => "Dense, smaller type and tight spacing to fit on a single page.",
            Layout::Cards => "Every section in its own card on a tinted background.",
        }
    }

    /// Parse a layout identifier (`"sidebar"`, `"cards"`...).
    pub fn parse(id: &str) -> Option<Layout> {
        Layout::ALL.into_iter().find(|l| l.id() == id)
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Sidebar => "Sidebar",
            Layout::Classic => "Classic",
            Layout::Banner => "Banner",
            Layout::Profile => "Profile",
            Layout::Academic => "Academic",
            Layout::Compact => "Compact",
            Layout::Cards => "Cards",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub layout: Layout,
    #[serde(default)]
    pub colors: Colors,
    #[serde(default)]
    pub fonts: Fonts,
    /// Corner radius of cards and chips, as a CSS length.
    #[serde(default = "default_radius")]
    pub radius: String,
    /// Extra CSS appended after the generated stylesheet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub custom_css: String,
    /// Built-in themes cannot be modified or deleted.
    #[serde(default)]
    pub builtin: bool,
}

fn default_radius() -> String {
    "10px".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Colors {
    /// Page background around the sheet.
    pub background: String,
    /// The CV sheet itself.
    pub surface: String,
    /// Sidebar / header band background.
    pub sidebar: String,
    /// Text on top of `sidebar`.
    pub sidebar_text: String,
    pub text: String,
    pub muted: String,
    pub accent: String,
    /// Soft accent tint used for chips and highlights.
    pub accent_soft: String,
    pub border: String,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: "#eef0f5".into(),
            surface: "#ffffff".into(),
            sidebar: "#f6f7fb".into(),
            sidebar_text: "#1f2333".into(),
            text: "#1f2333".into(),
            muted: "#646b80".into(),
            accent: "#4f46e5".into(),
            accent_soft: "#eceafd".into(),
            border: "#e3e6ee".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Fonts {
    /// CSS font-family stack for headings.
    pub heading: String,
    /// CSS font-family stack for body text.
    pub body: String,
    /// Optional stylesheet URL loading the fonts (e.g. Google Fonts).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub import_url: String,
}

impl Default for Fonts {
    fn default() -> Self {
        Self {
            heading: SANS.into(),
            body: SANS.into(),
            import_url: String::new(),
        }
    }
}

const SANS: &str = "\"Inter\", system-ui, -apple-system, \"Segoe UI\", Roboto, sans-serif";

impl Theme {
    /// Validate user supplied values so that they cannot break out of the
    /// generated `<style>` element.
    pub fn validate(&self) -> Result<(), String> {
        if !crate::model::is_valid_id(&self.id) {
            return Err(format!(
                "invalid theme id '{}': use lowercase letters, digits and dashes",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            return Err("theme name must not be empty".into());
        }
        let c = &self.colors;
        let tokens = [
            ("colors.background", &c.background),
            ("colors.surface", &c.surface),
            ("colors.sidebar", &c.sidebar),
            ("colors.sidebarText", &c.sidebar_text),
            ("colors.text", &c.text),
            ("colors.muted", &c.muted),
            ("colors.accent", &c.accent),
            ("colors.accentSoft", &c.accent_soft),
            ("colors.border", &c.border),
            ("fonts.heading", &self.fonts.heading),
            ("fonts.body", &self.fonts.body),
            ("radius", &self.radius),
        ];
        for (name, value) in tokens {
            if value
                .chars()
                .any(|ch| matches!(ch, ';' | '{' | '}' | '<' | '>'))
            {
                return Err(format!(
                    "{name} contains forbidden characters (; {{ }} < >)"
                ));
            }
        }
        if self.custom_css.to_ascii_lowercase().contains("</style") {
            return Err("customCss must not contain '</style'".into());
        }
        let url = &self.fonts.import_url;
        if !url.is_empty() && (!url.starts_with("https://") || url.contains(['"', '<', '>', ' '])) {
            return Err("fonts.importUrl must be an https:// URL".into());
        }
        Ok(())
    }

    /// CSS custom properties for this theme, followed by its custom CSS.
    pub fn to_css(&self) -> String {
        let c = &self.colors;
        let mut css = format!(
            ":root {{\n  --bg: {};\n  --surface: {};\n  --sidebar: {};\n  --sidebar-text: {};\n  \
             --text: {};\n  --muted: {};\n  --accent: {};\n  --accent-soft: {};\n  --border: {};\n  \
             --font-heading: {};\n  --font-body: {};\n  --radius: {};\n}}\n",
            c.background,
            c.surface,
            c.sidebar,
            c.sidebar_text,
            c.text,
            c.muted,
            c.accent,
            c.accent_soft,
            c.border,
            self.fonts.heading,
            self.fonts.body,
            self.radius,
        );
        if !self.custom_css.is_empty() {
            css.push_str(&self.custom_css);
            css.push('\n');
        }
        css
    }
}

/// Themes shipped with the application.
pub fn builtin_themes() -> Vec<Theme> {
    let inter = "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap";
    vec![
        Theme {
            id: "aurora".into(),
            name: "Aurora".into(),
            description: "Clean and modern, indigo accents with a soft sidebar.".into(),
            layout: Layout::Sidebar,
            colors: Colors::default(),
            fonts: Fonts {
                import_url: inter.into(),
                ..Fonts::default()
            },
            radius: "10px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "midnight".into(),
            name: "Midnight".into(),
            description: "Elegant dark theme with teal highlights.".into(),
            layout: Layout::Sidebar,
            colors: Colors {
                background: "#0b0f17".into(),
                surface: "#121826".into(),
                sidebar: "#0f1521".into(),
                sidebar_text: "#dbe3f0".into(),
                text: "#e4e9f2".into(),
                muted: "#8b97ad".into(),
                accent: "#2dd4bf".into(),
                accent_soft: "#123a3a".into(),
                border: "#232c3d".into(),
            },
            fonts: Fonts {
                import_url: inter.into(),
                ..Fonts::default()
            },
            radius: "10px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "classic".into(),
            name: "Classic".into(),
            description: "Timeless serif typography on warm paper.".into(),
            layout: Layout::Classic,
            colors: Colors {
                background: "#ece7df".into(),
                surface: "#fffdf8".into(),
                sidebar: "#f7f2e8".into(),
                sidebar_text: "#2b2620".into(),
                text: "#2b2620".into(),
                muted: "#7a6f60".into(),
                accent: "#9a3b26".into(),
                accent_soft: "#f4e4dc".into(),
                border: "#e6dccb".into(),
            },
            fonts: Fonts {
                heading: "\"Playfair Display\", Georgia, serif".into(),
                body: "\"Source Serif 4\", Georgia, serif".into(),
                import_url: "https://fonts.googleapis.com/css2?family=Playfair+Display:wght@600;700&family=Source+Serif+4:wght@400;600&display=swap".into(),
            },
            radius: "4px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "forest".into(),
            name: "Forest".into(),
            description: "Calm greens with a bold header band.".into(),
            layout: Layout::Banner,
            colors: Colors {
                background: "#e7ede9".into(),
                surface: "#ffffff".into(),
                sidebar: "#1f4d3a".into(),
                sidebar_text: "#f1f7f3".into(),
                text: "#1c2a24".into(),
                muted: "#5e7168".into(),
                accent: "#2f7d5b".into(),
                accent_soft: "#e1f0e8".into(),
                border: "#dbe5df".into(),
            },
            fonts: Fonts {
                heading: "\"Manrope\", system-ui, sans-serif".into(),
                body: "\"Manrope\", system-ui, sans-serif".into(),
                import_url: "https://fonts.googleapis.com/css2?family=Manrope:wght@400;500;600;700;800&display=swap".into(),
            },
            radius: "12px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "mono".into(),
            name: "Mono".into(),
            description: "Minimal black and white with a technical feel.".into(),
            layout: Layout::Classic,
            colors: Colors {
                background: "#f2f2f2".into(),
                surface: "#ffffff".into(),
                sidebar: "#fafafa".into(),
                sidebar_text: "#111111".into(),
                text: "#111111".into(),
                muted: "#666666".into(),
                accent: "#111111".into(),
                accent_soft: "#efefef".into(),
                border: "#e4e4e4".into(),
            },
            fonts: Fonts {
                heading: "\"JetBrains Mono\", ui-monospace, monospace".into(),
                body: "\"IBM Plex Sans\", system-ui, sans-serif".into(),
                import_url: "https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;500;600&family=JetBrains+Mono:wght@500;700&display=swap".into(),
            },
            radius: "0px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "slate".into(),
            name: "Slate".into(),
            description: "Deep slate profile column with amber accents.".into(),
            layout: Layout::Profile,
            colors: Colors {
                background: "#e9ecf1".into(),
                surface: "#ffffff".into(),
                sidebar: "#1e293b".into(),
                sidebar_text: "#e2e8f0".into(),
                text: "#1e293b".into(),
                muted: "#64748b".into(),
                accent: "#d97706".into(),
                accent_soft: "#fdf1dc".into(),
                border: "#e2e8f0".into(),
            },
            fonts: Fonts {
                heading: "\"Sora\", system-ui, sans-serif".into(),
                body: "\"Inter\", system-ui, sans-serif".into(),
                import_url: "https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600&family=Sora:wght@600;700&display=swap".into(),
            },
            radius: "8px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "scholar".into(),
            name: "Scholar".into(),
            description: "Sober navy and serif type with a date gutter.".into(),
            layout: Layout::Academic,
            colors: Colors {
                background: "#eef0f3".into(),
                surface: "#ffffff".into(),
                sidebar: "#f5f6f8".into(),
                sidebar_text: "#1b2433".into(),
                text: "#1b2433".into(),
                muted: "#5f6b7d".into(),
                accent: "#1d3a6e".into(),
                accent_soft: "#e7ecf5".into(),
                border: "#dfe3ea".into(),
            },
            fonts: Fonts {
                heading: "\"EB Garamond\", Georgia, serif".into(),
                body: "\"EB Garamond\", Georgia, serif".into(),
                import_url: "https://fonts.googleapis.com/css2?family=EB+Garamond:wght@400;500;600;700&display=swap".into(),
            },
            radius: "2px".into(),
            custom_css: String::new(),
            builtin: true,
        },
        Theme {
            id: "studio".into(),
            name: "Studio".into(),
            description: "Playful cards on a soft lavender canvas.".into(),
            layout: Layout::Cards,
            colors: Colors {
                background: "#efeafd".into(),
                surface: "#ffffff".into(),
                sidebar: "#f7f4ff".into(),
                sidebar_text: "#231942".into(),
                text: "#231942".into(),
                muted: "#6b6489".into(),
                accent: "#7c3aed".into(),
                accent_soft: "#f0e9ff".into(),
                border: "#e6defa".into(),
            },
            fonts: Fonts {
                heading: "\"Outfit\", system-ui, sans-serif".into(),
                body: "\"Outfit\", system-ui, sans-serif".into(),
                import_url: "https://fonts.googleapis.com/css2?family=Outfit:wght@400;500;600;700&display=swap".into(),
            },
            radius: "16px".into(),
            custom_css: String::new(),
            builtin: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_are_valid() {
        for theme in builtin_themes() {
            theme
                .validate()
                .unwrap_or_else(|e| panic!("{}: {e}", theme.id));
        }
    }

    #[test]
    fn layout_ids_match_serde() {
        for layout in Layout::ALL {
            assert_eq!(serde_json::to_value(layout).unwrap(), layout.id());
            assert_eq!(Layout::parse(layout.id()), Some(layout));
        }
        assert_eq!(Layout::parse("nope"), None);
    }

    #[test]
    fn rejects_css_injection() {
        let mut theme = builtin_themes().remove(0);
        theme.colors.accent = "red; } body { display:none".into();
        assert!(theme.validate().is_err());

        let mut theme = builtin_themes().remove(0);
        theme.custom_css = "</style><script>alert(1)</script>".into();
        assert!(theme.validate().is_err());
    }

    #[test]
    fn minimal_theme_uses_defaults() {
        let theme: Theme = serde_json::from_str(r#"{"id":"mine","name":"Mine"}"#).unwrap();
        assert_eq!(theme.layout, Layout::Sidebar);
        assert!(theme.validate().is_ok());
        assert!(theme.to_css().contains("--accent: #4f46e5"));
    }
}
