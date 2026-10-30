#ifndef SIDECAST_RING_H
#define SIDECAST_RING_H

// Lock-free single-producer / single-consumer audio ring indexed by sample
// time, shared by the two devices of SidecastMic.driver: the hidden
// "Sidecast Mic Feed" output writes frames at its output sample times, the
// "Sidecast Microphone" input reads them back at the same sample times (both
// devices run on one clock, so sample time T means the same instant on both).
//
// Every slot carries the sample time it was written for. A read only returns
// a frame whose tag matches the requested sample time, so the input delivers
// silence for anything that was not fed — never stale audio from a previous
// lap of the ring, even if nobody read it.
//
// Real-time safe: no locks, no allocation, O(frames) per call.

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/// Frames held by the ring: 32768 (≈ 683 ms at 48 kHz). Power of two.
#define SCM_RING_FRAMES 32768

typedef struct scm_ring scm_ring;

/// The ring the driver uses (static storage, no allocation).
scm_ring *scm_ring_shared(void);

/// A separately allocated ring (tests). Not for the IO path.
scm_ring *scm_ring_create(void);
void scm_ring_destroy(scm_ring *ring);

/// Marks every slot empty. Not real-time safe against concurrent IO.
void scm_ring_init(scm_ring *ring);

/// Writes `frames` mono samples for sample times [sample_time, sample_time + frames).
void scm_ring_write(scm_ring *ring, int64_t sample_time, const float *src, uint32_t frames);

/// Reads `frames` mono samples for sample times [sample_time, sample_time + frames)
/// into `dst`; frames that were not written for exactly that sample time are 0.
/// Returns how many frames held fed audio.
uint32_t scm_ring_read(scm_ring *ring, int64_t sample_time, float *dst, uint32_t frames);

#ifdef __cplusplus
}
#endif

#endif
