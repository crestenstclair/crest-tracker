#include "parameter_adapter.h"
#include "../../vendor/audio/mda-deesser/mdaDeEsserProcessor.cpp"
class DeEsserCore final : public Steinberg::Vst::mda::DeEsserProcessor {
public:
    void parameter(size_t i,float v) { params[i]=v; recalculate(); }
};
class CrestDeEsser final : public ParameterAdapter {
    DeEsserCore core_;
public:
    CrestDeEsser(): ParameterAdapter({{"Threshold",.15},{"Frequency",.60},{"HF Drive",.50}}) {
        core_.initialize(nullptr); core_.setProcessing(true);
    }
    void set(size_t i,float v) override { core_.parameter(i,v); }
    void process(float* left,float* right,size_t frames) override {
        float* channels[]={left,right};
        Steinberg::Vst::AudioBusBuffers bus{channels};
        Steinberg::Vst::ProcessData data{static_cast<int32_t>(frames),&bus,&bus};
        core_.doProcessing(data);
    }
};
CrestProcessor* make_mda_DeEsser(float,size_t) { return new CrestDeEsser; }
