#pragma once
#include "DelayL.h"
#include <algorithm>

namespace stk {
// BandedWG only needs scalar ticks, delay changes, and clear. Every write is
// contiguous around the ring; untouched capacity is already zero. Retain the
// original DelayL arithmetic, pointers, and cached output across each clear.
// Private inheritance keeps untracked tap/frame writes out of this interface.
class BandedDelay : private DelayL {
    unsigned long written_ = 0;
public:
    using DelayL::setDelay;
    using DelayL::getDelay;
    using DelayL::lastOut;

    void setMaximumDelay(unsigned long delay) {
        DelayL::setMaximumDelay(delay);
        written_ = inputs_.size();
    }
    StkFloat tick(StkFloat input) {
        written_ += written_ < inputs_.size();
        return DelayL::tick(input);
    }
    void clear() override {
        const auto size = inputs_.size();
        const auto start = (inPoint_ + size - written_) % size;
        const auto first = std::min<unsigned long>(written_, size - start);
        if (first) std::fill_n(&inputs_[start], first, 0.0);
        if (written_ > first) std::fill_n(&inputs_[0], written_ - first, 0.0);
        written_ = 0;
        for (unsigned i=0; i<outputs_.size(); ++i) outputs_[i]=0.0;
        for (unsigned i=0; i<lastFrame_.size(); ++i) lastFrame_[i]=0.0;
    }
};
}
