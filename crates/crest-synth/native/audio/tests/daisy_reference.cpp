// Compile the retained, unmodified upstream implementation under another
// namespace; the witness compares it with the staged production implementation.
#include "random.h"
#define daisysp crest_reference_daisy
#include "../../../vendor/audio/daisysp/Source/Filters/svf.cpp"
#include "../../../vendor/audio/daisysp/Source/PhysicalModeling/resonator.h"
#include "../../../vendor/audio/daisysp/Source/PhysicalModeling/resonator.cpp"
#include "../../../vendor/audio/daisysp/Source/Drums/analogbassdrum.cpp"
#include "../../../vendor/audio/daisysp/Source/Drums/synthbassdrum.cpp"
#include "../../../vendor/audio/daisysp/Source/PhysicalModeling/KarplusString.cpp"
#include "../../../vendor/audio/daisysp/Source/PhysicalModeling/modalvoice.cpp"
#include "../../../vendor/audio/daisysp/Source/Dynamics/crossfade.cpp"
#include "../../../vendor/audio/daisysp/Source/Utility/dcblock.cpp"
#include "daisy_sequence.h"

void daisy_reference_sequence(float* output) {
    daisy_filter_sequence<crest_reference_daisy::Svf,
        crest_reference_daisy::ResonatorSvf<4>>(output);
}

void daisy_reference_voices(float* output) {
    daisy_voice_sequence<crest_reference_daisy::AnalogBassDrum,
        crest_reference_daisy::Resonator>(output);
}

void daisy_reference_synthetic(float* output) {
    daisy_synthetic_sequence<crest_reference_daisy::SyntheticBassDrum>(output);
}

void daisy_reference_physical(float* output) {
    daisy_physical_sequence<crest_reference_daisy::String,
        crest_reference_daisy::ModalVoice>(output);
}
