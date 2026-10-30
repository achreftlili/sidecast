#include "SidecastRing.h"

#include <stdatomic.h>
#include <stdlib.h>

#define SCM_RING_MASK ((uint64_t)SCM_RING_FRAMES - 1)
#define SCM_EMPTY_TAG INT64_MIN

_Static_assert((SCM_RING_FRAMES & (SCM_RING_FRAMES - 1)) == 0, "ring size must be a power of two");

struct scm_ring {
    // Sample time each slot was last written for (SCM_EMPTY_TAG = never).
    _Atomic(int64_t) tags[SCM_RING_FRAMES];
    _Atomic(float) samples[SCM_RING_FRAMES];
};

_Static_assert(sizeof(_Atomic(int64_t)) == 8, "64-bit atomics expected");

static scm_ring gSharedRing;

scm_ring *scm_ring_shared(void) { return &gSharedRing; }

scm_ring *scm_ring_create(void) {
    scm_ring *ring = malloc(sizeof(scm_ring));
    if (ring) scm_ring_init(ring);
    return ring;
}

void scm_ring_destroy(scm_ring *ring) { free(ring); }

void scm_ring_init(scm_ring *ring) {
    for (size_t i = 0; i < SCM_RING_FRAMES; i++) {
        atomic_store_explicit(&ring->samples[i], 0.0f, memory_order_relaxed);
        atomic_store_explicit(&ring->tags[i], SCM_EMPTY_TAG, memory_order_relaxed);
    }
    atomic_thread_fence(memory_order_release);
}

static inline size_t slot_for(int64_t sample_time) {
    return (size_t)((uint64_t)sample_time & SCM_RING_MASK);
}

void scm_ring_write(scm_ring *ring, int64_t sample_time, const float *src, uint32_t frames) {
    if (!ring || !src) return;
    for (uint32_t i = 0; i < frames; i++) {
        int64_t t = sample_time + (int64_t)i;
        size_t slot = slot_for(t);
        atomic_store_explicit(&ring->samples[slot], src[i], memory_order_relaxed);
        // Release: a reader that sees this tag also sees the sample stored above.
        atomic_store_explicit(&ring->tags[slot], t, memory_order_release);
    }
}

uint32_t scm_ring_read(scm_ring *ring, int64_t sample_time, float *dst, uint32_t frames) {
    if (!dst) return 0;
    uint32_t fed = 0;
    for (uint32_t i = 0; i < frames; i++) {
        int64_t t = sample_time + (int64_t)i;
        if (ring) {
            size_t slot = slot_for(t);
            if (atomic_load_explicit(&ring->tags[slot], memory_order_acquire) == t) {
                dst[i] = atomic_load_explicit(&ring->samples[slot], memory_order_relaxed);
                fed++;
                continue;
            }
        }
        dst[i] = 0.0f;
    }
    return fed;
}
