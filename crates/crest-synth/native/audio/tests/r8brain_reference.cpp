#define r8b crest_reference_r8b
#include "../../../vendor/audio/r8brain/CDSPResampler.h"
#undef r8b
#include "r8brain_sequence.h"

ResamplerResult r8brain_reference(double source, double destination) {
    crest_reference_r8b::CDSPResampler24 resampler(source, destination, 257);
    return resampler_sequence(resampler);
}
