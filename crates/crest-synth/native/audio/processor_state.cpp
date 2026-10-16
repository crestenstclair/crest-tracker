#include "processor_state.h"
#include <atomic>
#ifdef __APPLE__
#include <pthread.h>
// Darwin lazily allocates C++ thread_local storage on first use. pthread TSD
// slots live in the existing pthread record and do not allocate on access.
// Allocate the key once during preparation, before any handle can render.
static pthread_key_t processor_key;
static pthread_once_t processor_once=PTHREAD_ONCE_INIT;
static std::atomic<bool> processor_ready{false};
static void create_processor_key() { processor_ready=pthread_key_create(&processor_key,nullptr)==0; }
bool crest_processor_initialize() noexcept {
    return pthread_once(&processor_once,create_processor_key)==0 && processor_ready;
}
static CrestProcessorState* current_state() noexcept { return static_cast<CrestProcessorState*>(pthread_getspecific(processor_key)); }
static void set_state(CrestProcessorState* state) noexcept { pthread_setspecific(processor_key,state); }
#else
// Constant-initialized POD TLS in the statically linked native library.
bool crest_processor_initialize() noexcept { return true; }
static CrestProcessorState* current_state() noexcept { return crest_native_detail::current; }
static void set_state(CrestProcessorState* state) noexcept { crest_native_detail::current=state; }
#endif
#ifdef __APPLE__
CrestProcessorState& crest_processor_state() noexcept { return *current_state(); }
#endif
CrestProcessorState* crest_enter_processor_state(CrestProcessorState& state) noexcept {
    auto* previous=current_state();set_state(&state);return previous;
}
void crest_leave_processor_state(CrestProcessorState* previous) noexcept { set_state(previous); }

double crest_stk_sample_rate(double fallback) noexcept {
#ifdef __APPLE__
    // Unscoped upstream witnesses may query STK before any Crest preparation.
    if(!processor_ready.load(std::memory_order_acquire)) return fallback;
#endif
    const auto* state=current_state();
    return state && state->sample_rate>0 ? state->sample_rate : fallback;
}
