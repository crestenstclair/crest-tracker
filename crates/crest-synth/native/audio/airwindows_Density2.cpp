#include "plugin_adapter.h"
namespace airwindows_Density2 {
#include "../../vendor/audio/airwindows/plugins/WinVST/Density2/Density2.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Density2/Density2Proc.cpp"
}
CrestProcessor* make_airwindows_Density2(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Density2::Density2(reinterpret_cast<void*>(1)), rate, frames, false);
}
