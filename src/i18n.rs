//! Languages of rendered CVs: section titles, dates and toolbar labels.
//!
//! The CV content itself is written by its author in whatever language they
//! want; this only translates the text the application adds around it.
//! Adding a language means adding a [`Lang`] variant and its [`Labels`].

use serde::{Deserialize, Serialize};

use crate::theme::Layout;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Fr,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::En, Lang::Fr];

    /// ISO 639-1 code, used in JSON, URLs and the `lang` HTML attribute.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
        }
    }

    /// Name of the language in that language.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Français",
        }
    }

    pub fn parse(code: &str) -> Option<Lang> {
        Lang::ALL.into_iter().find(|l| l.code() == code)
    }

    /// Abbreviated month names, used in date ranges.
    fn short_months(self) -> [&'static str; 12] {
        match self {
            Lang::En => [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ],
            Lang::Fr => [
                "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
                "nov.", "déc.",
            ],
        }
    }

    fn months(self) -> [&'static str; 12] {
        match self {
            Lang::En => [
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December",
            ],
            Lang::Fr => [
                "janvier",
                "février",
                "mars",
                "avril",
                "mai",
                "juin",
                "juillet",
                "août",
                "septembre",
                "octobre",
                "novembre",
                "décembre",
            ],
        }
    }

    /// `Mar 2021` / `mars 2021`. `month` is 1-based.
    pub fn short_month_year(self, month: usize, year: impl std::fmt::Display) -> String {
        format!("{} {year}", self.short_months()[month - 1])
    }

    /// Footer line telling when the CV was last updated.
    pub fn updated(self, month: usize, year: i32) -> String {
        let month = self.months()[month - 1];
        match self {
            Lang::En => format!("Updated {month} {year}"),
            Lang::Fr => format!("Mis à jour en {month} {year}"),
        }
    }

    pub fn layout_name(self, layout: Layout) -> &'static str {
        match (self, layout) {
            (Lang::En, layout) => layout.name(),
            (Lang::Fr, Layout::Sidebar) => "Barre latérale",
            (Lang::Fr, Layout::Classic) => "Classique",
            (Lang::Fr, Layout::Banner) => "Bandeau",
            (Lang::Fr, Layout::Profile) => "Profil",
            (Lang::Fr, Layout::Academic) => "Académique",
            (Lang::Fr, Layout::Compact) => "Compact",
            (Lang::Fr, Layout::Cards) => "Cartes",
        }
    }

    pub fn labels(self) -> Labels {
        match self {
            Lang::En => Labels {
                contact: "Contact",
                skills: "Skills",
                languages: "Languages",
                certificates: "Certificates",
                interests: "Interests",
                profile: "Profile",
                experience: "Experience",
                projects: "Projects",
                education: "Education",
                volunteering: "Volunteering",
                awards: "Awards",
                publications: "Publications",
                references: "References",
                present: "Present",
                your_name: "Your Name",
                cv_tools: "CV tools",
                theme: "Theme",
                layout: "Layout",
                default_layout: "Default",
                apply: "Apply",
                download_json: "Download JSON Resume",
                save_pdf: "Save as PDF",
                save_pdf_hint: "Print or save as PDF",
            },
            Lang::Fr => Labels {
                contact: "Contact",
                skills: "Compétences",
                languages: "Langues",
                certificates: "Certifications",
                interests: "Centres d’intérêt",
                profile: "Profil",
                experience: "Expérience",
                projects: "Projets",
                education: "Formation",
                volunteering: "Bénévolat",
                awards: "Distinctions",
                publications: "Publications",
                references: "Références",
                present: "aujourd’hui",
                your_name: "Votre nom",
                cv_tools: "Outils du CV",
                theme: "Thème",
                layout: "Mise en page",
                default_layout: "Par défaut",
                apply: "Appliquer",
                download_json: "Télécharger au format JSON Resume",
                save_pdf: "Enregistrer en PDF",
                save_pdf_hint: "Imprimer ou enregistrer en PDF",
            },
        }
    }
}

/// Text added by the application around the CV content.
#[derive(Debug, Clone, Serialize)]
pub struct Labels {
    pub contact: &'static str,
    pub skills: &'static str,
    pub languages: &'static str,
    pub certificates: &'static str,
    pub interests: &'static str,
    pub profile: &'static str,
    pub experience: &'static str,
    pub projects: &'static str,
    pub education: &'static str,
    pub volunteering: &'static str,
    pub awards: &'static str,
    pub publications: &'static str,
    pub references: &'static str,
    /// End of a date range that is still ongoing.
    pub present: &'static str,
    pub your_name: &'static str,
    pub cv_tools: &'static str,
    pub theme: &'static str,
    pub layout: &'static str,
    pub default_layout: &'static str,
    pub apply: &'static str,
    pub download_json: &'static str,
    pub save_pdf: &'static str,
    pub save_pdf_hint: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_serde() {
        for lang in Lang::ALL {
            assert_eq!(serde_json::to_value(lang).unwrap(), lang.code());
            assert_eq!(Lang::parse(lang.code()), Some(lang));
        }
        assert_eq!(Lang::parse("xx"), None);
    }

    #[test]
    fn dates() {
        assert_eq!(Lang::En.short_month_year(3, 2021), "Mar 2021");
        assert_eq!(Lang::Fr.short_month_year(2, 2021), "févr. 2021");
        assert_eq!(Lang::En.updated(10, 2026), "Updated October 2026");
        assert_eq!(Lang::Fr.updated(8, 2026), "Mis à jour en août 2026");
    }

    #[test]
    fn every_layout_has_a_french_name() {
        for layout in Layout::ALL {
            assert!(!Lang::Fr.layout_name(layout).is_empty());
        }
    }
}
