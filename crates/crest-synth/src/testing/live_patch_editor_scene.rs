//! The `demo-live-patch-editor` scene: the functional Patch editor's live
//! demonstration, whose subject is the **second** installed Patch.
//!
//! Phase 3 proved the effect-slot journey for `patches.first()` only, because
//! the vocabulary had no patch-switching gesture. The gesture landed; this
//! scene is what it buys. Nothing here resolves a subject through
//! `patches.first()` — the subject is reached by dispatching
//! `SemanticAction::SelectPatch` through the production reducer and is then
//! asserted by id, and every editable target, effect-slot position, and
//! audible probe is derived from **that** Patch's own descriptors.

use crate::control::app_event::{AppEvent, Direction};
use crate::control::state_tree::StateTree;
use crate::control::{InteractionMode, PatchControlId, SemanticAction, SurfaceId, TopLevelContext};
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::voice_limit::VoiceLimit;
use crate::synth::EffectCapabilityId;
use crate::testing::live_demo_scene::{decode_state_tree, LiveDemoScene, LiveDemoSceneError};
use crate::testing::live_effects_and_buses_scene::{
    LiveTopologyAudibleWitness, LiveTopologySupport, LiveTopologyTransition,
};
use std::time::Duration;

/// The scene's stable name. `demo-live` keeps pointing at the cumulative
/// effects-and-buses scene; this one is additive.
pub const PATCH_EDITOR_SCENE_NAME: &str = "functional-patch-editor-live-demo";

/// Maximum elapsed control time. The base scene's scalar sweep and engine
/// phase are inherited whole, and this scene appends its own journey.
pub const PATCH_EDITOR_TOTAL_TIMEOUT: Duration = Duration::from_secs(300);

/// How many notes past the lowered ceiling the burst sounds. Each one is a
/// refusal the renderer records; three keeps the count unambiguous while
/// staying far inside the event budget.
const VOICE_BURST_OVERSHOOT: u8 = 3;

/// Whether the scene reaches its subject through the production gesture, or
/// has that gesture removed as the declared controlled negative.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PatchSelectionMode {
    /// The shipped behaviour: reach the second Patch by dispatching
    /// `SemanticAction::SelectPatch` through `AppLoop` and the reducer.
    Gesture,
    /// `--defeat-patch-selection`: the switch is removed and the journey stays
    /// on the first Patch. Every step still runs and still succeeds — what
    /// fails is *where the work landed*, which is the only thing the reach
    /// predicates measure. A negative that errored out early on an unrelated
    /// path would falsify nothing.
    Defeated,
}

impl PatchSelectionMode {
    const fn is_defeated(self) -> bool {
        matches!(self, Self::Defeated)
    }
}

/// Freezes the functional Patch editor scene from the installed state.
///
/// The base scene supplies the fixture preconditions and, deliberately, the
/// three engine transitions and the teardown contract this runner requires of
/// every live scene (mission finding F-49). This phase is appended after them.
pub fn from_installed_state(
    tree: &StateTree,
    mode: PatchSelectionMode,
) -> Result<LiveDemoScene, LiveDemoSceneError> {
    let base = LiveDemoScene::from_installed_state(tree)?;
    let state = decode_state_tree(tree)?;
    if state.patches.len() < 2 {
        return Err(LiveDemoSceneError::NoInstalledPatches);
    }

    // The subject. Under `Gesture` it is the second installed Patch, reached
    // by the gesture below; under `Defeated` the scene never leaves the first.
    // Nothing downstream reads `patches.first()`: every target, slot, probe,
    // and boundary direction derives from `subject`.
    let subject_index = if mode.is_defeated() { 0 } else { 1 };
    let subject = &state.patches[subject_index];
    let subject_id = PatchId::new(subject.id).map_err(|_| LiveDemoSceneError::InvalidPatchId)?;
    let subject_channel = MidiChannel::new(subject.channel)
        .map_err(|_| LiveDemoSceneError::InvalidMidiChannel(subject.channel))?;
    // The adjacent-choice order an occupancy row cycles through: empty, then
    // each installed registry entry in declared order. Addressed positionally
    // — nothing in this scene names an effect.
    let choices: Vec<Option<EffectCapabilityId>> = std::iter::once(None)
        .chain(
            state
                .effects
                .descriptors()
                .iter()
                .map(|descriptor| Some(descriptor.id().clone())),
        )
        .collect();
    // What each position on the subject starts holding, and therefore what
    // the journey must put back. The subject's starting grid is *read*, not
    // assumed empty: the negative's subject carries the startup occupant, and
    // a plan that assumed otherwise would fail on its own scaffold rather
    // than on reach.
    let starting = |slot: EffectSlotIndex| -> Option<EffectCapabilityId> {
        subject
            .post_effects
            .iter()
            .find(|effect| effect.slot_id() == slot.instance_identity())
            .map(|effect| effect.capability_id().clone())
    };
    // One accepted adjacent step from whatever a position starts with. Filling
    // is one step right, restoring is one step left, in either mode.
    let filled_with = |slot: EffectSlotIndex| -> Result<EffectCapabilityId, LiveDemoSceneError> {
        let current = starting(slot);
        let index = choices
            .iter()
            .position(|choice| *choice == current)
            .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
        choices
            .get(index.saturating_add(1))
            .and_then(Option::as_ref)
            .cloned()
            .ok_or(LiveDemoSceneError::InvalidEffectConfig)
    };

    let edited_slot = EffectSlotIndex::ALL[0];
    let entry = filled_with(edited_slot)?;
    let entry_descriptor = state
        .effects
        .descriptor(&entry)
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;

    // The audible edit's target, derived from the exact occupant the fill will
    // produce: the registry entry's own descriptor-default configuration. The
    // scene never guesses a value it has not derived.
    let default_occupant = entry_descriptor
        .default_config(edited_slot.instance_identity())
        .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;
    let predicate_satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
        predicate.is_none_or(|predicate| {
            default_occupant.value(predicate.parameter_id()) == Some(predicate.equals())
        })
    };
    let scalar_spec = entry_descriptor
        .parameters()
        .find(|spec| {
            spec.patch_interaction() == crate::synth::PatchInteraction::ScalarEdit
                && spec.update() == crate::synth::ParameterUpdate::Scalar
                && predicate_satisfied(spec.visible_when())
                && predicate_satisfied(spec.enabled_when())
        })
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    let scalar_current = default_occupant
        .value(scalar_spec.id())
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    // An accepted direction, derived rather than assumed: increase when the
    // default is not already at its ceiling, otherwise decrease.
    let scalar_direction = match scalar_spec.adjusted_scalar_value(
        scalar_current,
        crate::synth::ParameterAdjustment::FineIncrease,
    ) {
        Ok(next) if &next != scalar_current => Direction::Right,
        _ => Direction::Left,
    };
    let occupant_scalar_row =
        PatchControlId::Effect(edited_slot.instance_identity(), scalar_spec.id().clone());

    // How far the subject's own ceiling must fall for the limit to bite,
    // derived from the published per-Patch limit and the canonical descriptor
    // steps. An extra step past the minimum would be a boundary rejection, so
    // the count is computed rather than picked.
    let voice_limit_descriptor = VoiceLimit::descriptor();
    let starting_ceiling = state
        .parameters
        .patches
        .iter()
        .find(|patch| patch.patch_id == subject.id)
        .ok_or(LiveDemoSceneError::InvalidPlannedAdjustment)?
        .voice_limit;
    // Down to the declared minimum. The last coarse step clamps rather than
    // underflowing, so the count is exact: one more would be a boundary
    // rejection and the scene would fail on its own scaffold.
    let mut ceiling = starting_ceiling;
    let mut voice_limit_steps = 0_usize;
    while ceiling > voice_limit_descriptor.minimum() {
        ceiling = ceiling
            .saturating_sub(voice_limit_descriptor.coarse_step())
            .max(voice_limit_descriptor.minimum());
        voice_limit_steps = voice_limit_steps.saturating_add(1);
    }
    // And back. The way up is **not** the way down: a coarse step down clamps
    // at the minimum, a coarse step up does not clamp at the value it came
    // from, so restoring by the same count would overshoot. The return walk is
    // therefore as many coarse steps as fit, then fine steps for the
    // remainder — landing exactly on the ceiling the run found.
    let mut restored_ceiling = ceiling;
    let mut voice_limit_coarse_restore = 0_usize;
    while restored_ceiling.saturating_add(voice_limit_descriptor.coarse_step()) <= starting_ceiling
    {
        restored_ceiling = restored_ceiling.saturating_add(voice_limit_descriptor.coarse_step());
        voice_limit_coarse_restore = voice_limit_coarse_restore.saturating_add(1);
    }
    let voice_limit_fine_restore = usize::from(
        starting_ceiling.saturating_sub(restored_ceiling)
            / voice_limit_descriptor.fine_step().max(1),
    );

    // The switch itself, and the boundary refusal that answers for it. The
    // defeated scene stays on the first Patch and probes left, preserving the
    // controlled negative's one-Patch reach. The shipped scene walks from its
    // second-Patch subject through the *actual* last installed Patch into the
    // interaction-only trailing empty position before probing right, then
    // returns to the subject. Deriving that walk from the frozen installed
    // order matters: the deterministic fixture has two Patches, but the
    // physical fixture currently has fifteen.
    let boundary_direction = if subject_index == 0 {
        Direction::Left
    } else {
        Direction::Right
    };
    let boundary_index = state.patches.len().saturating_sub(1);
    let boundary_patch_id = PatchId::new(state.patches[boundary_index].id)
        .map_err(|_| LiveDemoSceneError::InvalidPatchId)?;
    let boundary_distance = boundary_index.saturating_sub(subject_index);
    let mut boundary_support_before = vec![LiveTopologySupport::Event {
        event: AppEvent::SelectContext(TopLevelContext::Patch),
    }];
    if subject_index == 0 {
        boundary_support_before.push(LiveTopologySupport::VerifyPatchSubject {
            patch_id: subject_id,
        });
    } else {
        for _ in 0..boundary_distance {
            boundary_support_before.push(LiveTopologySupport::Event {
                event: AppEvent::SelectPatch(Direction::Right),
            });
        }
        boundary_support_before.push(LiveTopologySupport::VerifyPatchSubject {
            patch_id: boundary_patch_id,
        });
        boundary_support_before.push(LiveTopologySupport::Event {
            event: AppEvent::SelectPatch(Direction::Right),
        });
        boundary_support_before.push(LiveTopologySupport::VerifyEmptyPatchPosition);
    }
    let mut boundary_support_after = Vec::new();
    if subject_index != 0 {
        boundary_support_after.push(LiveTopologySupport::Event {
            event: AppEvent::SelectPatch(Direction::Left),
        });
        for _ in 0..boundary_distance {
            boundary_support_after.push(LiveTopologySupport::Event {
                event: AppEvent::SelectPatch(Direction::Left),
            });
        }
    }
    boundary_support_after.push(LiveTopologySupport::VerifyPatchSubject {
        patch_id: subject_id,
    });
    let mut switch_support = vec![LiveTopologySupport::Event {
        event: AppEvent::SelectContext(TopLevelContext::Patch),
    }];
    if !mode.is_defeated() {
        for _ in 0..subject_index {
            switch_support.push(LiveTopologySupport::Event {
                event: AppEvent::SelectPatch(Direction::Right),
            });
        }
    }
    switch_support.push(LiveTopologySupport::VerifyPatchSubject {
        patch_id: subject_id,
    });

    let slot_journey = |slot: EffectSlotIndex| {
        let control = PatchControlId::EffectSlot(slot);
        vec![
            LiveTopologySupport::Event {
                event: AppEvent::SelectContext(TopLevelContext::Patch),
            },
            LiveTopologySupport::VerifyPatchSubject {
                patch_id: subject_id,
            },
            LiveTopologySupport::FocusPatchControl {
                control: control.clone(),
            },
            LiveTopologySupport::VerifyPatchFocus { control },
            LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
            },
        ]
    };
    let patch_edit_exit = || {
        vec![LiveTopologySupport::Event {
            event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
        }]
    };
    let occupy = |slot: EffectSlotIndex, entry: Option<EffectCapabilityId>| {
        SemanticAction::SetSlotOccupancy {
            patch_id: subject_id,
            slot,
            entry,
        }
    };
    let transition = |id: &str,
                      action: Option<SemanticAction>,
                      adjust: Option<Direction>,
                      rejection: Option<&'static str>,
                      note: u8,
                      before: Vec<LiveTopologySupport>,
                      after: Vec<LiveTopologySupport>| {
        LiveTopologyTransition::new(
            id,
            action,
            adjust,
            rejection,
            subject_id,
            subject_channel,
            note,
            before,
            after,
            // Every transition in this phase witnesses dry continuity. The
            // `PatchChain` witness correlates the *first* stem and the first
            // active effect chain, so a subject that is not the first Patch
            // could never satisfy it — and asserting it anyway would be a
            // predicate that passes for the wrong reason. The per-Patch
            // audible evidence this phase actually rests on is the per-track
            // RMS delta the patch-editor checkpoint measures by track
            // identity, which is Patch-local by construction.
            LiveTopologyAudibleWitness::DryContinuity,
        )
    };

    let mut transitions = vec![transition(
        "PatchEditor.switchToSubject",
        None,
        None,
        None,
        60,
        switch_support,
        Vec::new(),
    )];
    // Fill the first two positions, leaving the third empty for the refused
    // detail entry below.
    for slot in [EffectSlotIndex::ALL[0], EffectSlotIndex::ALL[1]] {
        transitions.push(transition(
            &format!("PatchEditor.slotFill.{}", slot.index()),
            Some(occupy(slot, Some(filled_with(slot)?))),
            Some(Direction::Right),
            None,
            62,
            slot_journey(slot),
            patch_edit_exit(),
        ));
    }
    // FR-014: the detail surface serves two subjects through one identity, and
    // refuses entry from a position with no occupant to describe. All three
    // are journeys on the subject Patch.
    transitions.push(transition(
        "PatchEditor.detailRoundTrip",
        None,
        None,
        None,
        64,
        {
            let mut support = slot_journey(EffectSlotIndex::ALL[2]);
            // The third position is still empty: entry is refused there.
            support.pop();
            support.push(LiveTopologySupport::ExpectRejected {
                event: AppEvent::EnterSurface(SurfaceId::PatchDetail),
                rejection: "actionUnavailableInContext",
            });
            // An occupied position: the effect subject.
            support.extend(slot_journey(EffectSlotIndex::ALL[0]));
            support.pop();
            support.push(LiveTopologySupport::Event {
                event: AppEvent::EnterSurface(SurfaceId::PatchDetail),
            });
            support.push(LiveTopologySupport::Event {
                event: AppEvent::Return,
            });
            // The engine row: the instrument subject, on the same identity.
            support.push(LiveTopologySupport::FocusPatchControl {
                control: PatchControlId::Engine,
            });
            support.push(LiveTopologySupport::VerifyPatchFocus {
                control: PatchControlId::Engine,
            });
            support.push(LiveTopologySupport::Event {
                event: AppEvent::EnterSurface(SurfaceId::PatchDetail),
            });
            support.push(LiveTopologySupport::Event {
                event: AppEvent::Return,
            });
            support
        },
        Vec::new(),
    ));
    // Fill the third position now that the refusal has been observed.
    transitions.push(transition(
        &format!("PatchEditor.slotFill.{}", EffectSlotIndex::ALL[2].index()),
        Some(occupy(
            EffectSlotIndex::ALL[2],
            Some(filled_with(EffectSlotIndex::ALL[2])?),
        )),
        Some(Direction::Right),
        None,
        62,
        slot_journey(EffectSlotIndex::ALL[2]),
        patch_edit_exit(),
    ));
    // The audible occupant parameter edit: one accepted step on an occupant of
    // an occupied slot on the subject, bracketed by the transition's own
    // bounded probe so the measured window is exact-generation.
    transitions.push(
        transition(
            "PatchEditor.occupantScalarEdit",
            None,
            Some(scalar_direction),
            None,
            65,
            vec![
                LiveTopologySupport::Event {
                    event: AppEvent::SelectContext(TopLevelContext::Patch),
                },
                LiveTopologySupport::VerifyPatchSubject {
                    patch_id: subject_id,
                },
                LiveTopologySupport::FocusPatchControl {
                    control: occupant_scalar_row.clone(),
                },
                LiveTopologySupport::VerifyPatchFocus {
                    control: occupant_scalar_row,
                },
                LiveTopologySupport::Event {
                    event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
                },
            ],
            patch_edit_exit(),
        )
        .measuring_audible_edit(),
    );
    // The voice limit made to bite from the Utility panel, then restored. The
    // ceiling falls through the canonical row; the burst is refused rather
    // than stealing anything already sounding.
    transitions.push(transition(
        "PatchEditor.voiceLimitBite",
        None,
        None,
        None,
        67,
        {
            let mut support = vec![
                LiveTopologySupport::Event {
                    event: AppEvent::SelectContext(TopLevelContext::Patch),
                },
                LiveTopologySupport::VerifyPatchSubject {
                    patch_id: subject_id,
                },
                LiveTopologySupport::Event {
                    event: AppEvent::EnterSurface(SurfaceId::PatchUtility),
                },
            ];
            support.extend(walk_to_utility_row(&PatchControlId::VoiceLimit));
            support.push(LiveTopologySupport::VerifyPatchFocus {
                control: PatchControlId::VoiceLimit,
            });
            support.push(LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
            });
            for _ in 0..voice_limit_steps {
                support.push(LiveTopologySupport::Event {
                    event: AppEvent::Adjust(Direction::Down),
                });
            }
            support.push(LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
            });
            support.push(LiveTopologySupport::Event {
                event: AppEvent::Return,
            });
            // Sound past the ceiling. The first note latches; the rest are
            // refused, and nothing already sounding is truncated.
            for offset in 0..(voice_limit_descriptor.minimum() as u8 + VOICE_BURST_OVERSHOOT) {
                support.push(LiveTopologySupport::Event {
                    event: AppEvent::Midi {
                        patch_id: subject_id,
                        message: MidiMessage::try_new(
                            subject_channel,
                            MidiMessageKind::NoteOn,
                            72 + offset,
                            112,
                        )
                        .expect("the declared voice-burst MIDI constants are valid"),
                    },
                });
            }
            support
        },
        {
            let mut support = vec![LiveTopologySupport::Event {
                event: AppEvent::Midi {
                    patch_id: subject_id,
                    message: MidiMessage::all_notes_off(subject_channel),
                },
            }];
            support.push(LiveTopologySupport::Event {
                event: AppEvent::EnterSurface(SurfaceId::PatchUtility),
            });
            support.extend(walk_to_utility_row(&PatchControlId::VoiceLimit));
            support.push(LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
            });
            for _ in 0..voice_limit_coarse_restore {
                support.push(LiveTopologySupport::Event {
                    event: AppEvent::Adjust(Direction::Up),
                });
            }
            for _ in 0..voice_limit_fine_restore {
                support.push(LiveTopologySupport::Event {
                    event: AppEvent::Adjust(Direction::Right),
                });
            }
            support.push(LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
            });
            support.push(LiveTopologySupport::Event {
                event: AppEvent::Return,
            });
            support
        },
    ));
    // The end-of-order refusal: a step past the trailing empty position (or
    // left of the first Patch in the controlled negative) leaves semantic
    // position focus unchanged.
    transitions.push(transition(
        "PatchEditor.switchRefusedAtEnd",
        Some(SemanticAction::SelectPatch(boundary_direction)),
        None,
        Some("parameterAtBoundary"),
        69,
        boundary_support_before,
        boundary_support_after,
    ));
    // Restore the subject's grid to the exact configuration it started from,
    // so the frozen teardown observes the baseline it froze.
    for slot in [
        EffectSlotIndex::ALL[2],
        EffectSlotIndex::ALL[1],
        EffectSlotIndex::ALL[0],
    ] {
        let mut exit = patch_edit_exit();
        if slot == EffectSlotIndex::ALL[0] && !mode.is_defeated() {
            // Return the focus to the first Patch through the same gesture,
            // so the inherited semantic tail runs where it was frozen.
            for _ in 0..subject_index {
                exit.push(LiveTopologySupport::Event {
                    event: AppEvent::SelectPatch(Direction::Left),
                });
            }
        }
        transitions.push(transition(
            &format!("PatchEditor.slotRestored.{}", slot.index()),
            Some(occupy(slot, starting(slot))),
            Some(Direction::Left),
            None,
            71,
            slot_journey(slot),
            exit,
        ));
    }

    Ok(base.with_patch_editor_extension(
        PATCH_EDITOR_SCENE_NAME,
        transitions,
        PATCH_EDITOR_TOTAL_TIMEOUT,
    ))
}

/// Walks the PATCH Utility panel from its entry row down to `control` through
/// the one declared five-row order.
///
/// Entering the panel focuses its first row; deriving the distance keeps the
/// scene walking the declared order instead of assuming adjacency, so
/// inserting a row moves the scene with it rather than leaving it adjusting a
/// neighbour.
fn walk_to_utility_row(control: &PatchControlId) -> Vec<LiveTopologySupport> {
    let order = PatchControlId::utility_surface_descriptor();
    let index = |target: &PatchControlId| {
        order
            .iter()
            .position(|candidate| candidate == target)
            .expect("the scene only walks declared Utility rows")
    };
    // Entering the panel focuses the Patch's own trim row, not the panel's
    // first row: master gain is seated above it and is not Patch-local.
    // Deriving the distance keeps the walk on the declared order rather than
    // on an assumed adjacency.
    let entry = PatchControlId::Output(crate::mixer::patch_output::PatchOutputParameter::TrimGain);
    let distance = index(control).saturating_sub(index(&entry));
    std::iter::repeat_with(|| LiveTopologySupport::Event {
        event: AppEvent::Navigate(Direction::Down),
    })
    .take(distance)
    .collect()
}

#[cfg(test)]
mod nfr_004_projection_throughput {
    //! NFR-004's measurement, owned by WP06 (F-23, F-30).
    //!
    //! F-30 re-measured the real number: the **full** projection pipeline per
    //! accepted event, release, 89 MIXER rows, is **3.00 ms** (2997 µs), not
    //! the 2.92 ms F-23 first reported — that earlier figure timed
    //! `SemanticGraphicalViewModel::project` alone. WP06 measures against
    //! 3.00 ms and grades honestly.
    //!
    //! F-29 is the cheap half of the remedy and is measured here beside it:
    //! each availability probe used to clone `AppState` twice, and the second
    //! clone is redundant because `AppState::apply` never writes a field on a
    //! rejected event. Hoisting one scratch clone per row takes a sweep from
    //! `2·|vocabulary|` clones to `1 + |vocabulary| + |accepted|`.
    //!
    //! Run it deliberately — it is a measurement, not a gate:
    //!
    //! ```text
    //! cargo test --release --lib -- --ignored --nocapture nfr_004
    //! ```

    use crate::control::app_event::AppEvent;
    use crate::control::app_state::{AppState, SemanticActionAvailability};
    use crate::control::{SemanticAction, StateProjector, TopLevelContext};
    use crate::kernel::midi_channel::MidiChannel;
    use crate::kernel::patch_id::PatchId;
    use crate::mixer::global_parameters::GlobalParameters;
    use crate::mixer::mixer_track_id::MixerTrackId;
    use crate::mixer::patch_output::PatchOutput;
    use crate::synth::patch::Patch;
    use crate::synth::sound_font_instrument::SoundFontInstrument;
    use std::time::Instant;

    /// Installs the production SoundFont registry and effect registry with two
    /// Patches, exactly the shape the live fixture starts from.
    fn installed_state() -> AppState {
        let provider = crate::adapter::production_instruments::production_soundfont_capability()
            .expect("the production SoundFont capability composes");
        let registry = provider
            .registry()
            .expect("the production registry composes");
        let effects = crate::adapter::production_effects::production_effect_registry()
            .expect("the production effect registry composes");
        let mut state = AppState::new_with_effects(
            registry,
            effects,
            GlobalParameters::new(0.0).expect("0 dB is a valid master gain"),
        );
        let patch = |id: u32, channel: u8| {
            Patch::new(
                PatchId::new(id).expect("a one-based fixture id is valid"),
                format!("Patch {id}"),
                crate::testing::automatic_midi_test::create_soundfont_config(
                    &provider,
                    SoundFontInstrument::new(0, id as u8, false).expect("fixture preset is valid"),
                )
                .expect("the fixture instrument config composes"),
                MidiChannel::new(channel).expect("fixture channel is valid"),
                PatchOutput::new(
                    MixerTrackId::new(channel).expect("fixture track is valid"),
                    0.0,
                )
                .expect("fixture output is valid"),
            )
        };
        state
            .apply(AppEvent::InstallPatches(vec![patch(1, 0), patch(2, 1)]))
            .expect("installing the fixture Patches is accepted");
        state
    }

    /// The pre-F-29 sweep, written out literally so the comparison measures
    /// the change rather than a description of it: one clone to obtain a
    /// `&mut`, one more inside `apply`, for every action in the vocabulary.
    fn baseline_sweep(state: &AppState) -> usize {
        SemanticAction::surface_descriptor()
            .iter()
            .filter(|action| {
                let mut candidate = state.clone();
                candidate.apply_semantic_action((*action).clone()).is_ok()
            })
            .count()
    }

    /// The post-F-29 sweep: one scratch clone per row, reused across refusals.
    fn hoisted_sweep(state: &AppState) -> usize {
        let mut availability = SemanticActionAvailability::new(state);
        SemanticAction::surface_descriptor()
            .iter()
            .filter(|action| availability.accepts(action))
            .count()
    }

    fn median_micros(mut samples: Vec<f64>) -> f64 {
        samples.sort_by(|left, right| left.partial_cmp(right).expect("timings are finite"));
        samples[samples.len() / 2]
    }

    #[test]
    #[ignore = "a measurement, not a gate; run with --release --ignored --nocapture"]
    fn nfr_004_full_projection_pipeline_per_accepted_event() {
        let mut mixer = installed_state();
        mixer
            .apply(AppEvent::SelectContext(TopLevelContext::Mixer))
            .expect("MIXER is selectable");
        let mut patch = installed_state();
        patch
            .apply(AppEvent::SelectContext(TopLevelContext::Patch))
            .expect("PATCH is selectable");

        for (name, state) in [("MIXER Main", &mixer), ("PATCH Main", &patch)] {
            let projector = StateProjector::new();
            let (_, _, _, shell, _) = projector
                .project_with_shell(state)
                .expect("the fixture state projects");
            let rows: usize = shell
                .semantic_model()
                .surfaces()
                .iter()
                .map(|surface| surface.controls().len())
                .sum();

            // Warm.
            for _ in 0..3 {
                let _ = projector.project_with_shell(state);
            }
            let mut samples = Vec::new();
            for _ in 0..15 {
                let started = Instant::now();
                let projected = projector.project_with_shell(state);
                samples.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                assert!(projected.is_ok(), "the fixture state projects");
            }
            let full = median_micros(samples);

            let mut sweep_baseline = Vec::new();
            let mut sweep_hoisted = Vec::new();
            for _ in 0..2_000 {
                let started = Instant::now();
                let accepted = baseline_sweep(state);
                sweep_baseline.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                let started = Instant::now();
                let hoisted = hoisted_sweep(state);
                sweep_hoisted.push(started.elapsed().as_secs_f64() * 1_000_000.0);
                assert_eq!(
                    accepted, hoisted,
                    "the hoisted sweep must answer the identical question",
                );
            }
            let baseline = median_micros(sweep_baseline);
            let hoisted = median_micros(sweep_hoisted);

            println!(
                "NFR-004 {name}: rows={rows} full project_with_shell={full:.0}us \
                 per-row availability sweep baseline={baseline:.2}us hoisted={hoisted:.2}us \
                 (projected saving over {rows} rows = {:.0}us)",
                (baseline - hoisted) * rows as f64,
            );
        }
    }

    /// The property F-29's hoist rests on, proved rather than assumed: a
    /// rejected `apply_semantic_action` leaves the state it was given
    /// byte-for-byte identical, so one scratch may be reused across refusals.
    ///
    /// This is the whole correctness argument. If any reducer path ever
    /// mutates before rejecting, this test fails and the hoist is unsound.
    #[test]
    fn a_refused_semantic_action_never_touches_the_state_it_was_given() {
        let base = installed_state();
        let mut states = vec![base.clone()];
        for context in [TopLevelContext::Patch, TopLevelContext::Mixer] {
            let mut state = base.clone();
            state
                .apply(AppEvent::SelectContext(context))
                .expect("both contexts are selectable");
            states.push(state.clone());
            for surface in [
                crate::control::SurfaceId::PatchUtility,
                crate::control::SurfaceId::MixerInspector,
            ] {
                let mut entered = state.clone();
                if entered.apply(AppEvent::EnterSurface(surface)).is_ok() {
                    states.push(entered);
                }
            }
            let mut adjusting = state;
            if adjusting
                .apply(AppEvent::SetInteractionMode(
                    crate::control::InteractionMode::Adjust,
                ))
                .is_ok()
            {
                states.push(adjusting);
            }
        }

        let mut refusals = 0_usize;
        for state in &states {
            for action in SemanticAction::surface_descriptor() {
                let mut candidate = state.clone();
                if candidate.apply_semantic_action(action.clone()).is_err() {
                    refusals += 1;
                    assert_eq!(
                        &candidate, state,
                        "a refused {action:?} must leave the candidate identical",
                    );
                }
            }
        }
        assert!(
            refusals > 0,
            "the fixture must actually exercise refusals for this to prove anything",
        );
    }

    /// The hoisted sweep and the one-shot probe must answer identically for
    /// every action on every reachable fixture state. Same reducer, same
    /// question — this is what keeps the per-row list from drifting.
    #[test]
    fn the_hoisted_sweep_agrees_with_the_one_shot_probe_everywhere() {
        let base = installed_state();
        let mut states = vec![base.clone()];
        for context in [TopLevelContext::Patch, TopLevelContext::Mixer] {
            let mut state = base.clone();
            state
                .apply(AppEvent::SelectContext(context))
                .expect("both contexts are selectable");
            states.push(state);
        }
        for state in &states {
            let one_shot: Vec<bool> = SemanticAction::surface_descriptor()
                .iter()
                .map(|action| state.accepts_semantic_action(action))
                .collect();
            let mut availability = SemanticActionAvailability::new(state);
            let swept: Vec<bool> = SemanticAction::surface_descriptor()
                .iter()
                .map(|action| availability.accepts(action))
                .collect();
            assert_eq!(one_shot, swept);
        }
    }
}
