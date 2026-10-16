#include "random.h"
#include "parameter_adapter.h"
#include <mutex>
#include "Shakers.h"
#include "ModalBar.h"
#include "BandedWG.h"
#include "Mesh2D.h"
#include "VoicForm.h"
#include "Clarinet.h"
#include "Flute.h"
#include "Brass.h"
#include "Bowed.h"
#include "Mandolin.h"
#include "BeeThree.h"
#include "Rhodey.h"
#include "Wurley.h"
// STK reads the prepared instance's host rate through CrestProcessorScope.
// Global sample rate never changes. Only off-thread construction/destruction
// touches the upstream observer list; serialize those operations here.
static std::mutex stk_ownership_mutex;
class PreparedBandedWG : public stk::BandedWG {
public:
    PreparedBandedWG() {
        // Reserve every preset's longest mode at MIDI note 0 with the full
        // two-semitone downward bend. Capacity follows the prepared rate.
        double lowest_mode=1.0;
        for(int preset:{0,1,2,3}) {
            setPreset(preset);
            for(int i=0;i<presetModes_;++i) lowest_mode=std::min(lowest_mode,modes_[i]);
        }
        const double lowest_note=440.0*std::exp2(-71.0/12.0);
        const auto capacity=static_cast<unsigned long>(std::ceil(stk::Stk::sampleRate()/(lowest_note*lowest_mode)));
        for(auto& delay:delay_) delay.setMaximumDelay(capacity);
        setPreset(0);
    }
};
class StkShakers final : public ParameterAdapter {
    std::unique_ptr<stk::Shakers> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkShakers(): ParameterAdapter({{"Object", 0, 0, 22, true}, {"Decay", 64, 0, 128, false}, {"Objects", 64, 0, 128, false}, {"Resonance", 64, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Shakers());
        for (int i=0; i<23; ++i) voice_->controlChange(1071,i);
        voice_->controlChange(1071,0);
    }
    ~StkShakers() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(1071,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((values_[0]+32-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Shakers(float rate,size_t frames) { return new StkShakers; }
class StkModalBar final : public ParameterAdapter {
    std::unique_ptr<stk::ModalBar> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkModalBar(): ParameterAdapter({{"Material", 0, 0, 8, true}, {"Stick Hardness", 55, 0, 128, false}, {"Strike Position", 57, 0, 128, false}, {"Direct Gain", 12, 0, 128, false}, {"Vibrato Rate", 64, 0, 128, false}, {"Vibrato Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::ModalBar());
    }
    ~StkModalBar() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(16,v); break;
        case 1: voice_->controlChange(2,v); break;
        case 2: voice_->controlChange(4,v); break;
        case 3: voice_->controlChange(8,v); break;
        case 4: voice_->controlChange(11,v); break;
        case 5: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_ModalBar(float rate,size_t frames) { return new StkModalBar; }
class StkBandedWG final : public ParameterAdapter {
    std::unique_ptr<PreparedBandedWG> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkBandedWG(): ParameterAdapter({{"Material", 0, 0, 3, true}, {"Bow Pressure", 0, 0, 128, false}, {"Bow Motion", 64, 0, 128, false}, {"Resonance", 127, 0, 128, false}, {"Integration", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new PreparedBandedWG());
    }
    ~StkBandedWG() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(16,v); break;
        case 1: voice_->controlChange(2,v); break;
        case 2: voice_->controlChange(4,v); break;
        case 3: voice_->controlChange(1,v); break;
        case 4: voice_->controlChange(11,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_BandedWG(float rate,size_t frames) { return new StkBandedWG; }
class StkMesh2D final : public ParameterAdapter {
    std::unique_ptr<stk::Mesh2D> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkMesh2D(): ParameterAdapter({{"X Dimension", 6, 2, 12, true}, {"Y Dimension", 6, 2, 12, true}, {"Decay", 64, 0, 128, false}, {"Strike Position", 64, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Mesh2D(6,6));
    }
    ~StkMesh2D() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->setNX(v); break;
        case 1: voice_->setNY(v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
        }
    }
    void reset() override { voice_->clear();  }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Mesh2D(float rate,size_t frames) { return new StkMesh2D; }
class StkVoicForm final : public ParameterAdapter {
    std::unique_ptr<stk::VoicForm> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkVoicForm(): ParameterAdapter({{"Phoneme", 0, 0, 127, true}, {"Unvoiced Mix", 0, 0, 128, false}, {"Vibrato Rate", 64, 0, 128, false}, {"Vibrato Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::VoicForm());
    }
    ~StkVoicForm() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(4,v); break;
        case 1: voice_->controlChange(2,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_VoicForm(float rate,size_t frames) { return new StkVoicForm; }
class StkClarinet final : public ParameterAdapter {
    std::unique_ptr<stk::Clarinet> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkClarinet(): ParameterAdapter({{"Reed Stiffness", 64, 0, 128, false}, {"Noise", 25, 0, 128, false}, {"Vibrato Rate", 64, 0, 128, false}, {"Vibrato Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Clarinet(1.0));
    }
    ~StkClarinet() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Clarinet(float rate,size_t frames) { return new StkClarinet; }
class StkFlute final : public ParameterAdapter {
    std::unique_ptr<stk::Flute> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkFlute(): ParameterAdapter({{"Jet Delay", 64, 0, 128, false}, {"Noise", 25, 0, 128, false}, {"Vibrato Rate", 64, 0, 128, false}, {"Vibrato Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Flute(1.0));
    }
    ~StkFlute() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Flute(float rate,size_t frames) { return new StkFlute; }
class StkBrass final : public ParameterAdapter {
    std::unique_ptr<stk::Brass> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkBrass(): ParameterAdapter({{"Lip Tension", 64, 0, 128, false}, {"Slide Length", 64, 0, 128, false}, {"Vibrato Rate", 64, 0, 128, false}, {"Vibrato Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Brass(1.0));
    }
    ~StkBrass() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Brass(float rate,size_t frames) { return new StkBrass; }
class StkBowed final : public ParameterAdapter {
    std::unique_ptr<stk::Bowed> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkBowed(): ParameterAdapter({{"Bow Pressure", 64, 0, 128, false}, {"Bow Position", 32, 0, 128, false}, {"Vibrato Rate", 64, 0, 128, false}, {"Vibrato Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Bowed(1.0));
    }
    ~StkBowed() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Bowed(float rate,size_t frames) { return new StkBowed; }
class StkMandolin final : public ParameterAdapter {
    std::unique_ptr<stk::Mandolin> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkMandolin(): ParameterAdapter({{"Body Size", 64, 0, 128, false}, {"Pick Position", 51, 0, 128, false}, {"String Damping", 100, 0, 127, false}, {"String Detune", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Mandolin(1.0));
    }
    ~StkMandolin() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Mandolin(float rate,size_t frames) { return new StkMandolin; }
class StkBeeThree final : public ParameterAdapter {
    std::unique_ptr<stk::BeeThree> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkBeeThree(): ParameterAdapter({{"Feedback Operator Gain", 64, 0, 128, false}, {"Operator 3 Gain", 64, 0, 128, false}, {"LFO Rate", 64, 0, 128, false}, {"LFO Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::BeeThree());
    }
    ~StkBeeThree() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_BeeThree(float rate,size_t frames) { return new StkBeeThree; }
class StkRhodey final : public ParameterAdapter {
    std::unique_ptr<stk::Rhodey> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkRhodey(): ParameterAdapter({{"Modulation Index", 64, 0, 128, false}, {"Output Crossfade", 64, 0, 128, false}, {"LFO Rate", 64, 0, 128, false}, {"LFO Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Rhodey());
    }
    ~StkRhodey() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Rhodey(float rate,size_t frames) { return new StkRhodey; }
class StkWurley final : public ParameterAdapter {
    std::unique_ptr<stk::Wurley> voice_;
    float note_ = 60, bend_ = 0;
public:
    StkWurley(): ParameterAdapter({{"Modulation Index", 64, 0, 128, false}, {"Output Crossfade", 64, 0, 128, false}, {"LFO Rate", 64, 0, 128, false}, {"LFO Depth", 0, 0, 128, false}}) {
        std::lock_guard<std::mutex> lock(stk_ownership_mutex);
        voice_.reset(new stk::Wurley());
    }
    ~StkWurley() override { std::lock_guard<std::mutex> lock(stk_ownership_mutex); voice_.reset(); }
    void set(size_t i, float v) override {
        values_[i]=v;
        switch(i) {
        case 0: voice_->controlChange(2,v); break;
        case 1: voice_->controlChange(4,v); break;
        case 2: voice_->controlChange(11,v); break;
        case 3: voice_->controlChange(1,v); break;
        }
    }
    void note(int status,int a,int b) override {
        if (status==0x90 && b) {
            note_=a;
            voice_->noteOn(440 * std::exp2((note_+bend_-69)/12.0), b/127.0);
        } else if (status==0x80 || (status==0x90 && !b)) {
            voice_->noteOff(0.5);
        } else if (status==0xe0) {
            bend_=(a+128*b-8192)*(2.f/8192);
            voice_->setFrequency(440 * std::exp2((note_+bend_-69)/12.0));
        }
    }
    void reset() override { voice_->clear(); voice_->noteOff(0.5); }
    void process(float* left,float* right,size_t frames) override {
        for (size_t i=0; i<frames; ++i) left[i]=right[i]=voice_->tick();
    }
};
CrestProcessor* make_stk_Wurley(float rate,size_t frames) { return new StkWurley; }
