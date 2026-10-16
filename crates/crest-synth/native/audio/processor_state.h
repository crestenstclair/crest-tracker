#pragma once
#include <cstdint>
#include <random>

// Prepared instance state selected at each native boundary. PRNG streams and
// sample rate belong to the processor, never to a shared device/global clock.
struct CrestProcessorState {
    std::minstd_rand standard{1};
    uint32_t mutable_state=0x21;
    double sample_rate=0; // Zero preserves STK's global rate for reference use.
};
bool crest_processor_initialize() noexcept;
#ifdef __APPLE__
CrestProcessorState& crest_processor_state() noexcept;
#else
// Constant-initialized POD TLS needs no first-use allocation. Keeping its
// accessor inline avoids a native call for every upstream random draw.
namespace crest_native_detail {
inline thread_local CrestProcessorState* current = nullptr;
}
inline CrestProcessorState& crest_processor_state() noexcept {
    return *crest_native_detail::current;
}
#endif
CrestProcessorState* crest_enter_processor_state(CrestProcessorState&) noexcept;
void crest_leave_processor_state(CrestProcessorState*) noexcept;
double crest_stk_sample_rate(double fallback) noexcept;

class CrestProcessorScope {
    CrestProcessorState* previous_;
public:
    explicit CrestProcessorScope(CrestProcessorState& state) noexcept
        : previous_(crest_enter_processor_state(state)) {}
    ~CrestProcessorScope() { crest_leave_processor_state(previous_); }
    CrestProcessorScope(const CrestProcessorScope&) = delete;
    CrestProcessorScope& operator=(const CrestProcessorScope&) = delete;
};
