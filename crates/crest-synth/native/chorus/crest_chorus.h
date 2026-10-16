#ifndef CREST_CHORUS_H_
#define CREST_CHORUS_H_

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct CrestChorus CrestChorus;

CrestChorus* crest_chorus_create(size_t max_frames);
void crest_chorus_destroy(CrestChorus* chorus);
int32_t crest_chorus_process(
    CrestChorus* chorus,
    float* interleaved_stereo,
    size_t frame_count,
    float amount,
    float depth);
uint64_t crest_chorus_instances_created(void);
uint64_t crest_chorus_instances_destroyed(void);
uint64_t crest_chorus_instances_active(void);

#ifdef __cplusplus
}
#endif

#endif
