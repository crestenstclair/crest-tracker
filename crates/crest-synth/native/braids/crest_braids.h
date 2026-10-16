#ifndef CREST_BRAIDS_H_
#define CREST_BRAIDS_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
#define CREST_BRAIDS_NOEXCEPT noexcept
extern "C" {
#else
#define CREST_BRAIDS_NOEXCEPT
#endif

typedef struct CrestBraidsBank CrestBraidsBank;

enum CrestBraidsStatus {
  CREST_BRAIDS_OK = 0,
  CREST_BRAIDS_NULL_BANK = 1,
  CREST_BRAIDS_INVALID_VOICE = 2,
  CREST_BRAIDS_INVALID_MODEL = 3,
  CREST_BRAIDS_INVALID_FRAME_COUNT = 4,
  CREST_BRAIDS_NULL_OUTPUT = 5,
};

CrestBraidsBank* crest_braids_bank_create(size_t voice_count) CREST_BRAIDS_NOEXCEPT;
void crest_braids_bank_destroy(CrestBraidsBank* bank) CREST_BRAIDS_NOEXCEPT;

size_t crest_braids_voice_count(const CrestBraidsBank* bank) CREST_BRAIDS_NOEXCEPT;
int crest_braids_voice_reset(CrestBraidsBank* bank, size_t voice)
    CREST_BRAIDS_NOEXCEPT;
int crest_braids_voice_configure(CrestBraidsBank* bank,
                                 size_t voice,
                                 uint8_t model,
                                 int16_t pitch,
                                 int16_t timbre,
                                 int16_t color) CREST_BRAIDS_NOEXCEPT;
int crest_braids_voice_strike(CrestBraidsBank* bank, size_t voice)
    CREST_BRAIDS_NOEXCEPT;
int crest_braids_voice_render(CrestBraidsBank* bank,
                              size_t voice,
                              int16_t* output,
                              size_t frame_count) CREST_BRAIDS_NOEXCEPT;

uint64_t crest_braids_banks_created(void) CREST_BRAIDS_NOEXCEPT;
uint64_t crest_braids_banks_destroyed(void) CREST_BRAIDS_NOEXCEPT;
uint64_t crest_braids_banks_active(void) CREST_BRAIDS_NOEXCEPT;

#ifdef __cplusplus
}
#endif

#undef CREST_BRAIDS_NOEXCEPT

#endif  // CREST_BRAIDS_H_
