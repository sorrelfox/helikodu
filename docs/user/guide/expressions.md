# Expressions

Naming patterns are written in Helikodu's expression language. The [organizer](file-tools.md#naming-patterns), the [converter and folder sync](converting-and-sync.md#naming-patterns) all read it the same way. A pattern such as `{albumartist}/{album}/[{disc}-]{track} {title}` is already an expression. This page shows what else it can do.

## Text, fields and brackets

- Text is written as it is.
- `{…}` holds an expression, and its value is written in its place. The simplest expression is a field, with the field names of the search language: `{albumartist}`, `{year}`, `{track}`, `{tag:MOOD}`. Names may be written in any case.
- Text in square brackets is left out when an expression directly inside it gives nothing: `{album}[ ({year})]`. Brackets can be nested, and an inner bracket is left out on its own.
- In a naming pattern, a `/` starts a folder. A `/` inside a value or inside quotes never does.
- `{track}` has two digits (three on an album of a hundred tracks or more). `{disc}` has a value only on an album of several discs.
- In a naming pattern, an expression that gives nothing outside brackets is written as "Unknown".
- Fields you defined yourself work too: a computed field such as `{decade}` gives the text of its own expression, and a custom field such as `{mood}` the tag it stands for. `{label}` gives a track's labels. See [Labels and your own fields](labels-and-fields.md).

## Inside the braces

- **Text** in double or single quotes: `"Live"` or `'Live'`. A backslash takes the next character as it is: `"say \"hi\""`.
- **Numbers**: `2000`, `-1`, `4.5`.
- **Fields**: `title`, `albumartist`, `tag:MOOD`.
- **Functions**: `upper(album)`, `if(composer, composer, artist)`. Functions can hold other functions.
- **Parentheses** group: `(year > 2000) and title`.

Every value is text. Empty text counts as false, and any other text counts as true.

| Write | Gives |
|---|---|
| `a or b` (also `a \|\| b`) | `a` when it is not empty, otherwise `b` |
| `a and b` (also `a && b`) | `b` when `a` is not empty, otherwise nothing |
| `not a` (also `!a`) | `1` when `a` is empty, otherwise nothing |
| `a = b`, `a != b` | `1` when they are equal (or different), otherwise nothing |
| `a < b`, `a <= b`, `a > b`, `a >= b` | `1` when the comparison holds, otherwise nothing |
| `a : b` | `1` when `a` contains `b`, otherwise nothing |

Comparisons compare numbers as numbers when both sides are numbers. Otherwise they compare text, and capitals and small letters count as the same.

## Functions

| Function | Gives |
|---|---|
| `if(test, then)`, `if(test, then, else)` | `then` when `test` is not empty, otherwise `else` (or nothing) |
| `concat(a, b, …)` | the values one after another |
| `upper(x)`, `lower(x)` | `x` in capitals or in small letters |
| `left(x, n)`, `right(x, n)` | the first or the last `n` characters |
| `substr(x, start)`, `substr(x, start, length)` | the characters from `start` on (the first is 1), all of them or `length` of them |
| `len(x)` | the number of characters |
| `trim(x)` | `x` without spaces at either end |
| `replace(x, from, to)` | `x` with every `from` replaced by `to` |
| `pad(x, width)`, `pad(x, width, char)` | `x` filled at the end with spaces (or `char`) to `width` characters |
| `num(x, width)` | a whole number with zeros in front, to `width` digits; anything else as it is |
| `firstword(x)` | the first word |
| `sortname(x)` | "The Beatles" as "Beatles, The" (also for "A" and "An") |
| `noarticle(x)` | "The Beatles" as "Beatles" |
| `initial(x)` | the first letter in capitals, or `#` when it starts with a digit |
| `year(x)`, `month(x)`, `day(x)`, `date(x)` | parts of a date such as `2021-03-04`: `2021`, `03`, `04`, and the date without its time |
| `time(x)` | a number of seconds as `3:45`, or `1:02:05` from an hour on |
| `join(field)`, `join(field, separator)` | every value of a field with several values (composers, performers, conductors, lyricists, genres, a tag given more than once), with `, ` or the separator between them |
| `first(field)`, `last(field)` | the first or the last of those values |
| `count(field)` | how many values the field has |
| `raw(field)` | the value as it is stored: `raw(track)` is `3` where `{track}` is `03`, and `raw(disc)` has a value on an album of one disc too |
| `let(name, value, body)` | `body`, where `name` stands for `value` |

Function names may be written in any case.

## Examples

| Pattern | Gives |
|---|---|
| `{initial(noarticle(albumartist))}/{albumartist}/{album}` | `B/The Beatles/Abbey Road` |
| `{if(genre : "classical", composer or artist, albumartist)}/{album}` | the composer for classical music, the album artist for the rest |
| `{albumartist}/{year} {album}/{num(raw(track), 3)} {title}` | three-digit track numbers on every album |
| `{albumartist}/{album}/{track} {replace(title, ":", " -")}` | a colon in a title written as a dash |
| `{join(composer, " & ")} - {title}` | `Mari Kask & Okto - Departure` |
| `{let(a, sortname(albumartist), concat(left(a, 1), " - ", a))}` | `B - Beatles, The` |
| `{upper(left(title, 1))}{lower(substr(title, 2))}` | the title with only its first letter in capitals |

## Problems and limits

A pattern that cannot be read is explained under its entry, with the character where the problem is: an unknown field or function, a function with too few or too many values, a quote, parenthesis, brace or bracket that is not closed, or something that does not belong where it is.

Brackets can be nested 16 deep and expressions 32 deep. No value grows beyond 64 KiB, and padding adds at most 1024 characters. Every folder and file name is still cut to 255 bytes.

**Fill tags from file names** in the [tag tools](tags.md) works the other way round: it reads values out of file names. Its patterns hold only plain fields such as `{track} - {title}`, and text in square brackets is matched as it is.
