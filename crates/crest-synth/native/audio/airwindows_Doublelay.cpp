#include "plugin_adapter.h"
namespace airwindows_Doublelay {
#include "../../vendor/audio/airwindows/plugins/WinVST/Doublelay/Doublelay.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Doublelay/DoublelayProc.cpp"
}
CrestProcessor* make_airwindows_Doublelay(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Doublelay::Doublelay(reinterpret_cast<void*>(1)), rate, frames, false);
}
