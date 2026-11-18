# Troubleshooting

## The ready-built programs do not start

Run `helikodu` in a terminal. If it says that `libavformat.so.61` (or another FFmpeg library) cannot be found, your system has a newer FFmpeg than the programs were built for. Build Helikodu from the source code instead; see [Installing Helikodu](installing.md).

## There is no sound

- Check the output button in the player deck: is the device you expect selected? `helikodu-cli list-outputs` lists every device Helikodu can see.
- If the device was unplugged, Helikodu pauses. Plug it in again, or choose another device, and press Play.
- With **Fixed at 100 %** volume, Helikodu waits for you to confirm full output level once per device.
- Try `helikodu-cli play some-file.flac` in a terminal: it prints any error it meets.

## "The device is busy" in exclusive mode

Another program holds the sound card, or PipeWire did not hand it over. The message names the program if it can. Close that program, or use **Shared** mode. Exclusive mode on a `hw:` device needs a session manager that supports device reservation (WirePlumber does).

## Other programs lost their sound

In exclusive mode on an ALSA hardware device, Helikodu has the whole sound card to itself. Pause or stop playback, or switch to **Shared** mode; the card is given back after the time set under **Release the device**.

## Albums are missing or split

- Albums are grouped by their tags: the album title and the album artist. Tracks with different album artist tags end up in different albums. Fix the tags with a tag editor such as MusicBrainz Picard or Kid3; Helikodu notices the change.
- Check that the folder is in **Settings → Library**, and that it is not left out by a **Skip** pattern, a `.nomedia` file or a hidden (dot) folder name.
- Video files are only added when **Include video files** is on.
- A folder on a drive that is not connected is hidden until the drive is back.

## New files do not appear

Folders on network shares and FUSE file systems are scanned at an interval, not watched. Very large libraries can also exceed the system's limit of watched folders, after which Helikodu falls back to scanning at an interval. Use **Rescan all** in **Settings → Library**, or raise the limit:

```sh
sudo sysctl fs.inotify.max_user_watches=524288
```

## Some text in tags looks garbled

Old tags written in a legacy character set (such as Windows-1251 or Shift-JIS) are repaired automatically where Helikodu can tell. If a few still look wrong, correct them with a tag editor.

## Helikodu skips a track it used to play

If Helikodu ended abnormally twice in a row while the same track was loading or playing, it marks that track as suspect, starts paused and shows a notice. Suspect tracks are passed over in automatic playback until one of them plays to its end. Play the track yourself to try it again, and please [report the problem](../faq.md#how-do-i-report-a-bug) with the file's format (`helikodu-cli info FILE`).

## Starting over

Close Helikodu and delete `~/.config/helikodu/settings.toml` to return to the default settings. Deleting `~/.local/state/helikodu/session.json` starts with an empty queue. See [Where Helikodu keeps its files](files.md).
