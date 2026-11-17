// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! PipeWire output probe, built on the `pipewire` crate.
//!
//! ```text
//! pw_probe list [--watch SECS] [--hotplug]
//! pw_probe play --target SERIAL_OR_NAME [--rate HZ] [--force-rate] [--exclusive]
//!               [--volume AMPLITUDE] [--secs SECS] [--quantum FRAMES]
//!               [--ring-ms MS] [--stall-at-ms MS --stall-ms MS]
//!               [--second-stream] [--second-exclusive]
//!               [--exec-at-ms MS --exec "COMMAND ARGS"]
//! ```
//!
//! `play` sends digital silence only. Point it at a null sink.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow};
use pipewire as pw;
use pw::properties::properties;
use pw::spa;
use pw::spa::pod::Pod;
use pw::types::ObjectType;
use spike_output::{CallbackStats, RenderConfig, pull, run, spawn_render, ts};

const CHANNELS: usize = 2;

#[derive(Debug, Default, Clone)]
struct Args {
    cmd: String,
    watch: u64,
    hotplug: bool,
    target: Option<String>,
    rate: u32,
    force_rate: bool,
    exclusive: bool,
    volume: Option<f32>,
    secs: f64,
    quantum: u32,
    ring_ms: u32,
    stall_at_ms: Option<u64>,
    stall_ms: u64,
    second: bool,
    second_exclusive: bool,
    exec_at_ms: Option<u64>,
    exec: Option<String>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        secs: 3.0,
        rate: 48000,
        quantum: 256,
        ring_ms: 200,
        ..Default::default()
    };
    let mut it = std::env::args().skip(1);
    a.cmd = it.next().unwrap_or_else(|| "list".into());
    while let Some(k) = it.next() {
        let mut val = || it.next().ok_or_else(|| anyhow!("{k} needs a value"));
        match k.as_str() {
            "--watch" => a.watch = val()?.parse()?,
            "--hotplug" => a.hotplug = true,
            "--target" => a.target = Some(val()?),
            "--rate" => a.rate = val()?.parse()?,
            "--force-rate" => a.force_rate = true,
            "--exclusive" => a.exclusive = true,
            "--volume" => a.volume = Some(val()?.parse()?),
            "--secs" => a.secs = val()?.parse()?,
            "--quantum" => a.quantum = val()?.parse()?,
            "--ring-ms" => a.ring_ms = val()?.parse()?,
            "--stall-at-ms" => a.stall_at_ms = Some(val()?.parse()?),
            "--stall-ms" => a.stall_ms = val()?.parse()?,
            "--second-stream" => a.second = true,
            "--second-exclusive" => a.second_exclusive = true,
            "--exec-at-ms" => a.exec_at_ms = Some(val()?.parse()?),
            "--exec" => a.exec = Some(val()?),
            other => return Err(anyhow!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn main() -> Result<()> {
    let args = parse_args()?;
    pw::init();
    match args.cmd.as_str() {
        "list" => list(&args),
        "play" => play(&args),
        other => Err(anyhow!("unknown command {other}")),
    }
}

/// What the registry told us about one node.
#[derive(Debug, Clone)]
struct NodeInfo {
    serial: String,
    name: String,
    description: String,
    class: String,
}

/// What the registry told us about one link.
#[derive(Debug, Clone, Copy)]
struct LinkInfo {
    out_node: u32,
    in_node: u32,
}

#[derive(Debug, Default)]
struct Graph {
    nodes: BTreeMap<u32, NodeInfo>,
    links: BTreeMap<u32, LinkInfo>,
}

/// Registers a registry listener that keeps `graph` up to date and logs
/// sink nodes appearing and disappearing.
fn watch_registry(
    registry: &pw::registry::RegistryRc,
    graph: Rc<RefCell<Graph>>,
    t0: Instant,
    log: bool,
) -> pw::registry::Listener {
    let g_add = graph.clone();
    let g_rem = graph;
    registry
        .add_listener_local()
        .global(move |obj| {
            let Some(props) = obj.props else { return };
            match obj.type_ {
                ObjectType::Node => {
                    let info = NodeInfo {
                        serial: props.get("object.serial").unwrap_or("").into(),
                        name: props.get("node.name").unwrap_or("").into(),
                        description: props.get("node.description").unwrap_or("").into(),
                        class: props.get("media.class").unwrap_or("").into(),
                    };
                    if log && info.class.starts_with("Audio/Sink") {
                        println!(
                            "{} + sink id={} serial={} name={} desc={:?}",
                            ts(t0),
                            obj.id,
                            info.serial,
                            info.name,
                            info.description
                        );
                    }
                    g_add.borrow_mut().nodes.insert(obj.id, info);
                }
                ObjectType::Link => {
                    let out_node = props.get("link.output.node").and_then(|v| v.parse().ok());
                    let in_node = props.get("link.input.node").and_then(|v| v.parse().ok());
                    if let (Some(out_node), Some(in_node)) = (out_node, in_node) {
                        g_add
                            .borrow_mut()
                            .links
                            .insert(obj.id, LinkInfo { out_node, in_node });
                    }
                }
                _ => {}
            }
        })
        .global_remove(move |id| {
            let mut g = g_rem.borrow_mut();
            if let Some(n) = g.nodes.remove(&id)
                && log
                && n.class.starts_with("Audio/Sink")
            {
                println!(
                    "{} - sink id={id} serial={} name={}",
                    ts(t0),
                    n.serial,
                    n.name
                );
            }
            g.links.remove(&id);
        })
        .register()
}

/// Lists sink nodes; optionally watches for hot-plug and provokes it by
/// loading and unloading a second null sink through pipewire-pulse.
fn list(args: &Args) -> Result<()> {
    let t0 = Instant::now();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let graph = Rc::new(RefCell::new(Graph::default()));
    let _l = watch_registry(&registry, graph.clone(), t0, true);

    // A roundtrip: run until the server has answered a sync, so the initial
    // registry burst is complete.
    let done = Rc::new(AtomicBool::new(false));
    let pending = core.sync(0)?;
    let ml = mainloop.clone();
    let d2 = done.clone();
    let _cl = core
        .add_listener_local()
        .done(move |id, seq| {
            if id == pw::core::PW_ID_CORE && seq == pending {
                d2.store(true, Ordering::Relaxed);
                ml.quit();
            }
        })
        .register();
    mainloop.run();
    println!(
        "{} initial enumeration complete: {} nodes, {} sinks",
        ts(t0),
        graph.borrow().nodes.len(),
        graph
            .borrow()
            .nodes
            .values()
            .filter(|n| n.class.starts_with("Audio/Sink"))
            .count()
    );

    if args.watch == 0 {
        return Ok(());
    }

    // Hot-plug: a helper thread loads and unloads a second null sink.
    let hp = if args.hotplug {
        Some(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let t_load = Instant::now();
            let out = run(
                "pactl",
                &[
                    "load-module",
                    "module-null-sink",
                    "sink_name=helikodu_spike_hotplug",
                    "sink_properties=node.description=Helikodu-spike-hotplug",
                ],
            );
            let module = out.trim().to_string();
            println!(
                "{} [helper] loaded module {module} (pactl took {:?})",
                ts(t0),
                t_load.elapsed()
            );
            std::thread::sleep(Duration::from_millis(1000));
            let t_unload = Instant::now();
            run("pactl", &["unload-module", &module]);
            println!(
                "{} [helper] unloaded module {module} (pactl took {:?})",
                ts(t0),
                t_unload.elapsed()
            );
        }))
    } else {
        None
    };

    let ml = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| ml.quit());
    timer
        .update_timer(Some(Duration::from_secs(args.watch)), None)
        .into_result()?;
    mainloop.run();
    if let Some(h) = hp {
        let _ = h.join();
    }
    Ok(())
}

/// State shared between the main loop callbacks of one playback stream.
struct StreamCtx {
    label: &'static str,
    consumer: rtrb::Consumer<f32>,
    stats: Arc<CallbackStats>,
    graph_rate: Arc<AtomicU32>,
    delay: Arc<AtomicI64>,
}

fn format_param(rate: u32) -> Result<Vec<u8>> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(rate);
    info.set_channels(CHANNELS as u32);
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    position[0] = spa::sys::SPA_AUDIO_CHANNEL_FL;
    position[1] = spa::sys::SPA_AUDIO_CHANNEL_FR;
    info.set_position(position);
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(spa::pod::Object {
            type_: spa::sys::SPA_TYPE_OBJECT_Format,
            id: spa::sys::SPA_PARAM_EnumFormat,
            properties: info.into(),
        }),
    )
    .map_err(|e| anyhow!("serialize format: {e:?}"))?
    .0
    .into_inner();
    Ok(bytes)
}

struct Opened {
    stream: pw::stream::StreamRc,
    _listener: pw::stream::StreamListener<StreamCtx>,
    stats: Arc<CallbackStats>,
    graph_rate: Arc<AtomicU32>,
    delay: Arc<AtomicI64>,
    streaming_at: Rc<RefCell<Option<Duration>>>,
}

#[allow(clippy::too_many_arguments)]
fn open_stream(
    core: &pw::core::CoreRc,
    label: &'static str,
    target: &str,
    rate: u32,
    force_rate: bool,
    exclusive: bool,
    quantum: u32,
    consumer: rtrb::Consumer<f32>,
    t0: Instant,
) -> Result<Opened> {
    let mut props = properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Playback",
        *pw::keys::MEDIA_ROLE => "Music",
        *pw::keys::APP_NAME => "helikodu-output-spike",
        *pw::keys::NODE_NAME => format!("helikodu-spike-{label}"),
        // The stream follows its device or fails; it never moves silently.
        "target.object" => target,
        "node.dont-reconnect" => "true",
        // WirePlumber: if the target does not exist, fail instead of linking
        // the stream to the default device.
        "node.dont-fallback" => "true",
        *pw::keys::NODE_LATENCY => format!("{quantum}/{rate}"),
        // Ask the graph to run at the track rate.
        "node.rate" => format!("1/{rate}"),
    };
    if force_rate {
        props.insert("node.force-rate", rate.to_string());
    }
    if exclusive {
        props.insert("node.exclusive", "true");
    }
    let stream = pw::stream::StreamRc::new(core.clone(), &format!("spike-{label}"), props)?;

    let stats = CallbackStats::new();
    let graph_rate = Arc::new(AtomicU32::new(0));
    let delay = Arc::new(AtomicI64::new(0));
    let streaming_at = Rc::new(RefCell::new(None));
    let ctx = StreamCtx {
        label,
        consumer,
        stats: stats.clone(),
        graph_rate: graph_rate.clone(),
        delay: delay.clone(),
    };
    let st_at = streaming_at.clone();
    let listener = stream
        .add_local_listener_with_user_data(ctx)
        .state_changed(move |s, ctx, old, new| {
            println!(
                "{} [{}] state {old:?} -> {new:?} (node id {})",
                ts(t0),
                ctx.label,
                s.node_id()
            );
            if matches!(new, pw::stream::StreamState::Streaming) && st_at.borrow().is_none() {
                *st_at.borrow_mut() = Some(t0.elapsed());
            }
        })
        .param_changed(move |_s, ctx, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let mut info = spa::param::audio::AudioInfoRaw::new();
            if info.parse(param).is_ok() {
                println!(
                    "{} [{}] negotiated format {:?} {} Hz {} ch",
                    ts(t0),
                    ctx.label,
                    info.format(),
                    info.rate(),
                    info.channels()
                );
            }
        })
        .process(|s, ctx| {
            let Some(mut buffer) = s.dequeue_buffer() else {
                return;
            };
            let stride = CHANNELS * size_of::<f32>();
            // Frames the graph wants this cycle (the quantum, after resampling);
            // the mapped buffer itself is much larger.
            let requested = buffer.requested() as usize;
            let datas = buffer.datas_mut();
            let data = &mut datas[0];
            let mut n_frames = 0;
            if let Some(bytes) = data.data() {
                let max_frames = bytes.len() / stride;
                n_frames = if requested > 0 {
                    requested.min(max_frames)
                } else {
                    max_frames
                };
                let start = ctx.stats.begin(n_frames as u32);
                // Interpret the mapped buffer as f32 samples without copying.
                let (pre, samples, _) = {
                    // SAFETY: every bit pattern is a valid f32, so viewing the
                    // initialised bytes as f32 is sound; align_to_mut puts any
                    // misaligned head into `pre`, which is checked below. The
                    // stream negotiated F32LE interleaved, so the values mean
                    // what we write.
                    unsafe { bytes[..n_frames * stride].align_to_mut::<f32>() }
                };
                if pre.is_empty() {
                    let missing = pull(&mut ctx.consumer, samples);
                    ctx.stats.end(
                        start,
                        (samples.len() - missing) as u64 / CHANNELS as u64,
                        missing as u64 / CHANNELS as u64,
                    );
                } else {
                    // Misaligned mapping (not seen in practice): send zero bytes,
                    // which are silence in F32LE too.
                    bytes[..n_frames * stride].fill(0);
                    ctx.stats.end(start, 0, n_frames as u64);
                }
            }
            let chunk = data.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (stride * n_frames) as u32;
            if let Ok(t) = s.time() {
                ctx.graph_rate.store(t.rate().denom, Ordering::Relaxed);
                ctx.delay.store(t.delay(), Ordering::Relaxed);
            }
        })
        .register()?;

    let fmt = format_param(rate)?;
    let mut params = [Pod::from_bytes(&fmt).ok_or_else(|| anyhow!("bad pod"))?];
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    Ok(Opened {
        stream,
        _listener: listener,
        stats,
        graph_rate,
        delay,
        streaming_at,
    })
}

/// The second stream with its render thread and stop flag.
type Second = (Opened, std::thread::JoinHandle<u64>, Arc<AtomicBool>);

fn report_links(graph: &Graph, stream_node: u32, label: &str) {
    let mut found = false;
    for (id, l) in &graph.links {
        if l.out_node == stream_node {
            found = true;
            let peer = graph.nodes.get(&l.in_node);
            println!(
                "[{label}] link {id}: node {stream_node} -> node {} serial={} name={}",
                l.in_node,
                peer.map(|n| n.serial.as_str()).unwrap_or("?"),
                peer.map(|n| n.name.as_str()).unwrap_or("?")
            );
        }
    }
    if !found {
        println!("[{label}] no links from node {stream_node}");
    }
}

/// Extracts `"key": [ ... ]` or `"key": value` from pw-dump output, compacted.
fn json_field(dump: &str, key: &str) -> Vec<String> {
    let compact: String = dump.split_whitespace().collect();
    let needle = format!("\"{key}\":");
    let mut out = Vec::new();
    let mut rest = compact.as_str();
    while let Some(i) = rest.find(&needle) {
        let after = &rest[i + needle.len()..];
        let end = if after.starts_with('[') {
            after.find(']').map(|e| e + 1).unwrap_or(after.len())
        } else {
            after.find([',', '}']).unwrap_or(after.len())
        };
        out.push(after[..end].to_string());
        rest = &after[end..];
    }
    out
}

/// Prints the scheduling policy and real-time priority of every thread of
/// this process (from /proc), to see whether the data thread is real-time.
fn print_thread_policies() {
    let Ok(dir) = std::fs::read_dir("/proc/self/task") else {
        return;
    };
    for e in dir.flatten() {
        let stat = std::fs::read_to_string(e.path().join("stat")).unwrap_or_default();
        // The command name is in parentheses and may contain spaces.
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let comm = stat.find('(').map(|o| &stat[o + 1..close]).unwrap_or("?");
        let fields: Vec<&str> = stat[close + 2..].split(' ').collect();
        // Fields after the name start at field 3; rt_priority is field 40 and
        // policy is field 41 (see proc_pid_stat(5)).
        let rtprio = fields.get(40 - 3).copied().unwrap_or("?");
        let policy = match fields.get(41 - 3).copied() {
            Some("0") => "OTHER",
            Some("1") => "FIFO",
            Some("2") => "RR",
            Some("3") => "BATCH",
            Some("5") => "IDLE",
            _ => "?",
        };
        println!("[threads] {comm}: policy={policy} rtprio={rtprio}");
    }
}

fn play(args: &Args) -> Result<()> {
    let t0 = Instant::now();
    let target = args.target.clone().context("--target is required")?;
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    let graph = Rc::new(RefCell::new(Graph::default()));
    let _rl = watch_registry(&registry, graph.clone(), t0, false);
    let ml_err = mainloop.clone();
    let _cl = core
        .add_listener_local()
        .error(move |id, seq, res, msg| {
            println!("{} core error id={id} seq={seq} res={res}: {msg}", ts(t0));
            if id == pw::core::PW_ID_CORE {
                ml_err.quit();
            }
        })
        .register();

    // Ring buffer sized in milliseconds of audio, filled by the render thread.
    let ring_samples = (args.rate as usize * args.ring_ms as usize / 1000) * CHANNELS;
    let (producer, consumer) = rtrb::RingBuffer::<f32>::new(ring_samples);
    let stop = Arc::new(AtomicBool::new(false));
    let render = spawn_render(
        producer,
        RenderConfig {
            chunk_samples: 512 * CHANNELS,
            stall_after: args.stall_at_ms.map(Duration::from_millis),
            stall_for: Duration::from_millis(args.stall_ms),
        },
        stop.clone(),
    );
    // Let the render thread pre-fill before the stream starts pulling.
    std::thread::sleep(Duration::from_millis(20));

    let a = open_stream(
        &core,
        "A",
        &target,
        args.rate,
        args.force_rate,
        args.exclusive,
        args.quantum,
        consumer,
        t0,
    )?;
    println!(
        "{} [A] connect() returned; target.object={target} rate={} exclusive={} force_rate={}",
        ts(t0),
        args.rate,
        args.exclusive,
        args.force_rate
    );

    // Volume: set the stream's channel volumes after one second.
    let vol_timer = {
        let stream = a.stream.clone();
        let vol = args.volume;
        mainloop.loop_().add_timer(move |_| {
            if let Some(v) = vol {
                let r = stream.set_control(spa::sys::SPA_PROP_channelVolumes, &[v, v]);
                println!("{} [A] set channelVolumes [{v}, {v}] -> {r:?}", ts(t0));
            }
        })
    };
    vol_timer
        .update_timer(Some(Duration::from_millis(1000)), None)
        .into_result()?;

    // Second stream to the same target, to test exclusivity.
    let second: Rc<RefCell<Option<Second>>> = Rc::new(RefCell::new(None));
    let second_timer = {
        let core = core.clone();
        let target = target.clone();
        let second = second.clone();
        let want = args.second;
        let excl = args.second_exclusive;
        let rate = args.rate;
        let quantum = args.quantum;
        mainloop.loop_().add_timer(move |_| {
            if !want {
                return;
            }
            let (p, c) = rtrb::RingBuffer::<f32>::new(rate as usize / 5 * CHANNELS);
            let stop_b = Arc::new(AtomicBool::new(false));
            let r = spawn_render(p, RenderConfig::default(), stop_b.clone());
            match open_stream(&core, "B", &target, rate, false, excl, quantum, c, t0) {
                Ok(o) => {
                    println!("{} [B] connect() returned; exclusive={excl}", ts(t0));
                    *second.borrow_mut() = Some((o, r, stop_b));
                }
                Err(e) => println!("{} [B] open failed: {e:#}", ts(t0)),
            }
        })
    };
    second_timer
        .update_timer(Some(Duration::from_millis(1500)), None)
        .into_result()?;

    // Run a command mid-stream, for example to unload the target sink.
    let exec_timer = {
        let cmd = args.exec.clone();
        mainloop.loop_().add_timer(move |_| {
            if let Some(cmd) = &cmd {
                let mut parts = cmd.split_whitespace();
                let prog = parts.next().unwrap_or("true");
                let rest: Vec<&str> = parts.collect();
                println!("{} exec {cmd}", ts(t0));
                let out = run(prog, &rest);
                println!("{} exec done: {}", ts(t0), out.trim());
            }
        })
    };
    if let Some(ms) = args.exec_at_ms {
        exec_timer
            .update_timer(Some(Duration::from_millis(ms)), None)
            .into_result()?;
    }

    // Inspect what the server shows for our node, shortly before the end.
    let inspect_timer = {
        let stream = a.stream.clone();
        let second = second.clone();
        let graph = graph.clone();
        mainloop.loop_().add_timer(move |_| {
            let id = stream.node_id();
            report_links(&graph.borrow(), id, "A");
            if let Some((b, _, _)) = second.borrow().as_ref() {
                println!("[B] state {:?}", b.stream.state());
                report_links(&graph.borrow(), b.stream.node_id(), "B");
            }
            let dump = run("pw-dump", &[&id.to_string()]);
            println!(
                "[A] pw-dump node {id}: channelVolumes={:?} volume={:?} node.rate={:?} node.exclusive={:?} target.object={:?}",
                json_field(&dump, "channelVolumes").first(),
                json_field(&dump, "volume").first(),
                json_field(&dump, "node.rate").first(),
                json_field(&dump, "node.exclusive").first(),
                json_field(&dump, "target.object").first(),
            );
            print_thread_policies();
            for line in run("pactl", &["list", "sink-inputs"]).lines() {
                let l = line.trim();
                if l.starts_with("Volume:") || l.contains("node.name = \"helikodu-spike") {
                    println!("[A] pactl sink-inputs: {l}");
                }
            }
        })
    };
    let secs = Duration::from_secs_f64(args.secs);
    inspect_timer
        .update_timer(Some(secs.saturating_sub(Duration::from_millis(300))), None)
        .into_result()?;

    let ml = mainloop.clone();
    let quit_timer = mainloop.loop_().add_timer(move |_| ml.quit());
    quit_timer.update_timer(Some(secs), None).into_result()?;

    mainloop.run();

    let _ = a.stream.disconnect();
    stop.store(true, Ordering::Relaxed);
    let rendered = render.join().unwrap_or(0);
    let rate = a.graph_rate.load(Ordering::Relaxed);
    println!(
        "[A] first Streaming at {:?}; graph rate seen in process(): {} Hz; last delay {} ticks; rendered {} samples",
        a.streaming_at.borrow(),
        rate,
        a.delay.load(Ordering::Relaxed),
        rendered
    );
    let first_cb = a.stats.first_ns.load(Ordering::Relaxed);
    if first_cb > 0 {
        println!(
            "[A] first process() callback ~{:.1} ms after stream creation",
            first_cb as f64 / 1e6
        );
    }
    println!("{}", a.stats.report("A", rate));
    if let Some((b, r, s)) = second.borrow_mut().take() {
        let _ = b.stream.disconnect();
        s.store(true, Ordering::Relaxed);
        let _ = r.join();
        println!(
            "[B] first Streaming at {:?}; callbacks={}",
            b.streaming_at.borrow(),
            b.stats.callbacks.load(Ordering::Relaxed)
        );
    }
    Ok(())
}
