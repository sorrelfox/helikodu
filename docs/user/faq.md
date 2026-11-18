# Frequently asked questions

## Why is Helikodu offline? Why is there no internet radio, streaming or online lookup?

Helikodu is a player for the music you own and keep on your computer. Leaving out every online feature keeps it simple, private and dependable: it starts and works the same with or without a network, it never waits for a server, and nothing about your listening leaves your computer.

This is a deliberate rule, not a missing feature. Helikodu contains no network code at all: no internet radio, no streaming services, no online cover or lyrics lookup, no scrobbling, no update checks and no telemetry. Every release is tested inside a sandbox without network access, and the tests fail if anything tries to open an internet connection. The only connections Helikodu makes are local ones on your computer: D-Bus (for media keys and desktop integration) and the sound server.

Playlists that contain web addresses keep them, but those entries are shown as unavailable and never fetched.

## Which formats can Helikodu play?

Practically every audio format FFmpeg can decode, including:

- **Lossless:** FLAC, ALAC, WAV, AIFF, WavPack, Monkey's Audio (APE), TAK, TTA, Wave64, CAF
- **Lossy:** MP3, AAC and M4A, Ogg Vorbis, Opus, Musepack, WMA, AC-3, E-AC-3, DTS, MP2, Speex, AMR
- **DSD:** DSF and DFF (played as high-resolution PCM)
- **Containers:** Matroska audio (MKA), WebM audio, M4B audiobooks with chapters
- **CUE sheets** that split one album file into tracks, and files with chapters

Tracks of the same format play gaplessly. Helikodu recognises files by their content, so a file with the wrong extension still plays. Tracker modules, chiptunes and MIDI files are not supported yet.

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

Exclusive mode with the volume set to **Fixed at 100 %** and loudness normalization **Off** gives bit-perfect playback: the deck shows **Bit-perfect** when the samples reach the device unchanged.

## Why do the colours look different on KDE Plasma and GNOME?

Helikodu has its own design and does not use the desktop's widget theme, so it looks the same everywhere. What it takes from the desktop:

- **Light or dark:** it follows the desktop's preference. On GNOME that is **Settings → Appearance**. On KDE Plasma, the colour scheme you choose in **System Settings → Colours & Themes → Colours** is reported as light or dark from its window background. If Helikodu shows the wrong one, check that `xdg-desktop-portal` and your desktop's portal (`xdg-desktop-portal-gnome` or `xdg-desktop-portal-kde`) are installed.
- **Accent colour:** the desktop's accent colour where the desktop offers one (KDE Plasma, and GNOME 47 or newer), amber otherwise.
- **High contrast and reduced motion**, where the desktop offers them.
- **Window style:** on GNOME, Helikodu draws its own title bar (*Integrated*); elsewhere it uses the desktop's normal title bar (*Classic*), which on KDE matches your window decorations and can show a menu bar.

You can choose all of these yourself in **Settings → Appearance**, or try them once with `helikodu --scheme dark --accent violet --style classic`.

## Where are my settings and my library?

Settings are in `~/.config/helikodu/settings.toml`, the library (with ratings, play counts and playlists) in `~/.local/share/helikodu/library.db`, the last session in `~/.local/state/helikodu/`, and cover thumbnails in `~/.cache/helikodu/`. See [Where Helikodu keeps its files](guide/files.md).

## Does Helikodu change my music files?

No. It reads your files and keeps everything it learns, including your ratings and play counts, in its own library database. It does not move, rename, copy or retag files.

## How do I report a bug?

Open an issue on [GitHub](https://github.com/sorrelfox/helikodu/issues). Please include:

- the Helikodu version (`helikodu-cli --version`) and your distribution and desktop;
- what you did, what you expected and what happened;
- for playback problems, the output of `helikodu-cli list-outputs` and `helikodu-cli info FILE` for an affected file, and whether `helikodu-cli play FILE` shows the same problem;
- any messages Helikodu printed when started from a terminal.

Check these for personal information (such as folder names) before you post them. To report a security problem, follow `SECURITY.md` in the repository instead of opening a public issue.

## What does Helikodu know about me?

Only what is on your computer, and it keeps it there. Your library, play history and settings are stored in your home folder and are never sent anywhere: Helikodu has no network code, no accounts, no telemetry and no crash reporting. When you share a bug report, you decide what goes into it.
