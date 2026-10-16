#include "parameter_adapter.h"
#include "sfizz.h"
#include "crest_sample_default.h"
#include <string>
#include <array>
#include "sfizz_message.h"

class SfizzVoice : public ParameterAdapter {
    struct Delete { void operator()(sfizz_synth_t* synth) const { sfizz_free(synth); } };
    std::unique_ptr<sfizz_synth_t,Delete> synth_;
protected:
    sfizz_synth_t* synth(){return synth_.get();}
public:
    SfizzVoice(float rate,size_t frames):ParameterAdapter({}),synth_(sfizz_create_synth()) {
        if(!synth_)throw std::bad_alloc();
        sfizz_set_sample_rate(synth_.get(),rate);sfizz_set_samples_per_block(synth_.get(),frames);
        sfizz_set_volume(synth_.get(),0);
        if(!load(reinterpret_cast<const uint8_t*>(crest_sample_default),sizeof(crest_sample_default)-1))throw std::invalid_argument("invalid bundled sample");
    }
    bool load(const uint8_t* data,size_t size) override {
        std::string text(reinterpret_cast<const char*>(data),size);
        if(text.find('\0')!=std::string::npos)return false;
        if(!sfizz_load_string(synth_.get(),"/crest/embedded.sfz",text.c_str()))return false;
        char* unsupported=sfizz_get_unknown_opcodes(synth_.get());
        const bool known=!unsupported || unsupported[0]=='\0';
        sfizz_free_memory(unsupported);
        if(!known)return false;
        const int regions=sfizz_get_num_regions(synth_.get());
        if(regions<1 || regions>INT32_MAX/2)return false;
        // One host note may trigger layered and release regions. Capacity is
        // derived from this prepared library, not a fixed product voice cap.
        sfizz_set_num_voices(synth_.get(),regions*2);
        return true;
    }
    void note(int status,int a,int b) override {
        if(status==0x90&&b)sfizz_send_note_on(synth_.get(),0,a,b);
        else if(status==0x80||(status==0x90&&!b))sfizz_send_note_off(synth_.get(),0,a,b);
        else if(status==0xe0)sfizz_send_pitch_wheel(synth_.get(),0,a+128*b-8192);
        else if(status==0xb0)sfizz_send_cc(synth_.get(),0,a,b);
        else if(status==0xd0)sfizz_send_channel_aftertouch(synth_.get(),0,a);
    }
    void reset() override { sfizz_all_sound_off(synth_.get()); }
    void process(float* left,float* right,size_t frames) override {float* output[]={left,right};sfizz_render_block(synth_.get(),output,2,frames);}
};
CrestProcessor* make_sfizz_Sampler(float rate,size_t frames){return new SfizzVoice(rate,frames);}

// Existing Crest Sample schema, mapped to sfizz's public region controls.
// Playback, interpolation, looping, crossfade, and pitch remain upstream DSP.
class SfizzSample final : public SfizzVoice {
    struct DeleteClient { void operator()(sfizz_client_t* client) const { sfizz_delete_client(client); } };
    std::unique_ptr<sfizz_client_t,DeleteClient> client_{sfizz_create_client(nullptr)};
    size_t sample_frames_=0;
    std::array<float,7> values_{{60,0,1,0,0,1,0}};
    void integer(const char* path,int64_t value){sfizz_arg_t arg{};arg.h=value;sfizz_send_message(synth(),client_.get(),0,path,"h",&arg);}
public:
    SfizzSample(float rate,size_t frames):SfizzVoice(rate,frames){}
    size_t count() const override{return 7;}
    const char* label(size_t i) const override{static const char* names[]={"Root Note","Start","End","Loop","Loop Start","Loop End","Crossfade"};return names[i];}
    float initial(size_t i) const override{static const float v[]={60,0,1,0,0,1,0};return v[i];}
    float maximum(size_t i) const override{return i==0?127:i==6?200:1;}
    float minimum(size_t) const override{return 0;}
    bool stepped(size_t i) const override{return i==3;}
    bool load_sample(const uint8_t* data,size_t size,size_t frames) override{sample_frames_=frames;return load(data,size);}
    void set(size_t i,float value) override {
        values_[i]=value;if(!sample_frames_)return;
        sfizz_arg_t args[2]{};
        switch(i){
            case 0:{const int root=std::lround(value);args[0].i=root;sfizz_send_message(synth(),client_.get(),0,"/region0/pitch_keycenter","i",args);
                args[0].f=(root-value)*100;sfizz_send_message(synth(),client_.get(),0,"/region0/tune","f",args);break;}
            case 1:integer("/region0/offset",static_cast<int64_t>(value*sample_frames_));break;
            case 2:integer("/region0/end",std::max<int64_t>(0,static_cast<int64_t>(value*sample_frames_)-1));break;
            case 3:args[0].s=value==0?"no_loop":"loop_continuous";sfizz_send_message(synth(),client_.get(),0,"/region0/loop_mode","s",args);break;
            case 4:case 5:args[0].h=static_cast<int64_t>(values_[4]*sample_frames_);args[1].h=std::max<int64_t>(0,static_cast<int64_t>(values_[5]*sample_frames_)-1);sfizz_send_message(synth(),client_.get(),0,"/region0/loop_range","hh",args);break;
            case 6:args[0].f=value/1000;sfizz_send_message(synth(),client_.get(),0,"/region0/loop_crossfade","f",args);break;
        }
    }
};
CrestProcessor* make_sfizz_Sample(float rate,size_t frames){return new SfizzSample(rate,frames);}
