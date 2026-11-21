// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! `helikodu-cli dsp`: the order of the DSP chain, saved chains, the compressor,
//! convolution, pitch shift, the playback speed and the equalizers of single
//! tracks. Changes are
//! written to the settings file, which a running application follows.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand, ValueEnum};
use helikodu_model::library::TrackId;
use helikodu_model::settings::{
    BuiltInPreset, ChainStage, CompressorPreset, DspSettings, EqMode, EqualizerSettings,
    ImpulseFile, PitchSettings, SpeedSettings,
};
use helikodu_service::autoeq;
use helikodu_service::bus::EventBus;
use helikodu_service::dsp::{impulse, partition_for};
use helikodu_service::files::AppDirs;
use helikodu_service::settings::{SETTINGS_FILE, SettingsStore};
use serde_json::{Value, json};

use super::database::DatabaseArgs;
use crate::Outcome;

#[derive(Debug, Args)]
pub(crate) struct DspArgs {
    #[command(subcommand)]
    command: DspCommand,
    /// The settings file [default: $XDG_CONFIG_HOME/helikodu/settings.toml]
    #[arg(long, value_name = "PATH", global = true)]
    settings: Option<PathBuf>,
}

/// A stage of the chain, as `settings.toml` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum StageArg {
    Pitch,
    Equalizer,
    Convolution,
    Crossfeed,
    ChannelMix,
    Compressor,
    Limiter,
}

impl From<StageArg> for ChainStage {
    fn from(arg: StageArg) -> Self {
        match arg {
            StageArg::Pitch => Self::Pitch,
            StageArg::Equalizer => Self::Equalizer,
            StageArg::Convolution => Self::Convolution,
            StageArg::Crossfeed => Self::Crossfeed,
            StageArg::ChannelMix => Self::ChannelMix,
            StageArg::Compressor => Self::Compressor,
            StageArg::Limiter => Self::Limiter,
        }
    }
}

/// The compressor presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PresetArg {
    /// Strong compression with makeup gain, for listening at a low volume.
    Night,
    /// Light compression.
    Gentle,
    /// The values of `[dsp.compressor]` in the settings file.
    Custom,
}

#[derive(Debug, Subcommand)]
enum DspCommand {
    /// Show the chain: the stages in order, which are on, and the saved chains.
    Show {
        /// Print it as JSON, for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Put the stages in this order; stages left out follow in the default order.
    Order {
        /// The stages, first to last.
        #[arg(required = true, value_enum)]
        stages: Vec<StageArg>,
    },
    /// Put the stages back in the default order.
    DefaultOrder,
    /// Switch stages on.
    On {
        /// The stages.
        #[arg(required = true, value_enum)]
        stages: Vec<StageArg>,
    },
    /// Switch stages off.
    Off {
        /// The stages.
        #[arg(required = true, value_enum)]
        stages: Vec<StageArg>,
    },
    /// List, save, use and delete saved chains.
    #[command(subcommand)]
    Chain(ChainCommand),
    /// Switch the compressor on with a preset (night mode by default).
    Compressor {
        /// The preset.
        #[arg(value_enum, default_value_t = PresetArg::Night)]
        preset: PresetArg,
    },
    /// Set up convolution and check impulse responses.
    #[command(subcommand)]
    Convolution(ConvolutionCommand),
    /// Shift the pitch by this many semitones (-12 to 12), keeping the tempo; 0
    /// switches pitch shift off.
    Pitch {
        /// The shift in semitones; fractions are cents.
        #[arg(allow_negative_numbers = true)]
        semitones: f64,
    },
    /// Play at this speed (0.5 to 2); 1 switches the speed off. Without
    /// --keep-pitch or --tape the pitch does as set before (kept by default).
    Speed {
        /// The speed, as a factor of the normal one.
        rate: f64,
        /// Keep the pitch: the tempo changes.
        #[arg(long, conflicts_with = "tape")]
        keep_pitch: bool,
        /// Let the pitch follow the speed, as on a tape.
        #[arg(long)]
        tape: bool,
    },
    /// Show, set and remove the equalizers of single tracks, stored in the library.
    #[command(subcommand)]
    TrackEq(TrackEqCommand),
}

#[derive(Debug, Subcommand)]
enum ChainCommand {
    /// List the saved chains.
    List,
    /// Save the current order and the stages that are on as a chain.
    Save {
        /// The chain's name; a chain of that name is replaced.
        name: String,
    },
    /// Use a saved chain: its stages in its order, the others off.
    Apply {
        /// The chain's name.
        name: String,
    },
    /// Delete a saved chain.
    Delete {
        /// The chain's name.
        name: String,
    },
}

#[derive(Debug, Subcommand)]
enum ConvolutionCommand {
    /// Use the WAV files in a folder, one per sample rate, and switch convolution on.
    Folder {
        /// The folder.
        folder: PathBuf,
    },
    /// Use a WAV file for one sample rate, and switch convolution on.
    File {
        /// The sample rate in Hz the file is for.
        rate: u32,
        /// The WAV file.
        path: PathBuf,
    },
    /// Forget the folder and the files, and switch convolution off.
    Clear,
    /// Set the share of the processed signal (0 to 100 %) and its gain in dB.
    Mix {
        /// The share of the processed signal, in percent.
        #[arg(long, value_name = "PERCENT")]
        wet: Option<f64>,
        /// The gain of the processed signal, in dB (-24 to 12).
        #[arg(long, value_name = "DB", allow_negative_numbers = true)]
        gain: Option<f64>,
        /// Whether the impulse response is scaled to boost no frequency.
        #[arg(long, value_name = "BOOL")]
        normalize: Option<bool>,
    },
    /// Show the impulse response the settings give for sample rates, or describe
    /// WAV files: their rate, channels, length and the latency they add.
    Check {
        /// WAV files to describe [default: what the settings give at the common
        /// rates]
        files: Vec<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
enum TrackEqCommand {
    /// List the tracks that have an equalizer of their own.
    List {
        #[command(flatten)]
        database: DatabaseArgs,
    },
    /// Show the equalizer of a track as JSON.
    Show {
        /// The track's id.
        track: u64,
        #[command(flatten)]
        database: DatabaseArgs,
    },
    /// Give a track an equalizer of its own, which replaces the equalizer of the
    /// settings while it plays.
    Set {
        /// The track's id.
        track: u64,
        /// Take a copy of the equalizer of the settings.
        #[arg(long, group = "source")]
        from_settings: bool,
        /// Read an AutoEQ or EqualizerAPO ParametricEQ.txt file.
        #[arg(long, value_name = "FILE", group = "source")]
        autoeq: Option<PathBuf>,
        /// A built-in graphic preset (flat, bass-boost, vocal, …).
        #[arg(long, value_name = "ID", group = "source")]
        preset: Option<String>,
        /// No equalizer for this track, whatever the settings say.
        #[arg(long, group = "source")]
        none: bool,
        #[command(flatten)]
        database: DatabaseArgs,
    },
    /// Remove a track's own equalizer; it plays with the settings' again.
    Clear {
        /// The track's id.
        track: u64,
        #[command(flatten)]
        database: DatabaseArgs,
    },
}

pub(crate) fn run(args: &DspArgs) -> Result<Outcome> {
    let path = match &args.settings {
        Some(path) => path.clone(),
        None => AppDirs::from_env()?.config.join(SETTINGS_FILE),
    };
    let store = SettingsStore::open(path, EventBus::new());
    let change = |f: &mut dyn FnMut(&mut DspSettings) -> Result<()>| -> Result<()> {
        let mut failure = None;
        store
            .update(|settings| {
                if let Err(error) = f(&mut settings.dsp) {
                    failure = Some(error);
                }
            })
            .context("cannot save the settings")?;
        failure.map_or(Ok(()), Err)
    };
    let mut out = io::stdout().lock();
    match &args.command {
        DspCommand::Show { json } => {
            let dsp = store.get().dsp.clone();
            if *json {
                writeln!(out, "{}", serde_json::to_string_pretty(&show_json(&dsp))?)?;
            } else {
                print_chain(&mut out, &dsp)?;
            }
        }
        DspCommand::Order { stages } => {
            let order: Vec<ChainStage> = stages.iter().map(|s| (*s).into()).collect();
            change(&mut |dsp| {
                dsp.order = order.clone();
                Ok(())
            })?;
            print_chain(&mut out, &store.get().dsp)?;
        }
        DspCommand::DefaultOrder => {
            change(&mut |dsp| {
                dsp.order = ChainStage::DEFAULT_ORDER.to_vec();
                Ok(())
            })?;
            print_chain(&mut out, &store.get().dsp)?;
        }
        DspCommand::On { stages } | DspCommand::Off { stages } => {
            let on = matches!(args.command, DspCommand::On { .. });
            change(&mut |dsp| {
                for stage in stages {
                    dsp.set_enabled((*stage).into(), on);
                }
                Ok(())
            })?;
            print_chain(&mut out, &store.get().dsp)?;
        }
        DspCommand::Chain(command) => chain(&mut out, &store, &change, command)?,
        DspCommand::Compressor { preset } => {
            let preset = match preset {
                PresetArg::Night => CompressorPreset::Night,
                PresetArg::Gentle => CompressorPreset::Gentle,
                PresetArg::Custom => CompressorPreset::Custom,
            };
            change(&mut |dsp| {
                dsp.compressor.enabled = true;
                dsp.compressor.preset = preset;
                Ok(())
            })?;
            let values = store.get().dsp.compressor.effective();
            writeln!(
                out,
                "Compressor on: threshold {} dB, ratio {}:1, attack {} ms, release {} ms, \
                 knee {} dB, makeup {} dB.",
                values.threshold_db,
                values.ratio,
                values.attack_ms,
                values.release_ms,
                values.knee_db,
                values.makeup_db
            )?;
        }
        DspCommand::Convolution(command) => convolution(&mut out, &store, &change, command)?,
        DspCommand::Pitch { semitones } => {
            let semitones = *semitones;
            if !PitchSettings::SEMITONE_RANGE.contains(&semitones) {
                bail!("the shift must be from -12 to 12 semitones");
            }
            change(&mut |dsp| {
                dsp.pitch.semitones = semitones;
                dsp.pitch.enabled = semitones != 0.0;
                Ok(())
            })?;
            if semitones == 0.0 {
                writeln!(out, "Pitch shift is off.")?;
            } else {
                writeln!(out, "Pitch shift: {semitones:+} semitones.")?;
            }
        }
        DspCommand::Speed {
            rate,
            keep_pitch,
            tape,
        } => {
            let rate = *rate;
            if !SpeedSettings::RATE_RANGE.contains(&rate) {
                bail!("the speed must be from 0.5 to 2");
            }
            let mut kept = true;
            change(&mut |dsp| {
                if *keep_pitch || *tape {
                    dsp.speed.keep_pitch = *keep_pitch;
                }
                dsp.speed.rate = rate;
                dsp.speed.enabled = rate != 1.0;
                kept = dsp.speed.keep_pitch;
                Ok(())
            })?;
            if rate == 1.0 {
                writeln!(out, "The speed is normal.")?;
            } else if kept {
                writeln!(out, "Speed: {rate}×, keeping the pitch.")?;
            } else {
                writeln!(out, "Speed: {rate}×, the pitch following it.")?;
            }
        }
        DspCommand::TrackEq(command) => track_eq(&mut out, &store, command)?,
    }
    Ok(Outcome::Success)
}

type Change<'a> = dyn Fn(&mut dyn FnMut(&mut DspSettings) -> Result<()>) -> Result<()> + 'a;

fn chain(
    out: &mut impl Write,
    store: &SettingsStore,
    change: &Change<'_>,
    command: &ChainCommand,
) -> Result<()> {
    match command {
        ChainCommand::List => {
            let dsp = store.get().dsp.clone();
            if dsp.chains.is_empty() {
                writeln!(out, "No saved chains.")?;
            }
            for saved in &dsp.chains {
                writeln!(out, "{}: {}", saved.name, stage_list(&saved.stages))?;
            }
        }
        ChainCommand::Save { name } => {
            if name.trim().is_empty() {
                bail!("name the chain");
            }
            change(&mut |dsp| {
                dsp.save_chain(name);
                Ok(())
            })?;
            writeln!(out, "Saved the chain {name:?}.")?;
        }
        ChainCommand::Apply { name } => {
            change(&mut |dsp| {
                if dsp.apply_chain(name) {
                    Ok(())
                } else {
                    bail!("there is no saved chain named {name:?}")
                }
            })?;
            print_chain(out, &store.get().dsp)?;
        }
        ChainCommand::Delete { name } => {
            change(&mut |dsp| {
                let before = dsp.chains.len();
                dsp.chains.retain(|c| c.name != *name);
                if dsp.chains.len() == before {
                    bail!("there is no saved chain named {name:?}");
                }
                Ok(())
            })?;
            writeln!(out, "Deleted the chain {name:?}.")?;
        }
    }
    Ok(())
}

fn convolution(
    out: &mut impl Write,
    store: &SettingsStore,
    change: &Change<'_>,
    command: &ConvolutionCommand,
) -> Result<()> {
    match command {
        ConvolutionCommand::Folder { folder } => {
            let folder = absolute(folder)?;
            if !folder.is_dir() {
                bail!("{} is not a folder", folder.display());
            }
            change(&mut |dsp| {
                dsp.convolution.folder = Some(folder.clone());
                dsp.convolution.enabled = true;
                Ok(())
            })?;
            writeln!(
                out,
                "Convolution uses the WAV files in {}.",
                folder.display()
            )?;
        }
        ConvolutionCommand::File { rate, path } => {
            let path = absolute(path)?;
            let info = impulse::read_info(&path)?;
            change(&mut |dsp| {
                dsp.convolution.files.retain(|f| f.rate_hz != *rate);
                dsp.convolution.files.push(ImpulseFile {
                    rate_hz: *rate,
                    path: path.clone(),
                });
                dsp.convolution.files.sort_by_key(|f| f.rate_hz);
                dsp.convolution.enabled = true;
                Ok(())
            })?;
            writeln!(out, "{rate} Hz: {} ({})", path.display(), describe(&info))?;
            if info.rate != *rate {
                writeln!(
                    out,
                    "The file is at {} Hz; it is resampled to {rate} Hz.",
                    info.rate
                )?;
            }
        }
        ConvolutionCommand::Clear => {
            change(&mut |dsp| {
                dsp.convolution.folder = None;
                dsp.convolution.files.clear();
                dsp.convolution.enabled = false;
                Ok(())
            })?;
            writeln!(out, "Convolution is off.")?;
        }
        ConvolutionCommand::Mix {
            wet,
            gain,
            normalize,
        } => {
            change(&mut |dsp| {
                if let Some(wet) = wet {
                    dsp.convolution.wet_percent = *wet;
                }
                if let Some(gain) = gain {
                    dsp.convolution.gain_db = *gain;
                }
                if let Some(normalize) = normalize {
                    dsp.convolution.normalize = *normalize;
                }
                Ok(())
            })?;
            let c = &store.get().dsp.convolution;
            writeln!(
                out,
                "Wet {} %, gain {} dB, normalized: {}.",
                c.wet_percent,
                c.gain_db,
                if c.normalize { "yes" } else { "no" }
            )?;
        }
        ConvolutionCommand::Check { files } => {
            if files.is_empty() {
                let settings = store.get().dsp.convolution.clone();
                if !settings.has_source() {
                    bail!("no impulse response is set");
                }
                for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
                    match impulse::find(&settings, rate) {
                        Ok(found) => {
                            let from = if found.resampled() {
                                format!(", resampled from {} Hz", found.file_rate)
                            } else {
                                String::new()
                            };
                            writeln!(
                                out,
                                "{rate} Hz: {} ({} ch, {} taps{from}); latency {}",
                                found.path.display(),
                                found.channels.len(),
                                found.taps(),
                                latency(rate)
                            )?;
                        }
                        Err(error) => writeln!(out, "{rate} Hz: {error}")?,
                    }
                }
            }
            for file in files {
                let info = impulse::read_info(file)?;
                writeln!(out, "{}: {}", file.display(), describe(&info))?;
            }
        }
    }
    Ok(())
}

fn track_eq(out: &mut impl Write, store: &SettingsStore, command: &TrackEqCommand) -> Result<()> {
    match command {
        TrackEqCommand::List { database } => {
            let library = database.open_existing()?;
            for track in library.tracks_with_equalizer()? {
                writeln!(out, "{}", track.get())?;
            }
        }
        TrackEqCommand::Show { track, database } => {
            let library = database.open_existing()?;
            match library.track_equalizer(TrackId::new(*track))? {
                Some(eq) => writeln!(out, "{}", serde_json::to_string_pretty(&eq)?)?,
                None => writeln!(out, "Track {track} has no equalizer of its own.")?,
            }
        }
        TrackEqCommand::Set {
            track,
            from_settings,
            autoeq: file,
            preset,
            none,
            database,
        } => {
            let eq = if *from_settings {
                let mut eq = store.get().dsp.equalizer.clone();
                eq.enabled = true;
                eq.presets.clear();
                eq
            } else if let Some(file) = file {
                let bytes = std::fs::read(file)
                    .with_context(|| format!("cannot read {}", file.display()))?;
                let imported = autoeq::parse_bytes(&bytes)?;
                let mut eq = EqualizerSettings {
                    enabled: true,
                    mode: EqMode::Parametric,
                    ..EqualizerSettings::default()
                };
                eq.parametric = imported.filters;
                eq.parametric.truncate(EqualizerSettings::MAX_FILTERS);
                if let Some(db) = imported.preamp_db {
                    eq.auto_preamp = false;
                    eq.preamp_db = db.clamp(
                        *EqualizerSettings::PREAMP_RANGE.start(),
                        *EqualizerSettings::PREAMP_RANGE.end(),
                    );
                }
                eq
            } else if let Some(id) = preset {
                let Some(found) = BuiltInPreset::find(id) else {
                    let ids: Vec<&str> = BuiltInPreset::ALL.iter().map(|p| p.id).collect();
                    bail!("no preset {id:?}; the presets are {}", ids.join(", "));
                };
                EqualizerSettings {
                    enabled: true,
                    mode: EqMode::Graphic10,
                    graphic_10: found.octaves,
                    graphic_31: found.third_octaves(),
                    ..EqualizerSettings::default()
                }
            } else if *none {
                EqualizerSettings::default()
            } else {
                bail!(
                    "say where the equalizer comes from: --from-settings, --autoeq, --preset or --none"
                );
            };
            eq.validate()?;
            let library = database.open_existing()?;
            library.set_track_equalizer(TrackId::new(*track), Some(&eq))?;
            writeln!(
                out,
                "Track {track} has its own equalizer now; it applies from the next time it starts."
            )?;
        }
        TrackEqCommand::Clear { track, database } => {
            let library = database.open_existing()?;
            library.set_track_equalizer(TrackId::new(*track), None)?;
            writeln!(
                out,
                "Track {track} plays with the equalizer of the settings again."
            )?;
        }
    }
    Ok(())
}

fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

fn describe(info: &impulse::WavInfo) -> String {
    format!(
        "{} Hz, {} ch, {} taps ({:.1} ms); latency {}",
        info.rate,
        info.channels,
        info.frames,
        info.frames as f64 * 1000.0 / f64::from(info.rate),
        latency(info.rate)
    )
}

fn latency(rate: u32) -> String {
    let frames = partition_for(rate);
    format!(
        "{frames} frames ({:.1} ms)",
        frames as f64 * 1000.0 / f64::from(rate)
    )
}

fn stage_list(stages: &[ChainStage]) -> String {
    if stages.is_empty() {
        return "(no stages)".into();
    }
    stages
        .iter()
        .map(|s| s.key())
        .collect::<Vec<_>>()
        .join(" → ")
}

fn print_chain(out: &mut impl Write, dsp: &DspSettings) -> Result<()> {
    for (i, stage) in dsp.effective_order().into_iter().enumerate() {
        let state = if dsp.is_enabled(stage) { "on" } else { "off" };
        writeln!(out, "{}. {:<12} {state}", i + 1, stage.key())?;
    }
    if !dsp.track_equalizer {
        writeln!(out, "Track equalizers are not used.")?;
    }
    if dsp.speed.is_active() {
        let pitch = if dsp.speed.keep_pitch {
            "keeping the pitch"
        } else {
            "the pitch following it"
        };
        writeln!(out, "Speed: {}×, {pitch}.", dsp.speed.effective_rate())?;
    }
    Ok(())
}

fn show_json(dsp: &DspSettings) -> Value {
    let stages: Vec<Value> = dsp
        .effective_order()
        .into_iter()
        .map(|s| json!({ "stage": s.key(), "on": dsp.is_enabled(s) }))
        .collect();
    let chains: Vec<Value> = dsp
        .chains
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "stages": c.stages.iter().map(|s| s.key()).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "stages": stages,
        "chains": chains,
        "track_equalizer": dsp.track_equalizer,
        "speed": {
            "rate": dsp.speed.effective_rate(),
            "keep_pitch": dsp.speed.keep_pitch,
        },
    })
}
