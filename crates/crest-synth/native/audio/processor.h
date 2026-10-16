#pragma once
#include <cstddef>
#include <cstdint>
#include <vector>
#include <memory>
#include <stdexcept>

// Complete upstream processor adapter. Instances and scratch are prepared and
// destroyed off audio; set/note/process only touch instance-owned storage.
struct CrestProcessor {
    virtual ~CrestProcessor() = default;
    virtual size_t count() const = 0;
    virtual const char* label(size_t index) const = 0;
    virtual float initial(size_t index) const = 0;
    virtual float minimum(size_t) const { return 0; }
    virtual float maximum(size_t) const { return 1; }
    virtual bool stepped(size_t) const { return false; }
    virtual size_t latency() const { return 0; }
    virtual bool healthy() const { return true; }
    virtual void set(size_t index, float value) = 0;
    virtual void note(int status, int a, int b) {}
    // Called only during worker-side asset preparation.
    virtual bool load(const uint8_t*, size_t) { return false; }
    virtual bool load_sample(const uint8_t*, size_t, size_t) { return false; }
    virtual bool load_ir(const float*, size_t, size_t, float) { return false; }
    virtual void reset() {}
    virtual void process(float* left, float* right, size_t frames) = 0;
};
