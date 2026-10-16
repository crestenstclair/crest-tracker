#define stmlib crest_reference_stmlib
#define rings crest_reference_rings
#define elements crest_reference_elements
#include "../../../vendor/audio/stmlib/dsp/filter.h"
#include "../../../vendor/audio/mutable/rings/dsp/resonator.cc"
#include "../../../vendor/audio/mutable/rings/resources.cc"
// Upstream firmware resource headers have process-global macro names.
#undef LUT_SINE_SIZE
#undef LUT_4_DECADES
#undef LUT_STIFFNESS
#undef LUT_FM_FREQUENCY_QUANTIZER
#undef LUT_SVF_SHIFT
#include "../../../vendor/audio/mutable/elements/dsp/resonator.cc"
#include "../../../vendor/audio/mutable/elements/resources.cc"
#undef elements
#undef rings
#undef stmlib
#include "mutable_sequence.h"

std::vector<float> mutable_reference_sequence() {
    return mutable_resonator_sequence<crest_reference_rings::Resonator,
        crest_reference_elements::Resonator>();
}
