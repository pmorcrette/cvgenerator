//! File based storage: one pretty-printed JSON file per CV and per custom
//! theme. Easy to back up, version and edit by hand, and a perfect fit for a
//! Docker volume.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::Utc;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{Error, Result};
use crate::model::{CvDocument, CvSummary, Resume, is_valid_id, slugify};
use crate::theme::{Theme, builtin_themes};

pub struct Store {
    cvs_dir: PathBuf,
    themes_dir: PathBuf,
    /// Serializes read-modify-write cycles.
    write_lock: Mutex<()>,
}

impl Store {
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self> {
        let data_dir = data_dir.as_ref();
        let store = Self {
            cvs_dir: data_dir.join("cvs"),
            themes_dir: data_dir.join("themes"),
            write_lock: Mutex::new(()),
        };
        fs::create_dir_all(&store.cvs_dir)?;
        fs::create_dir_all(&store.themes_dir)?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn cv_path(&self, id: &str) -> Result<PathBuf> {
        check_id(id, "CV")?;
        Ok(self.cvs_dir.join(format!("{id}.json")))
    }

    fn theme_path(&self, id: &str) -> Result<PathBuf> {
        check_id(id, "theme")?;
        Ok(self.themes_dir.join(format!("{id}.json")))
    }

    // ----- CVs -----------------------------------------------------------

    pub fn list_cvs(&self) -> Result<Vec<CvSummary>> {
        let mut cvs: Vec<CvSummary> = read_all::<CvDocument>(&self.cvs_dir)?
            .iter()
            .map(CvSummary::from)
            .collect();
        cvs.sort_by_key(|cv| std::cmp::Reverse(cv.updated_at));
        Ok(cvs)
    }

    pub fn get_cv(&self, id: &str) -> Result<CvDocument> {
        read_json(&self.cv_path(id)?).map_err(|e| not_found(e, format!("CV '{id}'")))
    }

    /// Create a new CV. When `id` is not given it is derived from the
    /// person's name and made unique.
    pub fn create_cv(
        &self,
        id: Option<String>,
        resume: Resume,
        theme: Option<String>,
    ) -> Result<CvDocument> {
        let theme = theme.unwrap_or_else(crate::model::default_theme);
        self.get_theme(&theme)?;
        let _guard = self.lock();
        let id = match id {
            Some(id) => {
                check_id(&id, "CV")?;
                if self.cv_path(&id)?.exists() {
                    return Err(Error::Conflict(format!("CV '{id}'")));
                }
                id
            }
            None => {
                let base = resume
                    .basics
                    .name
                    .as_deref()
                    .map(slugify)
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "cv".into());
                let mut candidate = base.clone();
                let mut n = 2;
                while self.cv_path(&candidate)?.exists() {
                    candidate = format!("{base}-{n}");
                    n += 1;
                }
                candidate
            }
        };
        let now = Utc::now();
        let doc = CvDocument {
            id,
            theme,
            created_at: now,
            updated_at: now,
            resume,
        };
        write_json(&self.cv_path(&doc.id)?, &doc)?;
        Ok(doc)
    }

    /// Load a CV, let `change` modify it and persist the result atomically.
    pub fn update_cv(
        &self,
        id: &str,
        change: impl FnOnce(&mut CvDocument) -> Result<()>,
    ) -> Result<CvDocument> {
        let _guard = self.lock();
        let mut doc = self.get_cv(id)?;
        change(&mut doc)?;
        doc.id = id.to_string();
        self.get_theme(&doc.theme)?;
        doc.updated_at = Utc::now();
        write_json(&self.cv_path(id)?, &doc)?;
        Ok(doc)
    }

    pub fn delete_cv(&self, id: &str) -> Result<()> {
        let _guard = self.lock();
        fs::remove_file(self.cv_path(id)?).map_err(|e| not_found(e.into(), format!("CV '{id}'")))
    }

    // ----- Themes --------------------------------------------------------

    pub fn list_themes(&self) -> Result<Vec<Theme>> {
        let mut themes = builtin_themes();
        let mut custom = read_all::<Theme>(&self.themes_dir)?;
        custom.retain(|t| !themes.iter().any(|b| b.id == t.id));
        custom.sort_by(|a, b| a.name.cmp(&b.name));
        themes.extend(custom.into_iter().map(|mut t| {
            t.builtin = false;
            t
        }));
        Ok(themes)
    }

    pub fn get_theme(&self, id: &str) -> Result<Theme> {
        if let Some(theme) = builtin_themes().into_iter().find(|t| t.id == id) {
            return Ok(theme);
        }
        let mut theme: Theme =
            read_json(&self.theme_path(id)?).map_err(|e| not_found(e, format!("theme '{id}'")))?;
        theme.builtin = false;
        Ok(theme)
    }

    /// Create or replace a custom theme. Returns `true` when it was created.
    pub fn save_theme(&self, mut theme: Theme) -> Result<bool> {
        theme.builtin = false;
        theme.validate().map_err(Error::Invalid)?;
        if builtin_themes().iter().any(|t| t.id == theme.id) {
            return Err(Error::Forbidden(format!(
                "'{}' is a built-in theme and cannot be modified; save it under another id",
                theme.id
            )));
        }
        let _guard = self.lock();
        let path = self.theme_path(&theme.id)?;
        let created = !path.exists();
        write_json(&path, &theme)?;
        Ok(created)
    }

    pub fn delete_theme(&self, id: &str) -> Result<()> {
        if builtin_themes().iter().any(|t| t.id == id) {
            return Err(Error::Forbidden(format!(
                "'{id}' is a built-in theme and cannot be deleted"
            )));
        }
        let _guard = self.lock();
        let in_use: Vec<String> = read_all::<CvDocument>(&self.cvs_dir)?
            .into_iter()
            .filter(|cv| cv.theme == id)
            .map(|cv| cv.id)
            .collect();
        if !in_use.is_empty() {
            return Err(Error::Conflict(format!(
                "theme '{id}' is used by {}; switch them to another theme first",
                in_use.join(", ")
            )));
        }
        fs::remove_file(self.theme_path(id)?)
            .map_err(|e| not_found(e.into(), format!("theme '{id}'")))
    }
}

fn check_id(id: &str, kind: &str) -> Result<()> {
    if is_valid_id(id) {
        Ok(())
    } else {
        Err(Error::Invalid(format!(
            "invalid {kind} id '{id}': use 1-64 lowercase letters, digits and dashes"
        )))
    }
}

fn not_found(err: Error, what: String) -> Error {
    match err {
        Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound => Error::NotFound(what),
        other => other,
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path)?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// Read every `*.json` file of a directory, skipping (and logging) broken ones.
fn read_all<T: DeserializeOwned>(dir: &Path) -> Result<Vec<T>> {
    let mut items = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            match read_json(&path) {
                Ok(item) => items.push(item),
                Err(e) => tracing::warn!("skipping {}: {e}", path.display()),
            }
        }
    }
    Ok(items)
}

/// Write through a temporary file so readers never see a partial document.
fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Basics;

    fn resume(name: &str) -> Resume {
        Resume {
            basics: Basics {
                name: Some(name.into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn cv_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();

        let a = store.create_cv(None, resume("Ada Lovelace"), None).unwrap();
        let b = store.create_cv(None, resume("Ada Lovelace"), None).unwrap();
        assert_eq!(a.id, "ada-lovelace");
        assert_eq!(b.id, "ada-lovelace-2");
        assert!(matches!(
            store.create_cv(Some("ada-lovelace".into()), resume("x"), None),
            Err(Error::Conflict(_))
        ));

        let updated = store
            .update_cv("ada-lovelace", |doc| {
                doc.resume.basics.label = Some("Mathematician".into());
                Ok(())
            })
            .unwrap();
        assert_eq!(store.get_cv("ada-lovelace").unwrap(), updated);
        assert_eq!(store.list_cvs().unwrap()[0].id, "ada-lovelace");

        store.delete_cv("ada-lovelace").unwrap();
        assert!(matches!(
            store.get_cv("ada-lovelace"),
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            store.delete_cv("ada-lovelace"),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn rejects_path_traversal_and_unknown_theme() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(matches!(store.get_cv("../secret"), Err(Error::Invalid(_))));
        assert!(matches!(
            store.create_cv(None, resume("x"), Some("nope".into())),
            Err(Error::NotFound(_))
        ));
    }

    #[test]
    fn theme_lifecycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let mut theme = store.get_theme("aurora").unwrap();
        assert!(matches!(
            store.save_theme(theme.clone()),
            Err(Error::Forbidden(_))
        ));

        theme.id = "corporate".into();
        theme.name = "Corporate".into();
        assert!(store.save_theme(theme.clone()).unwrap());
        assert!(!store.save_theme(theme).unwrap());
        assert!(
            store
                .list_themes()
                .unwrap()
                .iter()
                .any(|t| t.id == "corporate")
        );

        store
            .create_cv(None, resume("Bob"), Some("corporate".into()))
            .unwrap();
        assert!(matches!(
            store.delete_theme("corporate"),
            Err(Error::Conflict(_))
        ));
        store.delete_cv("bob").unwrap();
        store.delete_theme("corporate").unwrap();
        assert!(matches!(
            store.delete_theme("aurora"),
            Err(Error::Forbidden(_))
        ));
    }
}
