#include "plugin_adapter.h"
namespace airwindows_DeRez3 {
#include "../../vendor/audio/airwindows/plugins/WinVST/DeRez3/DeRez3.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/DeRez3/DeRez3Proc.cpp"
}
CrestProcessor* make_airwindows_DeRez3(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_DeRez3::DeRez3(reinterpret_cast<void*>(1)), rate, frames, false);
}
