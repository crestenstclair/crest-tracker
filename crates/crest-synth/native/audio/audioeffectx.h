// SDK-free source compatibility for the embedded MIT Airwindows/mda DSP.
// This is not a VST host or a VST ABI implementation. No Steinberg SDK is used.
#pragma once
#define __audioeffect__
#include <cstdint>
#include <cstring>
#include <cstdio>
#include <cstdlib>
#include <cmath>
#include <cfloat>
#include <set>
#include <string>
#include <algorithm>
using VstInt32 = int32_t;
using audioMasterCallback = void*;
using VstPlugCategory = int;
constexpr int kPlugCategEffect = 1;
constexpr int kVstMaxProgNameLen = 24, kVstMaxParamStrLen = 32;
constexpr int kVstMaxProductStrLen = 64, kVstMaxVendorStrLen = 64;
constexpr int kVstMidiType = 1, kVstPinIsActive = 1, kVstPinIsStereo = 2;
struct VstEvent { int type = kVstMidiType; };
struct VstMidiEvent : VstEvent { int deltaFrames = 0; char midiData[4] = {}; };
struct VstEvents { int numEvents = 0; VstEvent* events[2] = {}; };
struct VstPinProperties { char label[64]; int flags; };
inline void vst_strncpy(char* out, const char* in, int count) {
    std::strncpy(out, in, count); out[count] = 0;
}
inline void float2string(float v, char* out, int n) { std::snprintf(out, n, "%.3f", v); }
inline void int2string(int v, char* out, int n) { std::snprintf(out, n, "%d", v); }
inline void dB2string(float v, char* out, int n) { float2string(20 * std::log10(v), out, n); }
#define DECLARE_VST_DEPRECATED(method) method
class AudioEffectX {
    float rate_ = 44100;
    int block_ = 64;
public:
    int numPrograms, numParams, curProgram = 0;
    AudioEffectX(audioMasterCallback, int programs, int params): numPrograms(programs), numParams(params) {}
    virtual ~AudioEffectX() = default;
    virtual void processReplacing(float**, float**, int) = 0;
    virtual float getParameter(int) = 0;
    virtual void setParameter(int, float) = 0;
    virtual void getParameterName(int, char*) = 0;
    virtual void getParameterDisplay(int, char*) {}
    virtual void getParameterLabel(int, char*) {}
    virtual int processEvents(VstEvents*) { return 0; }
    virtual void setSampleRate(float r) { rate_ = r; }
    float getSampleRate() const { return rate_; }
    virtual void setBlockSize(int n) { block_ = n; }
    int getBlockSize() const { return block_; }
    virtual void suspend() {}
    virtual void resume() {}
    virtual void setProgram(int p) { curProgram = p; }
    void setNumInputs(int) {}
    void setNumOutputs(int) {}
    void setUniqueID(unsigned long) {}
    void canProcessReplacing() {}
    void canDoubleReplacing() {}
    void programsAreChunks(bool) {}
    void isSynth() {}
    void wantEvents() {}
    void canMono() {}
};
using AudioEffect = AudioEffectX;
