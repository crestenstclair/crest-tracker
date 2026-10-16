#include "plugin_adapter.h"
namespace mda_Leslie {
#include "../../vendor/audio/mda/plugins/mdaLeslie.cpp"
}
CrestProcessor* make_mda_Leslie(float rate, size_t frames) {
    return new CrestPlugin(new mda_Leslie::mdaLeslie(reinterpret_cast<void*>(1)), rate, frames, false);
}
