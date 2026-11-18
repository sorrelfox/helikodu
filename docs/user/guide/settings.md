# Settings

Open **Settings** from the main menu. Changes apply immediately; there is no Save button. The **Search settings** field at the top finds a setting by name.

| Page | What it holds |
|---|---|
| **Library** | Your music folders, watching and scanning, video files, skip patterns, symbolic links, drives that are not connected, and storage limits for covers and logs. See [First run and music folders](first-run.md) |
| **Playback** | What **Previous** does, the **Seek step** of the seek shortcuts (5 s by default), when a listen counts as a play, whether **Playing an album** replaces the queue or adds to it, what happens **When Helikodu starts**, resuming long tracks, and keeping the computer awake while music plays |
| **Audio output** | The backend, device, mode, sample rate and format, volume control and buffer. See [Choosing the audio output](audio-output.md) |
| **Loudness** | ReplayGain and R128 normalization. See [Loudness and ReplayGain](loudness.md) |
| **Appearance** | Light or dark colours, the accent colour, the window style, the language and notifications |
| **Advanced** | Tag-writing options, how much the log records, and developer options |

**DSP and EQ** and **Shortcuts** are listed but come in a later version.

## Appearance

- **Colour scheme**: as the desktop says (the default), always light, or always dark.
- **Accent colour**: the desktop's accent colour (amber when the desktop has none), or amber, blue, green or violet.
- **Window style**: *Integrated* draws its own title bar with the window buttons in the top-right corner, as GNOME applications do; *Classic* uses your desktop's normal title bar and can show a menu bar (**Ctrl+M**). By default Helikodu chooses Integrated on GNOME and Classic elsewhere.
- **Notifications**: a desktop notification when the track changes.

High-contrast and reduced-motion preferences of the desktop are followed as well.

## The settings file

All settings are stored in `~/.config/helikodu/settings.toml`, a plain text file. You can edit it while Helikodu is closed. A value that is out of range resets its section to the defaults; a file that cannot be read at all is replaced by the defaults and kept as `settings.toml.bak`. To start over with all defaults, delete the file.
