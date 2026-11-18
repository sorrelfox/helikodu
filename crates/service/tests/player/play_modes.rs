// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The play modes through the player: stop-after the album and the queue, consume,
//! "Play only", album navigation, "Continue from the selected track", per-track
//! flags, the A-B loop and the sleep timer on a fake clock.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use helikodu_library::{DatabaseConfig, Library};
use helikodu_model::audio::MediaTime;
use helikodu_model::library::TrackRef;
use helikodu_model::player::{
    EntryId, PlayScope, PlaybackState, QueueContext, RepeatMode, ShuffleMode, StopAfter, TrackFlags,
};
use helikodu_service::clock::FakeClock;
use helikodu_service::library::MusicLibrary;
use helikodu_service::player::{
    AbLoop, EndReason, PlayerCommand, PlayerEvent, SinkPacing, SleepWhen,
};

use crate::media::{
    folder, next_path, null_output, player, player_with, until, wav_files, wav_output,
};
use crate::session::with_session;
use crate::stats::add;

fn ended(event: &PlayerEvent) -> bool {
    matches!(event, PlayerEvent::Ended { .. })
}

fn end_reason(event: &PlayerEvent) -> (EndReason, usize) {
    match event {
        PlayerEvent::Ended { reason, played, .. } => (*reason, *played),
        _ => unreachable!(),
    }
}

/// Two albums of loose files: two files in the folder `a`, two in `b`.
fn two_folders(test: &str, millis: usize) -> (PathBuf, Vec<PathBuf>) {
    let dir = folder(test);
    let mut files = Vec::new();
    for name in ["a", "b"] {
        let sub = dir.join(name);
        fs::create_dir_all(&sub).unwrap();
        files.extend(wav_files(&sub, 2, millis));
    }
    (dir, files)
}

#[test]
fn stop_after_the_album_stops_at_the_next_folder() {
    let (dir, files) = two_folders("stop_after_album", 200);
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::SetStopAfter(StopAfter::Album))
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    let (end, seen) = until(&events, ended);
    assert_eq!(end_reason(&end), (EndReason::StopAfter, 2));
    assert!(seen.contains(&PlayerEvent::StopAfterChanged(StopAfter::Off)));
    player.sync().unwrap();
    assert_eq!(player.queue().current, Some(2), "the next album is current");
    assert_eq!(player.status().stop_after, StopAfter::Off);
}

#[test]
fn stop_after_the_queue_overrides_repeat_all() {
    let dir = folder("stop_after_queue");
    let files = wav_files(&dir, 3, 150);
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::SetRepeat(RepeatMode::All))
        .unwrap();
    player
        .send(PlayerCommand::SetStopAfter(StopAfter::Queue))
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    let (end, _) = until(&events, ended);
    assert_eq!(end_reason(&end), (EndReason::StopAfter, 3));
}

#[test]
fn consume_removes_what_played() {
    let dir = folder("consume");
    let files = wav_files(&dir, 3, 150);
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player.send(PlayerCommand::SetConsume(true)).unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    assert_eq!(next_path(&events), files[1]);
    let (end, _) = until(&events, ended);
    assert_eq!(end_reason(&end), (EndReason::QueueFinished, 3));
    player.sync().unwrap();
    assert!(player.queue().entries.is_empty(), "{:?}", player.queue());
    assert!(player.queue().modes.consume);
}

#[test]
fn play_only_the_album_passes_over_the_rest() {
    let (dir, files) = two_folders("play_only_album", 150);
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player
        .send(PlayerCommand::SetPlayScope(PlayScope::Album))
        .unwrap();
    player
        .send(PlayerCommand::SetRepeat(RepeatMode::All))
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 2,
        })
        .unwrap();
    let played: Vec<PathBuf> = (0..5).map(|_| next_path(&events)).collect();
    assert_eq!(
        played,
        vec![
            files[2].clone(),
            files[3].clone(),
            files[2].clone(),
            files[3].clone(),
            files[2].clone()
        ]
    );
    player.send(PlayerCommand::Stop).unwrap();
    until(&events, ended);
}

#[test]
fn album_commands_move_between_folders() {
    let (_, files) = two_folders("album_commands", 20_000);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 1,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[1]);
    player.send(PlayerCommand::NextAlbum).unwrap();
    assert_eq!(next_path(&events), files[2]);
    player.send(PlayerCommand::PreviousAlbum).unwrap();
    assert_eq!(next_path(&events), files[0]);
    player.send(PlayerCommand::RandomAlbum).unwrap();
    assert_eq!(next_path(&events), files[2], "the only other album");
    player.send(PlayerCommand::Stop).unwrap();
    until(&events, ended);
}

#[test]
fn playback_continues_from_the_selected_track() {
    let dir = folder("follow_cursor");
    let files = wav_files(&dir, 4, 200);
    let (player, events) = player(wav_output(&dir.join("out.wav")), SinkPacing::Freewheel);
    player.send(PlayerCommand::SetFollowCursor(true)).unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    player
        .send(PlayerCommand::SetCursor(Some(EntryId::new(4))))
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    let (next, seen) = until(&events, |e| matches!(e, PlayerEvent::TrackChanged { .. }));
    assert!(matches!(next, PlayerEvent::TrackChanged { path, .. } if path == files[3]));
    assert!(
        seen.contains(&PlayerEvent::CursorChanged(None)),
        "the selection is used up"
    );
    let (end, _) = until(&events, ended);
    assert_eq!(end_reason(&end), (EndReason::QueueFinished, 2));
}

#[test]
fn track_flags_leave_tracks_out() {
    let dir = folder("track_flags");
    let files = wav_files(&dir, 3, 200);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    let tracks: Vec<_> = files
        .iter()
        .map(|file| add(&library, &dir, file, 200))
        .collect();
    let music = MusicLibrary::from_library(Arc::clone(&library), ":memory:");
    let (player, events) = player_with(
        wav_output(&dir.join("out.wav")),
        SinkPacing::Freewheel,
        |config| config.library = Some(music.clone()),
    );
    let flags = TrackFlags {
        manual_only: true,
        ..TrackFlags::default()
    }
    .with_volume_db(-3.0);
    player
        .send(PlayerCommand::SetTrackFlags {
            track: tracks[1],
            flags,
        })
        .unwrap();
    until(&events, |e| {
        *e == PlayerEvent::TrackFlagsChanged {
            track: tracks[1],
            flags,
        }
    });
    assert_eq!(music.track_flags(tracks[1]).unwrap(), flags);
    assert_eq!(music.track_flags(tracks[0]).unwrap(), TrackFlags::default());
    player
        .send(PlayerCommand::PlayTracks {
            context: QueueContext::files(),
            tracks: tracks.iter().map(|t| TrackRef::Library(*t)).collect(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    assert_eq!(
        next_path(&events),
        files[2],
        "the manual-only track is passed over"
    );
    until(&events, ended);
    // Started directly, it plays.
    player
        .send(PlayerCommand::PlayEntry(EntryId::new(2)))
        .unwrap();
    assert_eq!(next_path(&events), files[1]);
    until(&events, ended);
}

#[test]
fn the_ab_loop_keeps_playback_inside_it() {
    let dir = folder("ab_loop");
    let files = wav_files(&dir, 2, 20_000);
    let (player, events) = player(null_output(), SinkPacing::RealTime);
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    let at_least = |millis| {
        move |e: &PlayerEvent| {
            matches!(e, PlayerEvent::Position { position, .. }
                if *position >= MediaTime::from_millis(millis))
        }
    };
    until(&events, at_least(300));
    player.send(PlayerCommand::LoopStartHere).unwrap();
    let start = match until(&events, |e| matches!(e, PlayerEvent::LoopChanged(_))).0 {
        PlayerEvent::LoopChanged(Some(AbLoop {
            start: Some(start),
            end: None,
        })) => start,
        other => panic!("{other:?}"),
    };
    until(&events, at_least(900));
    player.send(PlayerCommand::LoopEndHere).unwrap();
    let end = match until(&events, |e| matches!(e, PlayerEvent::LoopChanged(_))).0 {
        PlayerEvent::LoopChanged(Some(ab)) => ab.range().unwrap().end,
        other => panic!("{other:?}"),
    };
    assert!(start < end);
    // Once the jump back is heard, positions stay inside the loop for a while.
    let slack = MediaTime::from_millis(150);
    let mut wrapped = false;
    let deadline = std::time::Instant::now() + Duration::from_millis(2_500);
    while std::time::Instant::now() < deadline {
        if let PlayerEvent::Position { position, .. } =
            until(&events, |e| matches!(e, PlayerEvent::Position { .. })).0
        {
            if wrapped {
                assert!(
                    position >= start
                        && position.as_duration() <= (end.as_duration() + slack.as_duration()),
                    "{position} outside {start} – {end}"
                );
            } else if position < end {
                wrapped = position.as_duration() + Duration::from_millis(50) < end.as_duration()
                    && position >= start
                    && player.status().ab_loop.is_some();
            }
        }
    }
    assert!(wrapped);
    assert!(player.status().ab_loop.is_some());
    // Another track ends the loop.
    player.send(PlayerCommand::Next).unwrap();
    until(&events, |e| *e == PlayerEvent::LoopChanged(None));
    player.send(PlayerCommand::Stop).unwrap();
    until(&events, ended);
}

fn fake_clock() -> FakeClock {
    FakeClock::new(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000))
}

#[test]
fn the_sleep_timer_fades_and_pauses_on_the_fake_clock() {
    let dir = folder("sleep_at_once");
    let files = wav_files(&dir, 1, 30_000);
    let clock = fake_clock();
    let (player, events) = player_with(null_output(), SinkPacing::RealTime, |config| {
        config.clock = clock.shared();
    });
    player
        .send(PlayerCommand::PlayFiles { files, start: 0 })
        .unwrap();
    until(&events, |e| {
        matches!(
            e,
            PlayerEvent::State {
                playback: PlaybackState::Playing,
                ..
            }
        )
    });
    player
        .send(PlayerCommand::StartSleepTimer {
            length: Duration::from_secs(15 * 60),
            when: SleepWhen::AtOnce,
        })
        .unwrap();
    player.sync().unwrap();
    let status = player.status().sleep_timer.unwrap();
    assert_eq!(status.remaining, Duration::from_secs(15 * 60));
    // "+10 min" adds to it.
    player
        .send(PlayerCommand::ExtendSleepTimer(Duration::from_secs(600)))
        .unwrap();
    player.sync().unwrap();
    assert_eq!(
        player.status().sleep_timer.unwrap().remaining,
        Duration::from_secs(25 * 60)
    );
    clock.advance(Duration::from_secs(25 * 60 - 20));
    player.sync().unwrap();
    assert_eq!(player.status().playback, PlaybackState::Playing, "fading");
    let volume = player.status().volume;
    clock.advance(Duration::from_secs(20));
    player.sync().unwrap();
    until(&events, |e| *e == PlayerEvent::SleepTimerChanged(None));
    until(&events, |e| {
        matches!(
            e,
            PlayerEvent::State {
                playback: PlaybackState::Paused,
                ..
            }
        )
    });
    assert_eq!(player.status().volume, volume, "the volume setting is kept");
    assert_eq!(player.status().sleep_timer, None);
}

#[test]
fn the_sleep_timer_can_wait_for_the_track_and_be_turned_off() {
    let dir = folder("sleep_after_track");
    let files = wav_files(&dir, 3, 2_000);
    let clock = fake_clock();
    let (player, events) = player_with(null_output(), SinkPacing::RealTime, |config| {
        config.clock = clock.shared();
    });
    player
        .send(PlayerCommand::StartSleepTimer {
            length: Duration::from_secs(60),
            when: SleepWhen::AfterTrack,
        })
        .unwrap();
    player.send(PlayerCommand::CancelSleepTimer).unwrap();
    player.sync().unwrap();
    assert_eq!(player.status().sleep_timer, None);
    player
        .send(PlayerCommand::StartSleepTimer {
            length: Duration::from_secs(60),
            when: SleepWhen::AfterTrack,
        })
        .unwrap();
    player
        .send(PlayerCommand::PlayFiles {
            files: files.clone(),
            start: 0,
        })
        .unwrap();
    assert_eq!(next_path(&events), files[0]);
    clock.advance(Duration::from_secs(61));
    player.sync().unwrap();
    let (end, _) = until(&events, ended);
    assert_eq!(end_reason(&end), (EndReason::SleepTimer, 1));
    player.sync().unwrap();
    assert_eq!(player.queue().current, Some(1));
    assert_eq!(player.status().sleep_timer, None);
}

#[test]
fn the_play_modes_survive_a_restart() {
    let dir = folder("modes_session");
    let state = dir.join("state");
    let files = wav_files(&dir, 3, 20_000);
    let commands = [
        PlayerCommand::SetShuffle(ShuffleMode::Albums),
        PlayerCommand::SetRepeat(RepeatMode::Album),
        PlayerCommand::SetConsume(true),
        PlayerCommand::SetPlayScope(PlayScope::Artist),
        PlayerCommand::SetFollowCursor(true),
        PlayerCommand::SetStopAfter(StopAfter::Queue),
    ];
    let modes = {
        let (player, events) = with_session(&state, true);
        for command in commands {
            player.send(command).unwrap();
        }
        player
            .send(PlayerCommand::PlayFiles { files, start: 1 })
            .unwrap();
        next_path(&events);
        player.sync().unwrap();
        player.queue()
    };
    let (player, _events) = with_session(&state, true);
    player.sync().unwrap();
    let restored = player.queue();
    assert_eq!(restored.modes, modes.modes);
    assert_eq!(
        restored.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
        modes.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
        "the stored shuffle order"
    );
    assert_eq!(player.status().stop_after, StopAfter::Queue);
    assert!(restored.modes.consume && restored.modes.follow_cursor);
}

#[test]
fn a_long_track_resumes_where_it_stopped() {
    let dir = folder("auto_resume");
    let files = wav_files(&dir, 1, 20_000);
    let library = Arc::new(Library::open(&DatabaseConfig::in_memory()).unwrap());
    // The library takes it for a long track (an audiobook chapter, say).
    let track = add(&library, &dir, &files[0], 25 * 60 * 1000);
    let music = MusicLibrary::from_library(Arc::clone(&library), ":memory:");
    let (player, events) = player_with(null_output(), SinkPacing::RealTime, |config| {
        config.library = Some(music.clone());
        config.prefs.resume_long_tracks = true;
    });
    let play = || {
        player
            .send(PlayerCommand::PlayTracks {
                context: QueueContext::files(),
                tracks: vec![TrackRef::Library(track)],
                start: 0,
            })
            .unwrap();
    };
    play();
    next_path(&events);
    player
        .send(PlayerCommand::Seek(MediaTime::from_millis(6_000)))
        .unwrap();
    until(&events, |e| {
        matches!(e, PlayerEvent::Position { position, .. }
            if *position >= MediaTime::from_millis(6_200))
    });
    player.send(PlayerCommand::Stop).unwrap();
    until(&events, |e| matches!(e, PlayerEvent::ListenRecorded { .. }));
    until(&events, ended);
    let resume = music.resume_position(track).unwrap().unwrap();
    assert!(resume >= Duration::from_millis(6_000), "{resume:?}");
    play();
    next_path(&events);
    let first = match until(&events, |e| matches!(e, PlayerEvent::Position { .. })).0 {
        PlayerEvent::Position { position, .. } => position,
        _ => unreachable!(),
    };
    assert!(first.as_duration() >= resume, "starts at {first}");
    player.send(PlayerCommand::Stop).unwrap();
    until(&events, ended);
}
