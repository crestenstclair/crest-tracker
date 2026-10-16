#include "plugin_adapter.h"
namespace airwindows_PurestEcho {
#include "../../vendor/audio/airwindows/plugins/WinVST/PurestEcho/PurestEcho.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/PurestEcho/PurestEchoProc.cpp"
}
CrestProcessor* make_airwindows_PurestEcho(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_PurestEcho::PurestEcho(reinterpret_cast<void*>(1)), rate, frames, false);
}
