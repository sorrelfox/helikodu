# Choosing the audio output

Helikodu lets you decide exactly where your music goes.

## The quick switcher

The output button in the player deck shows the device in use. Click it to see the devices you can play to, with the system default marked, and pick one. Playback moves to the new device at the same position, without stopping. The same popover switches between **Shared** and **Exclusive** mode, and **Audio output settings** opens the full settings.

## The Audio output settings

**Settings → Audio output** has the full set of choices:

| Setting | Choices |
|---|---|
| **Backend** | **Automatic** (PipeWire when it is running, ALSA otherwise), **PipeWire** or **ALSA** |
| **Device** | **System default**, or a device of that backend |
| **Mode** | **Shared** or **Exclusive, bit-perfect** |
| **Sample rate** | **Match the track when possible** (the default), **The device's default rate**, or always one fixed rate |
| **Sample format** | **Automatic**, 16-bit, 24-bit (packed or in 32-bit words), 32-bit or 32-bit float |
| **Volume control** | **System volume** (the default), **Software volume in Helikodu**, the device's **Hardware mixer**, or **Fixed at 100 % (bit-perfect)** |
| **If the device disappears** | **Pause** (the default), **Continue on the default device** or **Stop** |
| **Buffer** | How much audio is prepared ahead, 50 to 2000 ms (default 200 ms) |
| **DAC warm-up** | Milliseconds of silence after the device opens, for converters that need a moment to lock on (default 0) |
| **Pause when headphones are unplugged** | On (the default): when the system default output moves away from a device that is removed, playback pauses instead of going on through the next device |

Changes apply while music plays, at the same position. Helikodu remembers the device by a stable name (a PipeWire node name such as `alsa_output.usb-Topping_D10s-00.analog-stereo`, or an ALSA name such as `hw:CARD=D10s,DEV=0`), so it finds it again after a restart or when you plug it into another USB port. If the chosen device is missing when Helikodu starts, it pauses and tells you; choose another device or plug it in.

PulseAudio and JACK are not supported yet. On most current systems PulseAudio programs are served by PipeWire anyway.

## Shared and exclusive mode

In **Shared** mode, Helikodu plays through the sound server like every other program, and you hear system sounds and other programs at the same time.

In **Exclusive** mode, Helikodu takes the device for itself:

- With an ALSA hardware device (`hw:…`), Helikodu asks PipeWire to hand over the sound card and plays to it directly. Other programs cannot use that card while Helikodu holds it, and the card's other inputs and outputs disappear from the sound settings for that time. Helikodu gives the card back when playback stops and after 30 seconds of pause.
- With a PipeWire device, Helikodu asks PipeWire for exclusive use and for the track's sample rate, which switches the device to that rate.

When nothing in the way changes the sound (no loudness gain, no resampling, volume at 100 % or outside Helikodu), playback is **bit-perfect**: the samples reach the device exactly as they are in the file. The deck shows **Bit-perfect** when this is the case.

## Volume

With **System volume** the volume control changes the volume of the system's mixer. With **Software volume in Helikodu** it scales the samples itself. With **Fixed at 100 % (bit-perfect)** the output is always at full level, for amplifiers and DACs with their own volume knob.

Because full level can be very loud, Helikodu asks before it first plays at full output on a device it has not asked about yet: **Play at full output level?** names the device and shows the volume now and after. Playback stays paused until you answer, and no sound reaches the device before you choose **Play at full output**; **Cancel** (or Escape) leaves it paused. You are asked once for each device. The question also comes when playback is started from outside the window, with media keys or your desktop's media controls, or by opening files with Helikodu, and when you switch to fixed volume or to such a device while music plays.

## Sample rates

With **Match the track when possible**, Helikodu opens the device at the track's own rate when the device supports it, so a 44.1 kHz album and a 96 kHz album both play unconverted. When the rate changes between tracks, the device is reopened, which can leave a very short gap. In shared mode on PipeWire, whether the device really switches depends on PipeWire's settings; see the [FAQ](../faq.md#what-does-exclusive-mode-do-and-why-does-my-dac-not-switch-sample-rates).
