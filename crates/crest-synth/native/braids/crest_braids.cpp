#include "crest_braids.h"

#include <atomic>
#include <cstdlib>
#include <cstring>
#include <new>

#include "braids/macro_oscillator.h"
#include "braids/settings.h"
#include "stmlib/utils/random.h"

namespace {

constexpr size_t kMaximumRenderFrames = 24;
constexpr uint8_t kPlayableModelCount = 47;
constexpr uint32_t kInitialRandomState = 0x21u;

std::atomic<uint64_t> g_banks_created(0);
std::atomic<uint64_t> g_banks_destroyed(0);
std::atomic<uint64_t> g_banks_active(0);

}  // namespace

struct CrestBraidsBank {
  braids::MacroOscillator* voices;
  size_t voice_count;
  uint8_t sync[kMaximumRenderFrames];
  uint32_t random_state;
};

namespace {

braids::MacroOscillator* Voice(CrestBraidsBank* bank, size_t index) noexcept {
  return bank->voices + index;
}

int ValidateVoice(CrestBraidsBank* bank, size_t voice) noexcept {
  if (bank == nullptr) {
    return CREST_BRAIDS_NULL_BANK;
  }
  if (voice >= bank->voice_count) {
    return CREST_BRAIDS_INVALID_VOICE;
  }
  return CREST_BRAIDS_OK;
}

}  // namespace

CrestBraidsBank* crest_braids_bank_create(size_t voice_count) noexcept {
  if (!voice_count || voice_count > SIZE_MAX / sizeof(braids::MacroOscillator)) return nullptr;
  void* allocation = std::calloc(1, sizeof(CrestBraidsBank));
  if (allocation == nullptr) {
    return nullptr;
  }

  CrestBraidsBank* bank = static_cast<CrestBraidsBank*>(allocation);
  bank->voices = static_cast<braids::MacroOscillator*>(std::calloc(voice_count, sizeof(braids::MacroOscillator)));
  if (!bank->voices) { std::free(bank); return nullptr; }
  bank->voice_count = voice_count;
  bank->random_state = kInitialRandomState;
  for (size_t index = 0; index < bank->voice_count; ++index) {
    braids::MacroOscillator* oscillator =
        new (bank->voices + index) braids::MacroOscillator();
    oscillator->Init();
    oscillator->set_shape(braids::MACRO_OSC_SHAPE_CSAW);
    oscillator->set_pitch(60 * 128);
    oscillator->set_parameters(0, 0);
  }

  g_banks_created.fetch_add(1, std::memory_order_relaxed);
  g_banks_active.fetch_add(1, std::memory_order_relaxed);
  return bank;
}

void crest_braids_bank_destroy(CrestBraidsBank* bank) noexcept {
  if (bank == nullptr) {
    return;
  }
  for (size_t index = bank->voice_count; index > 0; --index) {
    Voice(bank, index - 1)->~MacroOscillator();
  }
  std::free(bank->voices);
  std::free(bank);
  g_banks_destroyed.fetch_add(1, std::memory_order_relaxed);
  g_banks_active.fetch_sub(1, std::memory_order_relaxed);
}

size_t crest_braids_voice_count(const CrestBraidsBank* bank) noexcept { return bank ? bank->voice_count : 0; }

int crest_braids_voice_reset(CrestBraidsBank* bank, size_t voice) noexcept {
  const int status = ValidateVoice(bank, voice);
  if (status != CREST_BRAIDS_OK) {
    return status;
  }
  Voice(bank, voice)->Init();
  return CREST_BRAIDS_OK;
}

int crest_braids_voice_configure(CrestBraidsBank* bank,
                                 size_t voice,
                                 uint8_t model,
                                 int16_t pitch,
                                 int16_t timbre,
                                 int16_t color) noexcept {
  const int status = ValidateVoice(bank, voice);
  if (status != CREST_BRAIDS_OK) {
    return status;
  }
  if (model >= kPlayableModelCount) {
    return CREST_BRAIDS_INVALID_MODEL;
  }

  braids::MacroOscillator* oscillator = Voice(bank, voice);
  oscillator->set_shape(static_cast<braids::MacroOscillatorShape>(model));
  oscillator->set_pitch(pitch);
  oscillator->set_parameters(timbre, color);
  return CREST_BRAIDS_OK;
}

int crest_braids_voice_strike(CrestBraidsBank* bank, size_t voice) noexcept {
  const int status = ValidateVoice(bank, voice);
  if (status != CREST_BRAIDS_OK) {
    return status;
  }
  Voice(bank, voice)->Strike();
  return CREST_BRAIDS_OK;
}

int crest_braids_voice_render(CrestBraidsBank* bank,
                              size_t voice,
                              int16_t* output,
                              size_t frame_count) noexcept {
  const int status = ValidateVoice(bank, voice);
  if (status != CREST_BRAIDS_OK) {
    return status;
  }
  if (output == nullptr) {
    return CREST_BRAIDS_NULL_OUTPUT;
  }
  if (frame_count == 0 || frame_count > kMaximumRenderFrames) {
    return CREST_BRAIDS_INVALID_FRAME_COUNT;
  }

  std::memset(bank->sync, 0, frame_count);
  stmlib::Random::Seed(bank->random_state);
  Voice(bank, voice)->Render(bank->sync, output, frame_count);
  bank->random_state = stmlib::Random::state();
  return CREST_BRAIDS_OK;
}

uint64_t crest_braids_banks_created(void) noexcept {
  return g_banks_created.load(std::memory_order_relaxed);
}

uint64_t crest_braids_banks_destroyed(void) noexcept {
  return g_banks_destroyed.load(std::memory_order_relaxed);
}

uint64_t crest_braids_banks_active(void) noexcept {
  return g_banks_active.load(std::memory_order_relaxed);
}
