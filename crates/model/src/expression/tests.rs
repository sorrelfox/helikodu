// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

use super::legacy;
use super::*;

fn track() -> FieldMap {
    let mut map = FieldMap::default()
        .with(Field::Title, "Shoreline, 6 a.m.")
        .with(Field::AlbumArtist, "The Tidewater Trio")
        .with(Field::Album, "Tidewater Rooms")
        .with(Field::Year, "2021")
        .with(Field::Track, "3")
        .with(Field::Disc, "2")
        .with(Field::Duration, "225.4")
        .with(Field::Added, "2026-03-04 17:05:09")
        .with(Field::Tag("MOOD".into()), "calm");
    map.set_list(
        Field::Artist,
        vec!["Mari Kask".into(), "Okto".into(), " ".into()],
    );
    map.disc_count = Some(2);
    map.track_count = Some(12);
    map
}

fn eval(text: &str) -> String {
    Template::parse(text).unwrap().evaluate(&track())
}

fn problem(text: &str) -> Error {
    Template::parse(text).unwrap_err()
}

/// The pieces of `template` in the old reader's terms.
fn pieces(template: &Template, values: &dyn FieldValues) -> Vec<legacy::Piece> {
    let mut out = Vec::new();
    template.pieces(values, &mut |piece| {
        out.push(match piece {
            Piece::Text(t) => legacy::Piece::Text(t.to_owned()),
            Piece::Value(v) => legacy::Piece::Value(v),
            Piece::Folder => legacy::Piece::Folder,
        });
    });
    out
}

/// Value sets that exercise the brackets and the number rules.
fn value_sets() -> Vec<FieldMap> {
    let full = track();
    let mut one_disc = full.clone();
    one_disc.disc_count = Some(1);
    one_disc.values.insert(Field::Disc, "1".into());
    let mut sparse = FieldMap::default()
        .with(Field::Title, "Departure")
        .with(Field::Track, "117");
    sparse.track_count = Some(130);
    vec![full, one_disc, sparse, FieldMap::default()]
}

fn assert_compatible(text: &str) {
    let old =
        legacy::Pattern::parse(text).unwrap_or_else(|| panic!("the old reader refuses {text:?}"));
    let new =
        Template::parse_path(text).unwrap_or_else(|e| panic!("{text:?} no longer reads: {e}"));
    assert_eq!(new.fields(), old.fields(), "{text:?}");
    assert_eq!(new.text(), text);
    for values in value_sets() {
        assert_eq!(
            pieces(&new, &values),
            old.pieces(&values),
            "{text:?} with {values:?}"
        );
    }
}

/// Every pattern in the user guide, the design and the defaults.
#[test]
fn patterns_of_the_docs_and_the_design_read_as_before() {
    for text in [
        "{albumartist}/{album}/[{disc}-]{track} {title}",
        "{albumartist}/{year} {album}/[{disc}-]{track} {title}",
        "{album}[ ({year})]",
        "{album}/[Disc {disc}/]{track}",
        "[{disc}-]{track}",
        "[{composer} [({year})] ]",
        "[{year} [{disc}]x ]{track}",
        "[{composer} [{year}]]{track}",
        "{composer}/{title}",
        "{album_artist}/{tracknumber} {tag:mood} {track}",
        "{tag:MOOD}",
        "{ title }",
        "{Title} - {ARTIST}",
        "{title}/x",
        "CON",
        "com1.x",
        "{genre}",
        "{albumartist}/{album}/{track} {title}",
        "Mari's {album} (live)",
    ] {
        assert_compatible(text);
    }
}

/// A cheap generator of strings from pieces, without a random crate.
struct Pieces(u64);

impl Pieces {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap() % n
    }
}

/// Strings made of the old syntax's pieces read alike: the same ones are
/// refused (but for quotes, which are new inside braces), and the others give
/// the same fields and pieces.
#[test]
fn generated_old_patterns_read_as_before() {
    const ALPHABET: &[&str] = &[
        "{title}",
        "{disc}",
        "{track}",
        "{composer}",
        "{tag:mood}",
        "{ year }",
        "{nope}",
        "{}",
        "[",
        "]",
        "/",
        " ",
        "-",
        "x",
        "{",
        "}",
        ".",
        "'",
    ];
    let mut random = Pieces(7);
    let mut read = 0;
    for _ in 0..40_000 {
        let len = 1 + random.next(9);
        let text: String = (0..len)
            .map(|_| ALPHABET[random.next(ALPHABET.len())])
            .collect();
        match legacy::Pattern::parse(&text) {
            Some(_) => {
                assert_compatible(&text);
                read += 1;
            }
            // A quote inside braces starts text now, which the old reader refused.
            None if text.contains('\'') => {}
            None => assert!(
                Template::parse_path(&text).is_err(),
                "{text:?} was refused before"
            ),
        }
    }
    assert!(read > 1000, "only {read} patterns read");
}

#[test]
fn slashes_are_text_outside_paths() {
    let template = Template::parse("{artist}/{title}").unwrap();
    let values = FieldMap::default().with(Field::Title, "x");
    assert_eq!(template.evaluate(&values), "/x");
    assert_eq!(eval("/"), "/");
    assert!(Template::parse_path("/").is_err());
}

#[test]
fn fields_and_literals() {
    assert_eq!(eval("{title}"), "Shoreline, 6 a.m.");
    assert_eq!(eval("{track}"), "03");
    assert_eq!(eval("{disc}"), "2");
    assert_eq!(eval("{raw(track)}"), "3");
    assert_eq!(eval("{'a}b'}{\"q\\\"\"}"), "a}bq\"");
    assert_eq!(eval("{-1.5}"), "-1.5");
    assert_eq!(eval("{composer}"), "");
    assert_eq!(eval("[{composer} - ]{title}"), "Shoreline, 6 a.m.");
}

#[test]
fn conditionals_and_logic() {
    assert_eq!(eval("{if(composer, composer, artist)}"), "Mari Kask, Okto");
    assert_eq!(eval("{if(year > 2000, 'new', 'old')}"), "new");
    assert_eq!(eval("{if(year < 2000, 'old')}"), "");
    assert_eq!(eval("{composer or albumartist}"), "The Tidewater Trio");
    assert_eq!(eval("{title and 'yes'}"), "yes");
    assert_eq!(eval("{composer and 'yes'}"), "");
    assert_eq!(eval("{not composer}"), "1");
    assert_eq!(eval("{!title}"), "");
    assert_eq!(eval("{year = 2021 && track = 3}"), "1");
    assert_eq!(eval("{year != 2021 || 0}"), "0");
    assert_eq!(eval("{TITLE : 'SHORE'}"), "1");
    assert_eq!(eval("{album = 'tidewater rooms'}"), "1");
    assert_eq!(eval("{'b' > 'A'}"), "1");
    assert_eq!(eval("{10 > 9}"), "1", "numbers compare as numbers");
    assert_eq!(eval("{'10' >= 'x'}"), "", "text compares as text");
    assert_eq!(eval("{not (year = 2021) or 'no'}"), "no");
    assert_eq!(eval("{If(1, 'a', 'b')} {NOT 1}"), "a ");
}

#[test]
fn text_functions() {
    assert_eq!(eval("{upper(album)}"), "TIDEWATER ROOMS");
    assert_eq!(eval("{lower(album)}"), "tidewater rooms");
    assert_eq!(eval("{left(album, 4)}|{right(album, 5)}"), "Tide|Rooms");
    assert_eq!(
        eval("{left(album, -3)}|{right(album, 99)}"),
        "|Tidewater Rooms"
    );
    assert_eq!(
        eval("{substr(album, 5, 5)}|{substr(album, 11)}"),
        "water|Rooms"
    );
    assert_eq!(eval("{len(album)}"), "15");
    assert_eq!(eval("{trim('  a b  ')}"), "a b");
    assert_eq!(eval("{replace(album, 'o', '0')}"), "Tidewater R00ms");
    assert_eq!(eval("{replace(album, '', 'x')}"), "Tidewater Rooms");
    assert_eq!(
        eval("{pad(year, 6)}|{pad(year, 6, '.')}|{pad(year, 2)}"),
        "2021  |2021..|2021"
    );
    assert_eq!(
        eval("{num(track, 3)}|{num(raw(disc), 2)}|{num('x', 3)}"),
        "003|02|x"
    );
    assert_eq!(eval("{firstword(title)}"), "Shoreline,");
    assert_eq!(eval("{sortname(albumartist)}"), "Tidewater Trio, The");
    assert_eq!(eval("{noarticle(albumartist)}"), "Tidewater Trio");
    assert_eq!(eval("{sortname('Okto')}|{sortname('The')}"), "Okto|The");
    assert_eq!(eval("{initial(noarticle(albumartist))}"), "T");
    assert_eq!(eval("{initial('2 Unlimited')}|{initial('élan')}"), "#|É");
    assert_eq!(
        eval("{concat(album, ' (', year, ')')}"),
        "Tidewater Rooms (2021)"
    );
    assert_eq!(eval("{concat()}x"), "x");
}

#[test]
fn numbers_dates_and_times() {
    assert_eq!(eval("{time(duration)}"), "3:45");
    assert_eq!(
        eval("{time(3725)}|{time(0)}|{time('n/a')}"),
        "1:02:05|0:00|n/a"
    );
    assert_eq!(
        eval("{year(added)}-{month(added)}-{day(added)}"),
        "2026-03-04"
    );
    assert_eq!(
        eval("{date(added)}|{date('2021-07')}|{date(year)}"),
        "2026-03-04|2021-07|2021"
    );
    assert_eq!(eval("{year('nope')}|{year('20210')}"), "|");
}

#[test]
fn several_values() {
    assert_eq!(eval("{artist}"), "Mari Kask, Okto");
    assert_eq!(eval("{join(artist, ' & ')}"), "Mari Kask & Okto");
    assert_eq!(
        eval("{first(artist)}|{last(artist)}|{count(artist)}"),
        "Mari Kask|Okto|2"
    );
    assert_eq!(
        eval("{count(composer)}|{first(title)}"),
        "0|Shoreline, 6 a.m."
    );
    assert_eq!(eval("{join(upper(album))}"), "TIDEWATER ROOMS");
}

#[test]
fn let_binds_names() {
    assert_eq!(
        eval("{let(a, upper(album), concat(a, '/', a))}"),
        "TIDEWATER ROOMS/TIDEWATER ROOMS"
    );
    assert_eq!(
        eval("{let(title, 'x', let(y, title, concat(title, y)))}"),
        "xx",
        "a bound name hides the field in its body"
    );
    assert_eq!(eval("{let(a, 1, a)}{title}"), "1Shoreline, 6 a.m.");
    assert_eq!(
        problem("{let(a, 1, a)}{a}").problem,
        Problem::UnknownField("a".into()),
        "a binding ends with its let"
    );
}

#[test]
fn brackets_hold_expressions() {
    assert_eq!(eval("[{if(composer, 'c')} ]x"), "x");
    assert_eq!(eval("[{time(duration)} ]x"), "3:45 x");
    let path =
        Template::parse_path("{initial(noarticle(albumartist))}/{albumartist}/[{disc}-]{track}")
            .unwrap();
    let mut names = vec![String::new()];
    path.pieces(&track(), &mut |piece| match piece {
        Piece::Text(t) => names.last_mut().unwrap().push_str(t),
        Piece::Value(v) => names.last_mut().unwrap().push_str(&v.unwrap_or_default()),
        Piece::Folder => names.push(String::new()),
    });
    assert_eq!(names, ["T", "The Tidewater Trio", "2-03"]);
    assert_eq!(
        path.fields(),
        [Field::AlbumArtist, Field::Disc, Field::Track]
    );
    assert_eq!(
        Template::parse("{if(raw(disc), let(x, composer, x), join(tag:mood))}")
            .unwrap()
            .fields(),
        [Field::Disc, Field::Composer, Field::Tag("MOOD".into())]
    );
}

#[test]
fn problems_have_their_place() {
    let at = |text: &str| {
        let e = problem(text);
        (e.problem, e.start, e.end)
    };
    assert_eq!(at("{nope}"), (Problem::UnknownField("nope".into()), 0, 6));
    assert_eq!(
        at("{upper(nope)}"),
        (Problem::UnknownField("nope".into()), 7, 11)
    );
    assert_eq!(
        at("{shout(title)}"),
        (Problem::UnknownFunction("shout".into()), 1, 6)
    );
    assert_eq!(
        at("x{upper(title, 2)}"),
        (
            Problem::Arguments {
                function: "upper",
                min: 1,
                max: Some(1)
            },
            2,
            17
        )
    );
    assert_eq!(at("{'abc}"), (Problem::UnclosedQuote, 1, 6));
    assert_eq!(at("{upper(title}"), (Problem::UnclosedParenthesis, 6, 7));
    assert_eq!(at("{(title}"), (Problem::UnclosedParenthesis, 1, 2));
    assert_eq!(at("{title)}"), (Problem::UnexpectedClose(')'), 6, 7));
    assert_eq!(at("{title =}"), (Problem::MissingValue, 8, 9));
    assert_eq!(
        at("{upper(title,)}"),
        (Problem::UnexpectedClose(')'), 13, 14)
    );
    assert_eq!(
        at("{title year}"),
        (Problem::Unexpected("year".into()), 7, 11)
    );
    assert_eq!(at("{title # 2}"), (Problem::Unexpected("#".into()), 7, 8));
    assert_eq!(at("{let(1, 2, 3)}"), (Problem::NameExpected, 5, 6));
    assert_eq!(at("{raw('x')}"), (Problem::FieldExpected, 5, 8));
    assert_eq!(at("{title"), (Problem::UnclosedField, 0, 6));
    assert_eq!(at("{upper(title"), (Problem::UnclosedField, 0, 12));
    assert_eq!(at("{a{b}"), (Problem::NestedField, 2, 3));
    assert_eq!(at("{ }"), (Problem::EmptyField, 0, 3));
    assert_eq!(at("  "), (Problem::Empty, 0, 2));
    assert_eq!(at("a}"), (Problem::UnexpectedClose('}'), 1, 2));
    assert!(problem("{x}").to_string().contains("at character 1"));
    assert_eq!(
        problem("{if(1)}").to_string(),
        "if takes 2 to 3 values (at character 2)"
    );
}

#[test]
fn depth_and_length_are_limited() {
    let deep = format!("{{{}title{}}}", "(".repeat(10_000), ")".repeat(10_000));
    assert_eq!(problem(&deep).problem, Problem::TooComplex);
    let nots = format!("{{{}title}}", "not ".repeat(10_000));
    assert_eq!(problem(&nots).problem, Problem::TooComplex);
    let calls = format!("{{{}title{}}}", "upper(".repeat(10_000), ")".repeat(10_000));
    assert_eq!(problem(&calls).problem, Problem::TooComplex);
    let fine = format!(
        "{{{}title{}}}",
        "(".repeat(MAX_NESTING - 1),
        ")".repeat(MAX_NESTING - 1)
    );
    assert!(Template::parse(&fine).is_ok());
    let long_or = format!("{{title{}}}", " or title".repeat(50_000));
    assert_eq!(eval(&long_or), "Shoreline, 6 a.m.", "long chains stay flat");

    assert_eq!(eval("{pad('', 99999999999)}").len(), MAX_PAD);
    let doubled =
        "{let(a, pad('', 1000, 'x'), replace(replace(replace(a, 'x', a), 'x', a), 'x', a))}";
    assert!(eval(doubled).len() <= MAX_TEXT_BYTES);
    let many = "{pad('', 1000, 'é')}".repeat(200);
    let text = eval(&many);
    assert!(text.len() <= MAX_TEXT_BYTES);
    assert!(text.ends_with('é'));
}

#[test]
fn keys_sort_naturally() {
    let mut keys = vec!["Disc 10", "disc 2", "Disc 1", "b", "A", "", "Disc 02"];
    keys.sort_by(|a, b| compare_keys(a, b));
    assert_eq!(
        keys,
        ["", "A", "b", "Disc 1", "disc 2", "Disc 02", "Disc 10"]
    );
}

#[test]
fn function_names_are_listed_once() {
    for function in Function::all() {
        assert_eq!(Function::from_name(function.name()), Some(function));
        assert_eq!(
            Function::from_name(&function.name().to_uppercase()),
            Some(function)
        );
    }
    assert_eq!(Function::from_name("nope"), None);
}
