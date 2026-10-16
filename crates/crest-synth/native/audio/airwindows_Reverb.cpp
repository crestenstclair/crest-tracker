#include "plugin_adapter.h"
namespace airwindows_Reverb {
#include "../../vendor/audio/airwindows/plugins/WinVST/Reverb/Reverb.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Reverb/ReverbProc.cpp"
}
CrestProcessor* make_airwindows_Reverb(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Reverb::Reverb(reinterpret_cast<void*>(1)), rate, frames, false);
}
