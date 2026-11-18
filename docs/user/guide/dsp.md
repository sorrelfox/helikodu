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
