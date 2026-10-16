#include "plugin_adapter.h"
namespace airwindows_ToTape6 {
#include "../../vendor/audio/airwindows/plugins/WinVST/ToTape6/ToTape6.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/ToTape6/ToTape6Proc.cpp"
}
CrestProcessor* make_airwindows_ToTape6(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_ToTape6::ToTape6(reinterpret_cast<void*>(1)), rate, frames, false);
}
