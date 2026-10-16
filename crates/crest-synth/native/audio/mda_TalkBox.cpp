#include "plugin_adapter.h"
namespace mda_TalkBox {
#include "../../vendor/audio/mda/plugins/mdaTalkBox.cpp"
}
CrestProcessor* make_mda_TalkBox(float rate, size_t frames) {
    return new CrestPlugin(new mda_TalkBox::mdaTalkBox(reinterpret_cast<void*>(1)), rate, frames, false);
}
