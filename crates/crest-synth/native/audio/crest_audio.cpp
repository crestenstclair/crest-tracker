#include "processor.h"
#include "rate_adapter.h"
#include <cmath>
#include <algorithm>
#include <cstring>
#include "processor_state.h"
#define CREST_FACTORY(symbol, instrument, label) CrestProcessor* make_##symbol(float, size_t);
#include "plugin_factories.inc"
#include "daisy_factories.inc"
#include "stk_factories.inc"
#include "mutable_factories.inc"
#include "specialist_factories.inc"
#undef CREST_FACTORY
struct Factory {
    const char* id;
    const char* name;
    bool instrument;
    CrestProcessor* (*make)(float, size_t);
};
static const Factory factories[] = {
#define CREST_FACTORY(symbol, instrument, label) {#symbol, label, instrument, make_##symbol},
#include "plugin_factories.inc"
#include "daisy_factories.inc"
#include "stk_factories.inc"
#include "mutable_factories.inc"
#include "specialist_factories.inc"
#undef CREST_FACTORY
};
struct Handle {
    CrestProcessorState state;
    std::unique_ptr<CrestProcessor> processor;
    std::vector<float> left, right, parameters;
    Handle(std::unique_ptr<CrestProcessor> p, size_t n, CrestProcessorState state): state(state), processor(std::move(p)), left(n+1), right(n+1), parameters(processor->count(), NAN) {}
};
extern "C" {
size_t crest_audio_count() noexcept { return sizeof(factories)/sizeof(Factory); }
const char* crest_audio_id(size_t i) noexcept { return i < crest_audio_count() ? factories[i].id : nullptr; }
const char* crest_audio_name(size_t i) noexcept { return i < crest_audio_count() ? factories[i].name : nullptr; }
bool crest_audio_is_instrument(size_t i) noexcept { return i < crest_audio_count() && factories[i].instrument; }
void* crest_audio_create_scheduled(size_t i, float rate, size_t frames, unsigned phase) noexcept {
    if (i >= crest_audio_count() || !std::isfinite(rate) || rate < 8000 || frames == 0 || frames > INT32_MAX) return nullptr;
    if (!crest_processor_initialize()) return nullptr;
    try {
        RateAdapter::PreparationScope scheduling(phase);
        CrestProcessorState state;
        state.sample_rate=rate;
        std::unique_ptr<CrestProcessor> processor;
        { CrestProcessorScope scope(state); processor.reset(factories[i].make(rate,frames)); }
        return new Handle(std::move(processor),frames,state);
    }
    catch (...) { return nullptr; }
}
void* crest_audio_create(size_t i, float rate, size_t frames) noexcept {
    return crest_audio_create_scheduled(i, rate, frames, 0);
}
bool crest_audio_load_sample(void* h,const uint8_t* bytes,size_t size,size_t frames) noexcept {
    try { auto& v=*static_cast<Handle*>(h); CrestProcessorScope scope(v.state); return v.processor->load_sample(bytes,size,frames); } catch (...) {return false;}
}
bool crest_audio_load_ir(void* h,const float* pcm,size_t frames,size_t channels,float rate) noexcept {
    try { auto& v=*static_cast<Handle*>(h); CrestProcessorScope scope(v.state); return v.processor->load_ir(pcm,frames,channels,rate); } catch (...) { return false; }
}
bool crest_audio_load(void* h,const uint8_t* bytes,size_t size) noexcept {
    try { auto& v=*static_cast<Handle*>(h); CrestProcessorScope scope(v.state); return v.processor->load(bytes,size); } catch (...) { return false; }
}
size_t crest_audio_latency(void* h) noexcept { return static_cast<Handle*>(h)->processor->latency(); }
void crest_audio_destroy(void* h) noexcept { delete static_cast<Handle*>(h); }
size_t crest_audio_param_count(void* h) noexcept { return static_cast<Handle*>(h)->processor->count(); }
const char* crest_audio_param_label(void* h, size_t i) noexcept { return static_cast<Handle*>(h)->processor->label(i); }
float crest_audio_param_default(void* h, size_t i) noexcept { return static_cast<Handle*>(h)->processor->initial(i); }
float crest_audio_param_min(void* h, size_t i) noexcept { return static_cast<Handle*>(h)->processor->minimum(i); }
float crest_audio_param_max(void* h, size_t i) noexcept { return static_cast<Handle*>(h)->processor->maximum(i); }
bool crest_audio_param_stepped(void* h, size_t i) noexcept { return static_cast<Handle*>(h)->processor->stepped(i); }
bool crest_audio_set(void* h, const float* params, size_t count) noexcept {
    auto& handle = *static_cast<Handle*>(h); CrestProcessorScope scope(handle.state);
    auto& p = *handle.processor;
    if (count != p.count()) return false;
    auto& previous=handle.parameters;
    for (size_t i=0; i<count; ++i) {
        // Descriptors are fixed for a prepared processor. Equal values already
        // passed this boundary; reset invalidates the cache with NaNs. Validate
        // every changed value before applying any part of the new snapshot.
        if (previous[i]==params[i]) continue;
        if (!std::isfinite(params[i]) || params[i] < p.minimum(i) || params[i] > p.maximum(i) || (p.stepped(i) && std::floor(params[i]) != params[i])) return false;
    }
    for (size_t i=0; i<count; ++i) if (previous[i]!=params[i]) { p.set(i, params[i]); previous[i]=params[i]; }
    return true;
}
void crest_audio_note(void* h, int status, int a, int b) noexcept { auto& v=*static_cast<Handle*>(h); CrestProcessorScope scope(v.state); v.processor->note(status,a,b); }
void crest_audio_reset(void* h) noexcept { auto& v=*static_cast<Handle*>(h); CrestProcessorScope scope(v.state); v.processor->reset(); std::fill(v.parameters.begin(),v.parameters.end(),NAN); }
bool crest_audio_process(void* h, float* stereo, size_t frames) noexcept {
    auto& v = *static_cast<Handle*>(h); CrestProcessorScope scope(v.state);
    if (frames >= v.left.size()) return false;
    for (size_t i=0; i<frames; ++i) { v.left[i+1]=stereo[2*i]; v.right[i+1]=stereo[2*i+1]; }
    v.processor->process(v.left.data()+1, v.right.data()+1, frames);
    if (!v.processor->healthy()) return false;
    // Complete the finite-output check before exposing audio. A reduction
    // permits SIMD without weakening validation or short-circuiting channels.
    bool finite=true;
    for (size_t i=0; i<frames; ++i)
        finite &= std::isfinite(v.left[i+1]) & std::isfinite(v.right[i+1]);
    if (!finite) return false;
    for (size_t i=0; i<frames; ++i) {
        stereo[2*i]=v.left[i+1]; stereo[2*i+1]=v.right[i+1];
    }
    return true;
}
}
