#include "parameter_adapter.h"
#include "stretch/signalsmith-stretch.h"

class SignalsmithEffect final : public ParameterAdapter {
    signalsmith::stretch::SignalsmithStretch<float> stretch_{1};
    std::vector<float> output_left_,output_right_;
public:
    SignalsmithEffect(float rate,size_t frames): ParameterAdapter({
        {"Transpose (semitones)",0,-24,24},{"Formant (semitones)",0,-24,24},{"Preserve Formants",0,0,1,true}}),
        output_left_(frames),output_right_(frames) { stretch_.presetDefault(2,rate); }
    size_t latency() const override { return stretch_.inputLatency()+stretch_.outputLatency(); }
    void set(size_t i,float v) override {
        values_[i]=v;
        stretch_.setTransposeSemitones(values_[0]);
        stretch_.setFormantSemitones(values_[1],values_[2]!=0);
    }
    void process(float* left,float* right,size_t frames) override {
        const float* input[]={left,right}; float* output[]={output_left_.data(),output_right_.data()};
        stretch_.process(input,frames,output,frames);
        std::copy_n(output[0],frames,left);std::copy_n(output[1],frames,right);
    }
    void reset() override { stretch_.reset(); }
};
CrestProcessor* make_signalsmith_Stretch(float rate,size_t frames) { return new SignalsmithEffect(rate,frames); }
