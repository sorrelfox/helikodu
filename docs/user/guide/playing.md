# Playing music and the queue

## The player deck

The bar at the bottom of the window is the player deck. It shows the cover, title and artist of the playing track and has:

- **Previous**, **Play**/**Pause** and **Next**. "Previous" restarts the track when more than three seconds of it have played, and goes to the track before otherwise.
- The **seek bar**: click or drag to move through the track. Click the time to switch between the total length and the time left.
- **Shuffle** (on or off) and **Repeat** (off, all, this track).
- The **volume** control.
- The **output device** switcher; see [Choosing the audio output](audio-output.md).

Tracks of the same format follow each other without a gap, so live albums and DJ mixes play as they were recorded.

Right-click the play button (or press and hold it) for its menu: **Stop**, **Stop after this track** and **Jump to the playing track**. **Stop after this track** stops at the end of the current track, once. The **Playback** submenu of the main menu has **Play/Pause**, **Stop**, **Previous**, **Next**, **Seek Back**, **Seek Forward** and **Stop After This Track**.

## The queue

The panel on the right shows the **Queue**: what played before (**History**), what is playing (**Now playing**) and what comes next (**Next in queue**). Playing an album replaces the queue with that album. (Under **Playing an album** in the **Playback** settings you can choose **Adds to the end of the queue** instead. **Play next** and **Add to queue** always keep the queue.)

- Click an entry to play it.
- Right-click an entry for its menu. **Remove from queue**, or **Delete**, removes the selected entries.
- Drag tracks from a list onto the panel to add them to the queue.
- **Save as playlist** saves the queue as a playlist.
- **Clear** removes everything but the playing track.

With shuffle on, "Previous" retraces the tracks in the order they actually played; turning shuffle off returns to the album order.

**Ctrl+J** jumps to the playing track.

## When Helikodu starts again

Helikodu remembers the queue, the position in the playing track, the volume, shuffle, repeat and the output device. When you start it again, everything is as you left it, paused. (Choose **Start with an empty queue** under **When Helikodu starts** in the **Playback** settings if you prefer.)

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

## Media keys and desktop controls

Helikodu works with your keyboard's media keys, the media controls of GNOME and KDE Plasma, and other MPRIS remote controls. It keeps the computer from going to sleep while music plays (the screen can still lock); turn this off with **Keep the computer awake while music plays** in the **Playback** settings.

With the volume **Fixed at 100 % (bit-perfect)**, playback started from media keys, desktop controls or by opening files also waits for the full-output question; see [Volume](audio-output.md#volume).
