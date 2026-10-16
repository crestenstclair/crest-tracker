#pragma once
#include <algorithm>
#include <cmath>
#include <vector>

struct ResamplerResult {
    std::vector<double> samples;
    std::vector<int> counts;
    int required = 0;
};

// Exercise silence, impulses, a wideband signal, tails, partial blocks, and
// reset of a used converter. The reference compiles the retained upstream
// source in a separate translation unit/namespace.
template<class Resampler>
ResamplerResult resampler_sequence(Resampler& resampler) {
    ResamplerResult result;
    result.required = resampler.getInputRequiredForOutput(257);
    double input[257];
    constexpr size_t sizes[] = {1, 17, 127, 256, 31, 257, 64};
    for (size_t pass = 0; pass < 2; ++pass) {
        resampler.clear();
        size_t block = 0;
        for (size_t pos = 0; pos < 32768;) {
            const size_t count = std::min(sizes[block++ % 7], 32768 - pos);
            for (size_t i = 0; i < count; ++i) {
                const size_t t = pos + i;
                input[i] = t == 4096 ? 0.75 : t >= 8192 && t < 16384
                    ? 0.2 * std::sin(t * 0.172 + pass) + 0.1 * std::cos(t * 2.71) : 0;
            }
            double* output;
            const int available = resampler.process(input, int(count), output);
            result.counts.push_back(available);
            result.samples.insert(result.samples.end(), output, output + available);
            pos += count;
        }
    }
    return result;
}

ResamplerResult r8brain_reference(double source, double destination);
