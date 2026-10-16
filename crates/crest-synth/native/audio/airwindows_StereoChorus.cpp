#include "plugin_adapter.h"
namespace airwindows_StereoChorus {
#include "../../vendor/audio/airwindows/plugins/WinVST/StereoChorus/StereoChorus.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/StereoChorus/StereoChorusProc.cpp"
}
CrestProcessor* make_airwindows_StereoChorus(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_StereoChorus::StereoChorus(reinterpret_cast<void*>(1)), rate, frames, false);
}
