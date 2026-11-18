// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask check-translations`: checks the translations in `po/` before
//! they ship.
//!
//! Every language listed in `po/LINGUAS` needs its `po/<code>.po`, and every
//! `.po` file there must be listed. Each file must pass `msgfmt --check`,
//! which checks the header, that a message and its translation begin and end
//! alike with a line break, the number of plural forms, and the placeholders
//! of every message xgettext marked `rust-format`: as many `{}` as the English
//! text, or only numbered ones, `{0}` for its first `{}`, `{1}` for the second
//! and so on, each at least once (a plural form used for one number only may
//! leave them out), and names in braces such as `{skip}` as they are. That
//! takes GNU gettext 0.24 or newer; in CI a missing or older msgfmt is an
//! error, elsewhere its part is skipped with a notice.
//!
//! On top of that, each file:
//!
//! - holds exactly the messages of the template `po/helikodu.pot`, with the
//!   template's plurals and format flags, so that a file left behind by a
//!   newer template is caught (`msgmerge --update` brings it up to date);
//! - has no fuzzy and no untranslated messages (`--allow-untranslated` lets
//!   untranslated ones pass with a count, for work in progress);
//! - follows the placeholder rules above also in the messages without the
//!   `rust-format` mark, which msgfmt does not look at;
//! - keeps the Pango markup tags (`<b>`, `</span>`) and the text in backticks
//!   of each message;
//! - names its language in the `Language` header and has the `Plural-Forms`
//!   rule of that language.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// The template the translations are made from.
const TEMPLATE: &str = "helikodu.pot";

/// Numbers the plural rules are compared at: enough to see every rule of
/// gettext's table and CLDR's "many" forms.
const PLURAL_SAMPLES: [u64; 5] = [1_000_000, 1_000_001, 1_000_011, 2_000_000, 10_000_000];

pub(crate) fn run(args: &[String]) -> Result<()> {
    let mut allow_untranslated = false;
    for arg in args {
        match arg.as_str() {
            "--allow-untranslated" => allow_untranslated = true,
            other => bail!("unknown option `{other}` for `check-translations`"),
        }
    }
    let root = crate::repository_root()?;
    let po = root.join("po");
    let languages = linguas(&read(&po.join("LINGUAS"))?);
    let template = Catalog::parse(&read(&po.join(TEMPLATE))?)
        .map_err(|e| anyhow::anyhow!("po/{TEMPLATE}:{e}"))?;
    let msgfmt = msgfmt_checks_rust()?;

    let mut problems = Vec::new();
    let mut files: Vec<String> = fs::read_dir(&po)
        .context("cannot read po/")?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".po").map(String::from))
        .collect();
    files.sort();
    for code in files.iter().filter(|code| !languages.contains(code)) {
        problems.push(format!(
            "po/{code}.po: not listed in po/LINGUAS, so it is not installed"
        ));
    }
    for code in &languages {
        let path = po.join(format!("{code}.po"));
        let Ok(text) = fs::read_to_string(&path) else {
            problems.push(format!("po/{code}.po: missing, but listed in po/LINGUAS"));
            continue;
        };
        let catalog = match Catalog::parse(&text) {
            Ok(catalog) => catalog,
            Err(error) => {
                problems.push(format!("po/{code}.po:{error}"));
                continue;
            }
        };
        let report = check_language(code, &catalog, &template, allow_untranslated);
        println!(
            "{code}: {} translated, {} fuzzy, {} untranslated",
            report.translated, report.fuzzy, report.untranslated
        );
        problems.extend(
            report
                .problems
                .into_iter()
                .map(|p| format!("po/{code}.po:{p}")),
        );
        if msgfmt && let Err(error) = run_msgfmt(&root, &format!("po/{code}.po")) {
            problems.push(format!(
                "po/{code}.po: msgfmt --check failed:\n    {}",
                error.replace('\n', "\n    ")
            ));
        }
    }
    if !problems.is_empty() {
        bail!(
            "the translations have {} problem(s):\n  {}",
            problems.len(),
            problems.join("\n  ")
        );
    }
    println!(
        "check-translations: {} languages, {} messages each",
        languages.len(),
        template.messages.len()
    );
    Ok(())
}

fn read(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))
}

/// The language codes of a `LINGUAS` file: separated by white space, after
/// `#` a comment.
fn linguas(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default())
        .flat_map(str::split_whitespace)
        .map(String::from)
        .collect()
}

/// Whether msgfmt can run and checks Rust format strings (GNU gettext 0.24
/// and newer do). Outside CI, when it cannot, its check is skipped with a
/// notice; in CI that is an error.
fn msgfmt_checks_rust() -> Result<bool> {
    // Translations that swap the placeholders, and that leave the second one
    // out: msgfmt must take the first and refuse the second.
    let sample = |translation: &str| {
        format!(
            "msgid \"\"\nmsgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n\n\
             #, rust-format\nmsgid \"{{}} of {{}}\"\nmsgstr \"{translation}\"\n"
        )
    };
    let found = Command::new("msgfmt")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    let checks = found
        && run_msgfmt_on(&sample("{1} von {0}")).is_ok()
        && run_msgfmt_on(&sample("{}")).is_err();
    if !checks {
        let reason = if found {
            "msgfmt does not check Rust format strings (GNU gettext 0.24 or newer does)"
        } else {
            "msgfmt is not installed (it comes with GNU gettext)"
        };
        if env::var_os("CI").is_some() {
            bail!("{reason}");
        }
        println!(
            "check-translations: {reason}; the placeholders of the marked messages, \
             the line breaks and the plural forms are not checked"
        );
    }
    Ok(checks)
}

/// Runs `msgfmt --check` on the PO text `po`.
fn run_msgfmt_on(po: &str) -> std::result::Result<(), String> {
    use std::io::Write;
    let mut child = Command::new("msgfmt")
        .args(["--check", "--output-file=/dev/null", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(po.as_bytes()).map_err(|e| e.to_string())?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_owned())
    }
}

/// Runs `msgfmt --check` on the file `path`, relative to `root`.
fn run_msgfmt(root: &Path, path: &str) -> std::result::Result<(), String> {
    let output = Command::new("msgfmt")
        .args(["--check", "--output-file=/dev/null", path])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr)
            .trim_end()
            .to_owned())
    }
}

// --- Reading PO files ------------------------------------------------------

/// One message of a PO file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Message {
    /// The line of its `msgid`.
    line: usize,
    context: Option<String>,
    id: String,
    plural: Option<String>,
    /// `msgstr`, or `msgstr[0]`, `msgstr[1]`, … of a message with a plural.
    strings: Vec<String>,
    flags: Vec<String>,
}

impl Message {
    fn key(&self) -> (Option<&str>, &str) {
        (self.context.as_deref(), &self.id)
    }

    fn fuzzy(&self) -> bool {
        self.flags.iter().any(|f| f == "fuzzy")
    }

    fn format_flags(&self) -> BTreeSet<&str> {
        self.flags
            .iter()
            .map(String::as_str)
            .filter(|f| f.ends_with("-format"))
            .collect()
    }

    /// The English text, shortened for a report.
    fn shown(&self) -> String {
        let text: String = self.id.chars().take(60).collect();
        if text.len() < self.id.len() {
            format!("{text:?}…")
        } else {
            format!("{text:?}")
        }
    }
}

/// The messages of a PO file, without obsolete ones, and its header.
#[derive(Debug, Default)]
struct Catalog {
    header: BTreeMap<String, String>,
    header_fuzzy: bool,
    messages: Vec<Message>,
}

/// Where a string continued on the next line belongs.
#[derive(Clone, Copy)]
enum Part {
    Context,
    Id,
    Plural,
    Text(usize),
}

impl Catalog {
    fn parse(text: &str) -> std::result::Result<Self, String> {
        let mut catalog = Self::default();
        let mut current = Message::default();
        let mut seen_id = false;
        let mut part: Option<Part> = None;
        let mut flags: Vec<String> = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let number = index + 1;
            let line = raw.trim();
            let after_text = matches!(part, Some(Part::Text(_)));
            let starts_message = line.is_empty()
                || line.starts_with('#')
                || line.starts_with("msgctxt")
                || (line.starts_with("msgid ") && !line.starts_with("msgid_plural"));
            if starts_message && after_text {
                catalog.finish(std::mem::take(&mut current), seen_id);
                seen_id = false;
                part = None;
            }
            if line.is_empty() || line.starts_with("#~") {
                continue;
            }
            if let Some(found) = line.strip_prefix("#,") {
                flags.extend(
                    found
                        .split(',')
                        .map(str::trim)
                        .filter(|f| !f.is_empty())
                        .map(String::from),
                );
                continue;
            }
            if line.starts_with('#') {
                continue;
            }
            let (keyword, rest) = line.split_once([' ', '\t']).unwrap_or((line, ""));
            let value = || quoted(rest).map_err(|e| format!("{number}: {e}"));
            match keyword {
                "msgctxt" => {
                    current.context = Some(value()?);
                    part = Some(Part::Context);
                }
                "msgid" => {
                    current.id = value()?;
                    current.line = number;
                    current.flags = std::mem::take(&mut flags);
                    seen_id = true;
                    part = Some(Part::Id);
                }
                "msgid_plural" => {
                    current.plural = Some(value()?);
                    part = Some(Part::Plural);
                }
                "msgstr" => {
                    current.strings = vec![value()?];
                    part = Some(Part::Text(0));
                }
                _ if keyword.starts_with("msgstr[") && keyword.ends_with(']') => {
                    let form: usize = keyword["msgstr[".len()..keyword.len() - 1]
                        .parse()
                        .map_err(|_| format!("{number}: cannot read `{keyword}`"))?;
                    if current.strings.len() <= form {
                        current.strings.resize(form + 1, String::new());
                    }
                    current.strings[form] = value()?;
                    part = Some(Part::Text(form));
                }
                _ if line.starts_with('"') => {
                    let more = quoted(line).map_err(|e| format!("{number}: {e}"))?;
                    match part {
                        Some(Part::Context) => {
                            current.context.get_or_insert_default().push_str(&more);
                        }
                        Some(Part::Id) => current.id.push_str(&more),
                        Some(Part::Plural) => {
                            current.plural.get_or_insert_default().push_str(&more);
                        }
                        Some(Part::Text(form)) => current.strings[form].push_str(&more),
                        None => return Err(format!("{number}: a string without a keyword")),
                    }
                }
                _ => return Err(format!("{number}: cannot read `{line}`")),
            }
        }
        catalog.finish(current, seen_id);
        Ok(catalog)
    }

    fn finish(&mut self, message: Message, seen_id: bool) {
        if !seen_id {
            return;
        }
        if message.id.is_empty() && message.context.is_none() {
            self.header_fuzzy = message.fuzzy();
            for line in message.strings.first().map_or("", String::as_str).lines() {
                if let Some((name, value)) = line.split_once(':') {
                    self.header
                        .insert(name.trim().to_owned(), value.trim().to_owned());
                }
            }
            return;
        }
        self.messages.push(message);
    }
}

/// The text of a quoted PO string, its escapes resolved.
fn quoted(text: &str) -> std::result::Result<String, String> {
    let text = text.trim();
    let inner = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .filter(|_| text.len() >= 2)
        .ok_or_else(|| format!("expected a quoted string, found `{text}`"))?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('a') => out.push('\u{7}'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('v') => out.push('\u{b}'),
            Some(digit @ '0'..='7') => {
                let mut value = digit.to_digit(8).unwrap_or(0);
                for _ in 0..2 {
                    match chars.peek().and_then(|c| c.to_digit(8)) {
                        Some(d) => {
                            value = value * 8 + d;
                            chars.next();
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(value).unwrap_or('\u{fffd}'));
            }
            Some(other) => out.push(other),
            None => return Err("a string ends with a backslash".into()),
        }
    }
    Ok(out)
}

// --- Plural rules ------------------------------------------------------------

/// A plural rule: `nplurals=N; plural=EXPRESSION;`.
#[derive(Debug)]
struct PluralRule {
    forms: u64,
    expression: Expr,
}

impl PluralRule {
    fn parse(text: &str) -> std::result::Result<Self, String> {
        let mut forms = None;
        let mut expression = None;
        for field in text.split(';').map(str::trim).filter(|f| !f.is_empty()) {
            if let Some(n) = field.strip_prefix("nplurals") {
                let n = n.trim_start().strip_prefix('=').unwrap_or(n).trim();
                forms = Some(
                    n.parse()
                        .map_err(|_| format!("nplurals `{n}` is not a number"))?,
                );
            } else if let Some(e) = field.strip_prefix("plural") {
                let e = e.trim_start().strip_prefix('=').unwrap_or(e);
                expression = Some(Expr::parse(e)?);
            }
        }
        match (forms, expression) {
            // No language has more than six forms.
            (Some(forms), Some(expression)) if (1..=6).contains(&forms) => {
                Ok(Self { forms, expression })
            }
            _ => Err("it needs nplurals=N (1 to 6) and plural=EXPRESSION".into()),
        }
    }

    /// The form for `n`, if the rule can say.
    fn form(&self, n: u64) -> Option<u64> {
        self.expression.eval(n)
    }

    /// The numbers the rule is tried with.
    fn samples() -> impl Iterator<Item = u64> {
        (0..=1000).chain(PLURAL_SAMPLES)
    }

    /// Whether both rules choose the same forms.
    fn same_as(&self, other: &Self) -> bool {
        self.forms == other.forms && Self::samples().all(|n| self.form(n) == other.form(n))
    }

    /// How many of the numbers tried each form is used for.
    fn uses(&self) -> Vec<usize> {
        let mut uses = vec![0; usize::try_from(self.forms).unwrap_or(0)];
        for n in Self::samples() {
            if let Some(count) = self
                .form(n)
                .and_then(|f| usize::try_from(f).ok())
                .and_then(|f| uses.get_mut(f))
            {
                *count += 1;
            }
        }
        uses
    }
}

/// The plural rules a language may have: gettext's, and for some languages
/// CLDR's with a form for millions. Empty for a language not known here.
fn plural_rules_of(code: &str) -> &'static [&'static str] {
    const ONE_FORM: &str = "nplurals=1; plural=0;";
    const NOT_ONE: &str = "nplurals=2; plural=(n != 1);";
    const ABOVE_ONE: &str = "nplurals=2; plural=(n > 1);";
    const NOT_ONE_MILLIONS: &str =
        "nplurals=3; plural=n == 1 ? 0 : n != 0 && n % 1000000 == 0 ? 1 : 2;";
    const ABOVE_ONE_MILLIONS: &str =
        "nplurals=3; plural=(n == 0 || n == 1) ? 0 : n != 0 && n % 1000000 == 0 ? 1 : 2;";
    const EAST_SLAVIC: &str = "nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && \
                               n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);";
    const POLISH: &str = "nplurals=3; plural=(n==1 ? 0 : n%10>=2 && n%10<=4 && \
                          (n%100<10 || n%100>=20) ? 1 : 2);";
    const CZECH: &str = "nplurals=3; plural=(n==1) ? 0 : (n>=2 && n<=4) ? 1 : 2;";
    let language = code.split(['_', '-', '.', '@']).next().unwrap_or(code);
    match language {
        "ja" | "ko" | "zh" | "vi" | "th" | "id" | "ms" => &[ONE_FORM],
        "fr" => &[ABOVE_ONE, ABOVE_ONE_MILLIONS],
        "pt" if code.starts_with("pt_BR") || code.starts_with("pt-BR") => {
            &[ABOVE_ONE, ABOVE_ONE_MILLIONS]
        }
        "es" | "it" | "pt" | "ca" => &[NOT_ONE, NOT_ONE_MILLIONS],
        "de" | "nl" | "en" | "et" | "fi" | "sv" | "da" | "nb" | "nn" | "no" | "el" | "hu"
        | "tr" | "eo" | "gl" | "eu" => &[NOT_ONE],
        "ru" | "uk" | "be" => &[EAST_SLAVIC],
        "pl" => &[POLISH],
        "cs" | "sk" => &[CZECH],
        _ => &[],
    }
}

/// An expression of a plural rule, as in C: `n`, numbers, `! * / % + - < <=
/// > >= == != && || ?:` and parentheses.
#[derive(Debug)]
enum Expr {
    N,
    Number(u64),
    Not(Box<Expr>),
    Binary(Box<Expr>, Op, Box<Expr>),
    Choice(Box<Expr>, Box<Expr>, Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

impl Expr {
    fn parse(text: &str) -> std::result::Result<Self, String> {
        let tokens = tokens(text)?;
        let mut parser = Parser { tokens, at: 0 };
        let expr = parser.choice()?;
        if parser.at != parser.tokens.len() {
            return Err(format!("cannot read the plural rule `{}`", text.trim()));
        }
        Ok(expr)
    }

    /// The value for `n`; `None` on a division by zero.
    fn eval(&self, n: u64) -> Option<u64> {
        Some(match self {
            Self::N => n,
            Self::Number(value) => *value,
            Self::Not(inner) => u64::from(inner.eval(n)? == 0),
            Self::Choice(test, yes, no) => {
                if test.eval(n)? != 0 {
                    yes.eval(n)?
                } else {
                    no.eval(n)?
                }
            }
            Self::Binary(left, op, right) => {
                let l = left.eval(n)?;
                // `&&` and `||` look at the right side only when they need it.
                match op {
                    Op::And if l == 0 => return Some(0),
                    Op::Or if l != 0 => return Some(1),
                    _ => {}
                }
                let r = right.eval(n)?;
                match op {
                    Op::Or | Op::And => u64::from(r != 0),
                    Op::Eq => u64::from(l == r),
                    Op::Ne => u64::from(l != r),
                    Op::Lt => u64::from(l < r),
                    Op::Le => u64::from(l <= r),
                    Op::Gt => u64::from(l > r),
                    Op::Ge => u64::from(l >= r),
                    Op::Add => l.wrapping_add(r),
                    Op::Sub => l.wrapping_sub(r),
                    Op::Mul => l.wrapping_mul(r),
                    Op::Div => l.checked_div(r)?,
                    Op::Rem => l.checked_rem(r)?,
                }
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    N,
    Number(u64),
    Symbol(&'static str),
}

fn tokens(text: &str) -> std::result::Result<Vec<Token>, String> {
    const SYMBOLS: [&str; 19] = [
        "||", "&&", "==", "!=", "<=", ">=", "<", ">", "!", "?", ":", "(", ")", "+", "-", "*", "/",
        "%", ";",
    ];
    let mut out = Vec::new();
    let mut rest = text.trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('n') {
            out.push(Token::N);
            rest = after;
        } else if rest.starts_with(|c: char| c.is_ascii_digit()) {
            let digits = rest.chars().take_while(char::is_ascii_digit).count();
            let value = rest[..digits]
                .parse()
                .map_err(|_| format!("the number `{}` is too large", &rest[..digits]))?;
            out.push(Token::Number(value));
            rest = &rest[digits..];
        } else if let Some(symbol) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) {
            if *symbol != ";" {
                out.push(Token::Symbol(symbol));
            }
            rest = &rest[symbol.len()..];
        } else {
            return Err(format!("cannot read the plural rule at `{rest}`"));
        }
        rest = rest.trim_start();
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn eat(&mut self, symbol: &str) -> bool {
        let found = matches!(self.tokens.get(self.at), Some(Token::Symbol(s)) if *s == symbol);
        if found {
            self.at += 1;
        }
        found
    }

    fn choice(&mut self) -> std::result::Result<Expr, String> {
        let test = self.binary(0)?;
        if !self.eat("?") {
            return Ok(test);
        }
        let yes = self.choice()?;
        if !self.eat(":") {
            return Err("a `?` without its `:`".into());
        }
        let no = self.choice()?;
        Ok(Expr::Choice(Box::new(test), Box::new(yes), Box::new(no)))
    }

    /// Binary operators from the loosest (`||`) to the tightest (`* / %`).
    fn binary(&mut self, level: usize) -> std::result::Result<Expr, String> {
        const LEVELS: [&[(&str, Op)]; 6] = [
            &[("||", Op::Or)],
            &[("&&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        let Some(operators) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut left = self.binary(level + 1)?;
        'next: loop {
            for (symbol, op) in *operators {
                if self.eat(symbol) {
                    let right = self.binary(level + 1)?;
                    left = Expr::Binary(Box::new(left), *op, Box::new(right));
                    continue 'next;
                }
            }
            return Ok(left);
        }
    }

    fn unary(&mut self) -> std::result::Result<Expr, String> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("(") {
            let inner = self.choice()?;
            if !self.eat(")") {
                return Err("a `(` without its `)`".into());
            }
            return Ok(inner);
        }
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        match token {
            Some(Token::N) => Ok(Expr::N),
            Some(Token::Number(value)) => Ok(Expr::Number(value)),
            _ => Err("the plural rule ends too early or has a stray symbol".into()),
        }
    }
}

// --- Checking a language -----------------------------------------------------

/// What the check of one language found.
#[derive(Debug, Default)]
struct Report {
    translated: usize,
    fuzzy: usize,
    untranslated: usize,
    /// Each as `LINE: text`, or ` text` for the whole file.
    problems: Vec<String>,
}

fn check_language(
    code: &str,
    catalog: &Catalog,
    template: &Catalog,
    allow_untranslated: bool,
) -> Report {
    let mut report = Report::default();
    let problems = &mut report.problems;

    match catalog.header.get("Language").map(|l| l.replace('-', "_")) {
        Some(language) if language == code => {}
        Some(language) => problems.push(format!(
            " the Language header says `{language}`, the file is for `{code}`"
        )),
        None => problems.push(" the header has no Language".into()),
    }
    if catalog.header_fuzzy {
        problems.push(" the header is marked fuzzy".into());
    }
    let rule = match catalog
        .header
        .get("Plural-Forms")
        .map(|p| PluralRule::parse(p))
    {
        None => {
            problems.push(" the header has no Plural-Forms".into());
            None
        }
        Some(Err(error)) => {
            problems.push(format!(" Plural-Forms: {error}"));
            None
        }
        Some(Ok(rule)) => {
            check_plural_rule(code, &rule, problems);
            Some(rule)
        }
    };
    compare_with_template(catalog, template, problems);

    // How many of the numbers tried each plural form is used for.
    let uses = rule.as_ref().map(PluralRule::uses);
    for message in &catalog.messages {
        if message.fuzzy() {
            report.fuzzy += 1;
            continue;
        }
        if message.strings.is_empty() || message.strings.iter().any(String::is_empty) {
            report.untranslated += 1;
            continue;
        }
        report.translated += 1;
        // msgfmt checks the placeholders of the messages marked rust-format.
        let placeholders_checked = message.flags.iter().any(|f| f == "rust-format");
        for (form, translation) in message.strings.iter().enumerate() {
            let (english, single) = match (&message.plural, &uses) {
                (None, _) => (&message.id, false),
                // msgfmt reports a wrong number of forms.
                (Some(_), Some(uses)) if uses.len() != message.strings.len() => break,
                (Some(_), None) => break,
                (Some(plural), Some(uses)) => {
                    let english = if form == 0
                        && uses.len() > 1
                        && placeholders(&message.id).plain >= placeholders(plural).plain
                    {
                        &message.id
                    } else {
                        plural
                    };
                    // A form used for one number only may say it in words ("once").
                    (english, uses.get(form) == Some(&1))
                }
            };
            let mut found = Vec::new();
            if !placeholders_checked {
                found.extend(compare_placeholders(english, translation, single));
            }
            found.extend(compare_markup(english, translation));
            let which = if message.plural.is_some() {
                format!(" (form {form})")
            } else {
                String::new()
            };
            for problem in found {
                problems.push(format!(
                    "{}: {}{which}: {problem}",
                    message.line,
                    message.shown()
                ));
            }
        }
    }
    if report.fuzzy > 0 {
        problems.push(format!(
            " {} fuzzy message(s): check and confirm them, or clear them",
            report.fuzzy
        ));
    }
    if report.untranslated > 0 && !allow_untranslated {
        problems.push(format!(" {} untranslated message(s)", report.untranslated));
    }
    report
}

fn check_plural_rule(code: &str, rule: &PluralRule, problems: &mut Vec<String>) {
    if let Some(n) =
        PluralRule::samples().find(|&n| rule.form(n).is_none_or(|form| form >= rule.forms))
    {
        problems.push(format!(
            " Plural-Forms: the rule gives no form below nplurals for n = {n}"
        ));
        return;
    }
    let known = plural_rules_of(code);
    if known.is_empty() {
        return;
    }
    let matches = known
        .iter()
        .filter_map(|text| PluralRule::parse(text).ok())
        .any(|expected| rule.same_as(&expected));
    if !matches {
        problems.push(format!(
            " Plural-Forms is not the rule of `{code}`; expected {}",
            known.join(" or ")
        ));
    }
}

/// Whether the file holds the template's messages, no others, with the same
/// plurals and format flags.
fn compare_with_template(catalog: &Catalog, template: &Catalog, problems: &mut Vec<String>) {
    let ours: BTreeMap<_, _> = catalog.messages.iter().map(|m| (m.key(), m)).collect();
    let theirs: BTreeMap<_, _> = template.messages.iter().map(|m| (m.key(), m)).collect();
    let missing: Vec<&Message> = theirs
        .iter()
        .filter(|(key, _)| !ours.contains_key(*key))
        .map(|(_, m)| *m)
        .collect();
    let extra: Vec<&Message> = ours
        .iter()
        .filter(|(key, _)| !theirs.contains_key(*key))
        .map(|(_, m)| *m)
        .collect();
    let changed: Vec<&Message> = ours
        .iter()
        .filter_map(|(key, ours)| theirs.get(key).map(|theirs| (*ours, *theirs)))
        .filter(|(ours, theirs)| {
            ours.plural != theirs.plural || ours.format_flags() != theirs.format_flags()
        })
        .map(|(ours, _)| ours)
        .collect();
    let mut parts = Vec::new();
    for (list, what) in [
        (&missing, "of the template are missing"),
        (&extra, "are not in the template"),
        (
            &changed,
            "differ from the template in their plural or flags",
        ),
    ] {
        if let Some(first) = list.first() {
            parts.push(format!(
                "{} message(s) {what}, such as {}",
                list.len(),
                first.shown()
            ));
        }
    }
    if !parts.is_empty() {
        problems.push(format!(
            " out of date with po/{TEMPLATE}: {}; bring it up to date with \
             `msgmerge --update --backup=none po/<code>.po po/{TEMPLATE}`",
            parts.join("; ")
        ));
    }
}

/// The placeholders of a text.
#[derive(Debug, Default, PartialEq, Eq)]
struct Placeholders {
    /// `{}`.
    plain: usize,
    /// `{0}`, `{1}`, …
    numbered: Vec<u64>,
    /// `{skip}` and other names, sorted.
    named: Vec<String>,
}

fn placeholders(text: &str) -> Placeholders {
    let mut found = Placeholders::default();
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        let after = &rest[at + 1..];
        let Some(end) = after.find(['}', '{']) else {
            break;
        };
        let inner = &after[..end];
        if after[end..].starts_with('}') {
            if inner.is_empty() {
                found.plain += 1;
            } else if inner.bytes().all(|b| b.is_ascii_digit()) {
                found.numbered.push(inner.parse().unwrap_or(u64::MAX));
            } else if inner.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                found.named.push(inner.to_owned());
            }
            rest = &after[end + 1..];
        } else {
            rest = &after[end..];
        }
    }
    found.named.sort();
    found
}

/// What is wrong with the placeholders of `translation` of `english`, by the
/// rules msgfmt applies to the messages marked rust-format. `single`: the
/// plural form is used for one number only and may leave them out.
fn compare_placeholders(english: &str, translation: &str, single: bool) -> Vec<String> {
    let mut problems = Vec::new();
    let wanted = placeholders(english);
    let found = placeholders(translation);
    let count = u64::try_from(wanted.plain).unwrap_or(u64::MAX);
    if found.plain > 0 && !found.numbered.is_empty() {
        problems.push("mixes `{}` and numbered placeholders; use one kind".into());
    } else if !found.numbered.is_empty() {
        let used: BTreeSet<u64> = found.numbered.iter().copied().collect();
        let outside: Vec<String> = used
            .iter()
            .filter(|&&n| n >= count)
            .map(|n| format!("{{{n}}}"))
            .collect();
        let unused: Vec<String> = (0..count)
            .filter(|n| !used.contains(n))
            .map(|n| format!("{{{n}}}"))
            .collect();
        if !outside.is_empty() {
            problems.push(format!(
                "{} beyond the English text, which has {count} `{{}}` (numbering starts at {{0}})",
                outside.join(" ")
            ));
        }
        if !unused.is_empty() && !single {
            problems.push(format!("{} missing", unused.join(" ")));
        }
    } else if found.plain != wanted.plain && !(single && found.plain == 0) {
        problems.push(format!(
            "has {} `{{}}`, the English text {count}",
            found.plain
        ));
    }
    if found.named != wanted.named {
        problems.push(format!(
            "the names in braces must stay as they are: {} (the translation has {})",
            braces(&wanted.named),
            braces(&found.named)
        ));
    }
    problems
}

/// What is wrong with the markup of `translation` of `english`: its Pango
/// tags and the text in backticks must stay.
fn compare_markup(english: &str, translation: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if markup_tags(translation) != markup_tags(english) {
        problems.push(format!(
            "the markup tags must stay: {}",
            markup_tags(english).join(" ")
        ));
    }
    if backtick_spans(translation) != backtick_spans(english) {
        problems.push("the text in backticks must stay as it is".into());
    }
    problems
}

fn braces(names: &[String]) -> String {
    if names.is_empty() {
        "none".into()
    } else {
        names
            .iter()
            .map(|n| format!("{{{n}}}"))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The spans in backticks of a text, sorted.
fn backtick_spans(text: &str) -> Vec<&str> {
    let mut spans: Vec<&str> = text.split('`').skip(1).step_by(2).collect();
    if text.matches('`').count() % 2 == 1 {
        // The last one is not closed.
        spans.pop();
    }
    spans.sort_unstable();
    spans
}

/// The Pango markup tags of a text (`<b>`, `</span>`), by name, sorted. Other
/// words in angle brackets, such as `<keep>`, are text to translate.
fn markup_tags(text: &str) -> Vec<String> {
    const TAGS: [&str; 11] = [
        "a", "b", "big", "i", "s", "small", "span", "sub", "sup", "tt", "u",
    ];
    let mut tags = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('<') {
        let after = &rest[at + 1..];
        let (closing, name_from) = match after.strip_prefix('/') {
            Some(name) => (true, name),
            None => (false, after),
        };
        let name: String = name_from
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        let tail = &name_from[name.len()..];
        let ends = tail.starts_with('>') || tail.starts_with(' ') || tail.starts_with("/>");
        if TAGS.contains(&name.as_str()) && ends && tail.contains('>') {
            tags.push(format!("<{}{name}>", if closing { "/" } else { "" }));
        }
        rest = after;
    }
    tags.sort();
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE_TEXT: &str = r#"# The template.
#, fuzzy
msgid ""
msgstr ""
"Project-Id-Version: helikodu\n"
"Content-Type: text/plain; charset=UTF-8\n"
"Plural-Forms: nplurals=INTEGER; plural=EXPRESSION;\n"

#: crates/ui-gtk/src/format.rs:1
#, rust-format
msgid "{} of {}"
msgstr ""

#, rust-format
msgid "{} track"
msgid_plural "{} tracks"
msgstr[0] ""
msgstr[1] ""

msgid "<b>Bold</b> text"
msgstr ""

msgctxt "thousands separator"
msgid ","
msgstr ""

#, rust-format
msgid ""
"Use {skip} for a part "
"you do not need."
msgstr ""

msgid "Plain"
msgstr ""
"#;

    fn german(body: &str) -> String {
        format!(
            "msgid \"\"\nmsgstr \"\"\n\"Language: de\\n\"\n\
             \"Content-Type: text/plain; charset=UTF-8\\n\"\n\
             \"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n\n{body}"
        )
    }

    const GERMAN_BODY: &str = r#"#, rust-format
msgid "{} of {}"
msgstr "{1} von {0}"

#, rust-format
msgid "{} track"
msgid_plural "{} tracks"
msgstr[0] "{} Titel"
msgstr[1] "{} Titel"

msgid "<b>Bold</b> text"
msgstr "<b>Fett</b>er Text"

msgctxt "thousands separator"
msgid ","
msgstr "."

#, rust-format
msgid "Use {skip} for a part you do not need."
msgstr "{skip} steht für einen Teil, den Sie nicht brauchen."

msgid "Plain"
msgstr "Schlicht"

#~ msgid "Old"
#~ msgstr "Alt"
"#;

    fn check(po: &str) -> Report {
        let template = Catalog::parse(TEMPLATE_TEXT).unwrap();
        let catalog = Catalog::parse(po).unwrap();
        check_language("de", &catalog, &template, false)
    }

    #[test]
    fn a_complete_translation_passes() {
        let report = check(&german(GERMAN_BODY));
        assert_eq!(report.problems, Vec::<String>::new());
        assert_eq!(
            (report.translated, report.fuzzy, report.untranslated),
            (6, 0, 0)
        );
    }

    #[test]
    fn po_files_are_read() {
        let catalog = Catalog::parse(TEMPLATE_TEXT).unwrap();
        assert!(catalog.header_fuzzy);
        assert_eq!(catalog.header["Project-Id-Version"], "helikodu");
        assert_eq!(catalog.messages.len(), 6);
        let skip = &catalog.messages[4];
        assert_eq!(skip.id, "Use {skip} for a part you do not need.");
        assert_eq!(skip.flags, ["rust-format"]);
        let separator = &catalog.messages[3];
        assert_eq!(separator.key(), (Some("thousands separator"), ","));
        assert_eq!(catalog.messages[1].plural.as_deref(), Some("{} tracks"));
        assert_eq!(catalog.messages[1].strings, ["", ""]);
        assert_eq!(quoted(r#""a\"b\\c\n\t\101""#).unwrap(), "a\"b\\c\n\tA");
        assert!(Catalog::parse("msgid \"x\"\nmsgstr \"y\nmsgstr").is_err());
        assert!(Catalog::parse("bogus\n").is_err());
        // Obsolete messages are left out.
        let german = Catalog::parse(&german(GERMAN_BODY)).unwrap();
        assert!(german.messages.iter().all(|m| m.id != "Old"));
        assert_eq!(german.header["Language"], "de");
        // Messages need not be separated by blank lines.
        let tight =
            Catalog::parse("msgid \"a\"\nmsgstr \"b\"\nmsgid \"c\"\nmsgstr \"d\"\n").unwrap();
        assert_eq!(tight.messages.len(), 2);
    }

    #[test]
    fn linguas_lists_codes() {
        assert_eq!(
            linguas("# Languages\nde es\n\nfr # French\npt_BR\n"),
            ["de", "es", "fr", "pt_BR"]
        );
        assert!(linguas("# none yet\n").is_empty());
    }

    #[test]
    fn placeholders_follow_the_rules() {
        assert!(compare_placeholders("{} of {}", "{} von {}", false).is_empty());
        assert!(compare_placeholders("{} of {}", "{1} von {0}", false).is_empty());
        assert!(compare_placeholders("{} of {}", "{0}/{0}/{1}", false).is_empty());
        assert_eq!(
            compare_placeholders("{} of {}", "{}", false),
            ["has 1 `{}`, the English text 2"]
        );
        assert_eq!(
            compare_placeholders("{} of {}", "{1} von {}", false),
            ["mixes `{}` and numbered placeholders; use one kind"]
        );
        assert_eq!(
            compare_placeholders("{} of {}", "{2} von {1}", false),
            [
                "{2} beyond the English text, which has 2 `{}` (numbering starts at {0})",
                "{0} missing"
            ]
        );
        assert_eq!(
            compare_placeholders("Plain", "{} Schlicht", false),
            ["has 1 `{}`, the English text 0"]
        );
        assert_eq!(
            compare_placeholders("Plain", "{0} Schlicht", false),
            ["{0} beyond the English text, which has 0 `{}` (numbering starts at {0})"]
        );
        // A form for one number only may say it in words.
        assert!(compare_placeholders("{} times", "einmal", true).is_empty());
        assert!(compare_placeholders("{} times", "{0}-mal", true).is_empty());
        assert_eq!(
            compare_placeholders("Use {skip} here", "Nutze {springen} hier", false),
            ["the names in braces must stay as they are: {skip} (the translation has {springen})"]
        );
        assert!(
            compare_placeholders("A field name cannot hold {.", "Kein { im Namen.", false)
                .is_empty()
        );
    }

    #[test]
    fn markup_and_backticks_stay() {
        assert!(
            compare_markup("Run `helikodu-cli scan`", "Führe `helikodu-cli scan` aus").is_empty()
        );
        assert_eq!(
            compare_markup(
                "Run `helikodu-cli scan`",
                "Führe `helikodu-cli scannen` aus"
            ),
            ["the text in backticks must stay as it is"]
        );
        assert!(compare_markup("<b>Bold</b> text", "<b>Fett</b>er Text").is_empty());
        assert_eq!(
            compare_markup("<b>Bold</b> text", "Fetter Text"),
            ["the markup tags must stay: </b> <b>"]
        );
        // Words in angle brackets are text, not markup.
        assert!(compare_markup("<keep>", "<beibehalten>").is_empty());
        assert!(compare_markup("<{} new value>", "<{} neuer Wert>").is_empty());
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        let body = GERMAN_BODY
            .replace("msgstr \"Schlicht\"", "msgstr \"{} Schlicht\"")
            .replace("<b>Fett</b>er Text", "Fetter Text");
        let report = check(&german(&body));
        assert_eq!(
            report.problems,
            [
                r#"17: "<b>Bold</b> text": the markup tags must stay: </b> <b>"#,
                r#"28: "Plain": has 1 `{}`, the English text 0"#,
            ]
        );
        // msgfmt checks the placeholders of the messages marked rust-format.
        let marked = GERMAN_BODY.replace("{1} von {0}", "{2} von {1}");
        assert!(check(&german(&marked)).problems.is_empty());
    }

    #[test]
    fn fuzzy_and_untranslated_messages_are_counted() {
        let body = GERMAN_BODY
            .replace(
                "#, rust-format\nmsgid \"{} of {}\"",
                "#, fuzzy, rust-format\nmsgid \"{} of {}\"",
            )
            .replace("msgstr \".\"", "msgstr \"\"");
        let template = Catalog::parse(TEMPLATE_TEXT).unwrap();
        let catalog = Catalog::parse(&german(&body)).unwrap();
        let strict = check_language("de", &catalog, &template, false);
        assert_eq!(
            (strict.translated, strict.fuzzy, strict.untranslated),
            (4, 1, 1)
        );
        assert_eq!(
            strict.problems,
            [
                " 1 fuzzy message(s): check and confirm them, or clear them",
                " 1 untranslated message(s)"
            ]
        );
        let lenient = check_language("de", &catalog, &template, true);
        assert_eq!(
            lenient.problems,
            [" 1 fuzzy message(s): check and confirm them, or clear them"]
        );
    }

    #[test]
    fn a_file_behind_the_template_is_caught() {
        let body = GERMAN_BODY
            .replace("msgid \"{} of {}\"", "msgid \"{} von {}\"")
            .replace(
                "#, rust-format\nmsgid \"{} track\"",
                "#, no-rust-format\nmsgid \"{} track\"",
            );
        let report = check(&german(&body));
        assert_eq!(report.problems.len(), 1, "{:?}", report.problems);
        let problem = &report.problems[0];
        for part in [
            r#"1 message(s) of the template are missing, such as "{} of {}""#,
            r#"1 message(s) are not in the template, such as "{} von {}""#,
            r#"1 message(s) differ from the template in their plural or flags, such as "{} track""#,
            "msgmerge --update",
        ] {
            assert!(problem.contains(part), "{problem}");
        }
    }

    #[test]
    fn the_header_names_the_language_and_its_plural_rule() {
        let wrong = german(GERMAN_BODY)
            .replace("Language: de", "Language: fr")
            .replace("plural=(n != 1)", "plural=(n > 1)");
        assert_eq!(
            check(&wrong).problems,
            [
                " the Language header says `fr`, the file is for `de`",
                " Plural-Forms is not the rule of `de`; expected nplurals=2; plural=(n != 1);"
            ]
        );
        let missing =
            german(GERMAN_BODY).replace("\"Plural-Forms: nplurals=2; plural=(n != 1);\\n\"\n", "");
        assert_eq!(
            check(&missing).problems,
            [" the header has no Plural-Forms"]
        );
        let out_of_range = german(GERMAN_BODY).replace("plural=(n != 1)", "plural=n");
        assert_eq!(
            check(&out_of_range).problems,
            [" Plural-Forms: the rule gives no form below nplurals for n = 2"]
        );
    }

    #[test]
    fn plural_rules_are_compared_by_what_they_choose() {
        let rule = |text: &str| PluralRule::parse(text).unwrap();
        let east_slavic = rule(plural_rules_of("ru")[0]);
        assert_eq!(east_slavic.forms, 3);
        for (n, form) in [
            (1, 0),
            (21, 0),
            (1001, 0),
            (2, 1),
            (22, 1),
            (5, 2),
            (11, 2),
            (111, 2),
        ] {
            assert_eq!(east_slavic.form(n), Some(form), "n = {n}");
        }
        // The same rule written another way.
        let other = rule(
            "nplurals=3; plural=n%10==1 && n%100!=11 ? 0 : (n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20)) ? 1 : 2",
        );
        assert!(other.same_as(&east_slavic));
        assert!(
            !rule("nplurals=2; plural=(n != 1);").same_as(&rule("nplurals=2; plural=(n > 1);"))
        );
        assert!(rule("nplurals=1; plural=0;").same_as(&rule("nplurals = 1 ; plural = 0")));
        // Some languages may count millions apart, as CLDR does.
        let millions =
            rule("nplurals=3; plural=(n == 0 || n == 1) ? 0 : n != 0 && n % 1000000 == 0 ? 1 : 2;");
        assert_eq!(millions.form(2_000_000), Some(1));
        for code in ["fr", "pt_BR", "es", "it"] {
            assert_eq!(plural_rules_of(code).len(), 2, "{code}");
        }
        for code in ["de", "ja", "ko", "nl", "ru", "uk", "zh_CN"] {
            assert_eq!(plural_rules_of(code).len(), 1, "{code}");
        }
        assert!(plural_rules_of("tlh").is_empty());
        for text in [
            "nplurals=2; plural=n !=",
            "nplurals=x; plural=0;",
            "plural=n",
            "nplurals=2; plural=(n",
            "nplurals=2; plural=n ? 1",
            "nplurals=2; plural=m",
            "nplurals=0; plural=0",
            "nplurals=99999999999; plural=0",
        ] {
            assert!(PluralRule::parse(text).is_err(), "{text}");
        }
        assert_eq!(rule("nplurals=2; plural=n / 0;").form(3), None);
        assert_eq!(rule("nplurals=2; plural=!(n == 1);").form(1), Some(0));
        // How many numbers each form is for.
        assert_eq!(rule("nplurals=2; plural=(n != 1);").uses()[0], 1);
        assert!(east_slavic.uses()[0] > 1);
    }

    #[test]
    fn a_plural_form_for_one_number_may_leave_the_number_out() {
        let template = Catalog::parse(
            "msgid \"once\"\nmsgid_plural \"{} times\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n",
        )
        .unwrap();
        let problems = |po: String, code: &str| {
            let catalog = Catalog::parse(&po).unwrap();
            check_language(code, &catalog, &template, false).problems
        };
        let de = |forms: &str| {
            german(&format!(
                "msgid \"once\"\nmsgid_plural \"{{}} times\"\n{forms}"
            ))
        };
        assert!(problems(de("msgstr[0] \"einmal\"\nmsgstr[1] \"{}-mal\"\n"), "de").is_empty());
        let ru = format!(
            "msgid \"\"\nmsgstr \"\"\n\"Language: ru\\n\"\n\"Plural-Forms: {}\\n\"\n\n\
             msgid \"once\"\nmsgid_plural \"{{}} times\"\nmsgstr[0] \"один раз\"\n\
             msgstr[1] \"{{}} раза\"\nmsgstr[2] \"{{}} раз\"\n",
            plural_rules_of("ru")[0]
        );
        // The first form of Russian is also for 21, 31, …: it needs the number.
        assert_eq!(
            problems(ru, "ru"),
            [r#"6: "once" (form 0): has 0 `{}`, the English text 1"#]
        );
    }

    #[test]
    fn spans_and_tags_are_found() {
        assert_eq!(backtick_spans("a `b` c `d` e `f"), ["b", "d"]);
        assert_eq!(
            markup_tags("<span weight=\"bold\">x</span> <b>y</b> <keep> a < b"),
            ["</b>", "</span>", "<b>", "<span>"]
        );
        assert_eq!(
            placeholders("{} {2} {skip} {a b} {"),
            Placeholders {
                plain: 1,
                numbered: vec![2],
                named: vec!["skip".into()],
            }
        );
    }
}
