#include "rate_adapter.h"
#include "rate_adapter_reference.h"
#include <cstdio>

namespace {
struct Generator final : CrestProcessor {
    const bool stereo;
    const bool effect;
    size_t rendered = 0;
    bool gate = false;
    float gain = 0.25f;
    explicit Generator(bool stereo, bool effect): stereo(stereo), effect(effect) {}
    size_t count() const override { return 1; }
    const char* label(size_t) const override { return "Gain"; }
    float initial(size_t) const override { return 0.25f; }
    void set(size_t, float value) override { gain=value; }
    void note(int status, int, int velocity) override { gate=status==0x90 && velocity; }
    void reset() override { rendered=0; gate=false; }
    void process(float* left, float* right, size_t frames) override {
        for (size_t i=0; i<frames; ++i,++rendered) {
            const float in_left=left[i],in_right=right[i];
            left[i]=gate ? gain*std::sin(rendered*0.172) : 0;
            right[i]=stereo ? (gate ? gain*std::cos(rendered*0.713) : 0) : left[i];
            if(effect) { left[i]+=in_right*.2f; right[i]-=in_left*.3f; }
        }
    }
};
}

bool rate_adapter_witness() {
    double worst=0;
    for (double source_rate : {32000.,44100.,48000.,96000.}) {
        for (double host_rate : {44100.,48000.,96000.}) {
            for (bool stereo : {false,true}) {
              for (bool effect : {false,true}) {
                auto* original_source=new Generator(stereo,effect);
                auto* optimized_source=new Generator(stereo,effect);
                ScalarRateAdapter original(original_source,source_rate,host_rate,24,257);
                RateAdapter optimized(optimized_source,source_rate,host_rate,24,257,
                    effect ? RateAdapter::Signal::StereoEffect :
                        stereo ? RateAdapter::Signal::StereoGenerator : RateAdapter::Signal::MonoGenerator,
                    0x9e3779b9u);
                if (original.latency()!=optimized.latency()) return false;
                for (size_t block=0; block<768; ++block) {
                    constexpr size_t sizes[]={1,17,127,256,31,257,64};
                    const size_t frames=sizes[block%7];
                    if (block%256==0) { original.reset(); optimized.reset(); }
                    if (block%53==0) { original.note(0x90,60,100); optimized.note(0x90,60,100); }
                    if (block%53==37) { original.note(0x80,60,0); optimized.note(0x80,60,0); }
                    const float gain=block<500 ? 0.25f : 0.1f;
                    original.set(0,gain); optimized.set(0,gain);
                    float left[257]{},right[257]{},test_left[257]{},test_right[257]{};
                    // A generator must ignore incoming audio on either channel.
                    std::fill(left,left+frames,0.3f); std::fill(test_left,test_left+frames,0.3f);
                    std::fill(right,right+frames,-0.2f); std::fill(test_right,test_right+frames,-0.2f);
                    original.process(left,right,frames); optimized.process(test_left,test_right,frames);
                    if (!original.healthy() || !optimized.healthy()
                        || original_source->rendered!=optimized_source->rendered) {
                        std::printf("RATE ADAPTER clock mismatch %.0f/%.0f stereo=%d block=%zu\n",
                            source_rate,host_rate,stereo,block);
                        return false;
                    }
                    for (size_t i=0; i<frames; ++i) {
                        const double error=std::max(std::abs(left[i]-test_left[i]),std::abs(right[i]-test_right[i]));
                        if (!std::isfinite(error) || error>1e-7) {
                            std::printf("RATE ADAPTER signal mismatch %.0f/%.0f stereo=%d block=%zu error=%.12g\n",
                                source_rate,host_rate,stereo,block,error);
                            return false;
                        }
                        worst=std::max(worst,error);
                    }
                }
              }
            }
        }
    }
    std::printf("Rate adapter topology: max error %.12g\n",worst);
    return true;
}
