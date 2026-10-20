//! Operations shared by the REST API and the MCP server.

use serde_json::Value;

use crate::error::{Error, Result};
use crate::model::{Resume, SECTIONS, merge_patch};

/// Apply a JSON Merge Patch to a resume and validate the result.
pub fn patch_resume(resume: &mut Resume, patch: &Value) -> Result<()> {
    let mut value = serde_json::to_value(&*resume)?;
    merge_patch(&mut value, patch);
    *resume = parse_resume(value)?;
    Ok(())
}

pub fn parse_resume(value: Value) -> Result<Resume> {
    if !value.is_object() {
        return Err(Error::Invalid("resume must be a JSON object".into()));
    }
    serde_json::from_value(value).map_err(|e| Error::Invalid(format!("resume: {e}")))
}

/// An edit on one entry of a list section (`work`, `skills`...).
pub enum SectionEdit {
    /// Insert at `index` (or append when `None`).
    Add {
        index: Option<usize>,
        item: Value,
    },
    /// Merge-patch the entry at `index`.
    Update {
        index: usize,
        patch: Value,
    },
    Remove {
        index: usize,
    },
    /// Move the entry at `from` to position `to`.
    Move {
        from: usize,
        to: usize,
    },
}

pub fn edit_section(resume: &mut Resume, section: &str, edit: SectionEdit) -> Result<()> {
    if !SECTIONS.contains(&section) {
        return Err(Error::Invalid(format!(
            "unknown section '{section}', expected one of: {}",
            SECTIONS.join(", ")
        )));
    }
    let mut value = serde_json::to_value(&*resume)?;
    let root = value
        .as_object_mut()
        .expect("resume serializes to an object");
    let list = root
        .entry(section)
        .or_insert_with(|| Value::Array(vec![]))
        .as_array_mut()
        .expect("sections are arrays");
    let len = list.len();
    let check = |index: usize| {
        if index < len {
            Ok(index)
        } else {
            Err(Error::Invalid(format!(
                "index {index} out of range: '{section}' has {len} entries"
            )))
        }
    };
    match edit {
        SectionEdit::Add { index, item } => {
            if !item.is_object() {
                return Err(Error::Invalid("item must be a JSON object".into()));
            }
            let index = index.unwrap_or(len).min(len);
            list.insert(index, item);
        }
        SectionEdit::Update { index, patch } => merge_patch(&mut list[check(index)?], &patch),
        SectionEdit::Remove { index } => {
            list.remove(check(index)?);
        }
        SectionEdit::Move { from, to } => {
            let item = list.remove(check(from)?);
            list.insert(to.min(len - 1), item);
        }
    }
    *resume = parse_resume(value)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(resume: &Resume) -> Vec<String> {
        resume
            .work
            .iter()
            .map(|w| w.name.clone().unwrap_or_default())
            .collect()
    }

    #[test]
    fn section_edits() {
        let mut resume = Resume::default();
        let add = |name: &str| SectionEdit::Add {
            index: None,
            item: json!({ "name": name }),
        };
        edit_section(&mut resume, "work", add("a")).unwrap();
        edit_section(&mut resume, "work", add("b")).unwrap();
        edit_section(
            &mut resume,
            "work",
            SectionEdit::Add {
                index: Some(0),
                item: json!({ "name": "c" }),
            },
        )
        .unwrap();
        assert_eq!(names(&resume), ["c", "a", "b"]);

        edit_section(&mut resume, "work", SectionEdit::Move { from: 0, to: 2 }).unwrap();
        assert_eq!(names(&resume), ["a", "b", "c"]);

        edit_section(
            &mut resume,
            "work",
            SectionEdit::Update {
                index: 1,
                patch: json!({ "position": "CTO" }),
            },
        )
        .unwrap();
        assert_eq!(resume.work[1].position.as_deref(), Some("CTO"));

        edit_section(&mut resume, "work", SectionEdit::Remove { index: 0 }).unwrap();
        assert_eq!(names(&resume), ["b", "c"]);

        assert!(edit_section(&mut resume, "work", SectionEdit::Remove { index: 9 }).is_err());
        assert!(edit_section(&mut resume, "basics", add("x")).is_err());
        assert!(
            edit_section(
                &mut resume,
                "work",
                SectionEdit::Add {
                    index: None,
                    item: json!({ "highlights": "not a list" })
                }
            )
            .is_err()
        );
    }

    #[test]
    fn patch_validates() {
        let mut resume = Resume::default();
        patch_resume(&mut resume, &json!({ "basics": { "name": "Ada" } })).unwrap();
        assert_eq!(resume.basics.name.as_deref(), Some("Ada"));
        assert!(patch_resume(&mut resume, &json!({ "work": "nope" })).is_err());
        assert_eq!(resume.basics.name.as_deref(), Some("Ada"));
    }
}
