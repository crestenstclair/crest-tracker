#include "parameter_adapter.h"
#include "rate_adapter.h"
#include "synth.h"
#include "controllers.h"
#include "dx7note.h"
#include "patch.h"
#include "freqlut.h"
#include "exp2.h"
#include "sin.h"
#include "lfo.h"
#include "pitchenv.h"
#include "crest_msfa_default.h"
#include <mutex>
#include <array>
#include <cstring>

class MsfaVoice final : public ParameterAdapter {
    Dx7Note voice_{};
    Lfo lfo_{};
    Controllers controllers_{};
    std::array<char,156> patch_{};
    std::array<int32_t,N> output_{};
    bool active_ = false;
public:
    MsfaVoice(): ParameterAdapter({}) {
        static std::once_flag tables;
        std::call_once(tables, [] {
            Freqlut::init(48000); Exp2::init(); Tanh::init(); Sin::init();
            Lfo::init(48000); PitchEnv::init(48000);
        });
        UnpackPatch(crest_msfa_default,patch_.data());
        controllers_.values_[kControllerPitch]=8192;
        lfo_.reset(patch_.data()+137);
    }
    bool load(const uint8_t* bytes,size_t size) override {
        if (size!=155) return false;
        std::memcpy(patch_.data(),bytes,size); patch_[155]=0x3f;
        lfo_.reset(patch_.data()+137); return true;
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            voice_.init(patch_.data(),a,b); lfo_.keydown(); active_=true;
        } else if (status==0x80 || (status==0x90 && !b)) {
            if (active_) voice_.keyup();
        } else if (status==0xe0) controllers_.values_[kControllerPitch]=a+128*b;
    }
    void reset() override { active_=false; }
    void process(float* left,float* right,size_t frames) override {
        output_.fill(0);
        if (active_) voice_.compute(output_.data(),lfo_.getsample(),lfo_.getdelay(),&controllers_);
        for (size_t i=0;i<frames;++i) left[i]=right[i]=std::clamp(output_[i]/16777216.f,-1.f,1.f);
    }
};
CrestProcessor* make_msfa_DX7(float rate,size_t frames) { return new RateAdapter(new MsfaVoice,48000,rate,N,frames,RateAdapter::Signal::MonoGenerator); }
// Off-thread format adapters reuse MSFA's own unpacker and bundled patch.
extern "C" void crest_msfa_unpack(const uint8_t* packed,uint8_t* unpacked) noexcept {
    char patch[156]; UnpackPatch(reinterpret_cast<const char*>(packed),patch);
    std::memcpy(unpacked,patch,155);
}
extern "C" void crest_msfa_default_patch(uint8_t* unpacked) noexcept {
    crest_msfa_unpack(reinterpret_cast<const uint8_t*>(crest_msfa_default),unpacked);
}
