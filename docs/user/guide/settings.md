# Settings

Choose **Preferences** in the main menu, or press **Ctrl+,**, to open the **Settings** window. Changes apply immediately; there is no Save button.

| Page | What it holds |
|---|---|
| **Library** | Your music **Folders**, **Skip** patterns, **Scanning** (watching for changes, symbolic links, video files, drives that are not connected), and **Storage** limits for covers, analysis data, logs and the undo history. See [First run and music folders](first-run.md) |
| **Playback** | What the **Previous button** does, the **Seek step** of the seek shortcuts (5 s by default), when to **Count a play**, whether **Playing an album** replaces the queue or adds to it, what happens **When Helikodu starts**, resuming **Long tracks**, and keeping the computer awake while music plays (**Power**) |
| **Audio output** | The backend, device, mode, sample rate and format, volume control and buffer. See [Choosing the audio output](audio-output.md) |
| **Loudness** | ReplayGain and R128 normalization. See [Loudness and ReplayGain](loudness.md) |
| **Appearance** | Light or dark colours, the accent colour, the window style, the menu bar, striped tables, the language and notifications |
| **Advanced** | Tag writing, the library database, how much the log records, developer mode, and putting every setting back to its default |

**DSP and EQ** and **Shortcuts** are listed but come in a later version.

## Appearance

- **Colour scheme**: **System** (as the desktop says, the default), **Light** or **Dark**.
- **Accent colour**: **From the desktop** (amber when the desktop has none), or **Amber**, **Blue**, **Green** or **Violet**.
- **Window style**: **Integrated** draws its own header with the window buttons in it, as GNOME applications do; **Classic** uses your desktop's normal title bar and can show a menu bar. By default Helikodu chooses Integrated on GNOME and Classic elsewhere. A new window style is used when Helikodu starts again.
- **Menu bar**: **Show the menu bar**, only in the Classic window style; **Ctrl+M** shows or hides it at any time.
- **Tables**: **Shade every second row**.
- **Language**: **Same as the desktop**; Helikodu uses the language of your desktop.
- **Notifications**: **Show a notification when the track changes**. It is not shown while the Helikodu window is in front.

High-contrast and reduced-motion preferences of the desktop are followed as well.

## Advanced

- **Tags in MP3 files**: **ID3v2.4** (the default) or **ID3v2.3**, for new tags that the [tag editor](tags.md) writes into MP3 files. Choose ID3v2.3 if an old car stereo or portable player shows no tags.
- **Very large files**: **Change tags in place for files over** a size (2 GiB by default). Smaller files are changed through a checked copy, which is safe even if the power fails; a change in place is faster for huge files but has no such safety net. The tag editor tells you before it happens.
- **Ratings**: **Also write ratings to the tags of the files** is shown, but cannot be turned on in this version; ratings stay in the library.
- **Library database**: where the library is stored, with **Open folder**.
- **Logs**: **Normal detail** or **Full detail, for a bug report**, and **Copy diagnostics**, which puts a summary for a bug report on the clipboard. Logs stay on this computer.
- **Developer mode**: **Show tools for testing** adds the silent and WAV-file outputs.
- **Reset**: **Reset settings…** puts every setting back to its default. The library, playlists, ratings and play counts are kept.

## The settings file

All settings are stored in `~/.config/helikodu/settings.toml`, a plain text file. You can edit it while Helikodu is closed. A value that is out of range resets its section to the defaults; a file that cannot be read at all is replaced by the defaults and kept as `settings.toml.bak`. To start over with all defaults, delete the file.
