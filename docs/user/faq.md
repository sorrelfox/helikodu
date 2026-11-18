# Frequently asked questions

## Why is Helikodu offline? Why is there no internet radio or other online feature?

Helikodu is a player for the music you own and keep on your computer. Leaving out every online feature keeps it simple, private and dependable: it starts and works the same with or without a network, it never waits for a server, and nothing about your listening leaves your computer.

This is a deliberate rule, not a missing feature, and it will not change: internet radio, streaming and online lookups would each bring a network connection, accounts or services that change without notice, and a reason for the player to wait. Helikodu contains no network code at all: no internet radio, no streaming services, no online cover, artist photo or lyrics lookup, no scrobbling, no update checks and no telemetry. Covers, artist photos and lyrics come from your own files; the play history can be exported to a file for a scrobbling service, which you then submit yourself. Even the user guide (F1) is the copy installed with Helikodu. Every release is tested inside a sandbox without network access, and the tests fail if anything tries to open an internet connection. The only connections Helikodu makes are local ones on your computer: D-Bus (for media keys and desktop integration) and the sound server.

Playlists that contain web addresses keep them, but those entries are shown as unavailable and never fetched.

## Which formats can Helikodu play?

Practically every audio format FFmpeg can decode, including:

- **Lossless:** FLAC, ALAC, WAV, AIFF, WavPack, Monkey's Audio (APE), TAK, TTA, Wave64, CAF
- **Lossy:** MP3, AAC and M4A, Ogg Vorbis, Opus, Musepack, WMA, AC-3, E-AC-3, DTS, MP2, Speex, AMR
- **DSD:** DSF and DFF (played as high-resolution PCM, or sent to the DAC as DoP in exclusive mode)
- **Tracker modules** (MOD, S3M, XM, IT and more, through libopenmpt), **game music** (NSF, SPC, GBS, VGM and more, through libgme), **Commodore 64 SID** tunes and **MIDI** files (with a SoundFont); game audio streams through vgmstream in builds made with it
- **Containers:** Matroska audio (MKA), WebM audio, M4B audiobooks with chapters
- **CUE sheets** that split one album file into tracks, and files with chapters

Tracks of the same format play gaplessly. Helikodu recognises files by their content, so a file with the wrong extension still plays. Modules, game music files and SID tunes that hold several songs become one track per song.

Tags are read from every common tag format (ID3v2, ID3v1, APEv2, Vorbis comments, MP4 atoms, RIFF INFO, and others through FFmpeg), including multiple artists and genres.

## Can Helikodu play video files?

It plays their **sound track**, never the picture. Open an MP4, MKV, WebM, AVI, MOV or similar file and Helikodu plays its audio; no video is ever decoded or shown. To add the sound tracks of video files to your library, turn on **Include video files** in **Settings → Library** (it is off by default).

## What does exclusive mode do, and why does my DAC not switch sample rates?

In **shared** mode, Helikodu plays through PipeWire together with every other program. PipeWire runs the device at one rate for everyone, by default 48 kHz, and converts other rates to it. Helikodu asks PipeWire for the track's rate, but PipeWire switches only to rates listed in its `default.clock.allowed-rates` setting. To let the device follow your music in shared mode, create `~/.config/pipewire/pipewire.conf.d/rates.conf`:

```
context.properties = {
    default.clock.allowed-rates = [ 44100 48000 88200 96000 176400 192000 ]
}
```

and restart PipeWire (`systemctl --user restart pipewire`). PipeWire only changes the rate when no other program is playing at a different one.

In **exclusive** mode, Helikodu takes the device for itself:

- On an **ALSA hardware device** (`hw:CARD=…`), Helikodu asks PipeWire's session manager to hand over the whole sound card and plays to it directly, at each track's own rate. Other programs cannot play on that card meanwhile. Helikodu gives the card back when you stop and after 30 seconds of pause.
- On a **PipeWire device**, Helikodu asks PipeWire for exclusive use and forces the track's rate, whatever `allowed-rates` says.

Exclusive mode with the volume set to **Fixed at 100 %** **Even out loudness** set to **Off** and no stage on under **DSP and EQ** gives bit-perfect playback: the deck shows **Bit-perfect** when the samples reach the device unchanged.

## Why do the colours look different on KDE Plasma and GNOME?

Helikodu has its own design and does not use the desktop's widget theme, so it looks the same everywhere. What it takes from the desktop:

- **Light or dark:** it follows the desktop's preference. On GNOME that is **Settings → Appearance**. On KDE Plasma, the colour scheme you choose in **System Settings → Colours & Themes → Colours** is reported as light or dark from its window background. If Helikodu shows the wrong one, check that `xdg-desktop-portal` and your desktop's portal (`xdg-desktop-portal-gnome` or `xdg-desktop-portal-kde`) are installed.
- **Accent colour:** the desktop's accent colour where the desktop offers one (KDE Plasma, and GNOME 47 or newer), amber otherwise.
- **High contrast and reduced motion**, where the desktop offers them.
- **Window style:** on GNOME and KDE Plasma, Helikodu draws its own header with the window buttons in it (*Integrated*); on other desktops it uses the desktop's normal title bar (*Classic*), which can also show a menu bar.

You can choose all of these yourself in **Settings → Appearance**, or try them once with `helikodu --scheme dark --accent violet --style classic`.

## How do I get rid of the title bar on KDE Plasma?

Helikodu starts in the *Integrated* window style on KDE Plasma and asks KWin not to decorate the window. If you chose *Classic*, KDE draws its usual title bar; to have none, choose **Integrated** in **Settings → Appearance → Window style** and start Helikodu again (or try it once with `helikodu --style integrated`). Helikodu then draws its own header with the window buttons in it and tells KWin not to decorate the window.

If a title bar is still drawn, a KWin rule forces it: look in **System Settings → Window Management → Window Rules** for a rule that matches Helikodu (or every window) and sets **No titlebar and frame**, and remove that property or set it to **Do Not Affect**. A rule can also hide the title bar in the *Classic* style: add **No titlebar and frame**, **Force**, **Yes** for the window class `io.github.sorrelfox.Helikodu`.

## Where are my settings and my library?

Settings are in `~/.config/helikodu/settings.toml`, the library (with ratings, play counts and playlists) in `~/.local/share/helikodu/library.db`, the last session in `~/.local/state/helikodu/`, and cover thumbnails in `~/.cache/helikodu/`. See [Where Helikodu keeps its files](guide/files.md).

## Does Helikodu change my music files?

Only when you ask it to. It reads your files and keeps everything it learns, including your ratings and play counts, in its own library database. The files change only when you save tags in the [tag editor](guide/tags.md), write loudness values to the tags, organize files (renaming and moving them, which can be undone) or move copies to the desktop trash; Convert and Sync and Export write new files and leave the originals alone. Ratings are not written into the files.

## Can I use Helikodu with only the keyboard, or with a screen reader?

Yes. Every feature can be reached from the keyboard: **F6** moves between the sidebar, the content, the right panel and the player bar, **F10** opens the main menu, the arrow keys move inside lists and grids, **Return** plays or opens, and the Menu key or **Shift+F10** opens a context menu. See [Keyboard shortcuts](guide/keyboard-shortcuts.md). Every control has a name and a role for screen readers such as Orca. Helikodu also follows the desktop's high contrast and reduced motion settings. If something cannot be reached or is read wrongly, please [report it](#how-do-i-report-a-bug).

## How do I report a bug?

Open an issue on [GitHub](https://github.com/sorrelfox/helikodu/issues). Please include:

- the Helikodu version (`helikodu-cli --version`) and your distribution and desktop;
- what you did, what you expected and what happened;
- for playback problems, the output of `helikodu-cli list-outputs` and `helikodu-cli info FILE` for an affected file, and whether `helikodu-cli play FILE` shows the same problem;
- any messages Helikodu printed when started from a terminal.

Check these for personal information (such as folder names) before you post them. To report a security problem, follow `SECURITY.md` in the repository instead of opening a public issue.

## What does Helikodu know about me?

Only what is on your computer, and it keeps it there. Your library, play history and settings are stored in your home folder and are never sent anywhere: Helikodu has no network code, no accounts, no telemetry and no crash reporting. When you share a bug report, you decide what goes into it.
