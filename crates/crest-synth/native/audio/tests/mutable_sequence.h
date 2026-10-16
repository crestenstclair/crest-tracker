#pragma once
#include "zero_initialized.h"
#include <cmath>
#include <vector>

template<class Rings, class Elements>
std::vector<float> mutable_resonator_sequence() {
    ZeroInitialized<Rings> rings_storage;
    ZeroInitialized<Elements> elements_storage;
    auto& rings=rings_storage.get(); auto& elements=elements_storage.get();
    std::vector<float> output;
    float input[31],bow[31],left[31],right[31];
    constexpr size_t lengths[]={1,7,16,24,31};
    constexpr float frequencies[]={0.00025f,0.007f,0.027f,0.17f,0.49f};
    constexpr float positions[]={0,0.3f,0.999f,1,0.6f};
    for(size_t block=0; block<2048; ++block) {
        if(block%1024==0) { rings.Init(); elements.Init(); }
        const size_t section=block/64;
        const size_t count=lengths[block%5];
        for(size_t i=0; i<count; ++i) {
            input[i]=block%1024<128 || block%1024>768 ? 0
                : std::sin((block*31+i)*0.073f)*0.01f;
            bow[i]=block%1024>384 && block%1024<640 ? 0.15f : 0;
        }
        // Sweep modal counts, interpolation, bowed feedback, quiet starts,
        // active tails, endpoint controls, and reset of an already used model.
        rings.set_frequency(frequencies[section%5]);
        elements.set_frequency(frequencies[section%5]);
        rings.set_position(positions[section%5]);
        elements.set_position(positions[section%5]);
        rings.set_structure((section%7)/6.f); elements.set_geometry((section%7)/6.f);
        rings.set_brightness((section%3)/2.f); elements.set_brightness((section%3)/2.f);
        rings.set_damping((section%4)/3.f); elements.set_damping((section%4)/3.f);
        rings.set_resolution(2+2*(section%32)); elements.set_resolution(1+(section*17)%64);
        elements.set_modulation_frequency(0.5f/32000.f);
        elements.set_modulation_offset((section%3)*0.25f);
        rings.Process(input,left,right,count);
        for(size_t i=0; i<count; ++i) { output.push_back(left[i]); output.push_back(right[i]); }
        elements.Process(bow,input,left,right,count);
        for(size_t i=0; i<count; ++i) { output.push_back(left[i]); output.push_back(right[i]); }
    }
    // Higher Elements modes update on alternate blocks. Rapid changes followed
    // by one or two stable blocks must never reuse the other parity's stale
    // coefficients. Keep active filter histories throughout this sequence.
    for(size_t block=0; block<512; ++block) {
        const size_t count=lengths[block%5];
        const size_t section=block/3;
        elements.set_resolution(26+(section%39));
        elements.set_frequency(frequencies[section%3]);
        elements.set_geometry((section%7)/6.f);
        elements.set_brightness((section%3)/2.f);
        elements.set_damping((section%4)/3.f);
        elements.set_position(positions[section%5]);
        for(size_t i=0; i<count; ++i) {
            input[i]=std::sin((block*31+i)*.071f)*.01f;
            bow[i]=block%64<32 ? .15f : 0;
        }
        elements.Process(bow,input,left,right,count);
        for(size_t i=0; i<count; ++i) { output.push_back(left[i]); output.push_back(right[i]); }
    }
    return output;
}

std::vector<float> mutable_reference_sequence();
