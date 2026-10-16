#include "plugin_adapter.h"
namespace airwindows_Biquad2 {
#include "../../vendor/audio/airwindows/plugins/WinVST/Biquad2/Biquad2.cpp"
#include "../../vendor/audio/airwindows/plugins/WinVST/Biquad2/Biquad2Proc.cpp"
}
CrestProcessor* make_airwindows_Biquad2(float rate, size_t frames) {
    return new CrestPlugin(new airwindows_Biquad2::Biquad2(reinterpret_cast<void*>(1)), rate, frames, false);
}
