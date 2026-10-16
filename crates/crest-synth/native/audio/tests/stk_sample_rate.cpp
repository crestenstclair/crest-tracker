#include "processor.h"
#include "processor_state.h"
#include "Stk.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstring>

#define CREST_FACTORY(symbol, instrument, label) CrestProcessor* make_##symbol(float,size_t);
#include "stk_factories.inc"
#undef CREST_FACTORY

namespace {
struct Factory { const char* name; CrestProcessor* (*make)(float,size_t); };
const Factory factories[]={
#define CREST_FACTORY(symbol, instrument, label) {#symbol,make_##symbol},
#include "stk_factories.inc"
#undef CREST_FACTORY
};
struct Voice {
    CrestProcessorState state;
    std::unique_ptr<CrestProcessor> processor;
    Voice(const Factory& factory,double rate) {
        state.sample_rate=rate;
        CrestProcessorScope scope(state);
        processor.reset(factory.make(rate,64));
        for(size_t i=0;i<processor->count();++i) processor->set(i,processor->initial(i));
    }
    void render(size_t block,float* output) {
        CrestProcessorScope scope(state);
        if(block%32==0) {
            processor->reset();
            for(size_t i=0;i<processor->count();++i) processor->set(i,processor->initial(i));
            processor->note(0xe0,0,64);
            processor->note(0x90,36+(block/32)*17,100);
        }
        if(block%32==13) processor->note(0xe0,17,76);
        if(block%32==21) processor->note(0x80,36+(block/32)*17,0);
        processor->process(output,output+64,64);
    }
};
}

bool stk_sample_rate_witness() {
    if(!crest_processor_initialize()) return false;
    constexpr double rates[]={22050,44100,48000,96000,192000};
    constexpr size_t blocks=96;
    for(const auto& factory:factories) {
        std::array<std::vector<float>,5> reference;
        for(size_t r=0;r<5;++r) {
            // No prepared objects survive a global rate change. This reference
            // exercises STK's original global-rate API and equations.
            stk::Stk::setSampleRate(rates[r]);
            Voice voice(factory,0);
            reference[r].resize(blocks*128);
            for(size_t b=0;b<blocks;++b) voice.render(b,reference[r].data()+b*128);
        }
        stk::Stk::setSampleRate(44100);
        std::array<std::unique_ptr<Voice>,5> voices;
        for(size_t r=0;r<5;++r) voices[r]=std::make_unique<Voice>(factory,rates[r]);
        for(size_t b=0;b<blocks;++b) for(size_t r=0;r<5;++r) {
            float output[128]{};
            voices[r]->render(b,output);
            if(!voices[r]->processor->healthy() ||
               !std::all_of(output,output+128,[](float x){return std::isfinite(x);}) ||
               std::memcmp(output,reference[r].data()+b*128,sizeof(output))) {
                std::printf("STK rate mismatch %s %.0f block %zu\n",factory.name,rates[r],b);
                return false;
            }
        }
    }
    std::printf("STK sample rates: interleaved instances match upstream global-rate rendering\n");
    return true;
}
