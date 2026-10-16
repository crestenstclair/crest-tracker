#include "plugin_adapter.h"
namespace airwindows_Parametric {
#include "../../vendor/audio/airwindows/plugins/WinVST/Parametric/Parametric.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Parametric/ParametricProc.cpp"
}
CrestProcessor* make_airwindows_Parametric(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Parametric::Parametric(reinterpret_cast<void*>(1)), rate, frames, false);
}
