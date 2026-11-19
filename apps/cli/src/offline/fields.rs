// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli fields`: the fields the user defines. A computed field is a name
//! for an expression (`decade` = `{left(year,3)}0s`), a custom field a name for a
//! tag (`mood` = `MOOD`). Both can then be named in searches and naming patterns.

use std::io::{self, Write};

use anyhow::{Result, anyhow};
use clap::{Args, Subcommand};
use helikodu_model::library::TrackId;
use helikodu_service::library::{
    DefinitionError, DefinitionPart, FieldDefinition, FieldError, LibraryError, UserField,
};
use serde_json::{Value, json};

use super::database::DatabaseArgs;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct FieldsArgs {
    #[command(subcommand)]
    command: FieldsCommand,
    #[command(flatten)]
    database: DatabaseArgs,
}

#[derive(Debug, Subcommand)]
enum FieldsCommand {
    /// List the computed and custom fields.
    List {
        /// Print the fields as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Define a field, or change the field of that name.
    Define {
        /// The name: letters a to z and _, starting with a letter.
        name: String,
        /// The expression of a computed field, such as '{left(year,3)}0s'.
        #[arg(required_unless_present = "tag", conflicts_with = "tag")]
        expression: Option<String>,
        /// Make a custom field for this tag instead, such as MOOD (a TXXX: or
        /// ----:com.apple.iTunes: in front is left out).
        #[arg(long, value_name = "TAG")]
        tag: Option<String>,
    },
    /// Remove a field. A field a computed field uses cannot be removed.
    Remove {
        /// The name.
        name: String,
    },
    /// Show the value of a field for library tracks.
    Value {
        /// The name of the field: one you defined or any other.
        name: String,
        /// The tracks, by the ids `search` and `info` print.
        #[arg(required = true, value_name = "ID")]
        tracks: Vec<u64>,
    },
}

pub(crate) fn run(args: &FieldsArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let mut out = io::stdout().lock();
    match &args.command {
        FieldsCommand::List { json } => {
            let fields = library.user_fields()?;
            if *json {
                let fields: Vec<Value> = fields.iter().map(field_json).collect();
                serde_json::to_writer_pretty(&mut out, &json!({ "fields": fields }))?;
                writeln!(out)?;
            } else if fields.is_empty() {
                writeln!(out, "No fields defined.")?;
            } else {
                for field in &fields {
                    match &field.definition {
                        FieldDefinition::Computed { expression } => {
                            writeln!(out, "{} = {expression}", field.name)?;
                        }
                        FieldDefinition::Tag { key } => {
                            writeln!(out, "{} = tag {key}", field.name)?;
                        }
                    }
                }
            }
        }
        FieldsCommand::Define {
            name,
            expression,
            tag,
        } => {
            let field = match (expression, tag) {
                (_, Some(tag)) => UserField::tag(name, tag),
                (Some(expression), None) => UserField::computed(name, expression),
                (None, None) => return Err(anyhow!("give an expression or --tag")),
            };
            let text = match &field.definition {
                FieldDefinition::Computed { expression } => expression.clone(),
                FieldDefinition::Tag { key } => key.clone(),
            };
            match library.define_field(field) {
                Ok(_) => {}
                Err(LibraryError::Field(FieldError::Definition(error))) => {
                    return Err(anyhow!(explain(&error, name, &text)));
                }
                Err(error) => return Err(error.into()),
            }
        }
        FieldsCommand::Remove { name } => library.remove_field(name)?,
        FieldsCommand::Value { name, tracks } => {
            let tracks: Vec<TrackId> = tracks.iter().copied().map(TrackId::new).collect();
            for (track, value) in library.field_values(name, &tracks)? {
                writeln!(out, "{:>7}  {value}", track.get())?;
            }
        }
    }
    out.flush()?;
    Ok(Outcome::Success)
}

fn field_json(field: &UserField) -> Value {
    match &field.definition {
        FieldDefinition::Computed { expression } => {
            json!({ "name": field.name, "kind": "computed", "expression": expression })
        }
        FieldDefinition::Tag { key } => {
            json!({ "name": field.name, "kind": "tag", "tag": key })
        }
    }
}

/// The problem with the part of the definition it is in, marked below it.
fn explain(error: &DefinitionError, name: &str, definition: &str) -> String {
    let text = match error.part {
        DefinitionPart::Name => name,
        DefinitionPart::Definition => definition,
    };
    let width = error.end.saturating_sub(error.start).max(1);
    format!(
        "{}\n  {text}\n  {}{}",
        error.problem,
        " ".repeat(error.start),
        "^".repeat(width)
    )
}
