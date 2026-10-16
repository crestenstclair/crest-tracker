#include "plugin_adapter.h"
namespace mda_Piano {
#define printf(...) ((void)0)
#include "../../vendor/audio/mda/plugins/mdaPiano.cpp"
}
#undef printf
CrestProcessor* make_mda_Piano(float rate, size_t frames) {
    return new CrestPlugin(new mda_Piano::mdaPiano(reinterpret_cast<void*>(1)), rate, frames, true);
}
