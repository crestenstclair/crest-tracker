//! The retained `demo-live-effects-and-buses` scene.
//!
//! This is the phase-completion gate: the cumulative live scene inherits the
//! complete sixteen-track routing scene — scalar sweep, engine transitions,
//! semantic tail, and teardown contract — and appends the ordered topology
//! transitions that demonstrate the expandable-effects-and-bus-topology
//! behaviors through semantic actions and `AppState::apply` only: filling all
//! three slots of a Patch one at a time, exchanging two genuinely different
//! effects so the order change is audible, two instances of one registry
//! entry, sends raised toward several of the eight destinations, changing the
//! effect occupying a return, one controlled rejection with a visible reason
//! and uninterrupted audio, and a rerouted Patch whose chain follows it.
//!
//! The demonstration subject is the first installed Patch: the production
//! audio observation correlates the first stem (`primaryPatchId`) and the
//! first active effect chain (`patchEffect`), so its slot grid is the one
//! whose changes are measurable per block. Its startup chorus is cleared
//! first — while a held note rings — and the grid is restored to the startup
//! configuration before the frozen semantic tail, preserving the base
//! scene's structural-restoration teardown contract.
//!
//! Every slot and return occupancy change travels the player's on-screen
//! journey: focus visibly walks to the PATCH effect-slot row (or the MIXER
//! Inspector return row), the landing is verified against the canonical
//! projection, and occupancy cycles by the adjacent-choice Edit gesture —
//! the same nonwrapping contract as the engine row. A target that is not
//! adjacent to the current occupant is reached by cycling through the
//! intervening choices; each pass is its own audible transition, and the
//! retained identity always lands on the same resulting occupancy the scene
//! has always proven. The single exception is the controlled rejection,
//! which stays a direct injection because the UI cannot express an unknown
//! registry entry. One installed occupant's descriptor scalar is also edited
//! audibly from the PATCH page.
//!
//! Registry entries are always addressed positionally through the installed
//! registry — nothing in this scene names an effect.

use crate::control::app_event::{AppEvent, Direction};
use crate::control::state_tree::StateTree;
use crate::control::{
    InteractionMode, MixerControlId, PatchControlId, SemanticAction, SurfaceId, TopLevelContext,
};
use crate::kernel::midi_channel::MidiChannel;
use crate::kernel::midi_message::{MidiMessage, MidiMessageKind};
use crate::kernel::patch_id::PatchId;
use crate::mixer::bus_id::BusId;
use crate::mixer::patch_output::PatchOutputParameter;
use crate::synth::effect_slot_id::EffectSlotIndex;
use crate::synth::EffectCapabilityId;
use crate::testing::live_demo_scene::{decode_state_tree, LiveDemoScene, LiveDemoSceneError};
use serde::Serialize;
use std::time::Duration;

/// The retained scene's stable name.
pub const EFFECTS_AND_BUSES_SCENE_NAME: &str = "effects-and-buses-live-demo";

/// Maximum elapsed control time for the cumulative retained scene. The base
/// sixteen-track scene keeps its own bound; this scene appends the topology
/// phase and therefore declares a wider one.
pub const EFFECTS_AND_BUSES_TOTAL_TIMEOUT: Duration = Duration::from_secs(240);

/// The identity a refused topology request names: a namespaced id that is
/// deliberately absent from every registry.
pub const ABSENT_ENTRY_ID: &str = "effect.absent";

/// The frozen pre-journey topology checkpoint-identity baseline (spec C-001,
/// reasserted across the webview shell cutover by C-004): the exact ordered
/// identity sequence the phase gate froze before the journey rework. The
/// declared transition sequence must preserve these byte-identically and in
/// this order; every other declared identity is a pure insertion.
pub const FROZEN_TOPOLOGY_IDENTITY_BASELINE: [&str; 17] = [
    "Slot.startupOccupantCleared",
    "SlotFill.first",
    "SlotFill.second",
    "SlotFill.third",
    "SlotOrder.exchangeFirst",
    "SlotOrder.exchangeSecond",
    "SlotTwin.sameEntry",
    "Send.towardDestinations",
    "Return.contentChanged",
    "Return.emptyOccupied",
    "Topology.refused",
    "Topology.recoveredAfterRefusal",
    "Reroute.chainFollows",
    "Slot.startupOccupantRestored",
    "Slot.secondCleared",
    "Slot.thirdCleared",
    "Return.emptyRestored",
];

/// One support item executed before or after a topology transition: either a
/// semantic event that must be accepted, or a projection verification that
/// must match exactly. Support items are pacing and audibility setup — the
/// measured behavior is always the transition itself.
#[derive(Clone, Debug, PartialEq)]
pub enum LiveTopologySupport {
    Event {
        event: AppEvent,
    },
    /// Navigates the MIXER track selection horizontally, one accepted step
    /// per tick, until the named track is selected.
    FocusMixerTrack {
        track_id: crate::mixer::mixer_track_id::MixerTrackId,
    },
    /// Walks the PATCH Main focus vertically, one accepted step per tick,
    /// until the named control row is focused.
    FocusPatchControl {
        control: PatchControlId,
    },
    /// Walks the selected track's MIXER Inspector focus vertically, one
    /// accepted step per tick, until the named control is focused.
    FocusInspectorControl {
        track_id: crate::mixer::mixer_track_id::MixerTrackId,
        control: MixerControlId,
    },
    VerifyMixerFocus {
        control: MixerControlId,
    },
    VerifyPatchFocus {
        control: PatchControlId,
    },
    /// Asserts which Patch the projection is speaking for. A journey that
    /// believes it switched instruments and did not must fail here rather
    /// than quietly editing the one it was already on.
    VerifyPatchSubject {
        patch_id: PatchId,
    },
    /// Asserts that PATCH speaks for the interaction-only trailing empty
    /// position rather than a persisted Patch identity.
    VerifyEmptyPatchPosition,
    /// Dispatches one event that must be refused by the named typed
    /// rejection, leaving state unchanged. The falsifying half of a journey:
    /// an entry the surface does not offer, or a step past the end of an
    /// order.
    ExpectRejected {
        event: AppEvent,
        rejection: &'static str,
    },
}

/// The causal audio measurement demanded by one topology transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LiveTopologyAudibleWitness {
    /// The sounding Patch's effect stage is active and audibly altering the
    /// signal (slot occupancy, exchange, twin instances, reroute).
    PatchChain,
    /// The shared wet path is audible (return occupancy changes and send
    /// raises toward occupied destinations).
    WetReturn,
    /// The dry output stays audible and finite (rejection continuity,
    /// cleared slots, and clears that silence a wet path).
    DryContinuity,
}

/// One frozen ordered topology transition of the retained scene.
#[derive(Clone, Debug, PartialEq)]
pub struct LiveTopologyTransition {
    id: String,
    /// The occupancy result under proof; `None` for support-driven scalar
    /// transitions (send raises, the occupant scalar edit, and the reroute).
    /// When `adjust` is also declared the runner dispatches the gesture as
    /// the measured cause and verifies the committed occupancy against this
    /// declaration at Activating.
    action: Option<SemanticAction>,
    /// A focused-control adjustment dispatched as the measured cause: the
    /// adjacent-choice occupancy gesture when `action` declares the expected
    /// result, or a plain scalar edit when it does not.
    adjust: Option<Direction>,
    expected_rejection: Option<&'static str>,
    sounding_patch: PatchId,
    channel: MidiChannel,
    probe_note: u8,
    support_before: Vec<LiveTopologySupport>,
    support_after: Vec<LiveTopologySupport>,
    audible: LiveTopologyAudibleWitness,
    /// This transition's dispatch is the scene's audible parameter edit: the
    /// runner records its per-track RMS deltas as the measured audible
    /// consequence of editing an occupant on the subject Patch.
    measures_audible_edit: bool,
    /// Voice carry-over: the probe note sounded before the dispatch is
    /// held through preparation, activation, and the audible capture — the
    /// runner must NOT re-sound it after Ready, so the captured witness can
    /// only hold if the sounding voice itself survived the swap.
    hold_note_through_activation: bool,
}

impl LiveTopologyTransition {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        id: impl Into<String>,
        action: Option<SemanticAction>,
        adjust: Option<Direction>,
        expected_rejection: Option<&'static str>,
        sounding_patch: PatchId,
        channel: MidiChannel,
        probe_note: u8,
        support_before: Vec<LiveTopologySupport>,
        support_after: Vec<LiveTopologySupport>,
        audible: LiveTopologyAudibleWitness,
    ) -> Self {
        Self {
            id: id.into(),
            action,
            adjust,
            expected_rejection,
            sounding_patch,
            channel,
            probe_note,
            support_before,
            support_after,
            audible,
            measures_audible_edit: false,
            hold_note_through_activation: false,
        }
    }

    /// Marks this transition's dispatch as the scene's measured audible
    /// parameter edit.
    pub(crate) fn measuring_audible_edit(mut self) -> Self {
        self.measures_audible_edit = true;
        self
    }

    /// Whether the runner records this transition's per-track deltas as the
    /// audible parameter edit.
    pub const fn measures_audible_edit(&self) -> bool {
        self.measures_audible_edit
    }

    /// Marks this transition's probe note as held across activation.
    fn holding_note_through_activation(mut self) -> Self {
        self.hold_note_through_activation = true;
        self
    }

    pub fn identifier(&self) -> &str {
        &self.id
    }

    pub const fn action(&self) -> Option<&SemanticAction> {
        self.action.as_ref()
    }

    pub const fn adjust(&self) -> Option<Direction> {
        self.adjust
    }

    pub const fn expected_rejection(&self) -> Option<&'static str> {
        self.expected_rejection
    }

    pub const fn sounding_patch(&self) -> PatchId {
        self.sounding_patch
    }

    pub const fn channel(&self) -> MidiChannel {
        self.channel
    }

    pub const fn probe_note(&self) -> u8 {
        self.probe_note
    }

    pub fn support_before(&self) -> &[LiveTopologySupport] {
        &self.support_before
    }

    pub fn support_after(&self) -> &[LiveTopologySupport] {
        &self.support_after
    }

    pub const fn audible(&self) -> LiveTopologyAudibleWitness {
        self.audible
    }

    /// Whether the probe note is held — never re-sounded — through
    /// activation, so the audible capture measures voice carry-over.
    pub const fn hold_note_through_activation(&self) -> bool {
        self.hold_note_through_activation
    }

    /// EventLog budget for this transition: every support event, the probe
    /// note pair, the dispatch, and the worker-side lifecycle events.
    pub(crate) fn event_budget(&self) -> usize {
        self.support_before
            .len()
            .saturating_add(self.support_after.len())
            .saturating_add(8)
    }

    pub(crate) fn probe_note_on(&self) -> AppEvent {
        AppEvent::Midi {
            patch_id: self.sounding_patch,
            message: MidiMessage::try_new(
                self.channel,
                MidiMessageKind::NoteOn,
                self.probe_note,
                112,
            )
            .expect("the frozen topology probe MIDI constants are valid"),
        }
    }

    pub(crate) fn probe_note_off(&self) -> AppEvent {
        AppEvent::Midi {
            patch_id: self.sounding_patch,
            message: MidiMessage::try_new(
                self.channel,
                MidiMessageKind::NoteOff,
                self.probe_note,
                0,
            )
            .expect("the frozen topology probe MIDI constants are valid"),
        }
    }
}

/// Freezes the retained cumulative effects-and-buses scene from the installed
/// state. The base sixteen-track scene supplies the fixture preconditions:
/// exactly three registry entries with the startup chorus occupying the first
/// Patch's first slot, and every other Patch's slot grid empty.
/// Returns how many downward steps separate the PATCH Utility entry row from
/// `control` in the one declared five-row order.
///
/// Entering PatchUtility focuses `Output(TrimGain)`; deriving the distance
/// keeps this scene walking the declared order instead of assuming adjacency,
/// so inserting a row moves the scene with it rather than silently leaving it
/// adjusting a neighbour.
fn utility_rows_below_entry(control: &PatchControlId) -> usize {
    let order = PatchControlId::utility_surface_descriptor();
    let index = |target: &PatchControlId| {
        order
            .iter()
            .position(|candidate| candidate == target)
            .expect("the scene only walks declared Utility rows")
    };
    index(control)
        .checked_sub(index(&PatchControlId::Output(
            PatchOutputParameter::TrimGain,
        )))
        .expect("the scene walks the declared Utility order downward")
}

pub fn from_installed_state(tree: &StateTree) -> Result<LiveDemoScene, LiveDemoSceneError> {
    let base = LiveDemoScene::from_installed_state(tree)?;
    let state = decode_state_tree(tree)?;

    // Positional registry entries; the base scene requires exactly three.
    let entries: Vec<EffectCapabilityId> = state
        .effects
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.id().clone())
        .collect();
    if entries.len() < 3 {
        return Err(LiveDemoSceneError::InvalidEffectConfig);
    }
    let (first_entry, second_entry, third_entry) =
        (entries[0].clone(), entries[1].clone(), entries[2].clone());

    // The demonstration subject: the first installed Patch, whose stem and
    // effect chain the production audio observation correlates per block.
    let subject = state
        .patches
        .first()
        .ok_or(LiveDemoSceneError::NoInstalledPatches)?;
    let subject_id = PatchId::new(subject.id).map_err(|_| LiveDemoSceneError::InvalidPatchId)?;
    let subject_channel = MidiChannel::new(subject.channel)
        .map_err(|_| LiveDemoSceneError::InvalidMidiChannel(subject.channel))?;
    let subject_track = subject.output.track_id();
    let absent = EffectCapabilityId::new(ABSENT_ENTRY_ID)
        .map_err(|_| LiveDemoSceneError::InvalidEffectConfig)?;

    let slot = |index: usize| EffectSlotIndex::ALL[index];
    // Declared expected results, never dispatches: a journey transition
    // pairs one of these with an adjacent-choice gesture direction, and the
    // runner dispatches the gesture while verifying the committed occupancy
    // against the declaration. The only occupancy action dispatched directly
    // is the documented rejection below.
    let occupy =
        |slot_index: usize, entry: Option<EffectCapabilityId>| SemanticAction::SetSlotOccupancy {
            patch_id: subject_id,
            slot: slot(slot_index),
            entry,
        };
    let occupy_return = |bus: BusId, entry: Option<EffectCapabilityId>| {
        SemanticAction::SetReturnOccupancy { bus, entry }
    };

    // The player's slot journey: enter the PATCH context, walk focus to the
    // slot's occupancy row, prove the landing against the canonical
    // projection, and arm Edit mode for the adjacent-choice gesture. The
    // occupancy change itself is the transition's measured Adjust dispatch.
    let slot_journey = |slot_index: usize| {
        let control = PatchControlId::EffectSlot(slot(slot_index));
        vec![
            LiveTopologySupport::Event {
                event: AppEvent::SelectContext(TopLevelContext::Patch),
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
    // Leaving Edit mode closes every PATCH-side edit journey.
    let patch_edit_exit = || {
        vec![LiveTopologySupport::Event {
            event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
        }]
    };
    // The player's return journey: select the MIXER context, focus the
    // subject's track, enter the Inspector, walk focus to the return's
    // occupancy row, prove the landing, and arm Edit mode.
    let return_journey = |bus: BusId| {
        let control = MixerControlId::ReturnOccupancy { bus };
        vec![
            LiveTopologySupport::Event {
                event: AppEvent::SelectContext(TopLevelContext::Mixer),
            },
            LiveTopologySupport::FocusMixerTrack {
                track_id: subject_track,
            },
            LiveTopologySupport::Event {
                event: AppEvent::EnterSurface(SurfaceId::MixerInspector),
            },
            LiveTopologySupport::FocusInspectorControl {
                track_id: subject_track,
                control: control.clone(),
            },
            LiveTopologySupport::VerifyMixerFocus { control },
            LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
            },
        ]
    };
    // Leaving Edit mode and returning to MIXER Main closes every
    // Inspector-side edit journey.
    let return_edit_exit = || {
        vec![
            LiveTopologySupport::Event {
                event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
            },
            LiveTopologySupport::Event {
                event: AppEvent::Return,
            },
        ]
    };

    // The startup occupant's first visible descriptor scalar, edited from
    // the PATCH page as its own audible transition. The row identity and an
    // accepted fine-step direction derive from the installed descriptor and
    // config — positional and value-driven, never a name.
    let startup_occupant = subject
        .post_effects
        .first()
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    let occupant_descriptor = state
        .effects
        .descriptor(startup_occupant.capability_id())
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    let occupant_predicate_satisfied = |predicate: Option<&crate::synth::ParameterPredicate>| {
        predicate.is_none_or(|predicate| {
            startup_occupant.value(predicate.parameter_id()) == Some(predicate.equals())
        })
    };
    let scalar_spec = occupant_descriptor
        .parameters()
        .find(|spec| {
            spec.patch_interaction() == crate::synth::PatchInteraction::ScalarEdit
                && spec.update() == crate::synth::ParameterUpdate::Scalar
                && occupant_predicate_satisfied(spec.visible_when())
                && occupant_predicate_satisfied(spec.enabled_when())
        })
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    let scalar_current = startup_occupant
        .value(scalar_spec.id())
        .ok_or(LiveDemoSceneError::InvalidEffectConfig)?;
    let scalar_direction = match scalar_spec.adjusted_scalar_value(
        scalar_current,
        crate::synth::ParameterAdjustment::FineIncrease,
    ) {
        Ok(next) if &next != scalar_current => Direction::Right,
        _ => Direction::Left,
    };
    let occupant_scalar_row =
        PatchControlId::Effect(startup_occupant.slot_id(), scalar_spec.id().clone());
    let scalar_journey = vec![
        LiveTopologySupport::Event {
            event: AppEvent::SelectContext(TopLevelContext::Patch),
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
    ];

    // The send-raise support script. The frozen post-scalar MIXER context
    // still selects the subject's track (the base scene restores it), and
    // Inspector entry always focuses the selected track's first indexed
    // send, so the walk below is deterministic and loudly verified.
    // The MIXER context is selected when this script runs: the preceding
    // slot journey's exit restores it.
    let raised_buses = [BusId::ALL[0], BusId::ALL[1], BusId::ALL[2]];
    let mut raise_sends = vec![
        LiveTopologySupport::FocusMixerTrack {
            track_id: subject_track,
        },
        LiveTopologySupport::Event {
            event: AppEvent::EnterSurface(SurfaceId::MixerInspector),
        },
    ];
    for bus in raised_buses {
        if bus.index() > 0 {
            raise_sends.push(LiveTopologySupport::Event {
                event: AppEvent::Navigate(Direction::Down),
            });
        }
        raise_sends.push(LiveTopologySupport::VerifyMixerFocus {
            control: MixerControlId::Send {
                track_id: subject_track,
                bus,
            },
        });
        raise_sends.push(LiveTopologySupport::Event {
            event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
        });
        for _ in 0..4 {
            raise_sends.push(LiveTopologySupport::Event {
                event: AppEvent::Adjust(Direction::Up),
            });
        }
        raise_sends.push(LiveTopologySupport::Event {
            event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
        });
    }
    raise_sends.push(LiveTopologySupport::Event {
        event: AppEvent::Return,
    });

    // The mirror script lowering the same sends back to their frozen zeros.
    let mut lower_sends = vec![
        LiveTopologySupport::FocusMixerTrack {
            track_id: subject_track,
        },
        LiveTopologySupport::Event {
            event: AppEvent::EnterSurface(SurfaceId::MixerInspector),
        },
    ];
    for bus in raised_buses {
        if bus.index() > 0 {
            lower_sends.push(LiveTopologySupport::Event {
                event: AppEvent::Navigate(Direction::Down),
            });
        }
        lower_sends.push(LiveTopologySupport::VerifyMixerFocus {
            control: MixerControlId::Send {
                track_id: subject_track,
                bus,
            },
        });
        lower_sends.push(LiveTopologySupport::Event {
            event: AppEvent::SetInteractionMode(InteractionMode::Adjust),
        });
        for _ in 0..4 {
            lower_sends.push(LiveTopologySupport::Event {
                event: AppEvent::Adjust(Direction::Down),
            });
        }
        lower_sends.push(LiveTopologySupport::Event {
            event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
        });
    }
    lower_sends.push(LiveTopologySupport::Event {
        event: AppEvent::Return,
    });

    let transitions = vec![
        // The startup occupant's descriptor scalar edited from the PATCH
        // page: journey to the occupant's parameter row, one accepted
        // fine-step edit, audibly witnessed on the sounding chain.
        LiveTopologyTransition::new(
            "SlotOccupant.scalarEdited",
            None,
            Some(scalar_direction),
            None,
            subject_id,
            subject_channel,
            60,
            scalar_journey,
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        // FR-003/AS-1.5 (voice carry-over): clearing the startup occupant
        // travels the full lifecycle while the requesting note is HELD — it
        // rings through preparation, survives the block-boundary activation
        // via voice carry-over, and is never re-sounded, so the
        // dry-continuity witness measures the carried voice itself. The
        // clear is the adjacent-choice gesture from the focused slot row.
        LiveTopologyTransition::new(
            "Slot.startupOccupantCleared",
            Some(occupy(0, None)),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            60,
            slot_journey(0),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::DryContinuity,
        )
        .holding_note_through_activation(),
        // FR-002/FR-004: fill all three slots, one at a time, each audibly,
        // each from the focused slot row. The adjacent-choice gesture is
        // nonwrapping — empty, then each installed entry in declared order —
        // so a non-adjacent target entry is reached by cycling through the
        // intervening entries; each pass is its own added audible
        // transition, and the retained identity lands on the exact
        // occupancy the scene has always proven.
        LiveTopologyTransition::new(
            "SlotFill.first",
            Some(occupy(0, Some(first_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            60,
            slot_journey(0),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "SlotFill.secondCycle1",
            Some(occupy(1, Some(first_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            62,
            slot_journey(1),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "SlotFill.second",
            Some(occupy(1, Some(second_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            62,
            slot_journey(1),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "SlotFill.thirdCycle1",
            Some(occupy(2, Some(first_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            64,
            slot_journey(2),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "SlotFill.thirdCycle2",
            Some(occupy(2, Some(second_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            64,
            slot_journey(2),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "SlotFill.third",
            Some(occupy(2, Some(third_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            64,
            slot_journey(2),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        // FR-005/SC-002: exchange two genuinely different effects.
        LiveTopologyTransition::new(
            "SlotOrder.exchangeFirst",
            Some(occupy(0, Some(second_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            65,
            slot_journey(0),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "SlotOrder.exchangeSecond",
            Some(occupy(1, Some(first_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            65,
            slot_journey(1),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        // FR-006/SC-003: two instances of one registry entry (positions 0
        // and 2 both carry the third entry after this transition). The exit
        // also restores the MIXER context for the send-raise script.
        LiveTopologyTransition::new(
            "SlotTwin.sameEntry",
            Some(occupy(0, Some(third_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            67,
            slot_journey(0),
            vec![
                LiveTopologySupport::Event {
                    event: AppEvent::SetInteractionMode(InteractionMode::Navigate),
                },
                LiveTopologySupport::Event {
                    event: AppEvent::SelectContext(TopLevelContext::Mixer),
                },
            ],
            LiveTopologyAudibleWitness::PatchChain,
        ),
        // FR-010/FR-011: raise sends toward several of the eight
        // destinations through the focused Inspector controls.
        LiveTopologyTransition::new(
            "Send.towardDestinations",
            None,
            None,
            None,
            subject_id,
            subject_channel,
            69,
            raise_sends,
            Vec::new(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        // FR-013: change the effect occupying an already-occupied return,
        // from the focused Inspector return row, cycling adjacent choices.
        LiveTopologyTransition::new(
            "Return.contentChangedCycle1",
            Some(occupy_return(BusId::ALL[1], Some(second_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            69,
            return_journey(BusId::ALL[1]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        LiveTopologyTransition::new(
            "Return.contentChanged",
            Some(occupy_return(BusId::ALL[1], Some(first_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            69,
            return_journey(BusId::ALL[1]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        // FR-013/C-BR-6: occupy a previously empty destination the raised
        // send already feeds.
        LiveTopologyTransition::new(
            "Return.emptyOccupiedCycle1",
            Some(occupy_return(BusId::ALL[2], Some(first_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            69,
            return_journey(BusId::ALL[2]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        LiveTopologyTransition::new(
            "Return.emptyOccupiedCycle2",
            Some(occupy_return(BusId::ALL[2], Some(second_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            69,
            return_journey(BusId::ALL[2]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        LiveTopologyTransition::new(
            "Return.emptyOccupied",
            Some(occupy_return(BusId::ALL[2], Some(third_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            69,
            return_journey(BusId::ALL[2]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        // FR-015/SC-006: one controlled rejection with a visible reason and
        // uninterrupted audio.
        //
        // This dispatch stays a direct semantic injection by design: the
        // adjacent-choice gesture on a focused occupancy row can only reach
        // the installed registry entries and empty, so an unknown registry
        // entry is inexpressible through the UI. Requesting one therefore
        // requires the injection below — the single sanctioned exception to
        // the journey-driven occupancy contract.
        LiveTopologyTransition::new(
            "Topology.refused",
            Some(SemanticAction::SetSlotOccupancy {
                patch_id: subject_id,
                slot: slot(1),
                entry: Some(absent),
            }),
            None,
            Some("invalidEffectConfig"),
            subject_id,
            subject_channel,
            71,
            Vec::new(),
            Vec::new(),
            LiveTopologyAudibleWitness::DryContinuity,
        ),
        // SC-006: a valid change immediately after the rejection succeeds —
        // restoring the changed return's startup occupant through the same
        // Inspector journey.
        LiveTopologyTransition::new(
            "Topology.recoveredAfterRefusalCycle1",
            Some(occupy_return(BusId::ALL[1], Some(second_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            71,
            return_journey(BusId::ALL[1]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        LiveTopologyTransition::new(
            "Topology.recoveredAfterRefusal",
            Some(occupy_return(BusId::ALL[1], Some(third_entry.clone()))),
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            71,
            return_journey(BusId::ALL[1]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        // FR-018: reroute the subject Patch through the focused Utility
        // route control and show its chain follows; the mirror adjustment
        // restores the route after the checkpoint.
        LiveTopologyTransition::new(
            "Reroute.chainFollows",
            None,
            Some(Direction::Right),
            None,
            subject_id,
            subject_channel,
            72,
            vec![
                LiveTopologySupport::Event {
                    event: AppEvent::SelectContext(TopLevelContext::Patch),
                },
                LiveTopologySupport::Event {
                    event: AppEvent::EnterSurface(SurfaceId::PatchUtility),
                },
            ]
            .into_iter()
            // Walk to the output-track row through the declared five-row
            // Utility order rather than assuming it sits one step below the
            // entry row: MIDI input is seated between the two.
            .chain(
                std::iter::repeat_with(|| LiveTopologySupport::Event {
                    event: AppEvent::Navigate(Direction::Down),
                })
                .take(utility_rows_below_entry(&PatchControlId::Output(
                    PatchOutputParameter::OutputTrack,
                ))),
            )
            .chain(std::iter::once(LiveTopologySupport::VerifyPatchFocus {
                control: PatchControlId::Output(PatchOutputParameter::OutputTrack),
            }))
            .collect::<Vec<_>>(),
            vec![
                LiveTopologySupport::Event {
                    event: AppEvent::Adjust(Direction::Left),
                },
                LiveTopologySupport::Event {
                    event: AppEvent::Return,
                },
            ],
            LiveTopologyAudibleWitness::PatchChain,
        ),
        // Restore the subject grid to the startup configuration: the first
        // entry back at its startup position, the other slots cleared, so
        // the frozen teardown's structural-restoration check observes the
        // exact pre-engine baseline — every step from the focused slot row.
        LiveTopologyTransition::new(
            "Slot.startupOccupantRestoredCycle1",
            Some(occupy(0, Some(second_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            74,
            slot_journey(0),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "Slot.startupOccupantRestored",
            Some(occupy(0, Some(first_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            74,
            slot_journey(0),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "Slot.secondCleared",
            Some(occupy(1, None)),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            74,
            slot_journey(1),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::DryContinuity,
        ),
        LiveTopologyTransition::new(
            "Slot.thirdClearedCycle1",
            Some(occupy(2, Some(second_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            74,
            slot_journey(2),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "Slot.thirdClearedCycle2",
            Some(occupy(2, Some(first_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            74,
            slot_journey(2),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::PatchChain,
        ),
        LiveTopologyTransition::new(
            "Slot.thirdCleared",
            Some(occupy(2, None)),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            74,
            slot_journey(2),
            patch_edit_exit(),
            LiveTopologyAudibleWitness::DryContinuity,
        ),
        // Restore the raised sends and the extra destination so the frozen
        // semantic tail observes the documented end state. The clear cycles
        // back down through the intervening entries from the focused row.
        LiveTopologyTransition::new(
            "Return.emptyRestoredCycle1",
            Some(occupy_return(BusId::ALL[2], Some(second_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            76,
            return_journey(BusId::ALL[2]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        LiveTopologyTransition::new(
            "Return.emptyRestoredCycle2",
            Some(occupy_return(BusId::ALL[2], Some(first_entry.clone()))),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            76,
            return_journey(BusId::ALL[2]),
            return_edit_exit(),
            LiveTopologyAudibleWitness::WetReturn,
        ),
        LiveTopologyTransition::new(
            "Return.emptyRestored",
            Some(occupy_return(BusId::ALL[2], None)),
            Some(Direction::Left),
            None,
            subject_id,
            subject_channel,
            76,
            return_journey(BusId::ALL[2]),
            {
                let mut exit = return_edit_exit();
                exit.extend(lower_sends);
                exit
            },
            LiveTopologyAudibleWitness::DryContinuity,
        ),
    ];
    // C-004 (mission webview-shell-cutover): the frozen pre-journey topology
    // checkpoint-identity baseline survives the shell cutover byte-identically
    // and in order; every other identity declared above (the journey-era
    // Cycle insertions and any later webview-era addition) must be a pure
    // insertion around its members. Asserted at declaration time so every
    // deterministic twin and live run fails at scene construction — before
    // rig time is spent — if the frozen identity set regresses.
    assert!(
        crate::testing::live_demo_scene::frozen_identity_baseline_is_preserved(
            transitions.iter().map(LiveTopologyTransition::identifier),
            &FROZEN_TOPOLOGY_IDENTITY_BASELINE,
        ),
        "the frozen topology checkpoint-identity baseline must be preserved byte-identically and in order",
    );

    Ok(base.with_topology_extension(
        EFFECTS_AND_BUSES_SCENE_NAME,
        transitions,
        EFFECTS_AND_BUSES_TOTAL_TIMEOUT,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_entry_id_is_namespaced() {
        let id = EffectCapabilityId::new(ABSENT_ENTRY_ID).unwrap();
        assert_eq!(id.as_str(), "effect.absent");
    }
}
