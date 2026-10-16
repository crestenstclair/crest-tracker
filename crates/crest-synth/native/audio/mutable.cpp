#include <cstdio>
#include "parameter_adapter.h"
#include "rate_adapter.h"
#include "plaits/dsp/voice.h"
#include "plaits/dsp/drums/analog_snare_drum.h"
#include "plaits/dsp/drums/synthetic_snare_drum.h"
#include "rings/dsp/part.h"
#include "elements/dsp/part.h"
#include "clouds/dsp/granular_processor.h"
#include "warps/dsp/modulator.h"
#include "tides2/poly_slope_generator.h"
#include "peaks/drums/bass_drum.h"
#include "peaks/drums/snare_drum.h"
#include "peaks/drums/fm_drum.h"
#include "peaks/drums/high_hat.h"
#include <array>
#include "zero_initialized.h"

class MutableVoice : public ParameterAdapter {
protected:
    float note_=60, bend_=0, velocity_=1;
    bool gate_=false, trigger_=false;
public:
    using ParameterAdapter::ParameterAdapter;
    void note(int status,int a,int b) override {
        if (status==0x90 && b) { note_=a; velocity_=b/127.f; gate_=trigger_=true; }
        else if (status==0x80 || (status==0x90 && !b)) gate_=false;
        else if (status==0xe0) bend_=(a+128*b-8192)*(2.f/8192);
    }
    void reset() override { gate_=trigger_=false; }
};
// Original complete snare model; DaisySP's derivative diverges at high notes.
class MutableAnalogSnare final : public MutableVoice {
    ZeroInitialized<plaits::AnalogSnareDrum> storage_;
    plaits::AnalogSnareDrum& voice_ = storage_.get();
public:
    MutableAnalogSnare(): MutableVoice({{"Frequency",200,20,10000},{"Tone",.5},{"Decay",.3},{"Snappy",.7},{"Sustain",0,0,1,true}}) { voice_.Init(); }
    void reset() override { MutableVoice::reset(); voice_.Init(); }
    void process(float* left,float* right,size_t frames) override {
        const float frequency=std::min(.4f,values_[0]*std::exp2((note_-60+bend_)/12)/48000.f);
        voice_.Render(gate_&&values_[4]!=0,trigger_,velocity_,frequency,values_[1],values_[2],values_[3],left,frames);
        std::copy(left,left+frames,right); trigger_=false;
    }
};
CrestProcessor* make_mutable_AnalogSnare(float rate,size_t frames) { return new RateAdapter(new MutableAnalogSnare,48000,rate,24,frames); }

// Keep the original model and filter semantics instead of the unstable port.
class MutableSyntheticSnare final : public MutableVoice {
    ZeroInitialized<plaits::SyntheticSnareDrum> storage_;
    plaits::SyntheticSnareDrum& voice_ = storage_.get();
public:
    MutableSyntheticSnare(): MutableVoice({{"Frequency",200,20,10000},{"FM Amount",.1},{"Decay",.3},{"Snappy",.7},{"Sustain",0,0,1,true}}) { voice_.Init(); }
    void reset() override { MutableVoice::reset(); voice_.Init(); }
    void process(float* left,float* right,size_t frames) override {
        const float frequency=std::min(.4f,values_[0]*std::exp2((note_-60+bend_)/12)/48000.f);
        voice_.Render(gate_&&values_[4]!=0,trigger_,velocity_,frequency,values_[1],values_[2],values_[3],left,frames);
        std::copy(left,left+frames,right); trigger_=false;
    }
};
CrestProcessor* make_mutable_SyntheticSnare(float rate,size_t frames) { return new RateAdapter(new MutableSyntheticSnare,48000,rate,24,frames); }

class MutablePlaits final : public MutableVoice {
    ZeroInitialized<plaits::Voice> storage_;
    plaits::Voice& voice_ = storage_.get();
    alignas(16) std::array<uint8_t,16384> memory_{};
    std::array<plaits::Voice::Frame,24> frames_{};
public:
    MutablePlaits(): MutableVoice({{"Model",0,0,23,true},{"Harmonics",.5},{"Timbre",.5},{"Morph",.5},
        {"Decay",.5},{"LPG Colour",.5},{"Frequency Modulation",0,-1,1},{"Timbre Modulation",0,-1,1},{"Morph Modulation",0,-1,1}}) {
        stmlib::BufferAllocator allocator(memory_.data(),memory_.size()); voice_.Init(&allocator);
    }
    void process(float* left,float* right,size_t n) override {
        plaits::Patch patch{};
        patch.note=note_+bend_; patch.engine=values_[0]; patch.harmonics=values_[1]; patch.timbre=values_[2]; patch.morph=values_[3];
        patch.decay=values_[4]; patch.lpg_colour=values_[5]; patch.frequency_modulation_amount=values_[6]; patch.timbre_modulation_amount=values_[7]; patch.morph_modulation_amount=values_[8];
        plaits::Modulations mod{};
        mod.trigger_patched=true; mod.trigger=trigger_?1:0; mod.level_patched=true; mod.level=velocity_;
        voice_.Render(patch,mod,frames_.data(),n); trigger_=false;
        for (size_t i=0;i<n;++i) { left[i]=frames_[i].out/32768.f; right[i]=frames_[i].aux/32768.f; }
    }
};
class MutableRings final : public MutableVoice {
    ZeroInitialized<rings::Part> storage_;
    rings::Part& voice_ = storage_.get();
    std::array<uint16_t,32768> memory_{};
    std::array<float,24> silence_{};
public:
    MutableRings(): MutableVoice({{"Model",0,0,5,true},{"Structure",.25},{"Brightness",.5},{"Damping",.5},{"Position",.5},{"Chord",0,0,10,true}}) {
        voice_.Init(memory_.data()); voice_.set_polyphony(1);
    }
    void process(float* left,float* right,size_t n) override {
        voice_.set_model(static_cast<rings::ResonatorModel>(static_cast<int>(values_[0])));
        rings::Patch patch{values_[1],values_[2],values_[3],values_[4]};
        rings::PerformanceState performance{};
        performance.strum=trigger_; performance.internal_exciter=true; performance.internal_note=false;
        performance.internal_strum=false; performance.tonic=12; performance.note=note_+bend_-12; performance.chord=values_[5];
        voice_.Process(performance,patch,silence_.data(),left,right,n); trigger_=false;
        for (size_t i=0;i<n;++i) { left[i]*=velocity_; right[i]*=velocity_; }
    }
};
class MutableRingsResonator final : public ParameterAdapter {
    ZeroInitialized<rings::Part> storage_;
    rings::Part& voice_ = storage_.get();
    std::array<uint16_t,32768> memory_{};
    std::array<float,24> input_{};
public:
    MutableRingsResonator():ParameterAdapter({{"Model",0,0,5,true},{"Frequency (Hz)",220,20,20000},{"Structure",.25},{"Brightness",.5},{"Damping",.5},{"Position",.5}}) {
        voice_.Init(memory_.data());voice_.set_polyphony(1);
    }
    void process(float* left,float* right,size_t frames) override {
        // Rings has one external excitation input and a stereo resonator output.
        for(size_t i=0;i<frames;++i) input_[i]=.5f*(left[i]+right[i]);
        voice_.set_model(static_cast<rings::ResonatorModel>(static_cast<int>(values_[0])));
        rings::Patch patch{values_[2],values_[3],values_[4],values_[5]};
        rings::PerformanceState performance{};
        performance.internal_exciter=false;performance.internal_note=false;performance.internal_strum=true;
        performance.tonic=12;performance.note=69+12*std::log2(values_[1]/440)-12;
        voice_.Process(performance,patch,input_.data(),left,right,frames);
    }
};
CrestProcessor* make_mutable_RingsResonator(float rate,size_t frames) { return new RateAdapter(new MutableRingsResonator,48000,rate,24,frames); }

class MutableElements final : public MutableVoice {
    ZeroInitialized<elements::Part> storage_;
    elements::Part& voice_ = storage_.get();
    std::array<uint16_t,32768> memory_{};
    std::array<float,16> silence_{};
public:
    MutableElements(): MutableVoice({{"Model",0,0,3,true},{"Envelope Shape",1},
        {"Bow Level",0},{"Bow Timbre",.5},{"Blow Level",0},{"Blow Flow",.5},{"Blow Timbre",.5},
        {"Strike Level",.8},{"Strike Mallet",.5},{"Strike Timbre",.5},{"Exciter Signature",0},
        {"Geometry",.2},{"Brightness",.5},{"Damping",.25},{"Position",.3},
        // Elements adds a [0, .5] triangle LFO to this offset before calling
        // stmlib's approximate cosine oscillator, whose phase domain is [0, 1].
        {"Resonator Modulation Hz",.5,.1,10},{"Resonator Modulation Offset",.1,0,.5},
        {"Reverb Diffusion",.625},{"Reverb Low Pass",.7},{"Space",.5}}) { voice_.Init(memory_.data()); }
    void process(float* left,float* right,size_t n) override {
        voice_.set_easter_egg(values_[0]==3); voice_.set_resonator_model(static_cast<elements::ResonatorModel>(std::min(2,static_cast<int>(values_[0]))));
        auto& p=*voice_.mutable_patch();
        p.exciter_envelope_shape=values_[1]; p.exciter_bow_level=values_[2]; p.exciter_bow_timbre=values_[3];
        p.exciter_blow_level=values_[4]; p.exciter_blow_meta=values_[5]; p.exciter_blow_timbre=values_[6];
        p.exciter_strike_level=values_[7]; p.exciter_strike_meta=values_[8]; p.exciter_strike_timbre=values_[9]; p.exciter_signature=values_[10];
        p.resonator_geometry=values_[11]; p.resonator_brightness=values_[12]; p.resonator_damping=values_[13]; p.resonator_position=values_[14];
        p.resonator_modulation_frequency=values_[15]/32000.f; p.resonator_modulation_offset=values_[16];
        p.reverb_diffusion=values_[17]; p.reverb_lp=values_[18]; p.space=values_[19];
        elements::PerformanceState performance{gate_,note_+bend_,0,velocity_};
        voice_.Process(performance,silence_.data(),silence_.data(),left,right,n); trigger_=false;
    }
    void reset() override { MutableVoice::reset(); voice_.Init(memory_.data()); }
};
class MutableTides final : public MutableVoice {
    ZeroInitialized<tides::PolySlopeGenerator> storage_;
    tides::PolySlopeGenerator& voice_ = storage_.get();
    std::array<tides::PolySlopeGenerator::OutputSample,24> output_{};
    std::array<stmlib::GateFlags,24> gates_{};
public:
    MutableTides(): MutableVoice({{"Output Mode",2,0,3,true},{"Shape",.5},{"Slope",.5},{"Smoothness",.5},{"Shift",.5},{"Output A",0,0,3,true},{"Output B",1,0,3,true}}) { voice_.Init(); }
    void process(float* left,float* right,size_t n) override {
        float frequency=440*std::exp2((note_+bend_-69)/12)/48000;
        voice_.Render(tides::RAMP_MODE_LOOPING,static_cast<tides::OutputMode>(static_cast<int>(values_[0])),tides::RANGE_AUDIO,
            frequency,values_[2],values_[1],values_[3],values_[4],gates_.data(),nullptr,output_.data(),n);
        for(size_t i=0;i<n;++i) { left[i]=output_[i].channel[static_cast<int>(values_[5])]*velocity_*.2f; right[i]=output_[i].channel[static_cast<int>(values_[6])]*velocity_*.2f; }
    }
};
template<class Drum> class MutablePeaks final : public MutableVoice {
    ZeroInitialized<Drum> storage_;
    Drum& voice_ = storage_.get();
    std::array<peaks::GateFlags,24> gates_{};
    std::array<int16_t,24> output_{};
public:
    MutablePeaks(std::initializer_list<CrestParameter> parameters): MutableVoice(parameters) { voice_.Init(); }
    void process(float* left,float* right,size_t n) override {
        uint16_t parameters[4]{};
        for(size_t i=0;i<4 && i<values_.size();++i) parameters[i]=values_[i]*65535.f;
        voice_.Configure(parameters,peaks::CONTROL_MODE_FULL);
        gates_.fill(peaks::GATE_FLAG_LOW);
        if(trigger_) gates_[0]=peaks::GATE_FLAG_RISING|peaks::GATE_FLAG_HIGH;
        voice_.Process(gates_.data(),output_.data(),n); trigger_=false;
        for(size_t i=0;i<n;++i) left[i]=right[i]=output_[i]/32768.f*velocity_;
    }
    void reset() override { MutableVoice::reset(); voice_.Init(); }
};
class MutableClouds final : public ParameterAdapter {
    ZeroInitialized<clouds::GranularProcessor> storage_;
    clouds::GranularProcessor& processor_ = storage_.get();
    std::array<uint8_t,118784> large_{};
    std::array<uint8_t,65408> small_{};
    std::array<clouds::ShortFrame,32> input_{},output_{};
public:
    MutableClouds(int mode): ParameterAdapter({{"Position",.5},{"Size",.5},{"Pitch",0,-48,48},{"Density",.5},{"Texture",.5},
        {"Dry Wet",.5},{"Stereo Spread",.5},{"Feedback",0},{"Reverb",0},{"Freeze",0,0,1,true}}) {
        processor_.Init(large_.data(),large_.size(),small_.data(),small_.size());
        processor_.set_num_channels(2); processor_.set_low_fidelity(false);
        processor_.set_playback_mode(static_cast<clouds::PlaybackMode>(mode)); processor_.Prepare();
    }
    void process(float* left,float* right,size_t n) override {
        auto& p=*processor_.mutable_parameters();
        p.position=values_[0]; p.size=values_[1]; p.pitch=values_[2]; p.density=values_[3]; p.texture=values_[4];
        p.dry_wet=values_[5]; p.stereo_spread=values_[6]; p.feedback=values_[7]; p.reverb=values_[8]; p.freeze=values_[9]!=0;
        for(size_t i=0;i<n;++i) { input_[i].l=std::clamp(left[i],-1.f,.999969f)*32768; input_[i].r=std::clamp(right[i],-1.f,.999969f)*32768; }
        processor_.Prepare(); processor_.Process(input_.data(),output_.data(),n);
        for(size_t i=0;i<n;++i) { left[i]=output_[i].l/32768.f; right[i]=output_[i].r/32768.f; }
    }
};
class MutableWarps final : public ParameterAdapter {
    ZeroInitialized<warps::Modulator> storage_;
    warps::Modulator& processor_ = storage_.get();
    std::array<warps::ShortFrame,96> input_{},output_{};
public:
    MutableWarps(): ParameterAdapter({{"Algorithm",.5,0,1},{"Timbre",.5},{"Carrier Shape (0 = Left Input)",1,0,3,true},
        {"Carrier Note",60,0,127},{"Carrier Drive",.5},{"Modulator Drive",.5},{"Frequency Shifter",0,0,1,true},
        {"Shift Frequency",.5},{"Shift CV",0,-1,1},{"Phase Shift",0}}) { processor_.Init(96000); }
    void process(float* left,float* right,size_t n) override {
        auto& p=*processor_.mutable_parameters();
        p.modulation_algorithm=values_[0]; p.modulation_parameter=values_[1]; p.carrier_shape=values_[2]; p.note=values_[3];
        p.channel_drive[0]=values_[4]; p.channel_drive[1]=values_[5]; processor_.set_easter_egg(values_[6]!=0);
        p.frequency_shift_pot=values_[7]; p.frequency_shift_cv=values_[8]; p.phase_shift=values_[9];
        for(size_t i=0;i<n;++i) { input_[i].l=std::clamp(left[i],-1.f,.999969f)*32768; input_[i].r=std::clamp(right[i],-1.f,.999969f)*32768; }
        processor_.Process(input_.data(),output_.data(),n);
        for(size_t i=0;i<n;++i) { left[i]=output_[i].l/32768.f; right[i]=output_[i].r/32768.f; }
    }
};
CrestProcessor* make_mutable_Plaits(float rate,size_t n) { return new RateAdapter(new MutablePlaits,48000,rate,24,n,RateAdapter::Signal::StereoGenerator); }
CrestProcessor* make_mutable_Rings(float rate,size_t n) { return new RateAdapter(new MutableRings,48000,rate,24,n,RateAdapter::Signal::StereoGenerator); }
CrestProcessor* make_mutable_Elements(float rate,size_t n) { return new RateAdapter(new MutableElements,32000,rate,16,n,RateAdapter::Signal::StereoGenerator); }
CrestProcessor* make_mutable_Tides(float rate,size_t n) { return new RateAdapter(new MutableTides,48000,rate,24,n,RateAdapter::Signal::StereoGenerator); }
CrestProcessor* make_mutable_PeaksBass(float rate,size_t n) { return new RateAdapter(new MutablePeaks<peaks::BassDrum>({{"Frequency",.5},{"Punch",.6},{"Tone",.5},{"Decay",.5}}),48000,rate,24,n,RateAdapter::Signal::MonoGenerator); }
CrestProcessor* make_mutable_PeaksSnare(float rate,size_t n) { return new RateAdapter(new MutablePeaks<peaks::SnareDrum>({{"Frequency",.5},{"Tone",.5},{"Snappy",.5},{"Decay",.5}}),48000,rate,24,n,RateAdapter::Signal::MonoGenerator); }
CrestProcessor* make_mutable_PeaksFM(float rate,size_t n) { return new RateAdapter(new MutablePeaks<peaks::FmDrum>({{"Frequency",.5},{"FM Amount",.5},{"Decay",.5},{"Noise",.5}}),48000,rate,24,n,RateAdapter::Signal::MonoGenerator); }
// HighHat::Configure is empty upstream. Its native voice has no editable DSP
// controls; velocity and the canonical Patch ADSR provide performance control.
CrestProcessor* make_mutable_PeaksHat(float rate,size_t n) { return new RateAdapter(new MutablePeaks<peaks::HighHat>({}),48000,rate,24,n,RateAdapter::Signal::MonoGenerator); }
CrestProcessor* make_mutable_CloudsGranular(float rate,size_t n) { return new RateAdapter(new MutableClouds(0),32000,rate,32,n); }
CrestProcessor* make_mutable_CloudsStretch(float rate,size_t n) { return new RateAdapter(new MutableClouds(1),32000,rate,32,n); }
CrestProcessor* make_mutable_CloudsDelay(float rate,size_t n) { return new RateAdapter(new MutableClouds(2),32000,rate,32,n); }
CrestProcessor* make_mutable_CloudsSpectral(float rate,size_t n) { return new RateAdapter(new MutableClouds(3),32000,rate,32,n); }
CrestProcessor* make_mutable_Warps(float rate,size_t n) { return new RateAdapter(new MutableWarps,96000,rate,96,n); }
