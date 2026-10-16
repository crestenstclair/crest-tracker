#include "plugin_adapter.h"
namespace airwindows_TapeDelay2 {
#include "../../vendor/audio/airwindows/plugins/WinVST/TapeDelay2/TapeDelay2.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/TapeDelay2/TapeDelay2Proc.cpp"
}
CrestProcessor* make_airwindows_TapeDelay2(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_TapeDelay2::TapeDelay2(reinterpret_cast<void*>(1)), rate, frames, false);
}
