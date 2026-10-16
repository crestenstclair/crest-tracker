#pragma once
#include <vector>
#include <cmath>
template<class Banded> struct PreparedBanded: Banded {
 PreparedBanded() { for(auto& d:this->delay_) d.setMaximumDelay(16384); }
};
template<class Banded,class Mesh> std::vector<double> stk_model_sequence() {
 std::vector<double> output;
 PreparedBanded<Banded> banded;
 // Cleared plucked models remain exactly silent until re-excited, including
 // long silent intervals that would wrap upstream delay pointers repeatedly.
 for(int preset=0;preset<4;++preset) {
  banded.setPreset(preset); banded.controlChange(2,0);
  for(int step=0;step<70000;++step) {
   if(step%8193==0)banded.noteOn(40.+step%1000,.7);
   if(step%8193==127)banded.setFrequency(55.+step%1000);
   if(step%4099==0)banded.clear();
   output.push_back(banded.tick());
  }
 }
 for(int preset=0;preset<4;++preset) {
  banded.setPreset(preset);
  for(int step=0;step<40000;++step) {
   if(step%4097==0) banded.clear();
   if(step%103==0) banded.setFrequency(8. + (step%2000));
   if(step%4099==0) banded.noteOn(40.+(step%1000),.7);
   if(step%511==0) banded.controlChange(2,step%129);
   if(step%512==0) banded.controlChange(4,step%129);
   if(step%519==0) banded.noteOff(.5);
   output.push_back(banded.tick());
  }
 }
 Mesh mesh(6,6);
 for(int nx=2;nx<=12;++nx)for(int ny=2;ny<=12;++ny) {
  mesh.setNX(nx);mesh.setNY(ny);mesh.setInputPosition(.5,.5);
  for(int step=0;step<2048;++step) {
   if(step%1024==0)mesh.clear();
   if(step%277==0)mesh.noteOn(440,.2);
   if(step%511==0)mesh.setDecay(.9+(step%100)*.001);
   output.push_back(step%3 ? mesh.tick() : mesh.inputTick(std::sin(step*.03)*.01));
   if(step%19==0)output.push_back(mesh.energy());
  }
 }
 return output;
}
std::vector<double> stk_reference_sequence();
