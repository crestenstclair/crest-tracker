// Exercise the staged production sfizz library, including event timestamps
// which the host's usual delay-zero dispatch does not otherwise exercise.
#include "sfizz/MidiState.h"

bool sfizz_midi_flush_witness() {
    sfz::MidiState state;
    state.setSamplesPerBlock(32);
    auto is_current = [](const sfz::EventVector& events, float value) {
        return events.size() == 1 && events.front().delay == 0
            && events.front().value == value;
    };
    state.ccEvent(0, 1, .25f);
    state.pitchBendEvent(0, .5f);
    state.channelAftertouchEvent(0, .75f);
    state.polyAftertouchEvent(0, 60, .5f);
    for (int block = 0; block < 8; ++block) state.advanceTime(32);
    if (!is_current(state.getCCEvents(1), .25f)
        || !is_current(state.getPitchEvents(), .5f)
        || !is_current(state.getChannelAftertouchEvents(), .75f)
        || !is_current(state.getPolyAftertouchEvents(60), .5f)) return false;

    // Arrival order differs from timestamp order. Flush must retain the final
    // timestamp's value, including an immediate update before that event.
    state.ccEvent(12, 1, .75f);
    state.ccEvent(4, 1, .5f);
    state.ccEvent(0, 1, .125f);
    state.pitchBendEvent(3, -.5f);
    state.channelAftertouchEvent(7, .25f);
    state.polyAftertouchEvent(8, 60, .75f);
    if (state.getCCEvents(1).size() != 3) return false;
    state.advanceTime(32);
    if (!is_current(state.getCCEvents(1), .75f)
        || !is_current(state.getPitchEvents(), -.5f)
        || !is_current(state.getChannelAftertouchEvents(), .25f)
        || !is_current(state.getPolyAftertouchEvents(60), .75f)) return false;
    state.ccEvent(0, 1, .5f);
    state.flushEvents();
    if (!is_current(state.getCCEvents(1), .5f)) return false;

    state.ccEvent(10, 1, .75f);
    state.resetEventStates();
    state.flushEvents();
    if (!is_current(state.getCCEvents(1), 0.f)
        || !is_current(state.getPitchEvents(), 0.f)
        || !is_current(state.getChannelAftertouchEvents(), 0.f)
        || !is_current(state.getPolyAftertouchEvents(60), 0.f)) return false;
    state.ccEvent(9, 1, .25f);
    state.advanceTime(32);
    return is_current(state.getCCEvents(1), .25f);
}
