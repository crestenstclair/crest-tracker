#include "plugin_adapter.h"
namespace airwindows_Baxandall2 {
#include "../../vendor/audio/airwindows/plugins/WinVST/Baxandall2/Baxandall2.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Baxandall2/Baxandall2Proc.cpp"
}
CrestProcessor* make_airwindows_Baxandall2(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Baxandall2::Baxandall2(reinterpret_cast<void*>(1)), rate, frames, false);
}
