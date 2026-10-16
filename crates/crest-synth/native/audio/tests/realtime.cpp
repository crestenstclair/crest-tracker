// Allocation witness for the exact native archives linked by Crest.
#include <new>
#include <cstdlib>
#include <cstdio>
#include <cmath>
#include <cstring>
#include <vector>
#include <cstdint>
#include <thread>
#include <atomic>
#include <pthread.h>
static std::atomic<bool> measuring{false};
static std::atomic<pthread_t> measured_thread;
static size_t allocations=0,destructions=0,heap_operations=0,locks=0;
static bool is_measuring() {
    return measuring.load(std::memory_order_acquire) && pthread_equal(measured_thread.load(std::memory_order_relaxed),pthread_self());
}
#if defined(__APPLE__) || defined(__linux__)
#include <pthread.h>
extern "C" void crest_witness_begin();
extern "C" void crest_witness_end(size_t*,size_t*);
#endif
static void begin_measurement() {
    allocations=destructions=heap_operations=locks=0;
    measured_thread.store(pthread_self(),std::memory_order_relaxed);
    measuring.store(true,std::memory_order_release);
#if defined(__APPLE__) || defined(__linux__)
    crest_witness_begin();
#endif
}
static void end_measurement() {
#if defined(__APPLE__) || defined(__linux__)
    crest_witness_end(&heap_operations,&locks);
#endif
    measuring.store(false,std::memory_order_release);
}
void* operator new(size_t n) { if(is_measuring())++allocations; if(auto* p=std::malloc(n?n:1))return p;throw std::bad_alloc(); }
void* operator new[](size_t n) {return ::operator new(n);}
void operator delete(void* p) noexcept {if(p&&is_measuring())++destructions;std::free(p);}
void operator delete[](void* p) noexcept {::operator delete(p);}
void operator delete(void* p,size_t) noexcept {::operator delete(p);}
void operator delete[](void* p,size_t) noexcept {::operator delete(p);}
void* operator new(size_t n,std::align_val_t alignment) {if(is_measuring())++allocations;void* p=nullptr;if(posix_memalign(&p,static_cast<size_t>(alignment),n?n:1))throw std::bad_alloc();return p;}
void* operator new[](size_t n,std::align_val_t a){return ::operator new(n,a);}
void operator delete(void* p,std::align_val_t) noexcept{::operator delete(p);}
void operator delete[](void* p,std::align_val_t) noexcept{::operator delete(p);}
void operator delete(void* p,size_t,std::align_val_t) noexcept{::operator delete(p);}
void operator delete[](void* p,size_t,std::align_val_t) noexcept{::operator delete(p);}
extern "C" {
size_t crest_audio_count();
const char* crest_audio_id(size_t);
bool crest_audio_is_instrument(size_t);
void* crest_audio_create(size_t,float,size_t);
void crest_audio_destroy(void*);
size_t crest_audio_param_count(void*);
float crest_audio_param_default(void*,size_t);
float crest_audio_param_min(void*,size_t);
float crest_audio_param_max(void*,size_t);
bool crest_audio_param_stepped(void*,size_t);
bool crest_audio_set(void*,const float*,size_t);
void crest_audio_note(void*,int,int,int);
void crest_audio_reset(void*);
bool crest_audio_process(void*,float*,size_t);
bool crest_audio_load_sample(void*,const uint8_t*,size_t,size_t);
}
#include "crest_sample_default.h"
bool sfizz_midi_flush_witness();
bool daisy_coefficient_witness();
bool r8brain_scheduling_witness();
bool rate_adapter_witness();
bool mutable_resonator_witness();
bool stk_model_witness();
bool stk_sample_rate_witness();
static bool parameter_validation_witness() {
    size_t index=0;
    while(index<crest_audio_count() && std::strcmp(crest_audio_id(index),"daisy_AnalogBassDrum")) ++index;
    if(index==crest_audio_count()) return false;
    void* tested=crest_audio_create(index,48000,64);
    void* reference=crest_audio_create(index,48000,64);
    if(!tested || !reference) { crest_audio_destroy(tested); crest_audio_destroy(reference); return false; }
    const size_t count=crest_audio_param_count(tested);
    std::vector<float> valid(count);
    for(size_t i=0; i<count; ++i) valid[i]=crest_audio_param_default(tested,i);
    bool success=crest_audio_set(tested,valid.data(),count) && crest_audio_set(reference,valid.data(),count);
    auto invalid=valid;
    invalid[0]=1000; // A valid early edit must not survive a rejected late edit.
    for(float rejected : {NAN,INFINITY,-1.f,2.f,.5f}) {
        invalid.back()=rejected; // Sustain: integral 0 or 1 only.
        success &= !crest_audio_set(tested,invalid.data(),count);
    }
    success &= !crest_audio_set(tested,valid.data(),count-1);
    crest_audio_note(tested,0x90,60,100); crest_audio_note(reference,0x90,60,100);
    for(size_t block=0; block<32; ++block) {
        float a[128]{},b[128]{};
        success &= crest_audio_process(tested,a,64) && crest_audio_process(reference,b,64);
        success &= std::memcmp(a,b,sizeof(a))==0;
    }
    // Reset invalidates the cached snapshot, including unchanged scalar values.
    crest_audio_reset(tested); crest_audio_reset(reference);
    success &= crest_audio_set(tested,valid.data(),count) && crest_audio_set(reference,valid.data(),count);
    crest_audio_note(tested,0x90,60,100); crest_audio_note(reference,0x90,60,100);
    float a[128]{},b[128]{};
    success &= crest_audio_process(tested,a,64) && crest_audio_process(reference,b,64);
    success &= std::memcmp(a,b,sizeof(a))==0;
    crest_audio_destroy(tested); crest_audio_destroy(reference);
    if(!success) std::printf("NATIVE PARAMETER VALIDATION FAILED\n");
    return success;
}
// Verify instrumentation before accepting a zero-operation measurement.
static bool counter_self_test() {
    allocations=destructions=heap_operations=locks=0;
    begin_measurement();
    void* cpp=::operator new(64);
    ::operator delete(cpp);
    end_measurement();
    if(allocations!=1||destructions!=1){std::printf("SELF C++ new=%zu delete=%zu\n",allocations,destructions);return false;}
#if defined(__APPLE__) || defined(__linux__)
    pthread_mutex_t mutex=PTHREAD_MUTEX_INITIALIZER;
    pthread_rwlock_t rwlock=PTHREAD_RWLOCK_INITIALIZER;
    void* (*volatile allocate)(size_t)=malloc;
    void* (*volatile zero_allocate)(size_t,size_t)=calloc;
    void* (*volatile resize)(void*,size_t)=realloc;
    void (*volatile release)(void*)=free;
    heap_operations=locks=0;
    begin_measurement();
    void* p=allocate(64);p=resize(p,128);release(p);
    p=zero_allocate(2,64);release(p);
    pthread_mutex_lock(&mutex);pthread_mutex_unlock(&mutex);
    pthread_rwlock_rdlock(&rwlock);pthread_rwlock_unlock(&rwlock);
    pthread_rwlock_wrlock(&rwlock);pthread_rwlock_unlock(&rwlock);
    end_measurement();
    pthread_mutex_destroy(&mutex);pthread_rwlock_destroy(&rwlock);
    if(heap_operations!=5||locks!=3){std::printf("SELF C heap=%zu locks=%zu\n",heap_operations,locks);return false;}
#endif
    return true;
}
static bool idle_bend_witness(size_t index, float rate) {
    void* reference=crest_audio_create(index,rate,64);
    void* actual=crest_audio_create(index,rate,64);
    if(!reference || !actual) { crest_audio_destroy(reference); crest_audio_destroy(actual); return false; }
    std::vector<float> values(crest_audio_param_count(reference));
    for(size_t i=0;i<values.size();++i) values[i]=crest_audio_param_default(reference,i);
    bool same=true;
    for(void* p:{reference,actual}) {
        if(!std::strcmp(crest_audio_id(index),"sfizz_Sample"))
            same &= crest_audio_load_sample(p,reinterpret_cast<const uint8_t*>(crest_sample_default),sizeof(crest_sample_default)-1,48000);
        same &= crest_audio_set(p,values.data(),values.size());
        crest_audio_note(p,0x90,48,96);
    }
    for(int block=0;block<24;++block) {
        float a[128]{},b[128]{};
        same &= crest_audio_process(reference,a,64) && crest_audio_process(actual,b,64);
    }
    // Once the host envelope is idle, DSP no longer renders. Extra bends before
    // reuse must not affect sound after reset, current bend, and the next note.
    for(int bend:{0,127,32,96}) crest_audio_note(reference,0xe0,0,bend);
    for(void* p:{reference,actual}) {
        crest_audio_reset(p); same &= crest_audio_set(p,values.data(),values.size());
        crest_audio_note(p,0xe0,17,76); crest_audio_note(p,0x90,60,96);
    }
    for(int block=0;block<64;++block) {
        float a[128]{},b[128]{};
        same &= crest_audio_process(reference,a,64) && crest_audio_process(actual,b,64);
        same &= std::memcmp(a,b,sizeof(a))==0;
    }
    crest_audio_destroy(reference); crest_audio_destroy(actual);
    return same;
}
int main(){
    if(!counter_self_test()){std::printf("COUNTER SELF-TEST FAILED\n");return 1;}
    if(!sfizz_midi_flush_witness()){std::printf("SFIZZ MIDI FLUSH FAILED\n");return 1;}
    if(!daisy_coefficient_witness()){return 1;}
    if(!r8brain_scheduling_witness()){return 1;}
    if(!rate_adapter_witness()){return 1;}
    if(!mutable_resonator_witness()){return 1;}
    if(!stk_model_witness()){return 1;}
    if(!stk_sample_rate_witness()){return 1;}
    if(!parameter_validation_witness()){return 1;}
    size_t failures=0,checked=0;
    for(float rate:{44100.f,48000.f,96000.f})for(size_t index=0;index<crest_audio_count();++index){
        const char* id=crest_audio_id(index);auto* p=crest_audio_create(index,rate,256);
        if(!p){std::printf("CREATE %s %.0f\n",id,rate);++failures;continue;}
        if(!std::strcmp(id,"sfizz_Sample")&&!crest_audio_load_sample(p,reinterpret_cast<const uint8_t*>(crest_sample_default),sizeof(crest_sample_default)-1,48000)){++failures;}
        const size_t count=crest_audio_param_count(p);std::vector<float> values(count);
        for(size_t i=0;i<count;++i)values[i]=crest_audio_param_default(p,i);
        // Preparation occurs on the control thread. Measure first callback use
        // on a fresh thread, including upstream thread-local initialization.
        std::thread audio_thread([&] {
        int failed_parameter=-2;float failed_value=0;bool valid=true;float stereo[512]{};allocations=destructions=heap_operations=locks=0;
        begin_measurement();
        crest_audio_reset(p);valid &= crest_audio_set(p,values.data(),count);
        if(crest_audio_is_instrument(index))crest_audio_note(p,0x90,60,100);
        for(size_t block=0;block<32;++block){
            const size_t frames=block%4==0?1:block%4==1?17:block%4==2?127:256;
            for(size_t i=0;i<frames*2;++i)stereo[i]=std::sin(float(i+block)*.07f)*.1f;
            valid &= crest_audio_process(p,stereo,frames);
        }
        for(size_t i=0;i<count;++i){
            const float saved=values[i];
            for(float value:{crest_audio_param_min(p,i),crest_audio_param_max(p,i)}){
                values[i]=value;valid &= crest_audio_set(p,values.data(),count);
                const bool result=crest_audio_process(p,stereo,256); valid &= result;
                if(!result && failed_parameter==-2){failed_parameter=int(i);failed_value=value;}
            }
            if(crest_audio_param_stepped(p,i)) {
                for(float value=crest_audio_param_min(p,i);value<=crest_audio_param_max(p,i);value+=1) {
                    values[i]=value;valid &= crest_audio_set(p,values.data(),count);
                    if(crest_audio_is_instrument(index))crest_audio_note(p,0x90,60,100);
                    for(int b=0;b<4;++b)valid &= crest_audio_process(p,stereo,256);
                    if(crest_audio_is_instrument(index))crest_audio_note(p,0x80,60,0);
                }
            }
            values[i]=saved;valid &= crest_audio_set(p,values.data(),count);
        }
        if(crest_audio_is_instrument(index)){
            for(int note:{0,36,96,127}){
                crest_audio_reset(p);valid &= crest_audio_set(p,values.data(),count);
                crest_audio_note(p,0xe0,0,0);crest_audio_note(p,0x90,note,127);
                for(int b=0;b<8;++b){const bool result=crest_audio_process(p,stereo,256);valid &= result;if(!result&&failed_parameter==-2){failed_parameter=-3-note;}}
                crest_audio_note(p,0x80,note,0);
            }
        }
        // The Daisy snare derivatives could diverge only after sustained
        // rendering at interior frequencies; short endpoint probes missed it.
        if(!std::strcmp(id,"mutable_SyntheticSnare") || !std::strcmp(id,"mutable_AnalogSnare")) {
            for(float frequency:{200.f,2495.4f,10000.f})for(int note:{60,84,127})for(float sustain:{0.f,1.f}) {
                crest_audio_reset(p);
                values[0]=frequency;values[4]=sustain;
                valid &= crest_audio_set(p,values.data(),count);
                crest_audio_note(p,0xe0,0,64);
                crest_audio_note(p,0x90,note,127);
                double energy=0;
                for(size_t block=0;block<size_t(rate*2)/256+1;++block) {
                    const bool result=crest_audio_process(p,stereo,256);
                    valid &= result;
                    if(!result && failed_parameter==-2){failed_parameter=0;failed_value=frequency;}
                    for(float sample:stereo)energy+=double(sample)*sample;
                }
                valid &= std::isfinite(energy) && energy>1e-10;
                crest_audio_note(p,0x80,note,0);
            }
            for(size_t i=0;i<count;++i)values[i]=crest_audio_param_default(p,i);
        }
        crest_audio_reset(p);end_measurement();++checked;
        if(!valid||allocations||destructions||heap_operations||locks){std::printf("FAIL %s %.0f valid=%d parameter=%d value=%g new=%zu delete=%zu heap=%zu locks=%zu\n",id,rate,valid,failed_parameter,failed_value,allocations,destructions,heap_operations,locks);++failures;}
        });
        audio_thread.join();
        crest_audio_destroy(p);
        if(crest_audio_is_instrument(index) && !idle_bend_witness(index,rate)) {
            std::printf("IDLE BEND %s %.0f\n",id,rate); ++failures;
        }
        // A second instance must not perturb an already prepared instance's random sequence.
        if(std::strcmp(id,"sfizz_Sample")) {
            void* solo=crest_audio_create(index,rate,256);
            void* interleaved=crest_audio_create(index,rate,256);
            void* disturbance=crest_audio_create(index,rate,256);
            if(!solo||!interleaved||!disturbance) { ++failures; }
            else {
                for(auto* handle:{solo,interleaved,disturbance}) {
                    crest_audio_reset(handle);crest_audio_set(handle,values.data(),count);
                    if(crest_audio_is_instrument(index))crest_audio_note(handle,0x90,60,100);
                }
                bool independent=true;
                for(int block=0;block<24;++block) {
                    float a[512],b[512],noise[512];
                    for(int n=0;n<512;++n)a[n]=b[n]=noise[n]=std::sin(float(n+block)*.031f)*.1f;
                    independent &= crest_audio_process(solo,a,256);
                    independent &= crest_audio_process(disturbance,noise,256);
                    independent &= crest_audio_process(interleaved,b,256);
                    independent &= std::memcmp(a,b,sizeof(a))==0;
                }
                if(!independent){std::printf("INSTANCE %s %.0f\n",id,rate);++failures;}
            }
            crest_audio_destroy(solo);crest_audio_destroy(interleaved);crest_audio_destroy(disturbance);
        }
    }
    std::printf("Native witness: %zu preparations, %zu failures\n",checked,failures);return failures?1:0;
}
