// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Result tables: printed, and written to `target/tmp/corpus-report/<name>.txt`.

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// A table of results, one row per file.
#[derive(Debug)]
pub(crate) struct Table {
    name: &'static str,
    header: Vec<&'static str>,
    rows: Vec<Vec<String>>,
    failures: Vec<String>,
    gaps: usize,
    unsupported: usize,
}

impl Table {
    pub(crate) fn new(name: &'static str, header: &[&'static str]) -> Self {
        Self {
            name,
            header: header.to_vec(),
            rows: Vec::new(),
            failures: Vec::new(),
            gaps: 0,
            unsupported: 0,
        }
    }

    /// Adds a row; `problem` marks it failed (the last column says why).
    pub(crate) fn row(&mut self, cells: Vec<String>, problem: Option<String>) {
        let mut cells = cells;
        match problem {
            Some(problem) => {
                self.failures.push(format!("{}: {problem}", cells[0]));
                cells.push(format!("FAIL: {problem}"));
            }
            None => cells.push("ok".into()),
        }
        self.rows.push(cells);
    }

    /// Adds a row for a known gap: reported, but not a failure.
    pub(crate) fn gap(&mut self, mut cells: Vec<String>, reason: &str) {
        cells.push(format!("GAP: {reason}"));
        self.gaps += 1;
        self.rows.push(cells);
    }

    /// Adds a row for a file that is refused on purpose: neither a gap nor a
    /// failure.
    pub(crate) fn unsupported(&mut self, mut cells: Vec<String>, reason: &str) {
        cells.push(format!("UNSUPPORTED BY DESIGN: {reason}"));
        self.unsupported += 1;
        self.rows.push(cells);
    }

    fn render(&self) -> String {
        let mut header = self.header.clone();
        header.push("result");
        let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
        let mut out = String::new();
        let line = |out: &mut String, cells: &[String]| {
            let mut text = String::new();
            for (i, cell) in cells.iter().enumerate() {
                let pad = widths[i].saturating_sub(cell.chars().count());
                let _ = write!(text, "| {cell}{} ", " ".repeat(pad));
            }
            let _ = writeln!(out, "{}|", text);
        };
        line(
            &mut out,
            &header.iter().map(|h| (*h).to_owned()).collect::<Vec<_>>(),
        );
        line(
            &mut out,
            &widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>(),
        );
        for row in &self.rows {
            line(&mut out, row);
        }
        let _ = writeln!(
            out,
            "{} rows, {} failed, {} known gaps, {} unsupported by design",
            self.rows.len(),
            self.failures.len(),
            self.gaps,
            self.unsupported
        );
        out
    }

    /// Prints and saves the table, then fails the test if any row failed.
    pub(crate) fn finish(self) {
        let text = self.render();
        println!("{}:\n{text}", self.name);
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("corpus-report");
        if fs::create_dir_all(&dir).is_ok() {
            let _ = fs::write(dir.join(format!("{}.txt", self.name)), &text);
        }
        assert!(
            self.failures.is_empty(),
            "{}: {} failed:\n{}\n\n{text}",
            self.name,
            self.failures.len(),
            self.failures.join("\n")
        );
    }
}
