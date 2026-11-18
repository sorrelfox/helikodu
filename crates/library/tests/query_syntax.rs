// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The text form of the query language: what the parser reads, where it reports
//! problems, the plain-text fallback, and the round trip through the printer for
//! generated trees.

#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used)]

use helikodu_library::{SyntaxErrorKind, parse_query, parse_query_lenient, print_query};
use helikodu_model::query::{
    Age, AgeUnit, CalendarDate, Comparison, Decimal, Expr, Field, FieldKind, Flag, Limit,
    LimitUnit, MediaKind, Operator, Presence, Query, QuickFilter, SortBy, SortKey, Value,
};
use proptest::prelude::*;

fn cmp(field: Field, op: Operator, value: Value) -> Expr {
    Expr::compare(field, op, value)
}

fn text(t: &str) -> Value {
    Value::Text(t.to_owned())
}

fn parsed(text: &str) -> Query {
    parse_query(text).unwrap_or_else(|e| panic!("{text:?}: {e} at {:?}", e.span()))
}

#[test]
fn the_documented_examples_parse() {
    let q = parsed(r#"artist:"Pink Floyd" year:1970..1979 rating>=4"#);
    assert_eq!(
        q.filter,
        Some(Expr::All(vec![
            cmp(Field::Artist, Operator::Contains, text("Pink Floyd")),
            cmp(
                Field::Year,
                Operator::Contains,
                Value::range(Value::number(1970), Value::number(1979))
            ),
            cmp(Field::Rating, Operator::GreaterOrEqual, Value::number(4)),
        ]))
    );

    let q = parsed("genre:jazz -genre:fusion added<30d");
    let days30 = Value::Age(Age {
        amount: 30,
        unit: AgeUnit::Days,
    });
    assert_eq!(
        q.filter,
        Some(Expr::All(vec![
            cmp(Field::Genre, Operator::Contains, text("jazz")),
            cmp(Field::Genre, Operator::Contains, text("fusion")).negate(),
            cmp(Field::Added, Operator::Less, days30),
        ]))
    );

    let q = parsed("codec:flac samplerate>=88200 sort:added desc limit:50");
    assert_eq!(q.sort, vec![SortKey::descending(Field::Added)]);
    assert_eq!(q.limit, Some(Limit::tracks(50)));

    let q = parsed(r#"title~"^(Intro|Outro)$" OR duration<30s"#);
    assert_eq!(
        q.filter,
        Some(Expr::Any(vec![
            cmp(Field::Title, Operator::Matches, text("^(Intro|Outro)$")),
            cmp(Field::Duration, Operator::Less, Value::Duration(30_000)),
        ]))
    );

    let q = parsed("kind:audiobook resume>0 sort:lastplayed desc");
    assert_eq!(
        q.filter,
        Some(Expr::All(vec![
            cmp(
                Field::Kind,
                Operator::Contains,
                Value::Kind(MediaKind::Audiobook)
            ),
            cmp(Field::Resume, Operator::Greater, Value::Duration(0)),
        ]))
    );
    assert_eq!(q.sort, vec![SortKey::descending(Field::LastPlayed)]);

    for example in [
        "artist:kask year:2018..2021",
        "rating>=4 -genre:ambient",
        "added<30d sort:added desc",
        r#"title~"^Intro" OR duration<30s"#,
        "plays=0",
    ] {
        parsed(example);
    }
}

#[test]
fn boolean_operators_and_precedence() {
    let (a, b, c) = (Expr::text("a"), Expr::text("b"), Expr::text("c"));
    // AND binds tighter than OR, juxtaposition is AND.
    for input in ["a b OR c", "a AND b OR c", "a & b | c", "a && b || c"] {
        assert_eq!(
            parsed(input).filter,
            Some(Expr::Any(vec![
                Expr::All(vec![a.clone(), b.clone()]),
                c.clone()
            ])),
            "{input}"
        );
    }
    assert_eq!(
        parsed("a (b OR c)").filter,
        Some(Expr::All(vec![
            a.clone(),
            Expr::Any(vec![b.clone(), c.clone()])
        ]))
    );
    for input in ["-a", "!a", "NOT a", "NOT(a)", "-(a)"] {
        assert_eq!(parsed(input).filter, Some(a.clone().negate()), "{input}");
    }
    // Nested groups of one kind are flattened.
    assert_eq!(
        parsed("(a b) c").filter,
        Some(Expr::All(vec![a.clone(), b.clone(), c.clone()]))
    );
    // Lower-case words are text, not operators.
    assert_eq!(
        parsed("rock and roll").filter,
        Some(Expr::All(vec![
            Expr::text("rock"),
            Expr::text("and"),
            Expr::text("roll")
        ]))
    );
    assert_eq!(parsed("").filter, None);
    assert_eq!(parsed("  ()  ").filter, None);
    assert_eq!(parsed("-()").filter, Some(Expr::Any(vec![])));
}

#[test]
fn values_follow_their_fields() {
    let one = |input: &str| match parsed(input).filter {
        Some(Expr::Compare(Comparison { value, .. })) => value,
        other => panic!("{input}: {other:?}"),
    };
    assert_eq!(one("samplerate>=96k"), Value::number(96_000));
    assert_eq!(one("samplerate>=88.2kHz"), Value::number(88_200));
    assert_eq!(one("bitrate>=320kbps"), Value::number(320));
    assert_eq!(
        one("rating>=3.5"),
        Value::Number(Decimal::from_thousandths(3500))
    );
    assert_eq!(one("duration>4m"), Value::Duration(240_000));
    assert_eq!(one("duration:3:30"), Value::Duration(210_000));
    assert_eq!(one("size>1.5GB"), Value::Size(1_500_000_000));
    assert_eq!(
        one("lastplayed>6m"),
        Value::Age(Age {
            amount: 6,
            unit: AgeUnit::Months
        })
    );
    assert_eq!(
        one("added>=2024-05"),
        Value::Date(CalendarDate::month(2024, 5).unwrap())
    );
    assert_eq!(
        one("trackgain<-6.5"),
        Value::Number(Decimal::from_thousandths(-6500))
    );
    assert_eq!(one("year:\"1975\""), Value::number(1975));
    // `:` before another operator, `==`, aliases and any case.
    assert_eq!(parsed("rating:>=4"), parsed("rating>=4"));
    assert_eq!(parsed("YEAR==1975"), parsed("year=1975"));
    assert_eq!(parsed("length<30s"), parsed("duration<30s"));
    // A range after `=` is the same range.
    assert_eq!(parsed("year=1970..1979"), parsed("year:1970..1979"));
    // A regular expression may hold parentheses without quotes.
    assert_eq!(
        parsed("(title~^(a|b)$ OR x)").filter,
        Some(Expr::Any(vec![
            cmp(Field::Title, Operator::Matches, text("^(a|b)$")),
            Expr::text("x")
        ]))
    );
    // Backslashes other than before a quote or a backslash are kept.
    assert_eq!(one(r#"title:"\d+ \"x\" \\""#), text(r#"\d+ "x" \"#));
    assert_eq!(
        parsed("tag:mood=calm has:tag:mood is:hires has:lyrics").filter,
        Some(Expr::All(vec![
            cmp(Field::Tag("MOOD".into()), Operator::Equals, text("calm")),
            Expr::Has(Presence::Field(Field::Tag("MOOD".into()))),
            Expr::Is(Flag::HiRes),
            Expr::Has(Presence::Lyrics),
        ]))
    );
    let q = parsed("sort:artist,year desc, random limit:90min");
    assert_eq!(
        q.sort,
        vec![
            SortKey::ascending(Field::Artist),
            SortKey::descending(Field::Year),
            SortKey {
                by: SortBy::Random,
                descending: false
            }
        ]
    );
    assert_eq!(
        q.limit,
        Some(Limit {
            amount: 90,
            unit: LimitUnit::Minutes
        })
    );
    assert_eq!(
        parsed("limit:4GB").limit.unwrap().unit,
        LimitUnit::Gigabytes
    );
    // Words that only look like fields stay text.
    assert_eq!(parsed("Help!").filter, Some(Expr::text("Help!")));
    assert_eq!(parsed("1:30").filter, Some(Expr::text("1:30")));
    assert_eq!(parsed("AC/DC").filter, Some(Expr::text("AC/DC")));
    assert_eq!(parsed("título:x").filter, Some(Expr::text("título:x")));
}

/// The span and kind of the error for `input`.
fn error(input: &str) -> (std::ops::Range<usize>, SyntaxErrorKind) {
    let error = parse_query(input).expect_err(input);
    // The span is inside the text and on character boundaries.
    assert!(error.span().end <= input.len(), "{input}");
    assert!(input.is_char_boundary(error.span().start));
    assert!(input.is_char_boundary(error.span().end));
    assert!(!error.to_string().is_empty());
    (error.span(), error.kind().clone())
}

#[test]
fn errors_point_at_what_fails() {
    assert_eq!(
        error("genre:jazz artsit:kask"),
        (11..17, SyntaxErrorKind::UnknownField("artsit".into()))
    );
    assert_eq!(
        error(r#"title:"open"#),
        (6..11, SyntaxErrorKind::UnclosedQuote)
    );
    assert_eq!(error("a (b OR c"), (2..3, SyntaxErrorKind::UnclosedGroup));
    assert_eq!(error("a b) c"), (3..4, SyntaxErrorKind::UnexpectedClose));
    assert_eq!(error("a OR"), (2..4, SyntaxErrorKind::MissingOperand("OR")));
    assert_eq!(error("OR a"), (0..2, SyntaxErrorKind::MissingOperand("OR")));
    assert_eq!(
        error("a AND"),
        (2..5, SyntaxErrorKind::MissingOperand("AND"))
    );
    assert_eq!(error("x - y"), (2..3, SyntaxErrorKind::MissingOperand("-")));
    assert_eq!(
        error("year:"),
        (0..5, SyntaxErrorKind::MissingValue("year".into()))
    );
    assert_eq!(
        error("x year: 1970"),
        (2..7, SyntaxErrorKind::MissingValue("year".into()))
    );
    assert!(matches!(
        error("title<5"),
        (r, SyntaxErrorKind::InvalidOperator { .. }) if r == (5..6)
    ));
    assert!(matches!(
        error("year~19"),
        (r, SyntaxErrorKind::InvalidOperator { .. }) if r == (4..5)
    ));
    assert!(matches!(
        error("a year>=soon"),
        (r, SyntaxErrorKind::InvalidValue { .. }) if r == (8..12)
    ));
    assert!(matches!(
        error("rating>=4.2"),
        (r, SyntaxErrorKind::InvalidValue { .. }) if r == (8..11)
    ));
    assert!(matches!(
        error("year:1970..soon"),
        (r, SyntaxErrorKind::InvalidValue { .. }) if r == (11..15)
    ));
    assert!(matches!(
        error("added:2020..30d"),
        (r, SyntaxErrorKind::InvalidValue { .. }) if r == (12..15)
    ));
    assert_eq!(
        error("year<1970..1979"),
        (4..5, SyntaxErrorKind::RangeOperator)
    );
    assert!(matches!(
        error(r#"title~"(open""#),
        (r, SyntaxErrorKind::InvalidRegex(_)) if r == (6..13)
    ));
    assert!(matches!(
        error("has:wings"),
        (r, SyntaxErrorKind::UnknownName { prefix: "has", .. }) if r == (4..9)
    ));
    assert!(matches!(
        error("is:loud"),
        (r, SyntaxErrorKind::UnknownName { prefix: "is", .. }) if r == (3..7)
    ));
    assert_eq!(
        error("tag:mood"),
        (0..8, SyntaxErrorKind::MissingOperator("tag:MOOD".into()))
    );
    assert_eq!(
        error("x sort:colour"),
        (7..13, SyntaxErrorKind::UnknownSortKey("colour".into()))
    );
    assert_eq!(error("limit:lots"), (6..10, SyntaxErrorKind::InvalidLimit));
    assert_eq!(
        error("a limit:5 limit:6"),
        (10..17, SyntaxErrorKind::Duplicate("limit"))
    );
    assert_eq!(
        error("(a sort:title)"),
        (3..13, SyntaxErrorKind::Misplaced("sort"))
    );
    assert_eq!(
        error("-limit:5"),
        (1..8, SyntaxErrorKind::Misplaced("limit"))
    );
    assert_eq!(error(r#"a """#), (2..4, SyntaxErrorKind::EmptyText));
    // Byte offsets past text in other scripts.
    assert_eq!(
        error("ёлка wing:x"),
        (9..13, SyntaxErrorKind::UnknownField("wing".into()))
    );
    let deep = "(".repeat(100) + "a" + &")".repeat(100);
    assert_eq!(error(&deep).1, SyntaxErrorKind::TooDeep);
    assert!(parse_query("a year:1970 limit:5").unwrap().limit.is_some());
}

#[test]
fn errors_have_examples_that_parse() {
    for input in [
        "artsit:x",
        "\"x",
        "(x",
        "x)",
        "x OR",
        "year:",
        "title<5",
        "year:soon",
        "title~(",
        "year<1..2",
        "has:x",
        "is:x",
        "tag:x",
        "sort:x",
        "limit:x",
        "limit:1 limit:2",
        "(sort:title)",
    ] {
        let error = parse_query(input).expect_err(input);
        let example = error.example().expect(input);
        parsed(example);
    }
}

#[test]
fn lenient_parsing_falls_back_to_plain_text() {
    let lenient = parse_query_lenient(r#"re:member "the 80s"#);
    assert!(lenient.error.is_some());
    assert_eq!(
        lenient.query.filter,
        Some(Expr::All(vec![
            Expr::text("re:member"),
            Expr::text("the"),
            Expr::text("80s")
        ]))
    );
    let fine = parse_query_lenient("year>1970");
    assert_eq!(fine.error, None);
    assert_eq!(fine.query, parsed("year>1970"));
    assert_eq!(parse_query_lenient(" \" ").query.filter, None);
}

#[test]
fn printing_is_canonical() {
    for (input, printed) in [
        ("ARTIST:abc", "artist:abc"),
        ("a AND b | c", "a b OR c"),
        ("NOT (a OR b)", "-(a OR b)"),
        ("length:3:30", "duration:210s"),
        ("samplerate>=96k", "samplerate>=96000"),
        ("size<1.5gb", "size<1500MB"),
        ("added:2024-5", "added:2024-05"),
        ("rating:>=4", "rating>=4"),
        ("title:\"a b\" \"or\" 'x'", "title:\"a b\" or 'x'"),
        ("\"OR\" \"-x\" \"y:z\"", "\"OR\" \"-x\" \"y:z\""),
        (
            "sort:ADDED DESC,  title limit:2H",
            "sort:added desc,title limit:2h",
        ),
        ("-()", "-()"),
    ] {
        let query = parsed(input);
        assert_eq!(print_query(&query), printed, "{input}");
        assert_eq!(parsed(printed), query, "{input}");
    }
}

#[test]
fn quick_filters_print_and_parse_back() {
    for filter in QuickFilter::ALL {
        let query = filter.query();
        assert_eq!(parsed(&print_query(&query)), query, "{filter:?}");
    }
    assert_eq!(
        print_query(&QuickFilter::RecentlyAdded.query()),
        "added<30d sort:added desc"
    );
}

// --- Generated trees ------------------------------------------------------------------

fn field() -> impl Strategy<Value = Field> {
    let named: Vec<Field> = Field::named().collect();
    prop_oneof![
        4 => proptest::sample::select(named),
        1 => "[A-Z0-9_-]{1,8}".prop_map(|name| Field::tag(&name).unwrap()),
    ]
}

fn decimal() -> impl Strategy<Value = Decimal> {
    prop_oneof![
        (-3000_i64..3000).prop_map(Decimal::from_int),
        (-10_000_000_000_i64..10_000_000_000).prop_map(Decimal::from_thousandths),
    ]
}

fn date() -> impl Strategy<Value = Value> {
    prop_oneof![
        (1_u16..=9999, 1_u8..=12, 1_u8..=28, 0..3).prop_map(|(y, m, d, which)| {
            Value::Date(match which {
                0 => CalendarDate::year(y).unwrap(),
                1 => CalendarDate::month(y, m).unwrap(),
                _ => CalendarDate::day(y, m, d).unwrap(),
            })
        }),
        (
            0_u32..100_000,
            proptest::sample::select(vec![
                AgeUnit::Hours,
                AgeUnit::Days,
                AgeUnit::Weeks,
                AgeUnit::Months,
                AgeUnit::Years
            ])
        )
            .prop_map(|(amount, unit)| Value::Age(Age { amount, unit })),
    ]
}

/// A single value of a field of `kind` that is not text.
fn scalar(kind: FieldKind) -> BoxedStrategy<Value> {
    match kind {
        FieldKind::Rating => (0_i64..=10)
            .prop_map(|half| Value::Number(Decimal::from_thousandths(half * 500)))
            .boxed(),
        FieldKind::Duration => prop_oneof![
            (0_u64..100_000).prop_map(|s| Value::Duration(s * 1000)),
            (0_u64..1_000_000_000_000).prop_map(Value::Duration),
        ]
        .boxed(),
        FieldKind::Size => prop_oneof![
            (0_u64..10_000).prop_map(|n| Value::Size(n * 1_000_000)),
            (0_u64..1_000_000_000_000_000).prop_map(Value::Size),
        ]
        .boxed(),
        FieldKind::Date => date().boxed(),
        _ => decimal().prop_map(Value::Number).boxed(),
    }
}

fn comparison() -> impl Strategy<Value = Expr> {
    field().prop_flat_map(|field| {
        let kind = field.kind();
        let ops = |ops: &[Operator]| proptest::sample::select(ops.to_vec());
        let strategy: BoxedStrategy<(Operator, Value)> = match kind {
            FieldKind::Text => prop_oneof![
                (
                    ops(&[Operator::Contains, Operator::Equals, Operator::NotEquals]),
                    "\\PC{0,10}"
                )
                    .prop_map(|(op, t)| (op, Value::Text(t))),
                "\\PC{0,8}".prop_map(|t| (Operator::Matches, Value::Text(regex::escape(&t)))),
            ]
            .boxed(),
            FieldKind::Kind => (
                ops(&[Operator::Contains, Operator::Equals, Operator::NotEquals]),
                proptest::sample::select(MediaKind::ALL.to_vec()),
            )
                .prop_map(|(op, kind)| (op, Value::Kind(kind)))
                .boxed(),
            kind => {
                let all: Vec<Operator> = Operator::ALL
                    .into_iter()
                    .filter(|op| *op != Operator::Matches)
                    .collect();
                prop_oneof![
                    3 => (proptest::sample::select(all), scalar(kind)),
                    1 => (scalar(kind), scalar(kind))
                        .prop_filter("both ends of one kind", |(a, b)| {
                            std::mem::discriminant(a) == std::mem::discriminant(b)
                        })
                        .prop_map(|(a, b)| (Operator::Contains, Value::range(a, b))),
                ]
                .boxed()
            }
        };
        strategy.prop_map(move |(op, value)| Expr::compare(field.clone(), op, value))
    })
}

fn leaf() -> impl Strategy<Value = Expr> {
    prop_oneof![
        4 => comparison(),
        3 => prop_oneof![
            "\\PC{1,10}",
            proptest::sample::select(vec![
                "AND", "OR", "NOT", "&", "|", "-x", "!x", "a:b", "(x)", "\"", "\\", "a\\\"", "x\\",
                "sort:title", "has:lyrics", "1..2", "Help!", "ёлка", "月亮",
            ])
            .prop_map(str::to_owned),
        ]
        .prop_map(Expr::Text),
        1 => prop_oneof![
            Just(Presence::Lyrics),
            Just(Presence::Cover),
            field().prop_map(Presence::Field),
        ]
        .prop_map(Expr::Has),
        1 => proptest::sample::select(Flag::ALL.to_vec()).prop_map(Expr::Is),
    ]
}

fn expr() -> impl Strategy<Value = Expr> {
    leaf().prop_recursive(5, 48, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 2..4).prop_map(Expr::All),
            proptest::collection::vec(inner.clone(), 2..4).prop_map(Expr::Any),
            inner.prop_map(Expr::negate),
        ]
    })
}

fn sort_key() -> impl Strategy<Value = SortKey> {
    (
        prop_oneof![
            8 => field().prop_map(SortBy::Field),
            1 => Just(SortBy::Random),
        ],
        any::<bool>(),
    )
        .prop_map(|(by, descending)| SortKey { by, descending })
}

fn query() -> impl Strategy<Value = Query> {
    (
        proptest::option::weighted(0.9, expr()),
        proptest::collection::vec(sort_key(), 0..3),
        proptest::option::of(
            (
                any::<u32>(),
                proptest::sample::select(LimitUnit::ALL.to_vec()),
            )
                .prop_map(|(amount, unit)| Limit { amount, unit }),
        ),
    )
        .prop_map(|(filter, sort, limit)| {
            Query {
                filter,
                sort,
                limit,
            }
            .normalized()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn printed_trees_parse_back(query in query()) {
        prop_assert_eq!(query.problem(), None);
        let text = print_query(&query);
        match parse_query(&text) {
            Ok(back) => prop_assert_eq!(back, query, "{}", text),
            Err(error) => prop_assert!(false, "{text:?}: {error} at {:?}", error.span()),
        }
    }

    #[test]
    fn any_text_parses_or_fails_inside_it(text in "\\PC{0,40}|[-!()\"a-z:<>=~ .|&]{0,30}") {
        match parse_query(&text) {
            Ok(query) => {
                // What parses prints to text that parses to the same tree.
                prop_assert_eq!(parse_query(&print_query(&query)), Ok(query));
            }
            Err(error) => {
                prop_assert!(error.span().end <= text.len());
                prop_assert!(text.is_char_boundary(error.span().start));
                prop_assert!(text.is_char_boundary(error.span().end));
                prop_assert!(error.span().start < error.span().end || text.is_empty());
            }
        }
        let lenient = parse_query_lenient(&text);
        prop_assert_eq!(lenient.query.problem(), None);
    }
}
