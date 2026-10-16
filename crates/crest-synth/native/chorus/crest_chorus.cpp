#include "crest_chorus.h"

#include <atomic>
#include <cmath>
#include <new>

#include "rings/dsp/fx/chorus.h"

namespace {

constexpr size_t kDelaySamples = 2048;
constexpr int32_t kOk = 0;
constexpr int32_t kInvalidArgument = 1;
constexpr int32_t kCapacityExceeded = 2;
constexpr int32_t kNonFiniteOutput = 3;

std::atomic<uint64_t> g_created(0);
std::atomic<uint64_t> g_destroyed(0);
std::atomic<uint64_t> g_active(0);

}  // namespace

struct CrestChorus {
  rings::Chorus processor;
  uint16_t* delay;
  float* left;
  float* right;
  size_t max_frames;
};

extern "C" CrestChorus* crest_chorus_create(size_t max_frames) {
  if (max_frames == 0) {
    return nullptr;
  }
  CrestChorus* chorus = new (std::nothrow) CrestChorus();
  if (chorus == nullptr) {
    return nullptr;
  }
  chorus->delay = new (std::nothrow) uint16_t[kDelaySamples];
  chorus->left = new (std::nothrow) float[max_frames];
  chorus->right = new (std::nothrow) float[max_frames];
  chorus->max_frames = max_frames;
  if (chorus->delay == nullptr || chorus->left == nullptr || chorus->right == nullptr) {
    delete[] chorus->right;
    delete[] chorus->left;
    delete[] chorus->delay;
    delete chorus;
    return nullptr;
  }
  chorus->processor.Init(chorus->delay);
  chorus->processor.set_amount(0.5f);
  chorus->processor.set_depth(0.5f);
  g_created.fetch_add(1, std::memory_order_relaxed);
  g_active.fetch_add(1, std::memory_order_relaxed);
  return chorus;
}

extern "C" void crest_chorus_destroy(CrestChorus* chorus) {
  if (chorus == nullptr) {
    return;
  }
  delete[] chorus->right;
  delete[] chorus->left;
  delete[] chorus->delay;
  delete chorus;
  g_destroyed.fetch_add(1, std::memory_order_relaxed);
  g_active.fetch_sub(1, std::memory_order_relaxed);
}

extern "C" int32_t crest_chorus_process(
    CrestChorus* chorus,
    float* interleaved_stereo,
    size_t frame_count,
    float amount,
    float depth) {
  if (chorus == nullptr || interleaved_stereo == nullptr ||
      !std::isfinite(amount) || !std::isfinite(depth) ||
      amount < 0.0f || amount > 1.0f || depth < 0.0f || depth > 1.0f) {
    return kInvalidArgument;
  }
  if (frame_count > chorus->max_frames) {
    return kCapacityExceeded;
  }
  for (size_t frame = 0; frame < frame_count; ++frame) {
    chorus->left[frame] = interleaved_stereo[frame * 2];
    chorus->right[frame] = interleaved_stereo[frame * 2 + 1];
  }
  chorus->processor.set_amount(amount);
  chorus->processor.set_depth(depth);
  chorus->processor.Process(chorus->left, chorus->right, frame_count);
  for (size_t frame = 0; frame < frame_count; ++frame) {
    const float left = chorus->left[frame];
    const float right = chorus->right[frame];
    if (!std::isfinite(left) || !std::isfinite(right)) {
      return kNonFiniteOutput;
    }
    interleaved_stereo[frame * 2] = left;
    interleaved_stereo[frame * 2 + 1] = right;
  }
  return kOk;
}

extern "C" uint64_t crest_chorus_instances_created(void) {
  return g_created.load(std::memory_order_relaxed);
}

extern "C" uint64_t crest_chorus_instances_destroyed(void) {
  return g_destroyed.load(std::memory_order_relaxed);
}

extern "C" uint64_t crest_chorus_instances_active(void) {
  return g_active.load(std::memory_order_relaxed);
}
