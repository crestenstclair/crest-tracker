#include "plugin_adapter.h"
namespace airwindows_SoftGate {
#include "../../vendor/audio/airwindows/plugins/WinVST/SoftGate/SoftGate.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/SoftGate/SoftGateProc.cpp"
}
CrestProcessor* make_airwindows_SoftGate(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_SoftGate::SoftGate(reinterpret_cast<void*>(1)), rate, frames, false);
}
