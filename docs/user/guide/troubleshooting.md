# Troubleshooting

## The ready-built programs do not start

Run `helikodu` in a terminal. If it says that `libavformat.so.61` (or another FFmpeg library) cannot be found, your system has a newer FFmpeg than the programs were built for. Build Helikodu from the source code instead; see [Installing Helikodu](installing.md).

If it names another library, such as `libsidplayfp.so.6`, `libopenmpt.so.0` or `libgme.so.0`, that library is not installed. Install it with your distribution's package manager and start Helikodu again.

## There is no sound

- Check the output button in the player deck: is the device you expect selected? `helikodu-cli list-outputs` lists every device Helikodu can see.
- If the device was unplugged, Helikodu pauses. Plug it in again, or choose another device, and press Play.
- With **Fixed at 100 % (bit-perfect)** volume, Helikodu waits until you answer **Play at full output level?**, once for each device. This also applies when playback is started with media keys or by opening files with Helikodu.
- Try `helikodu-cli play some-file.flac` in a terminal: it prints any error it meets.

## "The device is busy" in exclusive mode

Another program holds the sound card, or PipeWire did not hand it over. The message names the program if it can. Close that program, or use **Shared** mode. Exclusive mode on a `hw:` device needs a session manager that supports device reservation (WirePlumber does).

## Other programs lost their sound

In exclusive mode on an ALSA hardware device, Helikodu has the whole sound card to itself. Pause or stop playback, or switch to **Shared** mode; the card is given back when playback stops and after 30 seconds of pause.

## Albums are missing or split

- Albums are grouped by their tags: the album title and the album artist. Tracks with different album artist tags end up in different albums. Fix the tags in Helikodu's [tag editor](tags.md), or with another tag editor such as MusicBrainz Picard or Kid3; Helikodu notices the change.
- Check that the folder is in **Settings → Library**, and that it is not left out by a **Skip** pattern, a `.nomedia` file or a hidden (dot) folder name.
- Video files are only added when **Include video files** is on.
- A folder on a drive that is not connected is hidden until the drive is back.

## New files do not appear

Folders on network shares and FUSE file systems are scanned at an interval, not watched. Very large libraries can also exceed the system's limit of watched folders, after which Helikodu falls back to scanning at an interval. Use **Rescan all** in **Settings → Library**, or raise the limit:

```sh
sudo sysctl fs.inotify.max_user_watches=524288
```

## Some text in tags looks garbled

Old tags written in a legacy character set (such as Windows-1251 or Shift-JIS) are repaired automatically where Helikodu can tell. If a few still look wrong, correct them in the [tag editor](tags.md).

## Helikodu skips a track it used to play

If Helikodu ended abnormally twice in a row while the same track was loading or playing, it marks that track as suspect, starts paused and shows a notice. Suspect tracks are passed over in automatic playback until one of them plays to its end. Play the track yourself to try it again, and please [report the problem](../faq.md#how-do-i-report-a-bug) with the file's format (`helikodu-cli info FILE`).

## Helikodu stopped unexpectedly

After a crash, Helikodu says so at the next start; **Details** shows the report, with **Copy** and **Open folder**. The report is in `~/.local/state/helikodu/crashes/` and is never sent anywhere; add it to a bug report if you like.

## The library is read-only or missing

If the library database cannot be written (a read-only folder or drive), Helikodu shows the library but keeps no changes, and says so at the start. If the database is damaged, Helikodu replaces it with the newest good backup and sets the damaged file aside in the backups folder. If it cannot be opened at all, Helikodu plays files without a library and tells you why.

## Starting over

Close Helikodu and delete `~/.config/helikodu/settings.toml` to return to the default settings. Deleting `~/.local/state/helikodu/session.json` starts with an empty queue. See [Where Helikodu keeps its files](files.md).
