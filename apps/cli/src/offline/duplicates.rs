// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli duplicates`: the groups of copies (byte-identical files) in the
//! library. It only lists them; nothing is moved or deleted.

use std::io::{self, Write};

use anyhow::Result;
use clap::Args;
use helikodu_service::library::{CopyFile, CopyGroup};
use serde_json::{Map, Value, json};

use super::database::{DatabaseArgs, insert_path};
use super::format;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct DuplicatesArgs {
    /// Print the groups as JSON, for scripts.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    database: DatabaseArgs,
}

pub(crate) fn run(args: &DuplicatesArgs) -> Result<Outcome> {
    let library = args.database.open_existing()?;
    let groups = library.copy_groups()?;
    let mut out = io::stdout().lock();
    if args.json {
        let groups: Vec<Value> = groups.iter().map(group_json).collect();
        serde_json::to_writer_pretty(&mut out, &json!({ "groups": groups }))?;
        writeln!(out)?;
    } else {
        print_groups(&mut out, &groups)?;
    }
    out.flush()?;
    Ok(Outcome::Success)
}

fn group_json(group: &CopyGroup) -> Value {
    let files: Vec<Value> = group.files.iter().map(file_json).collect();
    json!({
        "main_file_id": group.main.get(),
        "files": files,
    })
}

fn file_json(file: &CopyFile) -> Value {
    let mut object = Map::new();
    object.insert("file_id".into(), json!(file.file.get()));
    insert_path(&mut object, &file.path);
    object.insert("main".into(), json!(file.is_main));
    object.insert("missing".into(), json!(file.missing));
    object.insert(
        "same_file_as".into(),
        json!(file.same_file_as.map(|f| f.get())),
    );
    object.insert("format".into(), json!(file.format));
    object.insert("size_bytes".into(), json!(file.size));
    object.insert("added".into(), json!(file.added.map(format::unix_seconds)));
    object.insert("root".into(), json!(file.root.to_string_lossy()));
    object.insert("track_id".into(), json!(file.track.map(|t| t.get())));
    Value::Object(object)
}

fn print_groups(out: &mut impl Write, groups: &[CopyGroup]) -> io::Result<()> {
    if groups.is_empty() {
        return writeln!(out, "The library has no copies.");
    }
    let mut copies = 0;
    let mut size = 0;
    for group in groups {
        for file in &group.files {
            let mark = if file.is_main {
                "main"
            } else if file.same_file_as.is_some() {
                "same file"
            } else {
                copies += 1;
                size += file.size;
                "copy"
            };
            let missing = if file.missing { " (missing)" } else { "" };
            writeln!(out, "{mark:>9}  {}{missing}", file.path.display())?;
        }
        writeln!(out)?;
    }
    writeln!(
        out,
        "{} groups; {copies} copies besides the main copies, {}.",
        groups.len(),
        format::size(size)
    )
}
