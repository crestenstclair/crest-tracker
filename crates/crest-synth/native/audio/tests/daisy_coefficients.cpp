#include "Filters/svf.h"
#include "PhysicalModeling/resonator.h"
#include "Drums/analogbassdrum.h"
#include "Drums/synthbassdrum.h"
#include "PhysicalModeling/KarplusString.h"
#include "PhysicalModeling/modalvoice.h"
#include "daisy_sequence.h"
#include <array>
#include <cmath>
#include <cstdio>

void daisy_reference_sequence(float* output);
void daisy_reference_voices(float* output);
void daisy_reference_synthetic(float* output);
void daisy_reference_physical(float* output);

bool daisy_coefficient_witness() {
    if(!crest_processor_initialize()) return false;
    std::array<float, daisy_sequence_outputs> reference{}, actual{};
    daisy_reference_sequence(reference.data());
    daisy_filter_sequence<daisysp::Svf, daisysp::ResonatorSvf<4>>(actual.data());
    for (size_t i = 0; i < actual.size(); ++i) {
        // Reference TU and production archive may use different optimization
        // profiles; permit ordinary float rounding, never nonfinite output.
        if (!std::isfinite(actual[i]) || !std::isfinite(reference[i])
            || std::abs(actual[i] - reference[i]) > 1e-6f) {
            std::printf("DAISY COEFFICIENT MISMATCH %zu: %.9g != %.9g\n",
                i, actual[i], reference[i]);
            return false;
        }
    }
    std::array<float, daisy_voice_samples*2> reference_voices{}, actual_voices{};
    daisy_reference_voices(reference_voices.data());
    daisy_voice_sequence<daisysp::AnalogBassDrum,daisysp::Resonator>(actual_voices.data());
    for(size_t i=0; i<actual_voices.size(); ++i) {
        if(!std::isfinite(actual_voices[i]) || !std::isfinite(reference_voices[i])
            || std::abs(actual_voices[i]-reference_voices[i])>1e-6f) {
            std::printf("DAISY VOICE MISMATCH %zu: %.9g != %.9g\n",
                i,actual_voices[i],reference_voices[i]);
            return false;
        }
    }
    std::array<float,8192> reference_synthetic{}, actual_synthetic{};
    daisy_reference_synthetic(reference_synthetic.data());
    daisy_synthetic_sequence<daisysp::SyntheticBassDrum>(actual_synthetic.data());
    for(size_t i=0;i<actual_synthetic.size();++i) {
        if(!std::isfinite(actual_synthetic[i]) || !std::isfinite(reference_synthetic[i])
           || std::abs(actual_synthetic[i]-reference_synthetic[i])>1e-6f) {
            std::printf("DAISY SYNTHETIC DRUM MISMATCH %zu: %.9g != %.9g\n",
                i,actual_synthetic[i],reference_synthetic[i]);
            return false;
        }
    }
    std::array<float,daisy_physical_samples*2> reference_physical{}, actual_physical{};
    daisy_reference_physical(reference_physical.data());
    daisy_physical_sequence<daisysp::String,daisysp::ModalVoice>(actual_physical.data());
    for(size_t i=0;i<actual_physical.size();++i) {
        if(!std::isfinite(actual_physical[i]) || !std::isfinite(reference_physical[i])
           || std::abs(actual_physical[i]-reference_physical[i])>1e-6f) {
            std::printf("DAISY PHYSICAL MODEL MISMATCH %zu: %.9g != %.9g\n",
                i,actual_physical[i],reference_physical[i]);
            return false;
        }
    }
    return true;
}
