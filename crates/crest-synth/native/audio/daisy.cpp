#include "parameter_adapter.h"
#include "random.h"
#include "zero_initialized.h"
#include "Drums/analogbassdrum.h"
#include "Drums/synthbassdrum.h"
#include "Drums/hihat.h"
#include "PhysicalModeling/stringvoice.h"
#include "PhysicalModeling/modalvoice.h"
#include "Effects/flanger.h"
#include "Effects/phaser.h"
#include "Effects/tremolo.h"
#include "Effects/autowah.h"
#include "Dynamics/limiter.h"
class DaisyAnalogBassDrum final : public ParameterAdapter {
    ZeroInitialized<daisysp::AnalogBassDrum> voice_storage_;
    daisysp::AnalogBassDrum& voice_=voice_storage_.get();
    float rate_, note_ = 60, bend_ = 0, velocity_ = 1;
    bool gate_ = false;
    void configure() {
        voice_.SetFreq(values_[0] * std::exp2((note_ - 60 + bend_) / 12));
        voice_.SetAccent(velocity_);
        voice_.SetTone(values_[1]);
        voice_.SetDecay(values_[2]);
        voice_.SetAttackFmAmount(values_[3]);
        voice_.SetSelfFmAmount(values_[4]);
        voice_.SetSustain(gate_ && values_[5] != 0);
    }
public:
    DaisyAnalogBassDrum(float rate): ParameterAdapter({{"Frequency", 50, 20, 10000, false}, {"Tone", 0.1, 0, 1, false}, {"Decay", 0.3, 0, 1, false}, {"Attack FM Amount", 0.5, 0, 1, false}, {"Self FM Amount", 1, 0, 1, false}, {"Sustain", 0, 0, 1, true}}), rate_(rate) { voice_.Init(rate); configure(); }
    void note(int status, int a, int b) override {
        if (status == 0x90 && b) { note_ = a; velocity_ = b/127.f; gate_ = true; configure(); voice_.Trig(); }
        else if (status == 0x80 || (status == 0x90 && !b)) gate_ = false;
        else if (status == 0xe0) bend_ = (a + 128*b - 8192) * (2.f/8192);
    }
    void reset() override { voice_.Init(rate_); gate_ = false; }
    void process(float* left, float* right, size_t frames) override {
        configure();
        for (size_t i=0; i<frames; ++i) left[i] = right[i] = voice_.Process();
    }
};
CrestProcessor* make_daisy_AnalogBassDrum(float rate, size_t) { return new DaisyAnalogBassDrum(rate); }
class DaisySyntheticBassDrum final : public ParameterAdapter {
    ZeroInitialized<daisysp::SyntheticBassDrum> voice_storage_;
    daisysp::SyntheticBassDrum& voice_=voice_storage_.get();
    float rate_, note_ = 60, bend_ = 0, velocity_ = 1;
    bool gate_ = false;
    void configure() {
        voice_.SetFreq(values_[0] * std::exp2((note_ - 60 + bend_) / 12));
        voice_.SetAccent(velocity_);
        voice_.SetTone(values_[1]);
        voice_.SetDecay(values_[2]);
        voice_.SetDirtiness(values_[3]);
        voice_.SetFmEnvelopeAmount(values_[4]);
        voice_.SetFmEnvelopeDecay(values_[5]);
        voice_.SetSustain(gate_ && values_[6] != 0);
    }
public:
    DaisySyntheticBassDrum(float rate): ParameterAdapter({{"Frequency", 100, 20, 10000, false}, {"Tone", 0.6, 0, 1, false}, {"Decay", 0.7, 0, 1, false}, {"Dirtiness", 0.3, 0, 1, false}, {"FM Envelope Amount", 0.6, 0, 1, false}, {"FM Envelope Decay", 0.3, 0, 1, false}, {"Sustain", 0, 0, 1, true}}), rate_(rate) { voice_.Init(rate); configure(); }
    void note(int status, int a, int b) override {
        if (status == 0x90 && b) { note_ = a; velocity_ = b/127.f; gate_ = true; configure(); voice_.Trig(); }
        else if (status == 0x80 || (status == 0x90 && !b)) gate_ = false;
        else if (status == 0xe0) bend_ = (a + 128*b - 8192) * (2.f/8192);
    }
    void reset() override { voice_.Init(rate_); gate_ = false; }
    void process(float* left, float* right, size_t frames) override {
        configure();
        for (size_t i=0; i<frames; ++i) left[i] = right[i] = voice_.Process();
    }
};
CrestProcessor* make_daisy_SyntheticBassDrum(float rate, size_t) { return new DaisySyntheticBassDrum(rate); }
class DaisyHiHat final : public ParameterAdapter {
    daisysp::HiHat<> voice_{};
    float rate_, note_ = 60, bend_ = 0, velocity_ = 1;
    bool gate_ = false;
    void configure() {
        voice_.SetFreq(values_[0] * std::exp2((note_ - 60 + bend_) / 12));
        voice_.SetAccent(velocity_);
        voice_.SetTone(values_[1]);
        voice_.SetDecay(values_[2]);
        voice_.SetNoisiness(values_[3]);
        voice_.SetSustain(gate_ && values_[4] != 0);
    }
public:
    DaisyHiHat(float rate): ParameterAdapter({{"Frequency", 3000, 20, 10000, false}, {"Tone", 0.5, 0, 1, false}, {"Decay", 0.2, 0, 1, false}, {"Noisiness", 0.8, 0, 1, false}, {"Sustain", 0, 0, 1, true}}), rate_(rate) { voice_.Init(rate); configure(); }
    void note(int status, int a, int b) override {
        if (status == 0x90 && b) { note_ = a; velocity_ = b/127.f; gate_ = true; configure(); voice_.Trig(); }
        else if (status == 0x80 || (status == 0x90 && !b)) gate_ = false;
        else if (status == 0xe0) bend_ = (a + 128*b - 8192) * (2.f/8192);
    }
    void reset() override { voice_.Init(rate_); gate_ = false; }
    void process(float* left, float* right, size_t frames) override {
        configure();
        for (size_t i=0; i<frames; ++i) left[i] = right[i] = voice_.Process();
    }
};
CrestProcessor* make_daisy_HiHat(float rate, size_t) { return new DaisyHiHat(rate); }
class DaisyStringVoice final : public ParameterAdapter {
    ZeroInitialized<daisysp::StringVoice> voice_storage_;
    daisysp::StringVoice& voice_=voice_storage_.get();
    float rate_, note_ = 60, bend_ = 0, velocity_ = 1;
    bool gate_ = false;
    void configure() {
        voice_.SetFreq(440 * std::exp2((note_ - 69 + bend_) / 12));
        voice_.SetAccent(velocity_);
        voice_.SetStructure(values_[0]);
        voice_.SetBrightness(values_[1]);
        voice_.SetDamping(values_[2]);
        voice_.SetSustain(gate_ && values_[3] != 0);
    }
public:
    DaisyStringVoice(float rate): ParameterAdapter({{"Structure", 0.7, 0, 1, false}, {"Brightness", 0.2, 0, 1, false}, {"Damping", 0.7, 0, 1, false}, {"Sustain", 0, 0, 1, true}}), rate_(rate) { voice_.Init(rate); configure(); }
    void note(int status, int a, int b) override {
        if (status == 0x90 && b) { note_ = a; velocity_ = b/127.f; gate_ = true; configure(); voice_.Trig(); }
        else if (status == 0x80 || (status == 0x90 && !b)) gate_ = false;
        else if (status == 0xe0) bend_ = (a + 128*b - 8192) * (2.f/8192);
    }
    void reset() override { voice_.Init(rate_); gate_ = false; }
    void process(float* left, float* right, size_t frames) override {
        configure();
        for (size_t i=0; i<frames; ++i) left[i] = right[i] = voice_.Process();
    }
};
CrestProcessor* make_daisy_StringVoice(float rate, size_t) { return new DaisyStringVoice(rate); }
class DaisyModalVoice final : public ParameterAdapter {
    ZeroInitialized<daisysp::ModalVoice> voice_storage_;
    daisysp::ModalVoice& voice_=voice_storage_.get();
    float rate_, note_ = 60, bend_ = 0, velocity_ = 1;
    bool gate_ = false;
    void configure() {
        voice_.SetFreq(440 * std::exp2((note_ - 69 + bend_) / 12));
        voice_.SetAccent(velocity_);
        voice_.SetStructure(values_[0]);
        voice_.SetBrightness(values_[1]);
        voice_.SetDamping(values_[2]);
        voice_.SetSustain(gate_ && values_[3] != 0);
    }
public:
    DaisyModalVoice(float rate): ParameterAdapter({{"Structure", 0.6, 0, 1, false}, {"Brightness", 0.8, 0, 1, false}, {"Damping", 0.6, 0, 1, false}, {"Sustain", 0, 0, 1, true}}), rate_(rate) { voice_.Init(rate); configure(); }
    void note(int status, int a, int b) override {
        if (status == 0x90 && b) { note_ = a; velocity_ = b/127.f; gate_ = true; configure(); voice_.Trig(); }
        else if (status == 0x80 || (status == 0x90 && !b)) gate_ = false;
        else if (status == 0xe0) bend_ = (a + 128*b - 8192) * (2.f/8192);
    }
    void reset() override { voice_.Init(rate_); gate_ = false; }
    void process(float* left, float* right, size_t frames) override {
        configure();
        for (size_t i=0; i<frames; ++i) left[i] = right[i] = voice_.Process();
    }
};
CrestProcessor* make_daisy_ModalVoice(float rate, size_t) { return new DaisyModalVoice(rate); }
class DaisyFlanger final : public ParameterAdapter {
    ZeroInitialized<daisysp::Flanger> left_storage_, right_storage_;
    daisysp::Flanger& left_=left_storage_.get();
    daisysp::Flanger& right_=right_storage_.get();
public:
    DaisyFlanger(float rate): ParameterAdapter({{"Delay", 0.75, 0, 1, false}, {"Feedback", 0.2, 0, 1, false}, {"LFO Depth", 0.9, 0, 0.93, false}, {"LFO Freq", 0.3, 0.01, 20, false}}) { left_.Init(rate); right_.Init(rate); }
    void process(float* left, float* right, size_t frames) override {
        left_.SetDelay(values_[0]); right_.SetDelay(values_[0]);
        left_.SetFeedback(values_[1]); right_.SetFeedback(values_[1]);
        left_.SetLfoDepth(values_[2]); right_.SetLfoDepth(values_[2]);
        left_.SetLfoFreq(values_[3]); right_.SetLfoFreq(values_[3]);
        for (size_t i=0; i<frames; ++i) { left[i]=left_.Process(left[i]); right[i]=right_.Process(right[i]); }
    }
};
CrestProcessor* make_daisy_Flanger(float rate, size_t) { return new DaisyFlanger(rate); }
class DaisyPhaser final : public ParameterAdapter {
    ZeroInitialized<daisysp::Phaser> left_storage_, right_storage_;
    daisysp::Phaser& left_=left_storage_.get();
    daisysp::Phaser& right_=right_storage_.get();
public:
    DaisyPhaser(float rate): ParameterAdapter({{"Poles", 4, 1, 8, true}, {"Freq", 200, 0, 20000, false}, {"Feedback", 0.2, 0, 0.75, false}, {"LFO Depth", 0.9, 0, 1, false}, {"LFO Freq", 0.3, 0.01, 20, false}}) { left_.Init(rate); right_.Init(rate); }
    void process(float* left, float* right, size_t frames) override {
        left_.SetPoles(values_[0]); right_.SetPoles(values_[0]);
        left_.SetFreq(values_[1]); right_.SetFreq(values_[1]);
        left_.SetFeedback(values_[2]); right_.SetFeedback(values_[2]);
        left_.SetLfoDepth(values_[3]); right_.SetLfoDepth(values_[3]);
        left_.SetLfoFreq(values_[4]); right_.SetLfoFreq(values_[4]);
        for (size_t i=0; i<frames; ++i) { left[i]=left_.Process(left[i]); right[i]=right_.Process(right[i]); }
    }
};
CrestProcessor* make_daisy_Phaser(float rate, size_t) { return new DaisyPhaser(rate); }
class DaisyTremolo final : public ParameterAdapter {
    ZeroInitialized<daisysp::Tremolo> left_storage_, right_storage_;
    daisysp::Tremolo& left_=left_storage_.get();
    daisysp::Tremolo& right_=right_storage_.get();
public:
    DaisyTremolo(float rate): ParameterAdapter({{"Freq", 1, 0.01, 30, false}, {"Depth", 1, 0, 1, false}, {"Waveform", 0, 0, 7, true}}) { left_.Init(rate); right_.Init(rate); }
    void process(float* left, float* right, size_t frames) override {
        left_.SetFreq(values_[0]); right_.SetFreq(values_[0]);
        left_.SetDepth(values_[1]); right_.SetDepth(values_[1]);
        left_.SetWaveform(values_[2]); right_.SetWaveform(values_[2]);
        for (size_t i=0; i<frames; ++i) { left[i]=left_.Process(left[i]); right[i]=right_.Process(right[i]); }
    }
};
CrestProcessor* make_daisy_Tremolo(float rate, size_t) { return new DaisyTremolo(rate); }
class DaisyAutowah final : public ParameterAdapter {
    ZeroInitialized<daisysp::Autowah> left_storage_, right_storage_;
    daisysp::Autowah& left_=left_storage_.get();
    daisysp::Autowah& right_=right_storage_.get();
public:
    DaisyAutowah(float rate): ParameterAdapter({{"Wah", 0, 0, 1, false}, {"Dry Wet", 100, 0, 100, false}, {"Level", 0.1, 0, 1, false}}) { left_.Init(rate); right_.Init(rate); }
    void process(float* left, float* right, size_t frames) override {
        left_.SetWah(values_[0]); right_.SetWah(values_[0]);
        left_.SetDryWet(values_[1]); right_.SetDryWet(values_[1]);
        left_.SetLevel(values_[2]); right_.SetLevel(values_[2]);
        for (size_t i=0; i<frames; ++i) { left[i]=left_.Process(left[i]); right[i]=right_.Process(right[i]); }
    }
};
CrestProcessor* make_daisy_Autowah(float rate, size_t) { return new DaisyAutowah(rate); }
class DaisyLimiter final : public ParameterAdapter {
    ZeroInitialized<daisysp::Limiter> left_storage_, right_storage_;
    daisysp::Limiter& left_=left_storage_.get();
    daisysp::Limiter& right_=right_storage_.get();
public:
    DaisyLimiter(): ParameterAdapter({{"Pre Gain", 1, 0, 16}}) { left_.Init(); right_.Init(); }
    void process(float* left, float* right, size_t frames) override {
        left_.ProcessBlock(left,frames,values_[0]); right_.ProcessBlock(right,frames,values_[0]);
    }
};
CrestProcessor* make_daisy_Limiter(float, size_t) { return new DaisyLimiter; }
