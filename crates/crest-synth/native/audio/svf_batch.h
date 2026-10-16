#pragma once
#include "stmlib/dsp/filter.h"
#include <cmath>

namespace stmlib {
// Borrow a resonator's independent SVFs for one native block. Separate arrays
// let the compiler vectorize the original per-mode equations without changing
// their order or the caller's ordered sum. Capacity is the upstream model's
// mode count, not a limit on host voices. All storage is bounded stack scratch.
template<size_t capacity> class SvfBatch {
    alignas(16) float g_[capacity], r_[capacity], h_[capacity];
    alignas(16) float state_1_[capacity], state_2_[capacity];
    Svf* filters_;
    const size_t count_;
    bool silent_ = true;
public:
    SvfBatch(Svf* filters, size_t count): filters_(filters), count_(count) {
        for (size_t i=0; i<count_; ++i) {
            g_[i]=filters[i].g_; r_[i]=filters[i].r_; h_[i]=filters[i].h_;
            state_1_[i]=filters[i].state_1_; state_2_[i]=filters[i].state_2_;
            silent_ &= state_1_[i]==0 && state_2_[i]==0
                && std::isfinite(g_[i]) && std::isfinite(r_[i]) && std::isfinite(h_[i]);
        }
    }
    // False means exact silence and leaves output untouched. Oscillator/control
    // clocks outside the filters must still advance. Never truncate a tail.
    bool Process(float input, float* output) {
        if (silent_ && input==0) return false;
        silent_=false;
        for (size_t i=0; i<count_; ++i) {
            const float hp=(input-r_[i]*state_1_[i]-g_[i]*state_1_[i]-state_2_[i])*h_[i];
            const float bp=g_[i]*hp+state_1_[i];
            state_1_[i]=g_[i]*hp+bp;
            const float lp=g_[i]*bp+state_2_[i];
            state_2_[i]=g_[i]*bp+lp;
            output[i]=bp;
        }
        return true;
    }
    void Store() {
        for (size_t i=0; i<count_; ++i) {
            filters_[i].state_1_=state_1_[i]; filters_[i].state_2_=state_2_[i];
        }
    }
};
}
