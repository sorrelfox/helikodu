# Equalizer and sound processing

**Settings → DSP and EQ** shapes the sound before it reaches the device. At the top of the page is the chain: eight numbered stages in two rows, in the order the sound passes them. By default that is **Pitch**, **Equalizer**, **Convolution**, **Crossfeed**, **Channel mix**, **Compressor** and **Limiter**, then **Dither**. The switch beside a stage turns it on or off; choose the stage's name to edit it below, also while it is off. A stage that is off reads **Bypassed**. Every change is heard at once, without a click.

| Stage | What it does |
|---|---|
| **Pitch** | Moves the pitch up or down in whole semitones (up to 12 either way) without changing the speed, for singing or playing along in another key. **Reset to 0** turns the shift off; at 0 the stage does nothing. It adds about 33 ms of delay |
| **Equalizer** | **Graphic, 10 bands**, **Graphic, 31 bands** or **Parametric**. The graph shows the resulting response. The parametric equalizer takes peaking, shelf and pass filters (**Add filter**), and **Import AutoEQ file** reads the `ParametricEQ.txt` that AutoEQ gives for many headphones; **Export** writes your settings in the same format. **Save preset** keeps the current settings under a name, and **Reset to flat** starts over. The **Preamp** lowers the level so that boosts do not clip. **Use the equalizers of single tracks** decides whether a track with an equalizer of its own plays with it (see [An equalizer for one track](#an-equalizer-for-one-track)) |
| **Convolution** | Applies an impulse response, such as a room correction measured with REW (see [Convolution](#convolution)) |
| **Crossfeed** | Mixes a little of each channel into the other, so that music made for loudspeakers is less tiring on headphones. Three presets, from the strongest to the mildest, or your own **Strength** and **Cut-off frequency** |
| **Channel mix** | **Stereo**, **Mono** or **Swap left and right**, the **Balance**, and whether the LFE channel of surround files is added to the mix |
| **Compressor** | Evens out the level: **Night** brings loud passages down and lifts quiet ones strongly, for listening at a low volume; **Gentle** only a little; **Custom** lets you set the threshold, ratio, attack, release, knee and makeup gain, starting from the preset chosen before. The **Lookahead** is used by every preset. While the limiter is off, a note offers **Turn the limiter on**: it catches the first milliseconds of a sudden loud sound. While music plays, **Activity** shows the gain reduction now and the largest in this track |
| **Limiter** | Keeps peaks below full scale with a short **Lookahead**. The automatic preamp of the equalizer already prevents most clipping, so the limiter rarely has to act. While music plays, **Activity** shows the gain reduction now, the largest in this track and how often it limited |
| **Dither** | Added when the processed sound goes to a device with fewer bits (**Auto**). It is always last |

The readout pinned to the bottom of the page says whether the samples pass unchanged (**Bit-perfect**) or are processed (**DSP active**, in 32-bit float), lists the stages at work in their order, and gives the delay they add ("Adds 13 ms"). When every stage is off, the samples are untouched and bit-perfect playback stays bit-perfect. The [Signal path](audio-output.md#the-signal-path) tab of the right panel shows the stages in the same order while music plays, with the delay each adds.

## The order of the stages and saved chains

Drag a stage to another place in the chain; a bar shows where it will go. With the stage's name focused, **Alt+Left** and **Alt+Right** move it one place, and its menu (right click, the **Menu** key or **Shift+F10**) has **Move Earlier**, **Move Later**, **Turn On** or **Turn Off**, and **Default Order**. **Dither** always stays last. **Default order** in the chain's row puts the order back without changing which stages are on.

**Saved chains** keeps the chain as it is now under a name: the order of the stages and which are on. Type a name at the bottom of the popover and choose **Save** (**Replace** for a name that exists). Choosing a saved chain switches its stages on in its order and the others off; each stage keeps its own values. The bin beside a chain deletes it, and **Undo** in the message that follows brings it back. A saved chain is not an equalizer preset: presets belong to the equalizer and keep its values.

## Correction for your headphones

The AutoEQ project publishes equalizer settings that bring many headphones and earphones closer to a neutral sound. Helikodu does not include them (it never goes online, and the measurements belong to their authors), but it reads the file the project gives:

1. On the AutoEQ website (autoeq.app), choose your headphones and download the parametric equalizer settings as `ParametricEQ.txt` (the format for EqualizerAPO).
2. In **Settings → DSP and EQ**, choose **Parametric** and **Import AutoEQ file**, and pick the downloaded file. **Save preset** keeps it, for example under the name of the headphones; presets can be kept per output device.

The same file can also be the equalizer of single tracks: **Import AutoEQ File…** under **Equalizer for this track** in the tag editor (see [An equalizer for one track](#an-equalizer-for-one-track)), or `helikodu-cli dsp track-eq set ID --autoeq FILE`.

## Convolution

Convolution applies an impulse response, such as a room correction filter exported from REW or another measurement program. The files are WAV files, one for each sample rate: mono (used for every channel) or stereo, with 16, 24 or 32-bit integer or float samples, up to 20 seconds long.

Under **Impulse responses**, **Choose…** picks a folder of such files; the table lists every sample rate that has a file, with its length, its channels and where it comes from. **Choose…** in a row picks a file by hand for that rate; a file chosen by hand comes before the folder, and the cross beside it goes back to the file of the folder. A rate without a file of its own uses the file with the nearest rate, resampled; while music plays, the rate in use is marked **In use now**. **Clear** forgets the folder.

A file that cannot be used is marked in its row with the reason: it is missing, it is not a WAV file, it is longer than 20 seconds, or it cannot be read. When it is the file for the rate in use, convolution is left out until you choose another file: the chain box shows a warning, and the readout and the signal path leave the stage out. Music at another rate is still processed.

Under **Mix**, **Normalize** brings the loudest frequency of the response to 0 dB, so that it boosts nothing; **Gain** and **Share of the processed sound** set the level and how much of the output is processed. Convolution adds about 11 ms of delay.

## An equalizer for one track

A library track can have an equalizer of its own, which replaces the main equalizer while it plays, and only then. It is stored in the library with the track's play count and rating, and never written to the file. While such a track plays, the Equalizer box reads "Track:" and the equalizer's name, and a note above the editor says so; the editor below stays the main equalizer, heard again when another track plays. **Track settings…** opens the track in the tag editor. Turn off **Use the equalizers of single tracks** to play every track with the main equalizer. In exclusive mode a track's equalizer is left out when the rest of the path is bit-perfect, as the leveller is.

**In the tag editor:** **Equalizer for this track**, in the **Playback** group of the **File info** tab, names the track's equalizer ("None" without one) and chooses a whole one: **Use the Current Equalizer** takes a copy of the main equalizer as it is now (named "Copy of" its preset, or "Own settings"), **Use a Preset** one of the built-in or your own presets, and **Import AutoEQ File…** a `ParametricEQ.txt` file, named after the file. With several tracks in the editor, **Use for All 3 Tracks** gives the others a copy of it; **Remove** takes it away. The line under the row gives its kind, its size and its preamp, and says when it does not apply. Its filters are not edited here: shape the main equalizer by ear, then take it over with **Use the Current Equalizer**. **Equalizer for This Track…** in the context menu of tracks opens the tag editor there; **No Equalizer for This Track** makes the selected tracks play without any equalizer, the main one included (the row then reads "No equalizer"; **Remove** goes back to the main equalizer).

| Command | What it does |
|---|---|
| `helikodu-cli dsp track-eq set ID --from-settings` | Gives track `ID` a copy of the equalizer of the settings |
| `helikodu-cli dsp track-eq set ID --autoeq FILE` | Reads an AutoEQ `ParametricEQ.txt` file for the track |
| `helikodu-cli dsp track-eq set ID --preset bass-boost` | Uses a built-in graphic preset |
| `helikodu-cli dsp track-eq set ID --none` | Plays the track without any equalizer |
| `helikodu-cli dsp track-eq show ID` / `list` | Shows a track's equalizer, or lists the tracks that have one |
| `helikodu-cli dsp track-eq clear ID` | Removes it |

`helikodu-cli info` shows track ids. A change applies the next time the track starts.

## In the settings file

The page writes the `[dsp]` table of `settings.toml` (see [Settings](settings.md)); `helikodu-cli dsp` writes the same file, and the running program reads a changed file the next time it starts. Every stage is off by default.

```toml
[dsp]
order = ["equalizer", "convolution", "crossfeed", "channel-mix", "compressor", "limiter", "pitch"]
track_equalizer = true

[[dsp.chains]]
name = "Night"
stages = ["equalizer", "compressor", "limiter"]

[dsp.compressor]
enabled = true
preset = "night"         # night | gentle | custom; the values below are for custom
lookahead_ms = 2.0       # 1 to 5, in steps of 0.5

[dsp.convolution]
enabled = true
folder = "/home/me/Room correction"
normalize = true
gain_db = 0.0            # -24 to +12
wet_percent = 100.0      # 0 to 100

[[dsp.convolution.files]]
rate_hz = 48000
path = "/home/me/Room correction/room-48k.wav"

[dsp.pitch]
enabled = true
semitones = -2.0         # -12 to +12; the command line also takes fractions
```

The stage names are `pitch`, `equalizer`, `convolution`, `crossfeed`, `channel-mix`, `compressor` and `limiter`; stages `order` leaves out follow in the default order. The custom compressor values are `threshold_db` (-60 to 0), `ratio` (1 to 20), `attack_ms` (0.1 to 200), `release_ms` (10 to 3000), `knee_db` (0 to 24) and `makeup_db` (0 to 24).

| Command | What it does |
|---|---|
| `helikodu-cli dsp show [--json]` | Lists the stages in order, which are on, and the saved chains |
| `helikodu-cli dsp order STAGE…` / `default-order` | Sets the order, or goes back to the default |
| `helikodu-cli dsp on STAGE…` / `off STAGE…` | Switches stages on or off |
| `helikodu-cli dsp chain list` / `save NAME` / `apply NAME` / `delete NAME` | Works with saved chains |
| `helikodu-cli dsp compressor [night\|gentle\|custom]` | Switches the compressor on with a preset |
| `helikodu-cli dsp convolution folder DIR` / `file RATE FILE` / `clear` | Sets the impulse responses |
| `helikodu-cli dsp convolution mix --wet 50 --gain -3 --normalize true` | Sets the mix |
| `helikodu-cli dsp convolution check [FILE…]` | Shows which file each common rate gets, or describes WAV files |
| `helikodu-cli dsp pitch -2` | Sets the pitch shift; `0` switches it off |

## Playback speed

Plays faster or slower, from half the speed to twice the speed.

**On the deck:** **Speed…** in the **Playback** menu, or in the menu of the play button (right-click it), opens the speed popover above the deck: a slider from 0.5× to 2× in steps of 0.05 with the quick choices 0.75×, 1×, 1.25×, 1.5× and 2×; **The pitch**, which **Stays** or **Follows the speed**; the **Pitch shift** of the pitch stage, since the two are often used together; and **Back to normal**, which sets the speed to 1× and leaves the pitch shift alone. **Ends in** gives the listening time until the playing track ends. While the speed is not 1×, a chip with a gauge before the time left on the deck shows it ("1.25×") and opens the popover. The popover changes the speed until Helikodu closes; tick **Start with this speed next time** to also write it to the settings file, now and at every later change (untick it and the settings go back to the normal speed). When bit-perfect playback wins (below), the chip reads "1.25× · not applied" and the popover says so. With `keep_pitch = true` (the default) the tempo changes and the pitch stays, as for audiobooks, podcasts or practising along; with `keep_pitch = false` the pitch follows the speed, as when a tape or a record runs faster or slower.

```toml
[dsp.speed]
enabled = true
rate = 1.25              # 0.5 to 2
keep_pitch = true        # false: the pitch follows the speed
```

Times stay those of the track: a track of 4:00 still shows 4:00 at 1.25×, its position moves through it faster than the clock, and it ends after 3:12. Seeking, the A-B loop, jumping over silence and the position that desktop controls show all work in the track's own time. Crossfades, fades and the sleep timer keep their lengths in real time: a crossfade of 5 seconds lasts 5 seconds at any speed, and **Intro Scan** plays its seconds of listening, which cover more or less of each track. Desktop controls see the speed as the playback rate, and can set it (from 0.5 to 2).

The speed applies to each track before the tracks are mixed, so it is not one of the stages in `order` and is not kept in saved chains. In exclusive mode, when nothing else changes the samples, bit-perfect playback wins and tracks play at their normal speed; in shared mode, or when another stage is on, the speed applies. The signal path shows it with the resampler.

`helikodu-cli dsp speed 1.25` sets it (`--tape` lets the pitch follow, `--keep-pitch` keeps it; without either, it stays as it was), and `helikodu-cli dsp speed 1` goes back to the normal speed. `helikodu-cli speed 1.25 [--tape]` changes the speed of the running program at once, until it ends; playback goes on where it was. The settings file gives the speed it starts with.

## Trying it out

`helikodu-cli play --dsp FILE…` plays files through the DSP chain of the settings file; with `--backend wav --wav-out OUT.wav` the result is written to a file instead of played.
