#include "plugin_adapter.h"
namespace mda_JX10 {
#include "../../vendor/audio/mda/plugins/mdaJX10.cpp"
}
CrestProcessor* make_mda_JX10(float rate, size_t frames) {
    return new CrestPlugin(new mda_JX10::mdaJX10(reinterpret_cast<void*>(1)), rate, frames, true);
}
