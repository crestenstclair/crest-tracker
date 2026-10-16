#include "plugin_adapter.h"
namespace airwindows_kPlateD {
#include "../../vendor/audio/airwindows/plugins/WinVST/kPlateD/kPlateD.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/kPlateD/kPlateDProc.cpp"
}
CrestProcessor* make_airwindows_kPlateD(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_kPlateD::kPlateD(reinterpret_cast<void*>(1)), rate, frames, false);
}
