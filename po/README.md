# po

Translations, managed with GNU gettext.

- `LINGUAS` lists the languages, one code per line.
- `POTFILES.in` lists the source files that contain translatable strings, among them the desktop entry and the metainfo in `data/`, whose translations Meson merges into the installed files (`i18n.merge_file`).
- Each language has one `<code>.po` file.

To add a language, add its `.po` file and a line in `LINGUAS`.

## Contributing a translation

Translations are contributed as pull requests on GitHub; the project uses no separate translation platform.

1. Copy the template `helikodu.pot` to `<code>.po` (for example `et.po`) and translate it with any gettext editor, such as Poedit, Lokalize or GTranslator. For an existing language, update its `.po` file instead.
2. Add the language code to `LINGUAS` if it is new.
3. Open a pull request with the changed files. Please keep one language per pull request.
