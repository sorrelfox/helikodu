# Spike: audio output

## Question

Can Helikodu drive its output itself, with a native PipeWire backend (`pipewire` crate), a native ALSA backend (`alsa` crate) and cpal 0.18 for PulseAudio and JACK? In detail:

1. **PipeWire.** Enumerate sink nodes; open a playback stream aimed at one node by `object.serial`; request a rate with `node.rate`; use `node.exclusive`; set the stream volume; see devices appear and disappear.
2. **ALSA.** Enumerate cards and PCMs; open `hw:` and negotiate format, rate, channels, period and buffer exactly; find out what happens while PipeWire holds the card, and whether device reservation (`org.freedesktop.ReserveDevice1`) or PipeWire exclusive links give exclusive access.
3. **cpal 0.18.** Enumerate and open a stream on the PulseAudio and JACK hosts (JACK through pipewire-jack). The PipeWire host was tried too, as the fallback.
4. **Real-time pattern.** A render thread fills a lock-free ring buffer (`rtrb`); the real-time callback only copies out of it. Measure callback timing and underruns.

## Environment

Fedora, KDE Plasma (Wayland), kernel 7.3, PipeWire 1.6.11 with WirePlumber, pipewire-pulse and pipewire-jack, alsa-lib 1.2.16.1, Rust 1.100. 32 CPU threads. Five sound cards: two USB interfaces, two HDMI controllers, and the on-board Realtek ALC1220 (analog and S/PDIF).

Crates: `pipewire` 0.10.1 (feature `v1_2_0`), `alsa` **0.11.0** (see below), `cpal` 0.18.2 (features `pulseaudio`, `jack`, `pipewire`), `jack` 0.13.5, `rtrb` 0.4.1, `zbus` 6.3 (only for the reservation probe).

## Method

No sound was ever sent to a speaker or headphone. Every PipeWire, PulseAudio and JACK stream played digital silence into a temporary null sink:

```sh
pactl load-module module-null-sink sink_name=helikodu_spike sink_properties=node.description=Helikodu-spike-null-sink
# ... tests ...
pactl unload-module <module number>
```

Loading it did not change the default sink. On real `hw:` devices the probes only opened, negotiated and closed, except for one write of 150 ms of zeros to the S/PDIF output, which has nothing connected. To make PipeWire hold a card, a stream sent silence to that S/PDIF sink for under one second.

Four probe programs, all in `src/bin/`, and a shared library in `src/lib.rs`:

| Program | What it does |
|---|---|
| `pw_probe list [--watch S] [--hotplug]` | Lists sink nodes from the registry. With `--hotplug`, loads and unloads a second null sink and logs the registry events |
| `pw_probe play --target SERIAL_OR_NAME ...` | Opens a stream with `target.object`, `node.rate`, optional `node.force-rate`, `node.exclusive` and a stream volume. Optionally a second stream to the same target, a render stall, or a command run mid-stream. Reports state changes, the negotiated format, the links the registry shows, the graph rate seen in the callback, thread scheduling, and callback statistics |
| `alsa_probe list / caps / open / negotiate` | Cards, PCM devices, named PCMs and the hardware parameter space of each `hw:` device; exact negotiation; a short silent write |
| `reserve --card N --device hw:...` | Takes `org.freedesktop.ReserveDevice1.Audio<N>` from WirePlumber with `RequestRelease`, opens the device, and gives the name back |
| `cpal_probe run --host pulseaudio\|pipewire\|jack ...` | Enumerates, opens a stream on the null sink and measures the callback |

The render-thread pattern is the same everywhere: a normal-priority thread writes into an `rtrb` ring of 200 ms; the callback copies out of it and fills any shortfall with zeros, counting it as an underrun. Callback intervals and the time spent inside the callback are stored in preallocated atomics, so measuring does not disturb the real-time path.

Building the JACK part needs `jack.pc` (on Fedora, `pipewire-jack-audio-connection-kit-devel`); see "cpal" below. The JACK test must run with `PIPEWIRE_PROPS='{ jack.self-connect-mode = ignore-all }'`, and the probe refuses to run without it.

## Results

### PipeWire (`pipewire` 0.10.1)

| Measurement | Result |
|---|---|
| Enumeration | 19 nodes, 8 sinks, complete after one core sync in about 1 ms. Each sink has `object.serial`, `node.name`, `node.description` and `media.class` in its registry properties |
| Object id versus serial | Ids are reused (id 104 had serial 126). When WirePlumber re-created a sink, it kept id 73 but its serial changed from 73 to 8081 |
| Hot-plug | A null sink loaded with `pactl` appeared in the registry within the 6 ms the `pactl` call took; unloading it produced `global_remove` within 10 ms |
| `target.object` | A serial (`7855`) and a node name (`helikodu_spike`) both linked the stream to the null sink (checked through the registry's link objects). The object id (`142`) was **not** accepted |
| Missing target | Without `node.dont-fallback`, a stream whose target no longer exists is linked to the **default sink** instead. This happened once, by accident, with a stale serial: one second of silence went to the default device. With `node.dont-fallback = true`, the stream fails with `Error("defined target not found")` within 3 ms |
| Target removed while playing | With `node.dont-reconnect = true`, the stream went Streaming → Paused → `Error("target not found")` 5–9 ms after the sink was removed. It did not move. The core then reports several `unknown resource` errors for the dead link; these are not fatal |
| Time to first callback | Stream reached Streaming 26 ms after creation; first process callback after about 5 ms on the null sink, 19–182 ms on the real S/PDIF sink |
| `node.rate = 1/44100` | Accepted and visible on the node, but the graph stayed at 48 kHz, because `clock.allowed-rates` is `[ 48000 ]` (the default). PipeWire resampled: 235 frames per callback instead of 256 |
| `node.force-rate = 44100` | The null sink's graph switched to 44.1 kHz (the callback saw `rate 1/44100`, 256 frames every 5.805 ms), **without changing any global setting**. On a real device this changes the rate for every stream on that device |
| `node.exclusive = true` | Enforced. With an exclusive stream linked, a second stream to the same sink failed with `target not found`. An exclusive stream could also not join a sink that already had a shared stream. Two shared streams linked normally |
| Stream volume | `set_control(SPA_PROP_channelVolumes, [0.125, 0.125])` returned Ok; `pw-dump` showed `channelVolumes [0.125, 0.125]` and `pactl` showed 50 % (cubic: 0.5³ = 0.125), so the desktop mixer sees it |
| Volume memory | WirePlumber stored the volume under `application.name` and restored 0.125 on every later run. The engine has to reconcile this with its own stored volume |
| Real-time | The data thread (`data-loop.0`) ran with SCHED_RR priority 20 without any request from the program; the main and render threads were SCHED_OTHER |

Callback timing on the null sink (release build unless noted, silence through the ring buffer):

| Quantum | Callbacks | Interval (µs) min / median / p99 / max | Time in callback (ns) median / p99 / max | Underruns |
|---|---|---|---|---|
| 64 (1.33 ms) | 7496 | 1318 / 1333 / 1343 / 1352 | 130 / 350 / 531 | 0 |
| 256 (5.33 ms) | 936 | 5311 / 5333 / 5345 / 5355 | 4701 / 7580 / 9281 (debug build) | 0 |
| 1024 (21.3 ms) | 469 | 21310 / 21333 / 21348 / 21355 | 460 / 940 / 2070 | 0 |
| 256, 64 busy threads on 32 CPUs | 936 | 4996 / 5333 / 5343 / 5346 | 270 / 400 / 470 | 0 |
| 64, 20 ms ring, same load | 1111 | 957 / 1333 / 1343 / 1994 | — | 0 |

Underrun detection: a 150 ms stall of the render thread with a 200 ms ring caused no underrun; a 300 ms stall caused 20 short callbacks and 5120 missing frames (the expected ~100 ms), while the callback cadence stayed unchanged.

The first version of the probe filled the whole mapped buffer (12288 frames) on each callback, which made the callback run every 256 ms. The callback must fill `buffer.requested()` frames.

### ALSA (`alsa` 0.11.0)

**Version conflict.** `alsa` 0.12.1 cannot be used together with cpal 0.18.2: cpal depends on `alsa` 0.11 (`alsa-sys` 0.4) unconditionally on Linux, `alsa` 0.12 uses `alsa-sys` 0.6, and Cargo refuses two crates that link the same native library. The spike therefore uses 0.11.0. The native ALSA backend has to use the same `alsa` version as cpal, until cpal moves.

Enumeration of 5 cards and 14 playback devices through the control interface took 0.5 ms; `/proc/asound/cardN/pcmMp/sub0/status` gives the state and `owner_pid` of a busy device. Named PCMs come from the hint API (`default`, `pipewire`, `sysdefault:`, `front:`, `surround*:`, `iec958:`, `hdmi:`).

Parameter space of the `hw:` devices (opened non-blocking while PipeWire had them suspended):

| Device | Formats | Rates | Channels | Open time |
|---|---|---|---|---|
| `hw:CARD=AMS22,DEV=0` (USB) | S32_LE | 48000 only | 2 | 33 µs |
| `hw:CARD=CODEC,DEV=0` (USB, PCM2902) | S8 U8 S16_LE | 32000–48000 | 1–2 | 33 µs |
| `hw:CARD=Generic_1,DEV=0` (ALC1220 analog) | S16_LE S32_LE | 44100–192000 | 2 | 171 ms (codec power-up) |
| `hw:CARD=Generic_1,DEV=1` (ALC1220 S/PDIF) | S16_LE S32_LE | 32000–192000 (no 176400) | 2 | 0.08 ms |
| HDMI outputs (9 devices) | S16_LE S32_LE | 32000–192000, or 32000–48000 on two monitors | 2–8 or 2 | 14 ms first, then < 0.1 ms |

No device exposed a DSD format. Period sizes ranged from 4–16 frames up to 524288; buffers up to 1048576 frames.

Exact negotiation (no plug layer, `set_rate_resample(false)`, 200 ms buffer in 4 periods) on the S/PDIF device: S16_LE 44100 → buffer 8832 frames (200.3 ms), period 2208 frames; S32_LE 96000 → 19200 / 4800 frames. `hw_params` took 2.6–3.1 ms. Refusals came back immediately as `EINVAL`: 176400 Hz on S/PDIF, S24_3LE on S/PDIF, 44100 Hz on the AMS-22, S32_LE on the PCM2902. A 150 ms write of zeros started the device (start threshold of one period) within 31 µs, with `delay` reporting 6624 frames queued.

`Format::all()` contains `Unknown` (-1). Passing it to `test_format` triggers an assertion in alsa-lib (`snd_mask_leave`) and **aborts the process**. Only real formats may be tested.

**While PipeWire holds the card.** With a PipeWire stream playing to the S/PDIF sink, `hw:CARD=Generic_1,DEV=1`, `plughw:` and `iec958:` all failed at once with `EBUSY` (errno 16); `/proc` showed `owner_pid 4121` (pipewire). After the last stream ended, PipeWire kept the device open for **5.0 s** (its suspend timeout) before `hw:` could be opened. While a sink is suspended, `hw:` opens freely.

**Device reservation.** WirePlumber owns `org.freedesktop.ReserveDevice1.Audio0` to `Audio4` for every card, even while the card is idle, with `Priority = -20`, `ApplicationName = "WirePlumber"` and `ApplicationDeviceName = "alsa_card.pci-0000_78_00.6"`. The protocol worked exactly like JACK's:

| Step | Idle card | Card in use by a PipeWire stream |
|---|---|---|
| `RequestName(DO_NOT_QUEUE)` | refused (name taken) | refused |
| `RequestRelease(i32::MAX)` to WirePlumber | `true` in 0.34 ms | `true` in 0.39 ms |
| `RequestName(REPLACE_EXISTING)` | primary owner | primary owner |
| `hw:` open | at once | after 5 ms (2 attempts) |
| PipeWire nodes of the card | all removed (sink, monitor and the card's capture source) | all removed; the playing stream ended with `defined target not found` |
| WirePlumber asks back | `RequestRelease(-20)` on our object within 10 ms; we answered `false` | same |
| After releasing the name | WirePlumber owned it again after 5.2 ms; all nodes back after 49–54 ms | same |

No new D-Bus permission was needed outside a sandbox. Inside Flatpak, owning `org.freedesktop.ReserveDevice1.*` needs `--own-name=org.freedesktop.ReserveDevice1.*` and talking to the owner needs `--talk-name` for the same names (not tested here).

**PipeWire exclusive links** (`node.exclusive`) work on 1.6 (see above), but do not give access to `hw:`: PipeWire still owns the device and converts to float. Together with `node.force-rate` they give exclusive use of a PipeWire sink at the track's rate.

### cpal 0.18.2

| Host | Enumeration | Device ID | Stream on the null sink | Notes |
|---|---|---|---|---|
| PulseAudio (via pipewire-pulse) | host 2.5 ms, 8 sinks in 0.7 ms | `pulseaudio:<sink name>` | Linked to the chosen sink; 44.1 kHz sent with `node.rate 1/44100` and `target.object` set by pipewire-pulse; volume 100 % | Callback every 42.7 ms with ~1881 frames, regardless of `BufferSize`. With `BufferSize::Default` (latency 8192/44100) the server's first requests were ~1.85 s long, which emptied the 200 ms ring: 2 underruns, 81405 missing frames at start. With `BufferSize::Fixed(1024)`: 0 underruns |
| JACK (pipewire-jack) | host 13 ms; only two synthetic devices, `jack:output` and `jack:input` | `jack:output` | 1024 frames at 48 kHz every 21.333 ms (min 21316, max 21347 µs); 0 underruns | Only the server rate is offered; 44100 Hz fails with "Sample rate 44100 Hz does not match the server rate". cpal connects its ports to `system:playback_*` (the default device) when the stream is built, and `Host::set_connect_automatically(false)` has no effect on the devices `Host::new` already created (read in the source). Worked around with pipewire-jack's `jack.self-connect-mode = ignore-all` and a second JACK client that patched the ports to the null sink |
| PipeWire (fallback) | host 2 ms; 10 devices including two synthetic defaults | `pipewire:<node.name>` | Targets by `object.serial`; no `node.rate` (graph stayed at 48 kHz, 940 frames per callback); interval 18.5–24.1 ms; 0 underruns | Reports a spurious "Device disconnected" error when the stream is dropped normally |

Building with the `jack` feature needs `pkg-config jack` on Linux even though `jack-sys` loads `libjack.so.0` at run time: its build script calls `pkg_config::find_library("jack").unwrap()`. Fedora ships `jack.pc` only in `pipewire-jack-audio-connection-kit-devel`. For this spike a temporary `jack.pc` outside the repository pointed at `/usr/lib64/pipewire-0.3/jack/`.

cpal has no volume, mixer or exclusive API on any of these hosts, as expected.

### Dependencies and the offline rule

The probe tree contains no HTTP or network-client crate. `zbus` 6.3 (even with only `default-rt`, `proxy` and `service`) pulls `socket2` and builds TCP transport support into its runtime; the dependency audit for the real `desktop` crate should know that. The `pulseaudio` crate cpal uses speaks the native protocol over the local socket.

## What worked and what did not

- **PipeWire, native:** everything that was asked for works with the crate as it is: targeting by serial or name, rate request, forced rate, exclusive links, stream volume, registry hot-plug, device loss, timing. No crash, no missing API; one `unsafe` (viewing the mapped buffer as `f32`). Rough edges: the core error callback also reports harmless `unknown resource` errors after a node disappears; `pw_stream` behaviour around a missing target depends on WirePlumber properties.
- **ALSA, native:** enumeration, exact negotiation, refusal, silent write and timing all work. Exclusive access while PipeWire runs works through device reservation, quickly and cleanly. Blocked only by the version lock-step with cpal.
- **cpal PulseAudio:** works; the device choice is honoured. Needs a fixed buffer size or a tolerant pre-fill.
- **cpal JACK:** works through pipewire-jack after two workarounds (auto-connection, build-time `jack.pc`). Fixed rate, as JACK is.
- **cpal PipeWire:** works as a shared-mode fallback; no rate request.
- **Not tested:** bit-perfect output through PipeWire to a real device (would have meant playing to real hardware); ALSA hot-plug through udev (needs a physical plug or root to load a virtual card); DSD formats (no device offers them); the `alsa` 0.12 API; the Flatpak sandbox.

## Recommendation on the output-backend decision

**Accept it**: an own engine, native PipeWire and ALSA backends, cpal for PulseAudio and JACK. The spike found no blocking problem in the `pipewire` crate, so cpal's PipeWire host is not needed as the primary path; it stays a usable fallback for shared mode. Three amendments:

1. **Pin `alsa` to the version cpal uses** (0.11 today) and upgrade both together. 0.12.1 cannot be in the same build as cpal 0.18.2.
2. **ALSA exclusive mode uses device reservation**: own `org.freedesktop.ReserveDevice1.Audio<N>` (send `RequestRelease`, then `REPLACE_EXISTING`), answer `false` to WirePlumber's immediate `RequestRelease` while playing, and release the name when the device is closed. "Set the profile to Off" is only needed if reservation fails.
3. **PipeWire "exclusive" mode is `node.exclusive` plus `node.force-rate`**, not `node.rate` alone. The settings page should say that this switches the device's rate for every other application on it, and that output is exact only up to 24 bits.

## Lessons for the engine's output module

- Persist the **node name**, not the serial, and pass the name in `target.object` (it works directly). Serials change when WirePlumber re-creates a node (for example after a reservation); object ids are not accepted at all.
- Always set `node.dont-reconnect = true` **and** `node.dont-fallback = true` on a stream for a chosen device; without the second, a missing device silently becomes the default device. Leave both unset for "System default".
- Treat the stream state `Error(...target not found)` as "device lost" and apply the device-lost policy. Do not quit on `unknown resource` core errors that follow it.
- Fill exactly `buffer.requested()` frames in the PipeWire process callback.
- WirePlumber restores stream volume per `application.name`. Decide at stream creation who wins (set the engine's volume after the stream reaches Paused, or adopt the restored one).
- Keep the 200 ms render-ahead ring: with it, a 150 ms render stall is invisible, and twice-oversubscribed CPUs caused no underrun even at a 1.3 ms quantum. Size the ring for the largest request the backend can make (PulseAudio's first requests exceed 200 ms unless a fixed buffer is set).
- PipeWire's data thread is already real-time (SCHED_RR 20). The ALSA writer thread needs its own request.
- ALSA: open `hw:` non-blocking first to detect `EBUSY` quickly; read `/proc/asound/.../status` for the owner to show in the UI; expect a codec power-up of up to ~170 ms on the first open; never pass `Format::Unknown` to alsa-lib.
- PipeWire keeps a card for 5 s after its last stream, so switching from PipeWire to ALSA exclusive on the same card must use reservation rather than waiting.
- cpal JACK: set `jack.self-connect-mode` (or patch cpal) so that the stream does not connect to the default device behind the user's back; packaging needs the JACK development files at build time.

## Files

- `Cargo.toml`, `Cargo.lock`: standalone project (its own workspace).
- `src/lib.rs`: callback statistics, render thread, ring-buffer pull, helpers.
- `src/bin/pw_probe.rs`, `src/bin/alsa_probe.rs`, `src/bin/reserve.rs`, `src/bin/cpal_probe.rs`: the probes described above.
