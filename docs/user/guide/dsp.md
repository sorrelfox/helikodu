# Equalizer and sound processing

**Settings → DSP and EQ** shapes the sound before it reaches the device. The page shows the processing stages in the order the sound passes them: **Equalizer**, **Crossfeed**, **Channel mix** and **Limiter**, then **Dither**. The switch beside a stage turns it on or off; choose the stage's name to edit it below, also while it is off. Every change is heard at once, without a click.

| Stage | What it does |
|---|---|
| **Equalizer** | **Graphic, 10 bands**, **Graphic, 31 bands** or **Parametric**. The graph shows the resulting response. The parametric equalizer takes peaking, shelf and pass filters (**Add filter**), and **Import AutoEQ file** reads the `ParametricEQ.txt` that AutoEQ gives for many headphones; **Export** writes your settings in the same format. **Save preset** keeps the current settings under a name, and **Reset to flat** starts over. The **Preamp** lowers the level so that boosts do not clip |
| **Crossfeed** | Mixes a little of each channel into the other, so that music made for loudspeakers is less tiring on headphones. Three presets, from the strongest to the mildest, or your own **Strength** and **Cut-off frequency** |
| **Channel mix** | **Stereo**, **Mono** or **Swap left and right**, the **Balance**, and whether the LFE channel of surround files is added to the mix |
| **Limiter** | Keeps peaks below full scale with a short **Lookahead**. The automatic preamp of the equalizer already prevents most clipping, so the limiter rarely has to act. While music plays, **Activity** shows the gain reduction now, the largest in this track and how often it limited |
| **Dither** | Added when the processed sound goes to a device with fewer bits (**Auto**) |

The readout at the bottom says whether the samples pass unchanged (**Bit-perfect**) or are processed (**DSP active**, in 32-bit float). When every stage is off, the samples are untouched and bit-perfect playback stays bit-perfect. The [Signal path](audio-output.md#the-signal-path) tab of the right panel shows the stages while music plays.

## Correction for your headphones

The AutoEQ project publishes equalizer settings that bring many headphones and earphones closer to a neutral sound. Helikodu does not include them (it never goes online, and the measurements belong to their authors), but it reads the file the project gives:

1. On the AutoEQ website (autoeq.app), choose your headphones and download the parametric equalizer settings as `ParametricEQ.txt` (the format for EqualizerAPO).
2. In **Settings → DSP and EQ**, choose **Parametric** and **Import AutoEQ file**, and pick the downloaded file. **Save preset** keeps it, for example under the name of the headphones; presets can be kept per output device.

The same file can also be the equalizer of single tracks: **Import AutoEQ File…** under **Equalizer for this track** in the tag editor (see [An equalizer for one track](#an-equalizer-for-one-track)), or `helikodu-cli dsp track-eq set ID --autoeq FILE`.

## More processing in the settings file

The compressor, convolution, pitch shift, the order of the stages and saved chains have no page in the settings window yet; the playback speed is set from the deck, and the equalizer of a track in the tag editor (both below). You set them in `settings.toml` (see [Settings](settings.md)) or with `helikodu-cli dsp`, which writes the same file. The running program reads a changed file the next time it starts. Every one of these is off by default; when they are off, playback stays bit-perfect.

### Compressor and night mode

The compressor evens out the level: loud passages come down, and the makeup gain lifts the quiet ones. The **night** preset compresses strongly, for listening at a low volume; **gentle** only a little; **custom** uses your own values. It sits just before the limiter, and it is best used with the limiter on, which catches the first milliseconds of a sudden loud sound.

```toml
[dsp.compressor]
enabled = true
preset = "night"         # night | gentle | custom
threshold_db = -30.0     # custom only: -60 to 0
ratio = 4.0              # custom only: 1 to 20
attack_ms = 5.0          # custom only: 0.1 to 200
release_ms = 250.0       # custom only: 10 to 3000
knee_db = 10.0           # custom only: 0 to 24
makeup_db = 12.0         # custom only: 0 to 24
lookahead_ms = 2.0       # 1 to 5, in steps of 0.5
```

`helikodu-cli dsp compressor` switches it on in night mode; `helikodu-cli dsp compressor gentle` or `custom` chooses another preset, and `helikodu-cli dsp off compressor` switches it off.

### Convolution

Convolution applies an impulse response, such as a room correction filter exported from REW or another measurement program. Give a folder of WAV files, one per sample rate, or name a file for each rate; a file named for the rate comes first. When no file has the device's rate, the one with the nearest rate is resampled. Files may be mono (used for every channel) or stereo, with 16, 24 or 32-bit integer or float samples, up to 20 seconds long. Convolution adds a delay of about 11 ms.

```toml
[dsp.convolution]
enabled = true
folder = "/home/me/Room correction"
normalize = true         # scale the response so that it boosts no frequency
gain_db = 0.0            # -24 to +12
wet_percent = 100.0      # how much of the output is processed, 0 to 100

[[dsp.convolution.files]]
rate_hz = 48000
path = "/home/me/Room correction/room-48k.wav"
```

| Command | What it does |
|---|---|
| `helikodu-cli dsp convolution folder DIR` | Uses the WAV files in a folder and switches convolution on |
| `helikodu-cli dsp convolution file RATE FILE` | Uses a file for one sample rate |
| `helikodu-cli dsp convolution mix --wet 50 --gain -3 --normalize true` | Sets the mix, the gain and the normalization |
| `helikodu-cli dsp convolution check [FILE…]` | Shows which file each common rate gets, or describes WAV files: rate, channels, length and the delay |
| `helikodu-cli dsp convolution clear` | Forgets the files and switches convolution off |

### Pitch shift

Moves the pitch up or down by up to 12 semitones (fractions are cents) without changing the tempo. It adds a delay of about 33 ms.

```toml
[dsp.pitch]
enabled = true
semitones = -2.0         # -12 to +12
```

`helikodu-cli dsp pitch -2` sets it; `helikodu-cli dsp pitch 0` switches it off.

### Playback speed

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

### The order of the stages and saved chains

By default the sound passes pitch shift, equalizer, convolution, crossfeed, channel mix, compressor and limiter, in that order. `order` changes it; stages it leaves out follow in the default order. A saved chain keeps an order and the stages that are on under a name; choosing it switches its stages on in its order and the others off. The stages keep their own values. A change is heard at once, without stopping.

```toml
[dsp]
order = ["equalizer", "convolution", "crossfeed", "channel-mix", "compressor", "limiter", "pitch"]

[[dsp.chains]]
name = "Night"
stages = ["equalizer", "compressor", "limiter"]
```

The stage names are `pitch`, `equalizer`, `convolution`, `crossfeed`, `channel-mix`, `compressor` and `limiter`.

| Command | What it does |
|---|---|
| `helikodu-cli dsp show [--json]` | Lists the stages in order, which are on, and the saved chains |
| `helikodu-cli dsp order STAGE…` | Sets the order |
| `helikodu-cli dsp default-order` | Goes back to the default order |
| `helikodu-cli dsp on STAGE…` / `off STAGE…` | Switches stages on or off |
| `helikodu-cli dsp chain list` | Lists the saved chains |
| `helikodu-cli dsp chain save NAME` | Saves the order and the stages that are on |
| `helikodu-cli dsp chain apply NAME` | Uses a saved chain |
| `helikodu-cli dsp chain delete NAME` | Deletes a saved chain |

### An equalizer for one track

A library track can have an equalizer of its own, which replaces the equalizer of the settings while it plays, and only then. It is stored in the library with the track's play count and rating, and never written to the file. `dsp.track_equalizer = false` makes every track play with the equalizer of the settings. In exclusive mode a track's equalizer is left out when the rest of the path is bit-perfect, as the leveller is.

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

### Trying it out

`helikodu-cli play --dsp FILE…` plays files through the DSP chain of the settings file; with `--backend wav --wav-out OUT.wav` the result is written to a file instead of played.
