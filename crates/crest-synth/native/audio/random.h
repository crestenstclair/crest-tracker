#pragma once
#include <cstdlib>
#include <cstdint>
#include <random>
#include <algorithm>
#include <stdlib.h>
// Parse both standard-library interfaces before adapting upstream rand calls.
// libstdc++'s algorithm and C compatibility headers still refer to std::rand.
#include "processor_state.h"
// Select the prepared instance's original PRNG without libc rand locking.
inline int crest_audio_random() noexcept { return static_cast<int>(crest_processor_state().standard()); }
inline void crest_audio_seed(unsigned seed) noexcept { crest_processor_state().standard.seed(seed); }
#define rand crest_audio_random
#define srand crest_audio_seed
