// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The fields the user defines (see [`helikodu_model::user_fields`]): computed
//! fields and custom fields, kept in the library's database.
//!
//! They are kept in the database rather than in the settings because what uses
//! them is there too: saved searches and smart playlists name them, and labels
//! sit beside them, so that one export of the database carries all of it. Opening
//! the library puts its fields in use for the whole program; each change puts the
//! new set in use at once.
//!
//! In the query language a computed field is compared with text as a text field
//! is (`decade:1990s`, `decade=1990s`, `decade~^19`, `has:decade`): the library
//! works its value out for every track before the query runs, which takes a
//! moment on a large library. A query cannot sort by one; a list of tracks is
//! sorted and grouped by it with [`Library::sort_by_expression`] and
//! [`Library::group_by_expression`].

use std::collections::HashMap;

use helikodu_model::expression::Template;
use helikodu_model::library::TrackId;
use helikodu_model::query::{Expr, Field, Query, SortBy};
use helikodu_model::user_fields::{
    self, DefinitionError, DefinitionProblem, FieldDefinition, UserField, UserFields,
};
use rusqlite::{Connection, params};

use crate::Library;
use crate::catalog::sql_track_id;
use crate::db::DbError;
use crate::expressions::values_on;

/// Why a field could not be defined or removed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FieldError {
    /// The database failed.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The definition cannot be used: what is wrong, and where.
    #[error(transparent)]
    Definition(#[from] DefinitionError),
    /// There is no field of that name.
    #[error("there is no field {0}")]
    Unknown(String),
    /// A computed field uses the field, which so cannot be removed.
    #[error("{used_by} uses {field}")]
    InUse {
        /// The field to remove.
        field: String,
        /// The computed field that uses it.
        used_by: String,
    },
}

/// The definitions stored, in their order.
pub(crate) fn stored(c: &Connection) -> rusqlite::Result<Vec<UserField>> {
    c.prepare_cached("SELECT name, kind, definition FROM user_fields ORDER BY position, name")?
        .query_map([], |row| {
            let name: String = row.get(0)?;
            let definition: String = row.get(2)?;
            Ok(match row.get::<_, i64>(1)? {
                1 => UserField::tag(name, definition),
                _ => UserField::computed(name, definition),
            })
        })?
        .collect()
}

/// Replaces the stored definitions by those of `set`.
fn store(c: &Connection, set: &UserFields) -> rusqlite::Result<()> {
    c.execute("DELETE FROM user_fields", [])?;
    let mut insert = c.prepare_cached(
        "INSERT INTO user_fields (name, kind, definition, position) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (position, field) in set.definitions().iter().enumerate() {
        let (kind, definition) = match &field.definition {
            FieldDefinition::Computed { expression } => (0, expression),
            FieldDefinition::Tag { key } => (1, key),
        };
        insert.execute(params![
            field.name,
            kind,
            definition,
            i64::try_from(position).unwrap_or(i64::MAX)
        ])?;
    }
    Ok(())
}

impl Library {
    /// The fields the user defined, in their order, with their names in lower case.
    pub fn user_fields(&self) -> Result<Vec<UserField>, DbError> {
        self.database().read(stored)
    }

    /// Defines `field`, or replaces the field of its name, after checking it with
    /// every other field. The definition as stored: the name in lower case, a tag
    /// as its key.
    pub fn define_field(&self, field: UserField) -> Result<UserField, FieldError> {
        let built = self.database().writer()?.write(move |c| {
            let mut fields = stored(c)?;
            match fields
                .iter_mut()
                .find(|f| f.name.eq_ignore_ascii_case(&field.name))
            {
                Some(old) => *old = field.clone(),
                None => fields.push(field.clone()),
            }
            let set = match UserFields::build(&fields) {
                Ok(set) => set,
                Err(error) => return Ok(Err(error)),
            };
            store(c, &set)?;
            let stored = set.get(&field.name).cloned();
            Ok(Ok((set, stored)))
        })?;
        let (set, stored) = built?;
        user_fields::install(set);
        stored.ok_or(FieldError::Unknown(String::new()))
    }

    /// Removes the field `name`. A field a computed field uses stays.
    pub fn remove_field(&self, name: &str) -> Result<(), FieldError> {
        let name = name.trim().to_ascii_lowercase();
        let built = self.database().writer()?.write(move |c| {
            let mut fields = stored(c)?;
            let before = fields.len();
            fields.retain(|f| !f.name.eq_ignore_ascii_case(&name));
            if fields.len() == before {
                return Ok(Err(FieldError::Unknown(name)));
            }
            let set = match UserFields::build(&fields) {
                Ok(set) => set,
                Err(error) => {
                    let unknown = matches!(
                        &error.problem,
                        DefinitionProblem::Expression(
                            helikodu_model::expression::Problem::UnknownField(n)
                        ) if n.eq_ignore_ascii_case(&name)
                    );
                    return Ok(Err(if unknown {
                        FieldError::InUse {
                            field: name,
                            used_by: error.name,
                        }
                    } else {
                        FieldError::Definition(error)
                    }));
                }
            };
            store(c, &set)?;
            Ok(Ok(set))
        })?;
        user_fields::install(built?);
        Ok(())
    }

    /// Puts the fields of this library in use, leaving out any that cannot be
    /// used now. A library too old to keep fields has none.
    pub(crate) fn install_user_fields(&self) {
        let fields = self.database().read(stored).unwrap_or_default();
        let (set, _) = UserFields::build_lenient(&fields);
        user_fields::install(set);
    }

    /// The value of the computed or custom field `name` for each of `tracks`, in
    /// their order; tracks the library does not have are left out.
    pub fn field_values(
        &self,
        name: &str,
        tracks: &[TrackId],
    ) -> Result<Vec<(TrackId, String)>, FieldError> {
        let Some(field) = Field::from_name(name) else {
            return Err(FieldError::Unknown(name.to_owned()));
        };
        let template = Template::parse(&format!("{{raw({})}}", name.trim()))
            .map_err(|_| FieldError::Unknown(field.name().into_owned()))?;
        Ok(self.evaluate_expression(&template, tracks)?)
    }

    /// The values the tag `key` has in the library, each with its number of
    /// tracks, in name order (values that differ only in case or accents are
    /// one), when there are at most `limit` of them; `None` when there are more.
    /// A search completion offers them after the name of a custom field.
    pub fn tag_values(
        &self,
        key: &str,
        limit: usize,
    ) -> Result<Option<Vec<(String, usize)>>, DbError> {
        let key = key.trim().to_ascii_uppercase();
        let most = i64::try_from(limit.saturating_add(1)).unwrap_or(i64::MAX);
        let values: Vec<(String, usize)> = self.database().read(move |c| {
            c.prepare_cached(
                "SELECT min(value), count(DISTINCT track_id) FROM track_tags
                 WHERE key = ?1 AND trim(value) <> ''
                 GROUP BY hk_fold(value) ORDER BY min(value) COLLATE HK_LOCALE LIMIT ?2",
            )?
            .query_map(params![key, most], |row| {
                Ok((
                    row.get(0)?,
                    usize::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                ))
            })?
            .collect()
        })?;
        Ok((values.len() <= limit).then_some(values))
    }
}

/// The computed fields `query` compares or tests, each once.
pub(crate) fn computed_in(query: &Query) -> Vec<String> {
    fn walk(expr: &Expr, out: &mut Vec<String>) {
        let field = match expr {
            Expr::All(children) | Expr::Any(children) => {
                for child in children {
                    walk(child, out);
                }
                return;
            }
            Expr::Not(inner) => return walk(inner, out),
            Expr::Compare(comparison) => &comparison.field,
            Expr::Has(helikodu_model::query::Presence::Field(field)) => field,
            _ => return,
        };
        if let Field::Computed(name) = field
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    }
    let mut out = Vec::new();
    if let Some(filter) = &query.filter {
        walk(filter, &mut out);
    }
    for key in &query.sort {
        if let SortBy::Field(Field::Computed(name)) = &key.by
            && !out.contains(name)
        {
            out.push(name.clone());
        }
    }
    out
}

/// The value of each computed field of `names` for every track that has one, by
/// track id. A field no longer defined has no values.
pub(crate) fn computed_values(
    c: &Connection,
    names: &[String],
) -> rusqlite::Result<HashMap<String, HashMap<i64, String>>> {
    let mut out = HashMap::new();
    if names.is_empty() {
        return Ok(out);
    }
    let fields = user_fields::current();
    let tracks: Vec<TrackId> = c
        .prepare_cached("SELECT id FROM tracks")?
        .query_map([], |row| row.get(0).map(crate::catalog::track_id_from_sql))?
        .collect::<rusqlite::Result<_>>()?;
    for name in names {
        let mut values = HashMap::new();
        if let Some(template) = fields.expression(name) {
            for (track, map) in values_on(c, &tracks, Some(&template.fields()))? {
                let value = template.evaluate(&map);
                if !value.trim().is_empty() {
                    values.insert(sql_track_id(track), value);
                }
            }
        }
        out.insert(name.clone(), values);
    }
    Ok(out)
}
