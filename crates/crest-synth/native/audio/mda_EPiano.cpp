#include "plugin_adapter.h"
namespace mda_EPiano {
#include "crest_mda_epiano.cpp"
}
CrestProcessor* make_mda_EPiano(float rate, size_t frames) {
    return new CrestPlugin(new mda_EPiano::mdaEPiano(reinterpret_cast<void*>(1)), rate, frames, true);
}
