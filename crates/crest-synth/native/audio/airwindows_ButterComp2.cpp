#include "plugin_adapter.h"
namespace airwindows_ButterComp2 {
#include "crest_butter/ButterComp2.cpp"
#include "crest_butter/ButterComp2Proc.cpp"
}
CrestProcessor* make_airwindows_ButterComp2(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_ButterComp2::ButterComp2(reinterpret_cast<void*>(1)), rate, frames, false);
}
