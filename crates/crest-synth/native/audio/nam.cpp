#include "parameter_adapter.h"
#include "rate_adapter.h"
#include "nam/NAM/get_dsp.h"
#include "nam/NAM/linear.h"
#include "nam/NAM/convnet.h"
#include "nam/NAM/lstm.h"
#include "nam/NAM/container.h"
#include "nam/NAM/sequential.h"
#include "nam/NAM/wavenet/model.h"
#include "nam/NAM/wavenet/slimmable.h"
#include <mutex>
#include "crest_nam_default.h"
#include <cstring>

// Explicit references retain architecture registration units in static builds.
static void register_nam() {
    static std::once_flag once;
    std::call_once(once,[] {
        auto& registry=nam::ConfigParserRegistry::instance();
        if (!registry.has("Linear")) registry.registerParser("Linear",nam::linear::create_config);
        if (!registry.has("ConvNet")) registry.registerParser("ConvNet",nam::convnet::create_config);
        if (!registry.has("LSTM")) registry.registerParser("LSTM",nam::lstm::create_config);
        if (!registry.has("WaveNet")) registry.registerParser("WaveNet",nam::wavenet::create_config);
        if (!registry.has("SlimmableWaveNet")) registry.registerParser("SlimmableWaveNet",nam::slimmable_wavenet::create_config);
        if (!registry.has("Sequential")) registry.registerParser("Sequential",nam::sequential::create_config);
        if (!registry.has("Container")) registry.registerParser("Container",nam::container::create_config);
    });
}
class NamModel final : public ParameterAdapter {
    std::unique_ptr<nam::DSP> left_,right_;
public:
    NamModel(const nlohmann::json& config,double rate): ParameterAdapter({}) {
        left_=nam::get_dsp(config);left_->Reset(rate,64);
        if(left_->NumInputChannels()==1 && left_->NumOutputChannels()==1) {
            right_=nam::get_dsp(config);right_->Reset(rate,64);
        } else if(left_->NumInputChannels()!=2 || left_->NumOutputChannels()!=2) {
            throw std::invalid_argument("NAM requires mono or stereo input/output");
        }
    }
    void process(float* left,float* right,size_t frames) override {
        float* channels[]={left,right};
        left_->process(channels,channels,frames);
        if(right_) right_->process(channels+1,channels+1,frames);
    }
};
class NamEffect final : public ParameterAdapter {
    std::unique_ptr<RateAdapter> model_;
    float rate_;size_t frames_;
    std::vector<float> left_,right_;
public:
    NamEffect(float rate,size_t frames):ParameterAdapter({{"Input Gain (dB)",0,-24,24},{"Output Gain (dB)",0,-24,24}}),rate_(rate),frames_(frames) { register_nam(); if(!load(reinterpret_cast<const uint8_t*>(crest_nam_default),std::strlen(crest_nam_default))) throw std::invalid_argument("invalid bundled NAM example"); }
    bool load(const uint8_t* data,size_t size) override {
        auto config=nlohmann::json::parse(data,data+size);
        double rate=nam::get_sample_rate_from_nam_file(config);
        if(!std::isfinite(rate)||rate<8000) return false;
        model_=std::make_unique<RateAdapter>(new NamModel(config,rate),rate,rate_,64,frames_);
        return true;
    }
    size_t latency() const override { return model_?model_->latency():0; }
    bool healthy() const override { return model_ && model_->healthy(); }
    void process(float* left,float* right,size_t frames) override {
        if(!model_) return;
        const float input=std::pow(10.f,values_[0]/20),output=std::pow(10.f,values_[1]/20);
        for(size_t i=0;i<frames;++i){left[i]*=input;right[i]*=input;}
        model_->process(left,right,frames);
        for(size_t i=0;i<frames;++i){left[i]*=output;right[i]*=output;}
    }
};
CrestProcessor* make_nam_Model(float rate,size_t frames){return new NamEffect(rate,frames);}
