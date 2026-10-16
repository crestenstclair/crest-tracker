#include "plugin_adapter.h"
namespace airwindows_AutoPan {
#include "../../vendor/audio/airwindows/plugins/WinVST/AutoPan/AutoPan.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/AutoPan/AutoPanProc.cpp"
}
CrestProcessor* make_airwindows_AutoPan(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_AutoPan::AutoPan(reinterpret_cast<void*>(1)), rate, frames, false);
}
