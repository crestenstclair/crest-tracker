// Source-only adapter for the MIT mda DeEsser DSP; no VST3 SDK or host ABI.
#pragma once
#include <vector>
#include <cstdint>
#define SMTG_OVERRIDE override
#define PLUGIN_API
#define DECLARE_UID(name,a,b,c,d) int name = 0
#define USTRING(text) text
namespace Steinberg {
using int32 = int32_t;
using TBool = bool;
using tresult = int;
struct FUnknown { virtual ~FUnknown() = default; };
namespace Vst {
using IAudioProcessor = FUnknown;
constexpr tresult kResultTrue=0;
namespace SpeakerArr { constexpr int kStereo=2; }
struct AudioBusBuffers { float** channelBuffers32; };
struct ProcessData { int32 numSamples; AudioBusBuffers* inputs; AudioBusBuffers* outputs; };
namespace mda {
class BaseProcessor : public IAudioProcessor {
protected:
    std::vector<double> params;
    void allocParameters(int n) { params.resize(n); }
    void setControllerClass(int) {}
    void addAudioInput(const char*,int) {}
    void addAudioOutput(const char*,int) {}
    virtual void recalculate() = 0;
public:
    virtual int32 getVst2UniqueId() const { return 0; }
    virtual tresult initialize(FUnknown*) { return kResultTrue; }
    virtual tresult terminate() { return kResultTrue; }
    virtual tresult setActive(TBool) { return kResultTrue; }
    virtual tresult setProcessing(TBool) { return kResultTrue; }
    virtual void doProcessing(ProcessData&) = 0;
};
}
}
}
