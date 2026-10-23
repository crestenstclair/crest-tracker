use super::*;
use serde_json::json;

pub(super) fn queue(runtime: &Runtime, request: Request) {
    runtime.app.borrow_mut().requests.push(request);
}
pub(super) fn choose(runtime: &Runtime, path: &std::path::Path) -> bool {
    let app = runtime.app.borrow();
    let dialog = &app.file_dialog;
    assert!(
        dialog["error"].is_null(),
        "file browser: {}",
        dialog["error"]
    );
    if dialog["loading"] != false {
        return false;
    }
    let token = dialog["token"].as_u64();
    drop(app);
    if let Some(token) = token {
        queue(
            runtime,
            Request::FileChoose {
                token,
                path: path.to_str().unwrap().into(),
                overwrite: false,
            },
        );
        true
    } else {
        false
    }
}
pub(super) fn temp_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("crest-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    root.canonicalize().unwrap()
}

#[test]
#[ignore = "integrated production workers and sample DSP; run explicitly with --test-threads=1"]
fn empty_song_sample_notes_render_through_the_production_host() {
    let root = temp_root("empty-sample-song");
    let mut stage = 0;
    let mut started = 0;
    let evidence = run_with_instrument(
        &root,
        Box::new(move |runtime, audio, evidence| {
            match stage {
                0 if runtime.initial_sound.is_some() && runtime.sync.ready() => {
                    queue(
                        runtime,
                        Request::File {
                            action: FileAction::New,
                        },
                    );
                    stage = 1;
                }
                1 if runtime.pending_open.is_none()
                    && !runtime.app.borrow().mappings.is_empty() =>
                {
                    assert!(runtime.app.borrow().editor.song().phrases.is_empty());
                    // The same Confirm path as native empty Song, Chain and note entry.
                    queue(runtime, Request::Confirm);
                    queue(runtime, Request::Confirm);
                    for row in [0, 4, 8, 12] {
                        queue(runtime, Request::Focus { row, column: 0 });
                        queue(runtime, Request::Confirm);
                    }
                    stage = 2;
                }
                2 => {
                    let app = runtime.app.borrow();
                    let destinations = app.capture["destinations"].as_array().unwrap();
                    assert!(!destinations.is_empty());
                    assert!(destinations
                        .iter()
                        .all(|item| item["mpe_error"].is_string()));
                    let (_, phrase) = app.selected_phrase().unwrap();
                    assert_eq!(phrase.events.len(), 4);
                    assert!(phrase.events.iter().all(|event| {
                        event.note.as_ref().unwrap().duration == Beat::new(9, 40).unwrap()
                    }));
                    drop(app);
                    evidence.measured = true;
                    started = audio.rendered_frames();
                    queue(runtime, Request::Play);
                    stage = 3;
                }
                3 if audio.rendered_frames() > started + 48_000 * 3 => {
                    assert!(
                        evidence.audible_tracks[0],
                        "sample notes must render audible audio"
                    );
                    assert!(
                        evidence.peak_voices > 0,
                        "sample attacks must reach the renderer"
                    );
                    queue(runtime, Request::Stop);
                    stage = 4;
                }
                4 if audio.active_notes() == 0 && !runtime.app.borrow().playing => return true,
                _ => {}
            }
            false
        }),
        Duration::from_secs(60),
        crest_synth::adapter::sample_capability::SAMPLE_CAPABILITY_ID,
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}

fn install_dense_bass(runtime: &Runtime) {
    use crest_tracker::composition::{Event, EventId, Note, PhraseForm};
    let mut app = runtime.app.borrow_mut();
    let song = app.editor.song();
    let chain = song.tracks[1].slots[0]
        .as_ref()
        .unwrap()
        .chain
        .pattern()
        .unwrap();
    let id = song.chains[&chain].phrases[0].phrase;
    let mut phrase = song.phrases[&id].clone();
    let target = phrase.events[0].target.clone();
    phrase.form = PhraseForm::PianoRoll;
    phrase.steps = 8192;
    phrase.events = (0..8192)
        .map(|index| {
            Event::note(
                EventId::new(10000 + index).unwrap(),
                Beat::new(index, 4).unwrap(),
                target.clone(),
                Note::new(48 + (index % 24) as u8, 88, Beat::new(1, 8).unwrap()).unwrap(),
            )
        })
        .collect();
    phrase.events.insert(
        1,
        Event::note(
            EventId::new(20000).unwrap(),
            Beat::ZERO,
            target,
            Note::new(49, 88, Beat::integer(2048)).unwrap(),
        ),
    );
    phrase.events[0]
        .controls
        .push(Control::Bend { value: 8193 });
    let revision = app.editor.revision();
    // Fixture preparation uses the validated editor directly. The shared-edit
    // dialog is not part of this renderer witness; verify the admitted source.
    app.editor
        .apply(revision, Command::PutPhrase { id, phrase })
        .unwrap();
    assert_eq!(app.editor.song().phrases[&id].events.len(), 8193);
}

#[test]
#[ignore = "integrated dense Piano playback; run explicitly with --test-threads=1"]
fn dense_piano_notes_and_sustained_overlap_render_through_the_production_host() {
    dense_piano_playback(0);
}

#[test]
#[ignore = "production producer stall recovery; run explicitly with --test-threads=1"]
fn dense_piano_producer_stall_recovers_without_overdue_attacks() {
    dense_piano_playback(400);
}

#[test]
#[ignore = "production producer stall recovery; run explicitly with --test-threads=1"]
fn dense_piano_short_producer_stall_recovers_without_overdue_attacks() {
    dense_piano_playback(40);
}

fn dense_piano_playback(stall_blocks: u64) {
    let root = temp_root("dense-piano-playback");
    let mut stage = 0;
    let mut started = 0;
    let mut stalled = false;
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| {
            match stage {
                0 => {
                    queue(runtime, Request::Example);
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name == "Window Light" => {
                    install_dense_bass(runtime);
                    queue(runtime, Request::Play);
                    started = audio.rendered_frames();
                    evidence.measured = true;
                    stage = 2;
                }
                2 => {
                    let app = runtime.app.borrow();
                    assert!(
                        app.playing,
                        "{}; routing {}, refusals {}, frame {}, recoveries {}",
                        app.status,
                        audio.routing_failures(),
                        audio.voice_limit_refusals(),
                        audio.rendered_frames(),
                        runtime.playback_recoveries,
                    );
                    drop(app);
                    if !stalled && audio.rendered_frames() > started + 48_000 {
                        evidence.control_stall_blocks = stall_blocks;
                        stalled = true;
                    }
                    if audio.rendered_frames() > started + 48_000 * 12 {
                        assert!(runtime.app.borrow().playback_fault.is_none());
                        assert_eq!(runtime.playback_recoveries, u64::from(stall_blocks > 0));
                        assert!(runtime.app.borrow().play_beat.unwrap() > Beat::integer(12));
                        assert!(evidence.audible_tracks[..5].iter().all(|audible| *audible));
                        queue(runtime, Request::Stop);
                        stage = 3;
                    }
                }
                3 if audio.active_notes() == 0 && !runtime.app.borrow().playing => {
                    assert_eq!(runtime.app.borrow().status, "STOPPED");
                    return true;
                }
                _ => {}
            }
            false
        }),
        Duration::from_secs(60),
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}

fn exceed_bass_voice_budget(runtime: &Runtime) {
    use crest_tracker::composition::{Event, EventId, Note, PhraseForm};
    let mut app = runtime.app.borrow_mut();
    let song = app.editor.song();
    let chain = song.tracks[1].slots[0]
        .as_ref()
        .unwrap()
        .chain
        .pattern()
        .unwrap();
    let id = song.chains[&chain].phrases[0].phrase;
    let mut phrase = song.phrases[&id].clone();
    let target = phrase.events[0].target.clone();
    phrase.form = PhraseForm::PianoRoll;
    phrase.events = (0..17)
        .map(|index| {
            Event::note(
                EventId::new(10000 + index).unwrap(),
                Beat::ZERO,
                target.clone(),
                Note::new(48 + index as u8, 88, Beat::ONE).unwrap(),
            )
        })
        .collect();
    let revision = app.editor.revision();
    // Fixture preparation uses the validated editor directly. The shared-edit
    // dialog is not part of this renderer witness; verify the admitted source.
    app.editor
        .apply(revision, Command::PutPhrase { id, phrase })
        .unwrap();
    assert_eq!(app.editor.song().phrases[&id].events.len(), 17);
}

#[test]
#[ignore = "production voice refusal and persistent fault; run explicitly with --test-threads=1"]
fn playback_fault_survives_navigation_and_clears_after_successful_restart() {
    let root = temp_root("persistent-playback-fault");
    let mut stage = 0;
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, _| {
            match stage {
                0 => {
                    queue(runtime, Request::Example);
                    stage = 1;
                }
                1 if runtime.app.borrow().editor.song().name == "Window Light" => {
                    exceed_bass_voice_budget(runtime);
                    queue(runtime, Request::Play);
                    stage = 2;
                }
                2 if runtime.app.borrow().playback_fault.is_some() => {
                    let app = runtime.app.borrow();
                    assert!(!app.playing);
                    assert!(app
                        .playback_fault
                        .as_ref()
                        .unwrap()
                        .contains("voice budget refused 1 attacks"));
                    drop(app);
                    queue(
                        runtime,
                        Request::Move {
                            horizontal: 1,
                            vertical: 0,
                        },
                    );
                    queue(runtime, Request::View { view: View::Chain });
                    stage = 3;
                }
                3 => {
                    assert!(runtime.app.borrow().projection()["playbackFault"].is_string());
                    queue(runtime, Request::Undo);
                    queue(runtime, Request::Play);
                    stage = 4;
                }
                4 => {
                    assert!(
                        runtime.app.borrow().playing,
                        "{}",
                        runtime.app.borrow().status
                    );
                    assert!(runtime.app.borrow().playback_fault.is_none());
                    queue(runtime, Request::Stop);
                    stage = 5;
                }
                5 if audio.active_notes() == 0 && !runtime.app.borrow().playing => return true,
                _ => {}
            }
            false
        }),
        Duration::from_secs(60),
    );
    assert!(evidence.complete);
    assert_eq!(evidence.final_voices, 0);
    assert_eq!(evidence.voice_refusals, 1);
    assert_eq!(evidence.routing_failures, 0);
    std::fs::remove_dir_all(root).unwrap();
}

struct DemoJourney {
    stage: u8,
    started: u64,
    saved_path: PathBuf,
    baseline: Option<Document>,
}
impl DemoJourney {
    fn step(
        &mut self,
        runtime: &mut Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) -> bool {
        match self.stage {
            0 => {
                queue(runtime, Request::Example);
                self.stage = 1;
            }
            1 if runtime.app.borrow().editor.song().name == "Window Light" => {
                assert_eq!(runtime.pinned_sounds.len(), 5);
                queue(runtime, Request::Play);
                self.started = audio.rendered_frames();
                self.stage = 2;
                evidence.measured = true;
            }
            2 if audio.rendered_frames() > self.started + 48_000 * 9 => {
                assert!(
                    runtime.app.borrow().playing,
                    "{}",
                    runtime.app.borrow().status
                );
                assert!(evidence.audible_tracks[..5].iter().all(|audible| *audible));
                queue(runtime, Request::Stop);
                self.stage = 3;
            }
            3 if audio.active_notes() == 0 && !runtime.app.borrow().playing => {
                queue(
                    runtime,
                    Request::Rename {
                        name: "Window Light witness".into(),
                    },
                );
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::SaveAs,
                    },
                );
                self.stage = 4;
            }
            4 if choose(runtime, &self.saved_path) => {
                self.stage = 5;
            }
            5 if !runtime.saving
                && runtime.app.borrow().path.as_ref() == Some(&self.saved_path) =>
            {
                self.baseline = Some(Document::read(&self.saved_path).unwrap());
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::New,
                    },
                );
                self.stage = 6;
            }
            6 if runtime.app.borrow().editor.song().name == "Untitled"
                && runtime.pending_open.is_none() =>
            {
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::Open,
                    },
                );
                self.stage = 7;
            }
            7 if choose(runtime, &self.saved_path) => {
                self.stage = 8;
            }
            8 if runtime.app.borrow().editor.song().name == "Window Light witness" => {
                let baseline = self.baseline.as_ref().unwrap();
                assert_eq!(runtime.app.borrow().editor.song(), &baseline.song);
                assert_eq!(runtime.saved_sound.as_ref(), Some(&baseline.synth));
                assert_eq!(runtime.app.borrow().mappings, baseline.installed_patches);
                assert_eq!(
                    serde_json::to_value(&runtime.pinned_sounds).unwrap(),
                    serde_json::to_value(&baseline.pinned_sounds).unwrap()
                );
                assert!(!runtime.app.borrow().playing);
                return audio.active_notes() == 0;
            }
            _ => {}
        }
        false
    }
}

#[test]
#[ignore = "integrated production workers and DSP; run explicitly with --test-threads=1"]
fn bundled_demo_plays_expression_and_round_trips_complete_sound() {
    let root = temp_root("demo-journey");
    let mut journey = DemoJourney {
        stage: 0,
        started: 0,
        saved_path: root.join("saved.crest-song"),
        baseline: None,
    };
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| journey.step(runtime, audio, evidence)),
        Duration::from_secs(60),
    );
    evidence.check_cleanup();
    std::fs::remove_dir_all(root).unwrap();
}

pub(super) fn workload_document() -> Document {
    let mut song = Song {
        name: "Integrated 16-track workload".into(),
        ..Song::default()
    };
    let mut mappings = BTreeMap::new();
    for index in 0..16 {
        let id = InstrumentId::new(index as u64 + 17).unwrap();
        song.instruments.insert(
            id,
            InstrumentUse {
                name: format!("Sustained {}", index + 1),
                resource: None,
            },
        );
        mappings.insert(id, index as u32 + 1);
        add_workload_track(&mut song, index, id);
    }
    let saved = fixture_authoring::synth_session(16, true);
    let document = Document::new(song, serde_json::to_value(saved).unwrap(), mappings);
    document.validate().unwrap();
    document
}

fn add_workload_track(song: &mut Song, index: usize, instrument: InstrumentId) {
    let base = 100 + index as u64 * 10;
    let phrase = PhraseId::new(base).unwrap();
    let chain = ChainId::new(base + 1).unwrap();
    let target = Target::Instrument {
        id: instrument,
        channel: Channel::FIRST,
    };
    let events = [0, 4, 7, 12]
        .into_iter()
        .enumerate()
        .map(|(voice, offset)| {
            let mut note =
                Note::new(48 + offset + (index % 3) as u8, 80, Beat::integer(63)).unwrap();
            note.expression = vec![
                Expression {
                    offset: Beat::ZERO,
                    control: Control::Pressure { value: 110 },
                    sequence: None,
                },
                Expression {
                    offset: Beat::integer(16),
                    control: Control::Bend { value: 8300 },
                    sequence: None,
                },
                Expression {
                    offset: Beat::integer(48),
                    control: Control::Bend { value: 8192 },
                    sequence: None,
                },
            ];
            Event::note(
                EventId::new(base + voice as u64 + 4).unwrap(),
                Beat::ZERO,
                target.clone(),
                note,
            )
        })
        .collect();
    song.phrases.insert(
        phrase,
        Phrase {
            name: format!("Hold {}", index + 1),
            steps: 256,
            events,
            ..Phrase::default()
        },
    );
    song.chains.insert(
        chain,
        Chain {
            name: format!("Lane {}", index + 1),
            phrases: vec![PhraseUse {
                id: UseId::new(base + 2).unwrap(),
                phrase,
                repeats: 1,
            }],
        },
    );
    song.tracks[index].slots = vec![Some(ChainUse {
        id: UseId::new(base + 3).unwrap(),
        chain: chain.into(),
        repeats: 1,
    })];
    song.tracks[index].looped = true;
}

struct Workload {
    stage: u8,
    path: PathBuf,
    started: u64,
    seconds: u64,
}
impl Workload {
    fn step(
        &mut self,
        runtime: &mut Runtime,
        audio: AudioObservationSnapshot,
        evidence: &mut Evidence,
    ) -> bool {
        match self.stage {
            0 => {
                queue(
                    runtime,
                    Request::File {
                        action: FileAction::Open,
                    },
                );
                self.stage = 1;
            }
            1 if choose(runtime, &self.path) => {
                self.stage = 2;
            }
            2 if runtime.app.borrow().editor.song().name == "Integrated 16-track workload" => {
                queue(runtime, Request::Play);
                self.stage = 3;
            }
            3 if audio.active_notes() == 64 => {
                self.started = audio.rendered_frames();
                evidence.measured = true;
                self.stage = 4;
            }
            4 => {
                assert!(
                    runtime.app.borrow().playing,
                    "{}",
                    runtime.app.borrow().status
                );
                if audio.rendered_frames() >= self.started + self.seconds * 48_000 {
                    queue(runtime, Request::Stop);
                    self.stage = 5;
                } else if audio.rendered_blocks().is_multiple_of(128) {
                    // Exercise the real Tracker projection during the audio workload.
                    let _ = runtime.app.borrow().projection();
                }
            }
            5 if !runtime.app.borrow().playing && audio.active_notes() == 0 => {
                return true;
            }
            _ => {}
        }
        false
    }
}

#[test]
#[ignore = "optimized integrated audio workload; run explicitly and serially"]
fn sixteen_tracks_keep_sixty_four_real_voices_and_stop_cleanly() {
    let seconds = std::env::var("CREST_AUDIO_SECONDS")
        .map(|text| text.parse::<u64>().unwrap())
        .unwrap_or(40);
    assert!((1..=3600).contains(&seconds));
    let root = temp_root("integrated-audio");
    let path = root.join("workload.crest-song");
    std::fs::write(&path, serde_json::to_vec(&workload_document()).unwrap()).unwrap();
    let mut workload = Workload {
        stage: 0,
        path,
        started: 0,
        seconds,
    };
    let evidence = run(
        &root,
        Box::new(move |runtime, audio, evidence| workload.step(runtime, audio, evidence)),
        Duration::from_secs(seconds.max(120) * 2),
    );
    report_audio(&evidence, seconds, "sustained");
    evidence.check_cleanup();
    assert_eq!(evidence.peak_voices, 64);
    assert!(
        evidence.sustained_64_blocks > seconds * 150,
        "must sustain 64 real voices, not only observe an attack peak"
    );
    assert!(evidence.audible_tracks.iter().all(|audible| *audible));
    std::fs::remove_dir_all(root).unwrap();
}

pub(super) fn report_audio(evidence: &Evidence, seconds: u64, workload: &str) {
    let mut times = evidence.callback_ns.clone();
    times.sort_unstable();
    let percentile = |percent: usize| times[(times.len() * percent).div_ceil(100) - 1];
    let deadline = FRAMES as u64 * 1_000_000_000 / 48_000;
    let misses = times.iter().filter(|nanos| **nanos > deadline).count();
    let performance_passed = percentile(99) < deadline * 7 / 10 && misses == 0;
    let mut control = evidence.control_ns.clone();
    control.sort_unstable();
    let control_p99 = control[(control.len() * 99).div_ceil(100) - 1];
    let report = json!({"scope":"integrated_tracker_headless_audio", "audio_seconds":seconds,
        "workload":workload, "sample_rate":48000, "block_frames":FRAMES, "tracks":16,
        "queue_peak_sampling":"after successful producer publication",
        "device":"in-process output; production host, workers and DSP", "physical_latency_measured":false,
        "rust_callback_memory_only":true, "p50_ns":percentile(50), "p95_ns":percentile(95),
        "p99_ns":percentile(99), "max_ns":times.last(), "deadline_ns":deadline,
        "deadline_misses":misses, "performance_passed":performance_passed,
        "control_tick_p99_ns":control_p99, "control_tick_max_ns":control.last(),
        "evidence":evidence});
    if let Some(path) = std::env::var_os("CREST_AUDIO_REPORT") {
        atomic_write(
            std::path::Path::new(&path),
            &serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
    println!(
        "Integrated audio: p99={}ns max={}ns misses={} sustained64={} blocks; control p99={}ns",
        percentile(99),
        times.last().unwrap(),
        misses,
        evidence.sustained_64_blocks,
        control_p99
    );
    if std::env::var_os("CREST_AUDIO_STRICT").is_some() {
        assert!(performance_passed, "strict callback deadline gate failed");
    }
}
