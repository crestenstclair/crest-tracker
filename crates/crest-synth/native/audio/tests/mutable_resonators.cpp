#include "rings/dsp/resonator.h"
#include "elements/dsp/resonator.h"
#include "mutable_sequence.h"
#include <cstdio>

bool mutable_resonator_witness() {
    const auto reference=mutable_reference_sequence();
    const auto actual=mutable_resonator_sequence<rings::Resonator,elements::Resonator>();
    if(actual.size()!=reference.size()) return false;
    float worst=0;
    for(size_t i=0; i<actual.size(); ++i) {
        const float error=std::abs(actual[i]-reference[i]);
        if(!std::isfinite(actual[i]) || !std::isfinite(reference[i]) || error>2e-6f) {
            std::printf("MUTABLE resonator mismatch %zu: %.9g != %.9g\n",i,actual[i],reference[i]);
            return false;
        }
        worst=std::max(worst,error);
    }
    std::printf("Mutable resonators: %zu samples, max error %.9g\n",actual.size(),worst);
    return true;
}
