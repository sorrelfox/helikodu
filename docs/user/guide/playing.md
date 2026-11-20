# Playing music and the queue

## The player deck

The bar at the bottom of the window is the player deck. It shows the cover, title and artist of the playing track and has:

- **Previous**, **Play**/**Pause** and **Next**. "Previous" restarts the track when more than three seconds of it have played, and goes to the track before otherwise.
- The **seek bar**: click or drag to move through the track. Click the time to switch between the total length and the time left.
- **Shuffle** and **Repeat**. A click switches the mode off, or back to the one used last; right-click (or press and hold) to choose the mode. See [Play modes](#play-modes).
- The **volume** control.
- The **output device** switcher; see [Choosing the audio output](audio-output.md).
- The **heart** after the title adds the playing track to **Favourites**, or takes it out; see [Playlists and ratings](playlists-and-ratings.md).

Tracks of the same format follow each other without a gap, so live albums and DJ mixes play as they were recorded.

## The waveform

The bars of the seek bar show how loud each part of the track is. Helikodu works them out in the background for the playing track, the next few tracks in the queue and the last ones played; until then, and while your library is being scanned, the seek bar is a flat band. The waveforms are kept in the cache folder within the **Waveforms** limit under **Storage** in the **Library** settings; at the limit, the waveforms used longest ago are removed first.

## The analyzer

In Now Playing (**F11**, or click the cover in the deck), **Analyzer** shows the spectrum of the sound, with a line over each bar that holds its recent peak, and a meter for each channel: the RMS level over the last 300 ms, a line at the peak, and **CLIP** for two seconds when a peak reaches full scale. The sound is measured after the last processing stage, just before it goes to the device, so the volume and any DSP are included. The analyzer moves only while sound plays and the page is shown; with reduced motion it updates less often and the peak lines do not fall.

## Crossfades and fades

Under **Crossfade** in the **Playback** settings, tracks can blend into each other: **between tracks** sets how long the end of one track overlaps the start of the next, and **when you skip** does the same when you skip to another track. Both are **Off** by default. Tracks that follow each other on one album still join without a gap, and so does bit-perfect playback in exclusive mode.

When both tracks have been analysed (Helikodu analyses library tracks in the background; see [Track analysis](settings.md#track-analysis)) and **Start the crossfade where the track fades out** is on (the default; it is used while a crossfade between tracks is on), the crossfade between tracks follows the music instead of a fixed length: it starts where the ending track begins to fade out, leaves out the silence after its last sound and before the first sound of the next track, and is never longer than the length you set. A track that ends without a fade goes over into the next one within about half a second. Tracks that have not been analysed yet, and files outside the library, are crossfaded over the length you set.

**Fades** sets short fades for **Pause and resume**, **Stop**, **Seek** and **Skip to another track**, so that nothing clicks.

### Silence between and inside tracks

Two more options, under **Silence** in the **Playback** settings, use the same analysis. The line under them says how many tracks are analysed so far; **Settings › Library** in it opens the analysis.

- **Leave out silence at the start and end of each track** (off by default) leaves out the silence before the first sound and after the last sound of a track where tracks join, and when a track starts from its beginning.
- **Skip long silence inside a track** (off by default) jumps over long silent stretches inside a track, such as the minutes of silence before a hidden track at the end of an album. A stretch must last at least the time chosen beside it (**longer than** 10 seconds by default; 5 seconds to 10 minutes); one second of silence is kept at each side of the jump. Silence in the first 15 seconds of a track is kept.

Tracks that follow each other on one album are never trimmed, so pregaps and hidden tracks stay as the album has them, and bit-perfect playback in exclusive mode never trims or skips. Times stay those of the file: a trimmed track starts at the time where its sound begins, the time jumps forward over a skipped stretch as it would after a seek, and seeking goes exactly where you ask, into silence too. The length shown is the length of the file.

## Modules, game music, SID tunes and MIDI

Tracker modules, game music files and Commodore 64 SID tunes play like any other track; a file with several songs becomes one track per song. Looping tunes play their loop twice and then fade out (**Looping tunes** in the **Playback** settings). MIDI files need a SoundFont: choose one under **MIDI files**, or Helikodu uses one installed on the system, if there is one. For SID tunes, **Song lengths** can point to the `Songlengths.md5` file of the High Voltage SID Collection, so each tune plays for its real length.

Right-click the play button (or press and hold it) for its menu: **Stop**, **Stop after this track** and **Jump to the playing track**. **Stop after this track** stops at the end of the current track, once.

## Play modes

The **Playback** menu (in the main menu) has the transport and every play mode:

- **Previous album**, **Next album** and **Random album** jump between albums in the queue.
- **Shuffle**: **Tracks**, **Albums** (the albums in a random order, each played in order), **Folders**, **Groupings** (by the `GROUPING` tag) or **Tracks within each album**. "Previous" retraces the order in which tracks actually played.
- **Repeat**: **This track**, **This album** or **The whole queue**.
- **Stop after**: **This track**, **This album** or **The queue**.
- **Play only**: **This artist** or **This album** keeps playback within the artist or album of the playing track.
- **Queue**: **Remove tracks after they play**, and **Continue from the selected track**, which makes the next track follow the entry you selected in a list.
- **Loop a section**: **Set the start here** and **Set the end here** mark an A-B loop at the playing position; the section then repeats without a gap until you choose **Clear the loop**, here or with the cross on the loop's chip on the deck.
- **Sleep timer…** pauses playback after a number of minutes, fading out, or at the end of the track that is playing then. A chip on the deck shows the time left; click it to add ten minutes or turn the timer off.

The deck shows the active modes, the loop's marks on the seek bar, and chips for the loop and the timer.

Each track in your library can also have its own settings, under **Playback** in the **File info** tab of the [tag editor](tags.md): **Leave out when shuffling**, **Play only when I start it myself** (automatic playback passes over it), and **Volume for this track**. They are kept in the library, not in the file, and apply at once.

## The queue

The panel on the right shows the **Queue**: what played before (**History**), what is playing (**Now playing**) and what comes next (**Next in queue**). Playing an album replaces the queue with that album. (Under **Playing an album** in the **Playback** settings you can choose **Adds to the end of the queue** instead. **Play next** and **Add to queue** always keep the queue.)

- Click an entry to play it.
- Right-click an entry for its menu. **Remove from queue**, or **Delete**, removes the selected entries.
- Drag tracks from a list onto the panel to add them to the queue.
- **Save as playlist** saves the queue as a playlist.
- **Clear** removes everything but the playing track.

With shuffle on, "Previous" retraces the tracks in the order they actually played; turning shuffle off returns to the album order.

**Ctrl+J** jumps to the playing track. With **Follow the playing track** in the View menu, the queue panel scrolls to each new track; Helikodu remembers this setting.

## When Helikodu starts again

Helikodu remembers the queue, the position in the playing track, the volume, shuffle, repeat and the output device, and whether music was playing. When you start it again, everything is as you left it: if music was playing when you closed or quit Helikodu, it continues where it stopped, with a short fade in; if it was paused or stopped, it stays paused. Either way the track is selected in the queue panel, which scrolls to it. (Choose **Start with an empty queue** under **When Helikodu starts** in the **Playback** settings if you prefer.)

Playback does not continue on its own when you start Helikodu with files to play (from the command line or your file manager's **Open With**), which play instead; when the track's file is missing; or when Helikodu ended abnormally while that track was playing.

Long tracks and audiobooks, by default those over 20 minutes, continue where you stopped listening, even after you played something else (**Resume long tracks where they stopped**).

## Opening files directly

You can play files that are not in your library:

- **Open Files…** or **Open Folder…** in the main menu plays them.
- Drop files or folders from your file manager on the window to add them to the queue.
- Choose Helikodu in your file manager's **Open With** menu, or start it with file names:

```sh
helikodu ~/Downloads/concert.flac
```

If Helikodu is already running, the files go to the running window.

## Play counts

A track counts as played once you have listened to half of it or four minutes of it, whichever comes first. Skipping ahead does not count as listening. Both limits can be changed under **Count a play** in the **Playback** settings.

## Play history

**Library ▸ History** in the main menu lists everything you listened to, newest first and by day, with the time it started, where it was played from (an album, a playlist, a search, a folder or the queue) and how long you listened. A track you skipped before it counted as a play is dimmed and shows where you skipped it. Choose **Today**, **7 days**, **30 days** or **All** at the top, and type in **Filter** to find a track, artist, album or source. Double-click a track (or press Enter) to play it; right-click it for the usual menu.

**Save as playlist** makes a new playlist of what is shown, without the skipped tracks and with each track once.

**Library ▸ Export History…** writes the whole history to a file on your computer; nothing is sent anywhere. Name the file with `.jsonl` for ListenBrainz listens (one JSON object per line, in the listen format of the ListenBrainz API, plays only), or with `.log` for a `.scrobbler.log` (the Audioscrobbler portable format that Last.fm tools read; plays are marked `L` and skips `S`). You can submit such a file to a listening service later with a tool of your choice.

## The mini player

**Ctrl+Shift+M**, or **Mini player** in the tray menu, puts a small player in place of the main window: the cover, the track, the seek bar, the play controls and the output's format. A click on the cover opens Now Playing in the main window; **Back to the full window** returns to it. The button with the window icon opens a drawer underneath with the **Queue** (double-click a track to play it, **Delete** removes it) and the **Lyrics** (click a line to play from there); the mini player remembers whether the drawer was open and which tab it showed. Like the deck, it is dark in the dark colour scheme and light in the light one. Its close button closes it; with the main window hidden as well, that closes Helikodu, or keeps it playing in the tray where the settings say so.

## Pausing when the screen locks or the computer sleeps

In **Settings ▸ Playback ▸ Power**, **Pause when the screen locks** (off by default) and **Pause before the computer goes to sleep** (on by default) pause playback at those moments; it stays paused afterwards.

## Media keys and desktop controls

Helikodu works with your keyboard's media keys, the media controls of GNOME and KDE Plasma, and other MPRIS remote controls. Remote controls that show a track list see the queue (for a very long queue, the part around the playing track) and can jump to a track in it; those that list playlists can start one of your playlists. It keeps the computer from going to sleep while music plays (the screen can still lock); turn this off with **Keep the computer awake while music plays** in the **Playback** settings.

With the volume **Fixed at 100 % (bit-perfect)**, playback started from media keys, desktop controls or by opening files also waits for the full-output question; see [Volume](audio-output.md#volume).
