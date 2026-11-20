# Settings

Choose **Preferences** in the main menu, or press **Ctrl+,**, to open the **Settings** window. Changes apply immediately; there is no Save button.

| Page | What it holds |
|---|---|
| **Library** | Your music **Folders**, **Skip** patterns, **Scanning** (watching for changes, symbolic links, video files, drives that are not connected), the **Track analysis** in the background, and **Storage** limits for covers, waveforms, logs and the undo history. See [First run and music folders](first-run.md) and [Track analysis](#track-analysis) |
| **Playback** | What the **Previous button** does, the **Seek step** of the seek shortcuts (5 s by default), when to **Count a play**, whether **Playing an album** replaces the queue or adds to it, the **Crossfade** between tracks and when you skip (and whether it starts where the track fades out), the **Fades** for pause, stop, seek and skip, leaving out and skipping **Silence**, what happens **When Helikodu starts**, resuming **Long tracks**, keeping the computer awake while music plays and pausing when the screen locks or the computer goes to sleep (**Power**), and the SoundFont for **MIDI files**, the song lengths for **SID tunes** and how often **Looping tunes** repeat. See [Crossfades and fades](playing.md#crossfades-and-fades) and [Silence between and inside tracks](playing.md#silence-between-and-inside-tracks) |
| **Audio output** | The backend, device, mode, sample rate and format, volume control and buffer. See [Choosing the audio output](audio-output.md) |
| **Loudness** | ReplayGain and R128 normalization, the leveller for tracks without loudness values, and measuring the loudness of your tracks. See [Loudness and ReplayGain](loudness.md) |
| **DSP and EQ** | Equalizer, crossfeed, channel mix, limiter and dither. See [Equalizer and sound processing](dsp.md) |
| **Appearance** | Light or dark colours, the accent colour, the window style, the menu bar, striped tables, the language, notifications and the tray icon |
| **Shortcuts** | The keys of every action, and the shortcuts that work in other applications. See [Keyboard shortcuts](keyboard-shortcuts.md#changing-the-shortcuts) |
| **Advanced** | Tag writing, ratings and play counts in the files, the library database, how much the log records, developer mode, and putting every setting back to its default |

## Searching the settings

Type in **Search settings** above the pages to find a setting by its name, its explanation or the words of its choices. The settings found are shown in groups by page, with the matching words highlighted, and can be changed right there; **Open the page** shows the whole page, and **Esc** goes back to the page you were on. The number of settings found is shown beside each page.

## Appearance

- **Colour scheme**: **System** (as the desktop says, the default), **Light** or **Dark**.
- **Accent colour**: **From the desktop** (amber when the desktop has none), or **Amber**, **Blue**, **Green** or **Violet**.
- **Window style**: **Integrated** draws its own header with the window buttons in it, as GNOME applications do; **Classic** uses your desktop's normal title bar and can show a menu bar. By default Helikodu chooses Integrated on GNOME and KDE Plasma and Classic elsewhere. A new window style is used when Helikodu starts again.
- **Menu bar**: **Show the menu bar**, only in the Classic window style; **Ctrl+M** shows or hides it at any time.
- **Tables**: **Shade every second row**.
- **Language**: **Same as the desktop** (the default) uses the language of your desktop. You can also choose **English** or one of the translations: **Deutsch**, **Español**, **Français**, **Italiano**, **日本語**, **한국어**, **Nederlands**, **Português (Brasil)**, **Русский**, **Українська** or **简体中文**. A new language is used when Helikodu starts again; **Restart now** does that at once. The library is sorted for the new language right away, in the background, so that letters such as Å, Ö or Ж come where its alphabet puts them; dates, and the names of days and months, still follow the settings of your desktop.
- **Notifications**: **Show a notification when the track changes**. It is not shown while the Helikodu window is in front.
- **System tray**: **Show an icon in the tray** (off by default). A click on the icon shows or hides the window, scrolling over it changes the volume, and its menu has the playing track, the playback controls, **Hide the window**, **Mini player** and **Quit**.
- **Closing the window**: **Quits Helikodu** (the default) or **Keeps playing in the tray**. **Ctrl+Q** always quits. The first time the window goes to the tray, a notification says so.

The two tray rows are shown only where the desktop has a system tray (KDE Plasma has one; GNOME needs an extension such as AppIndicator). Without a tray, or with the icon off, there is no icon and closing the window quits. If the tray goes away while the window is in it (the panel restarts, the icon is turned off), the window comes back.

High-contrast and reduced-motion preferences of the desktop are followed as well.

## Advanced

- **Tags in MP3 files**: **ID3v2.4** (the default) or **ID3v2.3**, for new tags that the [tag editor](tags.md) writes into MP3 files. Choose ID3v2.3 if an old car stereo or portable player shows no tags.
- **Very large files**: **Change tags in place for files over** a size (2 GiB by default). Smaller files are changed through a checked copy, which is safe even if the power fails; a change in place is faster for huge files but has no such safety net. The tag editor tells you before it happens.
- **Statistics in files**: **Also write ratings to the tags of the files** (off by default) writes a rating you give into the file as well, so that other players can read it; see [Ratings](tags.md#ratings). **Also write play counts to the tags of the files** (off by default) does the same for play counts, in the background a few minutes after the last play and never into the file that is playing; see [Writing play counts and ratings into the files](moving-from-another-player.md#writing-play-counts-and-ratings-into-the-files). Switched on, what is waiting is written at once. Both are kept in the library either way, and these writes are not on the Undo history.
- **Library database**: where the library is stored, its size and the result of its last check, with **Open folder**.
- **Backups**: a backup of the library is made once a day while Helikodu runs, and the last 7 are kept in `~/.local/share/helikodu/backups/`. **Back up now** makes one at once. **Restore…** puts a backup back: choose the file, confirm, and Helikodu starts again with the library as it was then. The library as it was before is kept in the backups folder.
- **Your data**: **Export…** saves your ratings, play counts, history, playlists, labels and fields in one file, and **Import…** reads such a file, for moving to another computer. See [Moving from another player](moving-from-another-player.md#moving-your-data-to-another-computer).
- **Slow drives**: **Load each track into memory before it plays**, for drives that are slow or go to sleep (up to 512 MiB per track).
- **Logs**: **No logs** (the default), **Normal detail** or **Full detail, for a bug report**; **Open log folder**; and **Copy diagnostics**, which puts a summary for a bug report on the clipboard (versions, the output and its devices, the main settings; no file paths but the settings file's). With **No logs** nothing is written. The logs stay within the limit under Library › Storage (50 MiB by default) and stay on this computer.
- **Developer mode**: **Show tools for testing** adds the silent and WAV-file outputs.
- **Reset**: **Reset settings…** puts every setting back to its default. The library, playlists, ratings and play counts are kept.

## Track analysis

Helikodu analyses each library track once, in the background: where its sound starts and ends, its long silences and how its loudness changes. Crossfades, leaving out and skipping silence (**Playback** settings) and the leveller (**Loudness** settings) use it; nothing is written to the files. Under **Track analysis** in the **Library** settings:

- **Analyse tracks in the background** (on by default). Off, the analysis stops at once, crossfades use their fixed length and silence is not left out; on again, it carries on with the tracks not yet analysed.
- How many tracks are analysed, with a meter, and what the analysis does now: working, working slowly while music plays from the same drive, waiting for a library scan to finish, paused while the computer runs on battery, or done. Tracks whose files could not be read are counted after it.

The analysis works on one track at a time at the lowest priority, and is shown nowhere else.

## Copies of the same file

Helikodu notices when your library holds the same file more than once: files whose bytes are identical, content and tags (for example an album copied into a second folder). Files that were re-encoded or retagged are not copies. Two paths that lead to the very same file on the disk (a hard link, or a music folder reached through a link) count as one file.

- Copies share one set of **play counts, ratings, history and playlist entries**. Playing any of them plays the **main copy**: the copy that came into the library first, unless you choose another. When the main copy disappears or its drive is not connected, the next copy takes over.
- **What to do with copies** (`[library.duplicates]` in the settings file): `on_scan = "add"` (the default) gives copies their own entries in the library, as any other file; `on_scan = "skip"` keeps them out of every list, count and search, while Helikodu still knows them. `show_one = true` shows only the main copy of each file in every list, count, search and in the statistics; the folder view still shows every file. A change applies at once.
- **Combining play counts.** When Helikodu first finds copies in a library that counted them separately, it adds up their plays and keeps the main copy's rating (or the newest one). Before it does this the first time, it makes a copy of the library next to it, `library.db.backup-before-copies`.
- **Removing copies** moves them to the desktop trash, never deletes them, and only after you confirm. A copy is moved only if it is still identical to the main copy; the main copy itself, and a second path to the same file, are never moved.
- `helikodu-cli duplicates` lists the copies.

## The settings file

All settings are stored in `~/.config/helikodu/settings.toml`, a plain text file. You can edit it while Helikodu is closed. A value that is out of range resets its section to the defaults; a file that cannot be read at all is replaced by the defaults and kept as `settings.toml.bak`. To start over with all defaults, delete the file.

Settings files of earlier versions, without the keys added since, load with the defaults for them.
