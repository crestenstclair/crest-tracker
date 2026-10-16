#include "plugin_adapter.h"
namespace mda_DubDelay {
#include "../../vendor/audio/mda/plugins/mdaDubDelay.cpp"
}
CrestProcessor* make_mda_DubDelay(float rate, size_t frames) {
    return new CrestPlugin(new mda_DubDelay::mdaDubDelay(reinterpret_cast<void*>(1)), rate, frames, false);
}
