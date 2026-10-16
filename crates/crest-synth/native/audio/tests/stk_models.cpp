#include "BandedWG.h"
#include "Mesh2D.h"
#include "banded_delay.h"
#include "stk_sequence.h"
#include <cstdio>
#include <cstring>

bool stk_model_witness() {
    // Exercise wraparound, fractional delays, repeated clear, and growth after
    // use independently of BandedWG's integral delay settings.
    for (unsigned capacity : {1,2,3,32,4095,16384}) {
        stk::DelayL reference;
        stk::BandedDelay actual;
        reference.setMaximumDelay(capacity); actual.setMaximumDelay(capacity);
        for (unsigned step=0; step<100000; ++step) {
            if (step%7919==0 || (step%7==0 && step<500)) {
                reference.clear(); actual.clear();
            }
            if (step==50000) {
                reference.setMaximumDelay(32768); actual.setMaximumDelay(32768);
            }
            if (step%19==0) {
                const double delay=std::fmod(step*.113,capacity);
                reference.setDelay(delay); actual.setDelay(delay);
            }
            const double input=std::sin(step*.31);
            const double a=actual.tick(input), b=reference.tick(input);
            if (std::memcmp(&a,&b,sizeof(a))) {
                std::printf("STK delay mismatch %u/%u: %.18g != %.18g\n",capacity,step,a,b);
                return false;
            }
        }
    }
    const auto reference=stk_reference_sequence();
    const auto actual=stk_model_sequence<stk::BandedWG,stk::Mesh2D>();
    if (actual.size()!=reference.size()) return false;
    for (size_t i=0; i<actual.size(); ++i) {
        if (!std::isfinite(actual[i]) || std::memcmp(&actual[i],&reference[i],sizeof(double))) {
            std::printf("STK model mismatch %zu: %.18g != %.18g\n",i,actual[i],reference[i]);
            return false;
        }
    }
    std::printf("STK models: %zu bit-identical samples\n",actual.size());
    return true;
}
