#include "parameter_adapter.h"
// r8brain includes SSE intrinsics at global scope. Load it before FFTConvolver,
// whose upstream header includes xmmintrin.h inside its own namespace.
#include "CDSPResampler.h"
#include "fftconvolver/FFTConvolver.h"

class ConvolutionEffect final : public ParameterAdapter {
    fftconvolver::FFTConvolver left_,right_;
    float rate_; size_t frames_; bool loaded_=false;
    std::vector<float> left_output_,right_output_;
public:
    ConvolutionEffect(float rate,size_t frames):ParameterAdapter({{"Output Gain (dB)",0,-24,24},{"Dry Wet",1}}),rate_(rate),frames_(frames),left_output_(frames),right_output_(frames) {
        const float impulse=1;
        loaded_=left_.init(frames_,&impulse,1)&&right_.init(frames_,&impulse,1);
    }
    bool load_ir(const float* pcm,size_t frames,size_t channels,float rate) override {
        if(!frames || (channels!=1&&channels!=2) || !std::isfinite(rate)||rate<8000) return false;
        std::vector<float> ir[2];
        for(size_t channel=0;channel<2;++channel) {
            std::vector<double> input(frames),output;
            for(size_t i=0;i<frames;++i) {
                const float value=pcm[i*channels+std::min(channel,channels-1)];
                if(!std::isfinite(value))return false; input[i]=value;
            }
            if(rate==rate_) ir[channel].assign(input.begin(),input.end());
            else {
                r8b::CDSPResampler24 converter(rate,rate_,frames);
                const size_t required=static_cast<size_t>(std::ceil(frames*double(rate_)/rate));
                double* converted; int count=converter.process(input.data(),frames,converted);
                output.insert(output.end(),converted,converted+count);
                std::fill(input.begin(),input.end(),0);
                while(output.size()<required){count=converter.process(input.data(),frames,converted);output.insert(output.end(),converted,converted+count);}
                ir[channel].assign(output.begin(),output.begin()+required);
            }
        }
        loaded_=left_.init(frames_,ir[0].data(),ir[0].size())&&right_.init(frames_,ir[1].data(),ir[1].size());
        return loaded_;
    }
    bool healthy() const override{return loaded_;}
    void process(float* left,float* right,size_t frames) override {
        if(!loaded_)return;
        left_.process(left,left_output_.data(),frames);right_.process(right,right_output_.data(),frames);
        const float gain=std::pow(10.f,values_[0]/20),wet=values_[1];
        for(size_t i=0;i<frames;++i){left[i]=gain*(wet*left_output_[i]+(1-wet)*left[i]);right[i]=gain*(wet*right_output_[i]+(1-wet)*right[i]);}
    }
};
CrestProcessor* make_fft_Convolver(float rate,size_t frames){return new ConvolutionEffect(rate,frames);}
