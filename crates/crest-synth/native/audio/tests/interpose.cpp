// Darwin uses a separate image; Linux wraps calls from the linked archives.
// Count only the selected witness thread, with no TLS initialization in hooks.
#include <atomic>
#include <cstdlib>
#include <ctime>
#include <pthread.h>
// STK's Noise constructor seeds from time(). Numerical and instance-isolation
// comparisons require identical random inputs even when preparation crosses a
// wall-clock second. Only this witness freezes that seed source; steady clocks,
// native DSP equations, and the shipping application's randomness are intact.
static std::time_t witness_time(std::time_t* destination) noexcept {
    constexpr std::time_t seed_time=1700000000;
    if(destination) *destination=seed_time;
    return seed_time;
}
static std::atomic<bool> enabled{false};
static std::atomic<pthread_t> measured_thread;
static size_t heap_operations=0,locks=0;
static bool measuring() {
    return enabled.load(std::memory_order_acquire) && pthread_equal(measured_thread.load(std::memory_order_relaxed),pthread_self());
}
extern "C" void crest_witness_begin() {
    measured_thread.store(pthread_self(),std::memory_order_relaxed);heap_operations=locks=0;
    enabled.store(true,std::memory_order_release);
}
extern "C" void crest_witness_end(size_t* heap,size_t* lock_count) {
    enabled.store(false,std::memory_order_release);
    *heap=heap_operations;*lock_count=locks;
}
#if defined(__APPLE__)
#define INTERPOSE(replacement, original) \
__attribute__((used)) static struct { const void* replacement; const void* original; } interpose_##original \
__attribute__((section("__DATA,__interpose"))) = { (const void*)&replacement, (const void*)&original };
static void* tracked_malloc(size_t n) { if(measuring())++heap_operations; return malloc(n); }
static void* tracked_calloc(size_t n,size_t m) { if(measuring())++heap_operations; return calloc(n,m); }
static void* tracked_realloc(void* p,size_t n) { if(measuring())++heap_operations; return realloc(p,n); }
static void tracked_free(void* p) { if(p&&measuring())++heap_operations; free(p); }
static int tracked_mutex_lock(pthread_mutex_t* p) { if(measuring())++locks; return pthread_mutex_lock(p); }
static int tracked_rwlock_rdlock(pthread_rwlock_t* p) { if(measuring())++locks; return pthread_rwlock_rdlock(p); }
static int tracked_rwlock_wrlock(pthread_rwlock_t* p) { if(measuring())++locks; return pthread_rwlock_wrlock(p); }
INTERPOSE(tracked_malloc,malloc)
INTERPOSE(tracked_calloc,calloc)
INTERPOSE(tracked_realloc,realloc)
INTERPOSE(tracked_free,free)
INTERPOSE(tracked_mutex_lock,pthread_mutex_lock)
INTERPOSE(tracked_rwlock_rdlock,pthread_rwlock_rdlock)
INTERPOSE(tracked_rwlock_wrlock,pthread_rwlock_wrlock)
INTERPOSE(witness_time,time)
#elif defined(__linux__)
extern "C" {
std::time_t __wrap_time(std::time_t* destination) { return witness_time(destination); }
void* __real_malloc(size_t);
void* __real_calloc(size_t,size_t);
void* __real_realloc(void*,size_t);
void __real_free(void*);
int __real_pthread_mutex_lock(pthread_mutex_t*);
int __real_pthread_rwlock_rdlock(pthread_rwlock_t*);
int __real_pthread_rwlock_wrlock(pthread_rwlock_t*);
void* __wrap_malloc(size_t n) { if(measuring())++heap_operations; return __real_malloc(n); }
void* __wrap_calloc(size_t n,size_t m) { if(measuring())++heap_operations; return __real_calloc(n,m); }
void* __wrap_realloc(void* p,size_t n) { if(measuring())++heap_operations; return __real_realloc(p,n); }
void __wrap_free(void* p) { if(p&&measuring())++heap_operations; __real_free(p); }
int __wrap_pthread_mutex_lock(pthread_mutex_t* p) { if(measuring())++locks; return __real_pthread_mutex_lock(p); }
int __wrap_pthread_rwlock_rdlock(pthread_rwlock_t* p) { if(measuring())++locks; return __real_pthread_rwlock_rdlock(p); }
int __wrap_pthread_rwlock_wrlock(pthread_rwlock_t* p) { if(measuring())++locks; return __real_pthread_rwlock_wrlock(p); }
}
#else
#error Native heap and lock instrumentation is unavailable on this platform
#endif
