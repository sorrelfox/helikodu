# po

Translations, managed with GNU gettext.

- `helikodu.pot` is the template: every message of the program, the desktop entry and the metainfo, extracted from the sources.
- `LINGUAS` lists the languages, one code per line, sorted.
- `POTFILES.in` lists the source files that contain translatable strings, among them the desktop entry and the metainfo in `data/`, whose translations Meson merges into the installed files (`i18n.merge_file`).
- Each language has one `<code>.po` file.

Helikodu is translated into German (`de`), Spanish (`es`), French (`fr`), Italian (`it`), Japanese (`ja`), Korean (`ko`), Dutch (`nl`), Brazilian Portuguese (`pt_BR`), Russian (`ru`), Ukrainian (`uk`) and Simplified Chinese (`zh_CN`). It follows the language of the desktop, or the one chosen in **Settings › Appearance › Language**.

## Contributing a translation

Translations are contributed as pull requests on GitHub; the project uses no separate translation platform.

1. Copy the template `helikodu.pot` to `<code>.po` (for example `et.po`) and translate it with any gettext editor, such as Poedit, Lokalize or GTranslator. For an existing language, first bring its `.po` file up to date with the template (see [Checking a translation](#checking-a-translation)), then translate what is new.
2. A new language also needs its code in `LINGUAS`, and its name, written in the language itself, in the Language list of the settings (`LANGUAGES` in `crates/ui-gtk/src/dialogs/settings/appearance.rs`).
3. Check the file (below) and open a pull request with the changed files. Please keep one language per pull request.

A release ships only complete translations: no message may be untranslated or marked fuzzy.

## Rules for the messages

**Placeholders.** `{}` stands for something Helikodu fills in: a number, a name, a time. A translation keeps every one of them. To put them in another order, number them from 0, as in Rust: `{0}` is the first `{}` of the English text, `{1}` the second, and so on. For example, a translation of `"{} of {}"` (as in "3 of 12") that names the total first writes `"{1} … {0}"`. A translation uses either `{}` only or numbers only, and with numbers each one from `{0}` up appears at least once.

**Placeholders for controls.** In some messages a `{}` stands for a control, such as a dropdown, in the middle of a sentence: `"Match {} of these rules"` shows the words around a dropdown with "all" and "any". Move the `{}` (or `{0}`, `{1}`, …) to where the control belongs in your sentence; the text between them becomes the labels around the controls. The comment of each such message says what its controls are.

**Context.** A message with a context (`msgctxt`, such as `home folder` for "Home") is an English word that means something else elsewhere; translate it for the meaning its context and comment give.

**Names in braces**, such as `{skip}`, are words the user types (here in the patterns of the tag tools): keep them as they are.

**Plural forms** follow the `Plural-Forms` line of the header, which must be the rule of your language (your editor fills it in). A form that is used for one number only, such as the German singular, may say the number in words and leave out its `{}`: `"once"`/`"{} times"` can be `"einmal"`/`"{}-mal"`.

**Numbers.** The messages with the context `thousands separator` and `decimal separator` give the characters that numbers are written with, such as `.` and `,` in German, or a narrow no-break space and `,` in French.

**Dates.** Messages such as `%A %e %B` are date patterns (as for `strftime`): change the order and the punctuation, but keep the `%` codes; the system fills in the names of days and months.

**Markup and code.** Keep markup tags such as `<b>` and `</b>`, and text in backticks, as they are. Words in angle brackets that are not markup, such as `<keep>`, are translated. A line break at the start or the end of a message stays there.

## Checking a translation

```sh
msgfmt --check --statistics po/<code>.po
cargo xtask check-translations
```

`msgfmt --check` (from gettext 0.24 on) checks the header, the line breaks at the start and end, the number of plural forms and the placeholders of the messages marked `rust-format`. `cargo xtask check-translations` runs it for every language in `LINGUAS` and checks, in addition:

- that the file holds exactly the messages of the current template, so that a file left behind by a newer template is caught;
- that no message is fuzzy or untranslated (`--allow-untranslated` lets untranslated messages pass with a count, for work in progress);
- the placeholder rules above in the messages msgfmt does not check, markup tags and text in backticks;
- that the header names the language (`Language`) and has its plural rule.

It is part of `cargo xtask ci`. A file that is behind the template is brought up to date with:

```sh
msgmerge --update --backup=none po/<code>.po po/helikodu.pot
```

(`meson compile -C _build helikodu-update-po` does it for every language.) The template itself is made with `meson compile -C _build helikodu-pot`; see [docs/dev/releasing.md](../docs/dev/releasing.md#translations).

## Trying a translation

Compile the file into a folder of your own and point Helikodu at it with `HELIKODU_LOCALEDIR`:

```sh
mkdir -p /tmp/helikodu-locale/de/LC_MESSAGES
msgfmt -o /tmp/helikodu-locale/de/LC_MESSAGES/helikodu.mo po/de.po
HELIKODU_LOCALEDIR=/tmp/helikodu-locale LANGUAGE=de cargo run -p helikodu
```

`LANGUAGE=de` chooses the language for this start while **Settings › Appearance › Language** is **Same as the desktop**; a language chosen there comes first. Without `HELIKODU_LOCALEDIR`, Helikodu looks for its translations in `share/locale` beside the folder of the program (as in an unpacked ready-built release), and then where it was installed (`<prefix>/share/locale`).
