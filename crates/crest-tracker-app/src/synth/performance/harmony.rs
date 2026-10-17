//! Tracker controls select settings; the library owns chord rendering and voice
//! lifetime, including delayed articulation on the native host clock.
use super::*;
use crate::app::{PerformanceChordEdit, PerformanceHarmonyEdit as Edit, PerformanceInputMode};
use crest_performance_core::{
    Articulation, ChordExtension, Command, ExpressionPolicy, HarmonyPlaying, HarmonySettings,
    HarmonySource, Scale, Suspension,
};

/// Apply a relative control to current native state, not a stale frontend copy.
fn edited(mut settings: HarmonySettings, edit: Edit) -> Result<HarmonySettings, String> {
    match edit {
        Edit::Tonic { delta } => {
            settings.tonic = settings
                .tonic
                .checked_add_signed(delta)
                .ok_or("Tonic outside MIDI range")?
        }
        Edit::Anchor { delta } => {
            settings.input_anchor = settings
                .input_anchor
                .checked_add_signed(delta)
                .ok_or("Input anchor outside MIDI range")?
        }
        Edit::Scale => {
            settings.scale = match settings.scale {
                Scale::Major => Scale::NaturalMinor,
                Scale::NaturalMinor => Scale::Dorian,
                Scale::Dorian => Scale::Mixolydian,
                Scale::Mixolydian => Scale::Major,
            }
        }
        Edit::Extension => {
            settings.extension = match settings.extension {
                ChordExtension::Triad => ChordExtension::Seventh,
                ChordExtension::Seventh => ChordExtension::Ninth,
                ChordExtension::Ninth => ChordExtension::Triad,
            }
        }
        Edit::Suspension => {
            settings.suspension = match settings.suspension {
                Suspension::None => Suspension::Second,
                Suspension::Second => Suspension::Fourth,
                Suspension::Fourth => Suspension::None,
            }
        }
        Edit::Borrowed => settings.borrowed = !settings.borrowed,
        Edit::Inversion { delta } => {
            settings.inversion = settings
                .inversion
                .checked_add(delta)
                .ok_or("Inversion overflow")?
        }
        Edit::Register { delta } => {
            settings.register = settings
                .register
                .checked_add(delta)
                .ok_or("Register overflow")?
        }
        Edit::Spread => settings.spread = !settings.spread,
        Edit::VoiceLeading => settings.voice_leading = !settings.voice_leading,
        Edit::Expression => {
            settings.expression = match settings.expression {
                ExpressionPolicy::AllVoices => ExpressionPolicy::TopVoice,
                ExpressionPolicy::TopVoice => ExpressionPolicy::AllVoices,
            }
        }
        Edit::Playing => {
            settings.playing = match settings.playing {
                HarmonyPlaying::ChordPads => HarmonyPlaying::Strumplate,
                HarmonyPlaying::Strumplate => HarmonyPlaying::ChordPads,
            }
        }
        Edit::Degree { degree } => settings.degree = degree,
        Edit::Articulation { value } => settings.articulation = value,
        Edit::BlockArticulation => settings.articulation = Articulation::Block,
    }
    settings.validate().map_err(|e| e.to_string())?;
    Ok(settings)
}

impl Runtime {
    pub(in crate::synth) fn edit_performance_chord(
        &mut self,
        edit: PerformanceChordEdit,
    ) -> Result<(), String> {
        self.require_performance_change()?;
        let engine = self
            .performance_live
            .as_mut()
            .map(|live| &mut live.engine)
            .or(self.performance.as_mut())
            .ok_or("Open a performance session first")?;
        match edit {
            PerformanceChordEdit::LearnHeld => {
                engine.learn_held_chord().map_err(|e| e.to_string())?;
            }
            PerformanceChordEdit::RestoreDiatonic => {
                let degree = engine.snapshot().harmony.degree;
                engine
                    .command(Command::ClearLearnedChord { degree })
                    .map_err(|e| e.to_string())?;
            }
        }
        // The library owns one Undo history across chord-bank and clip edits.
        self.app.borrow_mut().performance_editor = None;
        self.performance_edit_committed(match edit {
            PerformanceChordEdit::LearnHeld => {
                "CHORD LEARNED · Selected degree · Lowest held note is root · Held voices unchanged"
            }
            PerformanceChordEdit::RestoreDiatonic => {
                "DIATONIC RESTORED · Selected degree · Held voices unchanged"
            }
        })
    }

    pub(in crate::synth) fn set_performance_input_mode(
        &mut self,
        sound: &mut Sound<'_>,
        mode: PerformanceInputMode,
    ) -> Result<(), String> {
        self.require_performance_change()?;
        if mode == PerformanceInputMode::Chords && self.performance_surface.is_some() {
            return Err(
                "Return the LinnStrument to Play and wait for its acknowledgement first".into(),
            );
        }
        let engine = self
            .performance_live
            .as_ref()
            .map(|live| &live.engine)
            .or(self.performance.as_ref())
            .ok_or("Open a performance session first")?;
        let current = engine.snapshot();
        if current.mode != mode.surface() {
            if let Some(live) = &mut self.performance_live {
                live.set_mode(sound, mode.surface())?;
            } else if let Some(engine) = &mut self.performance {
                engine
                    .command(Command::SetMode(mode.surface()))
                    .map_err(|e| e.to_string())?;
                engine.drain_output().for_each(drop);
            }
        }
        self.app.borrow_mut().status = match mode {
            PerformanceInputMode::Play => "PLAY · Ordinary MPE · Release held pads before playing again",
            PerformanceInputMode::Chords => "CHORDS · Library harmony · Stock MPE input · Release held pads before playing again",
        }.into();
        Ok(())
    }

    pub(in crate::synth) fn return_performance_to_play(
        &mut self,
        sound: &mut Sound<'_>,
    ) -> Result<(), String> {
        // Surface restoration also works without an open performance document.
        if self.performance.is_some() || self.performance_live.is_some() {
            self.set_performance_input_mode(sound, PerformanceInputMode::Play)?;
        }
        self.restore_performance_surface()
    }

    pub(in crate::synth) fn edit_performance_harmony(&mut self, edit: Edit) -> Result<(), String> {
        self.require_performance_change()?;
        let engine = self
            .performance_live
            .as_mut()
            .map(|live| &mut live.engine)
            .or(self.performance.as_mut())
            .ok_or("Open a performance session first")?;
        let current = engine.snapshot();
        let settings = edited(current.harmony, edit)?;
        engine
            .command(Command::SetHarmony(settings))
            .map_err(|e| e.to_string())?;
        engine
            .command(Command::SetHarmonySource(HarmonySource::Selected {}))
            .map_err(|e| e.to_string())?;
        self.checkpoint_dirty |= self.performance_recovery.is_some();
        let mut app = self.app.borrow_mut();
        app.performance_dirty = true;
        app.status =
            "HARMONY UPDATED · Held voices keep their pitches · Playing clips update on relaunch"
                .into();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crest_performance_core::{BeatTime, Gesture, StrumDirection, SurfaceMode, TICKS_PER_BEAT};

    #[test]
    fn relative_controls_produce_independent_musical_expectations() {
        let defaults = HarmonySettings::default();
        let mut settings = defaults;
        for edit in [
            Edit::Inversion { delta: 1 },
            Edit::Register { delta: 1 },
            Edit::Spread,
        ] {
            settings = edited(settings, edit).unwrap();
        }
        assert_eq!(settings.voicing(0, None).unwrap().notes(), &[76, 84, 91]);
        let borrowed = edited(edited(defaults, Edit::Borrowed).unwrap(), Edit::Extension).unwrap();
        assert_eq!(
            borrowed.voicing(0, None).unwrap().notes(),
            &[60, 63, 67, 70]
        );
        let ninth = edited(edited(defaults, Edit::Extension).unwrap(), Edit::Extension).unwrap();
        let suspended = edited(edited(ninth, Edit::Suspension).unwrap(), Edit::Suspension).unwrap();
        assert_eq!(
            suspended.voicing(0, None).unwrap().notes(),
            &[60, 65, 67, 71, 74]
        );
        assert!(edited(defaults, Edit::Degree { degree: 7 }).is_err());
        assert!(edited(defaults, Edit::Tonic { delta: 127 }).is_err());
        assert!(edited(settings, Edit::Register { delta: 127 }).is_err());
    }

    #[test]
    fn articulation_controls_retain_imported_timing_until_explicit_change() {
        let settings = HarmonySettings {
            articulation: Articulation::Strum {
                direction: StrumDirection::Ascending,
                interval: BeatTime(TICKS_PER_BEAT / 8),
            },
            ..HarmonySettings::default()
        };
        let changed = edited(settings, Edit::Playing).unwrap();
        assert_eq!(changed.articulation, settings.articulation);
        assert_eq!(
            edited(settings, Edit::BlockArticulation)
                .unwrap()
                .articulation,
            Articulation::Block
        );
        assert!(edited(
            settings,
            Edit::Articulation {
                value: Articulation::Strum {
                    direction: StrumDirection::Ascending,
                    interval: BeatTime(0),
                }
            }
        )
        .is_err());
        let arpeggio = Articulation::Arpeggio {
            direction: StrumDirection::Descending,
            interval: BeatTime(TICKS_PER_BEAT / 4),
            gate: BeatTime(TICKS_PER_BEAT / 8),
        };
        assert_eq!(
            edited(settings, Edit::Articulation { value: arpeggio })
                .unwrap()
                .articulation,
            arpeggio
        );
    }

    #[test]
    fn manual_strumplate_picks_selected_voicing_without_chord_fanout() {
        let mut document = super::super::example_document()
            .unwrap()
            .performance
            .unwrap();
        let mut settings = edited(HarmonySettings::default(), Edit::Playing).unwrap();
        settings = edited(settings, Edit::Degree { degree: 4 }).unwrap();
        settings = edited(settings, Edit::Inversion { delta: 1 }).unwrap();
        document.session.harmony = settings;
        let mut engine = Performance::new(document, Geometry::Pads200, Limits::default()).unwrap();
        engine
            .command(Command::SetMode(SurfaceMode::Chords))
            .unwrap();
        for (channel, key) in [(1, 30), (2, 31), (3, 32)] {
            engine
                .accept_midi(Beat::ZERO, &[0x90 | channel, key, 100])
                .unwrap();
        }
        let attacks: Vec<_> = engine.drain_output().collect();
        assert_eq!(
            attacks
                .iter()
                .filter_map(|e| match e.gesture {
                    Gesture::On { key, .. } => Some(key),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [71, 74, 79]
        );
        engine
            .accept_midi(Beat::new(1, 4).unwrap(), &[0xd2, 80])
            .unwrap();
        let expression: Vec<_> = engine.drain_output().collect();
        assert_eq!(expression.len(), 1);
        assert_eq!(expression[0].voice, attacks[1].voice);
        engine.command(Command::SetMode(SurfaceMode::Play)).unwrap();
        assert_eq!(engine.drain_output().count(), 3);
        assert_eq!(engine.snapshot().active_voices, 0);
    }

    #[test]
    fn held_chords_keep_expression_policy_while_next_attack_uses_new_voicing() {
        let root =
            std::env::temp_dir().join(format!("crest-harmony-test-{}", uuid::Uuid::new_v4()));
        let mut runtime =
            Runtime::with_data_directory(Rc::new(RefCell::new(App::new())), root.clone()).unwrap();
        let document = super::super::example_document()
            .unwrap()
            .performance
            .unwrap();
        runtime.performance =
            Some(Performance::new(document, Geometry::Pads200, Limits::default()).unwrap());
        let engine = runtime.performance.as_mut().unwrap();
        engine
            .command(Command::SetMode(SurfaceMode::Chords))
            .unwrap();
        engine.accept_midi(Beat::ZERO, &[0x91, 30, 100]).unwrap();
        let first: Vec<_> = engine.drain_output().collect();
        assert_eq!(
            first
                .iter()
                .filter_map(|event| match event.gesture {
                    Gesture::On { key, .. } => Some(key),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [60, 64, 67]
        );
        runtime.edit_performance_harmony(Edit::Expression).unwrap();
        runtime
            .edit_performance_harmony(Edit::Inversion { delta: 1 })
            .unwrap();
        let before = runtime.performance_document().unwrap();
        assert!(runtime
            .edit_performance_harmony(Edit::Degree { degree: 7 })
            .is_err());
        assert_eq!(runtime.performance_document().unwrap(), before);
        let engine = runtime.performance.as_mut().unwrap();
        assert_eq!(engine.snapshot().active_voices, 3);
        engine
            .accept_midi(Beat::new(1, 8).unwrap(), &[0xe1, 0, 65])
            .unwrap();
        let expression: Vec<_> = engine.drain_output().collect();
        assert_eq!(
            expression.len(),
            3,
            "held All voices policy survives selecting Top voice"
        );
        assert_eq!(
            expression.iter().map(|e| e.voice).collect::<Vec<_>>(),
            first.iter().map(|e| e.voice).collect::<Vec<_>>()
        );
        engine
            .accept_midi(Beat::new(1, 4).unwrap(), &[0x92, 31, 90])
            .unwrap();
        let second: Vec<_> = engine.drain_output().collect();
        assert_eq!(
            second
                .iter()
                .filter_map(|event| match event.gesture {
                    Gesture::On { key, .. } => Some(key),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            [65, 69, 74]
        );
        engine
            .accept_midi(Beat::new(3, 8).unwrap(), &[0xd2, 80])
            .unwrap();
        let top: Vec<_> = engine.drain_output().collect();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].voice, second[2].voice);
        engine
            .accept_midi(Beat::new(1, 2).unwrap(), &[0x81, 30, 0])
            .unwrap();
        assert_eq!(engine.drain_output().count(), 3);
        assert_eq!(engine.snapshot().active_voices, 3);
        engine
            .accept_midi(Beat::new(3, 4).unwrap(), &[0x82, 31, 0])
            .unwrap();
        assert_eq!(engine.drain_output().count(), 3);
        assert_eq!(engine.snapshot().active_voices, 0);
        let saved = engine.document().unwrap();
        let restored = serde_json::from_slice(&serde_json::to_vec(&saved).unwrap()).unwrap();
        let reopened = Performance::new(restored, Geometry::Pads200, Limits::default()).unwrap();
        assert_eq!(
            reopened.snapshot().harmony.expression,
            ExpressionPolicy::TopVoice
        );
        assert_eq!(reopened.snapshot().harmony.inversion, 1);
        assert_eq!(reopened.snapshot().mode, SurfaceMode::Play);
        drop(runtime);
        if root.exists() {
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
