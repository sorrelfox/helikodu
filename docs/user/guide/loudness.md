# Loudness and ReplayGain

Albums are mastered at very different levels. Loudness normalization evens them out, so you do not have to reach for the volume control between albums.

Helikodu uses the loudness values stored in your files' tags: ReplayGain tags (from tools such as foobar2000, `rsgain`, `loudgain` or MusicBrainz Picard), and the `R128_TRACK_GAIN` and `R128_ALBUM_GAIN` tags of Opus files. It can also measure your tracks itself (see below); measured values take precedence over tags. Files with neither play at their own level.

The **Loudness** page of the settings has:

| Setting | What it does |
|---|---|
| **Even out loudness** | **Off**; **Track** (every track at the same loudness); **Album** (whole albums at the same loudness, keeping the differences between their tracks); **Automatic** (the default: the album gain while an album plays in order, the track gain otherwise, for example in shuffle) |
| **Preamp** | Two sliders, −12 to +12 dB: **Files with loudness values** (added to their gain, default 0 dB) and **Files without loudness values** (their gain, default −6 dB, so they are not much louder than normalized ones) |
| **Prevent clipping** | **Lower the gain**: the gain is reduced until the loudest peak fits below full scale; or **Use the limiter**: the full gain is kept and peaks above full scale are limited |

Gains follow the ReplayGain 2.0 reference of −18 LUFS; Opus R128 gains are converted to it. A change takes effect at once, also on the playing track.

## Measuring loudness

Under **Loudness data**, **Measure the rest** measures every track and album that has no measured values yet, following EBU R128 (loudness, loudness range and peaks). **Measure loudness** in the right-click menu of tracks and albums measures just those. The measurement runs in the background, with its progress at the bottom of the sidebar, where **Stop measuring** ends it; **Measure the rest** continues later. A file that changes is measured again. The values are kept in the library; choose **Also write them to the tags of the files** to store them as ReplayGain 2.0 tags (and R128 tags in Opus files) as well. From the command line, `helikodu-cli loudness` does the same.

When a gain is applied, playback is no longer bit-perfect. Set **Even out loudness** to **Off** if bit-perfect output matters more to you.
