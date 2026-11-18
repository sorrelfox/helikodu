# Loudness and ReplayGain

Albums are mastered at very different levels. Loudness normalization evens them out, so you do not have to reach for the volume control between albums.

Helikodu uses the loudness values stored in your files' tags: ReplayGain tags (from tools such as foobar2000, `rsgain`, `loudgain` or MusicBrainz Picard), and the `R128_TRACK_GAIN` and `R128_ALBUM_GAIN` tags of Opus files. It does not measure files itself yet, so files without these tags play at their own level.

The **Loudness** page of the settings has:

| Setting | What it does |
|---|---|
| Mode | **Off**; **Track** (every track at the same loudness); **Album** (whole albums at the same loudness, keeping the differences between their tracks); **Automatic** (the default: the album gain while an album plays in order, the track gain otherwise) |
| Preamp | Added to the gain of tagged files, −12 to +12 dB (default 0) |
| Gain of untagged files | The gain for untagged files, −12 to +12 dB (default −6 dB, so they are not much louder than normalized ones) |
| Clipping prevention | Lowering the gain so that the loudest peak stays below full scale (the default) |

Gains follow the ReplayGain 2.0 reference of −18 LUFS; Opus R128 gains are converted to it. A change takes effect at once, also on the playing track.

When a gain is applied, playback is no longer bit-perfect. Set the mode to **Off** if bit-perfect output matters more to you.
