#include "CDSPResampler.h"
#include "r8brain_sequence.h"
#include <cstdio>

bool r8brain_scheduling_witness() {
    double worst = 0;
    size_t checked = 0;
    for (double source : {8000., 16000., 22050., 32000., 44100., 48000., 88200., 96000., 192000.}) {
        for (double destination : {8000., 16000., 22050., 32000., 44100., 48000., 88200., 96000., 192000.}) {
            const auto reference = r8brain_reference(source, destination);
            for (unsigned phase : {0u, 1u, 0x9e3779b9u, 0xffffffffu}) {
                r8b::CDSPResampler24 resampler(source, destination, 257, 2.0, phase);
                const auto actual = resampler_sequence(resampler);
                if (actual.required != reference.required || actual.counts != reference.counts
                    || actual.samples.size() != reference.samples.size()) {
                    std::printf("R8B timing mismatch %.0f/%.0f phase=%u\n", source, destination, phase);
                    return false;
                }
                for (size_t i = 0; i < actual.samples.size(); ++i) {
                    const double error = std::abs(actual.samples[i] - reference.samples[i]);
                    if (!std::isfinite(actual.samples[i]) || error > 1e-12) {
                        std::printf("R8B signal mismatch %.0f/%.0f phase=%u sample=%zu error=%.12g\n",
                            source, destination, phase, i, error);
                        return false;
                    }
                    worst = std::max(worst, error);
                }
                ++checked;
            }
        }
    }
    std::printf("R8B scheduling: %zu comparisons, max error %.12g\n", checked, worst);
    return true;
}
