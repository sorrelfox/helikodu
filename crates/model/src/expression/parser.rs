// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Reading template text: the text, brackets and folders of the template, and the
//! expressions in braces. Every error has the characters it is about.

use super::{Comparison, Error, Function, MAX_BRACKET_DEPTH, MAX_NESTING, Node, Part, Problem};
use crate::user_fields::{Lookup, UserFields};

type Result<T> = std::result::Result<T, Error>;

/// Reads `text`; with `path`, `/` starts a folder.
pub(super) fn parse(text: &str, path: bool, user: &UserFields) -> Result<Vec<Part>> {
    let chars: Vec<char> = text.chars().collect();
    if text.trim().is_empty() {
        return Err(error(Problem::Empty, 0, chars.len()));
    }
    let mut parser = Parser {
        chars: &chars,
        pos: 0,
        path,
        user,
        brace: 0,
        scopes: Vec::new(),
        depth: 0,
    };
    let parts = parser.parts(None, 0)?;
    if path {
        let mut check = FolderCheck::default();
        check.parts(&parts)?;
        check.end(chars.len())?;
    }
    Ok(parts)
}

fn error(problem: Problem, start: usize, end: usize) -> Error {
    Error {
        problem,
        start,
        end,
    }
}

/// A token inside braces.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Text(String),
    Number(String),
    Open,
    Close,
    Comma,
    Compare(Comparison),
    And,
    Or,
    Not,
    /// The `}` that ends the braces.
    End,
    /// The end of the template.
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    start: usize,
    end: usize,
}

struct Parser<'a> {
    chars: &'a [char],
    pos: usize,
    path: bool,
    /// The fields the user defined.
    user: &'a UserFields,
    /// Where the braces being read open.
    brace: usize,
    /// The names `let` binds, outermost first, in lower case.
    scopes: Vec<String>,
    depth: usize,
}

impl Parser<'_> {
    /// Reads parts until the end, or the `]` that closes the bracket opened at
    /// `open`.
    fn parts(&mut self, open: Option<usize>, depth: usize) -> Result<Vec<Part>> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, parts: &mut Vec<Part>| {
            if !text.is_empty() {
                parts.push(Part::Text(std::mem::take(text)));
            }
        };
        while let Some(&c) = self.chars.get(self.pos) {
            match c {
                '{' => {
                    flush(&mut text, &mut parts);
                    let node = self.braced()?;
                    parts.push(Part::Value(node));
                }
                '[' => {
                    flush(&mut text, &mut parts);
                    let start = self.pos;
                    if depth >= MAX_BRACKET_DEPTH {
                        return Err(error(Problem::TooDeep, start, start + 1));
                    }
                    self.pos += 1;
                    let inner = self.parts(Some(start), depth + 1)?;
                    parts.push(Part::Optional(inner));
                }
                ']' => {
                    if open.is_some() {
                        flush(&mut text, &mut parts);
                        self.pos += 1;
                        return Ok(parts);
                    }
                    return Err(error(Problem::UnexpectedClose(']'), self.pos, self.pos + 1));
                }
                '}' => {
                    return Err(error(Problem::UnexpectedClose('}'), self.pos, self.pos + 1));
                }
                '/' if self.path => {
                    flush(&mut text, &mut parts);
                    parts.push(Part::Folder(self.pos));
                    self.pos += 1;
                }
                c => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
        if let Some(start) = open {
            return Err(error(Problem::UnclosedBracket, start, self.chars.len()));
        }
        flush(&mut text, &mut parts);
        Ok(parts)
    }

    /// Reads `{expression}` from the `{` on.
    fn braced(&mut self) -> Result<Node> {
        let start = self.pos;
        self.brace = start;
        self.pos += 1;
        self.skip_ws();
        let first = self.pos;
        match self.chars.get(self.pos) {
            None => return Err(error(Problem::UnclosedField, start, self.chars.len())),
            Some('}') => {
                self.pos += 1;
                return Err(error(Problem::EmptyField, start, self.pos));
            }
            _ => {}
        }
        let node = match self.or() {
            Ok(node) => node,
            Err(mut e) => {
                // A lone unknown name: the whole braces, as the field it meant to be.
                if matches!(e.problem, Problem::UnknownField(_)) && e.start == first {
                    let mut after = e.end;
                    while self.chars.get(after).is_some_and(|c| c.is_whitespace()) {
                        after += 1;
                    }
                    if self.chars.get(after) == Some(&'}') {
                        e.start = start;
                        e.end = after + 1;
                    }
                }
                return Err(e);
            }
        };
        let token = self.next()?;
        match token.tok {
            Tok::End => Ok(node),
            _ => Err(self.unexpected(&token)),
        }
    }

    fn skip_ws(&mut self) {
        while self.chars.get(self.pos).is_some_and(|c| c.is_whitespace()) {
            self.pos += 1;
        }
    }

    /// The error for a token where it does not belong.
    fn unexpected(&self, token: &Token) -> Error {
        match token.tok {
            Tok::Eof => error(Problem::UnclosedField, self.brace, self.chars.len()),
            Tok::Close => error(Problem::UnexpectedClose(')'), token.start, token.end),
            Tok::End => error(Problem::MissingValue, token.start, token.end),
            _ => {
                let text: String = self.chars[token.start..token.end].iter().collect();
                error(Problem::Unexpected(text), token.start, token.end)
            }
        }
    }

    fn peek(&mut self) -> Result<Token> {
        let pos = self.pos;
        let token = self.next();
        self.pos = pos;
        token
    }

    /// The next token inside braces.
    fn next(&mut self) -> Result<Token> {
        self.skip_ws();
        let start = self.pos;
        let token = |tok: Tok, end: usize| Token { tok, start, end };
        let Some(&c) = self.chars.get(start) else {
            return Ok(token(Tok::Eof, start));
        };
        let at = |offset: usize| self.chars.get(start + offset).copied();
        let (tok, len) = match c {
            '}' => (Tok::End, 1),
            '{' => return Err(error(Problem::NestedField, start, start + 1)),
            '(' => (Tok::Open, 1),
            ')' => (Tok::Close, 1),
            ',' => (Tok::Comma, 1),
            '=' if at(1) == Some('=') => (Tok::Compare(Comparison::Equal), 2),
            '=' => (Tok::Compare(Comparison::Equal), 1),
            '!' if at(1) == Some('=') => (Tok::Compare(Comparison::NotEqual), 2),
            '!' => (Tok::Not, 1),
            '<' if at(1) == Some('=') => (Tok::Compare(Comparison::LessOrEqual), 2),
            '<' => (Tok::Compare(Comparison::Less), 1),
            '>' if at(1) == Some('=') => (Tok::Compare(Comparison::GreaterOrEqual), 2),
            '>' => (Tok::Compare(Comparison::Greater), 1),
            ':' => (Tok::Compare(Comparison::Contains), 1),
            '&' if at(1) == Some('&') => (Tok::And, 2),
            '|' if at(1) == Some('|') => (Tok::Or, 2),
            '"' | '\'' => return self.quoted(c),
            c if c.is_ascii_digit() || (c == '-' && at(1).is_some_and(|d| d.is_ascii_digit())) => {
                return Ok(self.number());
            }
            c if c.is_ascii_alphabetic() || c == '_' => return Ok(self.name()),
            _ => {
                return Err(error(Problem::Unexpected(c.to_string()), start, start + 1));
            }
        };
        self.pos = start + len;
        Ok(token(tok, self.pos))
    }

    fn quoted(&mut self, quote: char) -> Result<Token> {
        let start = self.pos;
        let mut text = String::new();
        self.pos += 1;
        loop {
            match self.chars.get(self.pos) {
                None => return Err(error(Problem::UnclosedQuote, start, self.chars.len())),
                Some(&c) if c == quote => {
                    self.pos += 1;
                    return Ok(Token {
                        tok: Tok::Text(text),
                        start,
                        end: self.pos,
                    });
                }
                Some('\\') => {
                    self.pos += 1;
                    match self.chars.get(self.pos) {
                        Some(&c) => text.push(c),
                        None => {
                            return Err(error(Problem::UnclosedQuote, start, self.chars.len()));
                        }
                    }
                    self.pos += 1;
                }
                Some(&c) => {
                    text.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Token {
        let start = self.pos;
        let mut text = String::new();
        if self.chars.get(self.pos) == Some(&'-') {
            text.push('-');
            self.pos += 1;
        }
        let digits = |p: &mut Self, text: &mut String| {
            while let Some(&c) = p.chars.get(p.pos).filter(|c| c.is_ascii_digit()) {
                text.push(c);
                p.pos += 1;
            }
        };
        digits(self, &mut text);
        if self.chars.get(self.pos) == Some(&'.')
            && self
                .chars
                .get(self.pos + 1)
                .is_some_and(char::is_ascii_digit)
        {
            text.push('.');
            self.pos += 1;
            digits(self, &mut text);
        }
        Token {
            tok: Tok::Number(text),
            start,
            end: self.pos,
        }
    }

    fn name(&mut self) -> Token {
        let start = self.pos;
        let mut name = String::new();
        while let Some(&c) = self
            .chars
            .get(self.pos)
            .filter(|c| c.is_ascii_alphanumeric() || **c == '_')
        {
            name.push(c);
            self.pos += 1;
        }
        if name.eq_ignore_ascii_case("tag") && self.chars.get(self.pos) == Some(&':') {
            name.push(':');
            self.pos += 1;
            while let Some(&c) = self
                .chars
                .get(self.pos)
                .filter(|c| c.is_ascii_alphanumeric() || **c == '_' || **c == '-')
            {
                name.push(c);
                self.pos += 1;
            }
        }
        let tok = if name.eq_ignore_ascii_case("and") {
            Tok::And
        } else if name.eq_ignore_ascii_case("or") {
            Tok::Or
        } else if name.eq_ignore_ascii_case("not") {
            Tok::Not
        } else {
            Tok::Name(name)
        };
        Token {
            tok,
            start,
            end: self.pos,
        }
    }

    /// Goes one level deeper, or refuses to.
    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_NESTING {
            let start = self.pos.min(self.chars.len());
            return Err(error(
                Problem::TooComplex,
                start,
                (start + 1).min(self.chars.len()),
            ));
        }
        Ok(())
    }

    fn or(&mut self) -> Result<Node> {
        self.enter()?;
        let mut operands = vec![self.and()?];
        while self.peek()?.tok == Tok::Or {
            self.next()?;
            operands.push(self.and()?);
        }
        self.depth -= 1;
        Ok(if operands.len() == 1 {
            operands.remove(0)
        } else {
            Node::Or(operands)
        })
    }

    fn and(&mut self) -> Result<Node> {
        let mut operands = vec![self.not()?];
        while self.peek()?.tok == Tok::And {
            self.next()?;
            operands.push(self.not()?);
        }
        Ok(if operands.len() == 1 {
            operands.remove(0)
        } else {
            Node::And(operands)
        })
    }

    fn not(&mut self) -> Result<Node> {
        if self.peek()?.tok == Tok::Not {
            self.next()?;
            self.enter()?;
            let inner = self.not()?;
            self.depth -= 1;
            return Ok(Node::Not(Box::new(inner)));
        }
        self.compare()
    }

    fn compare(&mut self) -> Result<Node> {
        let left = self.value()?;
        if let Tok::Compare(op) = self.peek()?.tok {
            self.next()?;
            let right = self.value()?;
            return Ok(Node::Compare(op, Box::new(left), Box::new(right)));
        }
        Ok(left)
    }

    fn value(&mut self) -> Result<Node> {
        let token = self.next()?;
        match token.tok {
            Tok::Text(text) | Tok::Number(text) => Ok(Node::Literal(text)),
            Tok::Open => {
                let inner = self.or()?;
                let close = self.next()?;
                match close.tok {
                    Tok::Close => Ok(inner),
                    Tok::End => Err(error(Problem::UnclosedParenthesis, token.start, token.end)),
                    _ => Err(self.unexpected(&close)),
                }
            }
            Tok::Name(name) => {
                if self.peek()?.tok == Tok::Open {
                    self.call(&name, token.start)
                } else {
                    self.resolve(&name, token.start, token.end)
                }
            }
            Tok::Comma | Tok::Compare(_) | Tok::And | Tok::Or | Tok::Not => {
                Err(error(Problem::MissingValue, token.start, token.end))
            }
            Tok::Close | Tok::End | Tok::Eof => Err(self.unexpected(&token)),
        }
    }

    /// The field or bound name `name`.
    fn resolve(&self, name: &str, start: usize, end: usize) -> Result<Node> {
        if let Some(index) = self
            .scopes
            .iter()
            .rposition(|bound| bound.eq_ignore_ascii_case(name))
        {
            return Ok(Node::Variable(index));
        }
        self.field(name)
            .ok_or_else(|| error(Problem::UnknownField(name.to_owned()), start, end))
    }

    /// The field `name`: one of the query language, or one the user defined.
    fn field(&self, name: &str) -> Option<Node> {
        Some(match self.user.lookup(name)? {
            Lookup::Field(field) => Node::Field(field),
            Lookup::Computed(computed) => Node::Computed(computed),
        })
    }

    /// Reads the arguments of the function `name` from its `(` on.
    fn call(&mut self, name: &str, start: usize) -> Result<Node> {
        let name_end = start + name.chars().count();
        let Some(function) = Function::from_name(name) else {
            return Err(error(
                Problem::UnknownFunction(name.to_owned()),
                start,
                name_end,
            ));
        };
        let open = self.next()?;
        let arity = |end: usize| {
            let (min, max) = function.arguments();
            error(
                Problem::Arguments {
                    function: function.name(),
                    min,
                    max,
                },
                start,
                end,
            )
        };
        match function {
            Function::Let => self.let_binding(open.start, &arity),
            Function::Raw => {
                let argument = self.next()?;
                let field = match &argument.tok {
                    Tok::Name(n) => self.field(n),
                    Tok::Close => return Err(arity(argument.end)),
                    Tok::End | Tok::Eof => return Err(self.unexpected(&argument)),
                    _ => None,
                };
                let Some(field) = field else {
                    return Err(error(Problem::FieldExpected, argument.start, argument.end));
                };
                let close = self.next()?;
                match close.tok {
                    Tok::Close => Ok(match field {
                        Node::Field(field) => Node::Raw(field),
                        computed => computed,
                    }),
                    Tok::Comma => Err(arity(close.end)),
                    Tok::End => Err(error(Problem::UnclosedParenthesis, open.start, open.end)),
                    _ => Err(self.unexpected(&close)),
                }
            }
            _ => {
                let mut arguments = Vec::new();
                let end;
                if self.peek()?.tok == Tok::Close {
                    end = self.next()?.end;
                } else {
                    loop {
                        arguments.push(self.or()?);
                        let token = self.next()?;
                        match token.tok {
                            Tok::Comma => {}
                            Tok::Close => {
                                end = token.end;
                                break;
                            }
                            Tok::End => {
                                return Err(error(
                                    Problem::UnclosedParenthesis,
                                    open.start,
                                    open.end,
                                ));
                            }
                            _ => return Err(self.unexpected(&token)),
                        }
                    }
                }
                let (min, max) = function.arguments();
                if arguments.len() < min || max.is_some_and(|max| arguments.len() > max) {
                    return Err(arity(end));
                }
                Ok(Node::Call(function, arguments))
            }
        }
    }

    /// `let(name, value, body)` after its `(`.
    fn let_binding(&mut self, open: usize, arity: &dyn Fn(usize) -> Error) -> Result<Node> {
        let unclosed = |p: &Self, token: &Token| match token.tok {
            Tok::End => error(Problem::UnclosedParenthesis, open, open + 1),
            Tok::Close => arity(token.end),
            _ => p.unexpected(token),
        };
        let name = self.next()?;
        let Tok::Name(bound) = &name.tok else {
            if name.tok == Tok::Close {
                return Err(arity(name.end));
            }
            return Err(error(Problem::NameExpected, name.start, name.end));
        };
        if bound.contains(':') {
            return Err(error(Problem::NameExpected, name.start, name.end));
        }
        let comma = self.next()?;
        if comma.tok != Tok::Comma {
            return Err(unclosed(self, &comma));
        }
        let value = self.or()?;
        let comma = self.next()?;
        if comma.tok != Tok::Comma {
            return Err(unclosed(self, &comma));
        }
        self.scopes.push(bound.clone());
        let body = self.or();
        self.scopes.pop();
        let body = body?;
        let close = self.next()?;
        match close.tok {
            Tok::Close => Ok(Node::Let(Box::new(value), Box::new(body))),
            Tok::Comma => Err(arity(close.end)),
            _ => Err(unclosed(self, &close)),
        }
    }
}

/// Refuses a path template whose folder names would always be empty: a `/` at
/// the start or the end, or two with nothing between them but brackets and
/// spaces.
#[derive(Default)]
struct FolderCheck {
    last_slash: Option<usize>,
    seen_content: bool,
}

impl FolderCheck {
    fn parts(&mut self, parts: &[Part]) -> Result<()> {
        for part in parts {
            match part {
                Part::Text(text) => {
                    if text.chars().any(|c| !c.is_whitespace()) {
                        self.seen_content = true;
                    }
                }
                Part::Value(_) => self.seen_content = true,
                Part::Optional(inner) => self.parts(inner)?,
                Part::Folder(at) => {
                    if !self.seen_content {
                        return Err(error(
                            Problem::EmptyFolder,
                            self.last_slash.unwrap_or(*at),
                            at + 1,
                        ));
                    }
                    self.last_slash = Some(*at);
                    self.seen_content = false;
                }
            }
        }
        Ok(())
    }

    fn end(&self, len: usize) -> Result<()> {
        if self.seen_content {
            return Ok(());
        }
        Err(error(
            Problem::EmptyFolder,
            self.last_slash.unwrap_or(0),
            len,
        ))
    }
}
