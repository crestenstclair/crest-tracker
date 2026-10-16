#include "plugin_adapter.h"
namespace airwindows_Vibrato {
#include "../../vendor/audio/airwindows/plugins/WinVST/Vibrato/Vibrato.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Vibrato/VibratoProc.cpp"
}
CrestProcessor* make_airwindows_Vibrato(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Vibrato::Vibrato(reinterpret_cast<void*>(1)), rate, frames, false);
}
