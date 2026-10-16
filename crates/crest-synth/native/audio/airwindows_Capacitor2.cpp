#include "plugin_adapter.h"
namespace airwindows_Capacitor2 {
#include "../../vendor/audio/airwindows/plugins/WinVST/Capacitor2/Capacitor2.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Capacitor2/Capacitor2Proc.cpp"
}
CrestProcessor* make_airwindows_Capacitor2(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Capacitor2::Capacitor2(reinterpret_cast<void*>(1)), rate, frames, false);
}
