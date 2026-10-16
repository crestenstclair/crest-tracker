#pragma once
#include "processor.h"
#include <initializer_list>
#include <cmath>
struct CrestParameter {
    const char* label;
    float initial;
    float minimum = 0;
    float maximum = 1;
    bool stepped = false;
};
class ParameterAdapter : public CrestProcessor {
protected:
    std::vector<CrestParameter> specs_;
    std::vector<float> values_;
public:
    ParameterAdapter(std::initializer_list<CrestParameter> specs): specs_(specs) {
        for (const auto& p: specs_) values_.push_back(p.initial);
    }
    size_t count() const override { return specs_.size(); }
    const char* label(size_t i) const override { return specs_[i].label; }
    float initial(size_t i) const override { return specs_[i].initial; }
    float minimum(size_t i) const override { return specs_[i].minimum; }
    float maximum(size_t i) const override { return specs_[i].maximum; }
    bool stepped(size_t i) const override { return specs_[i].stepped; }
    void set(size_t i, float v) override { values_[i] = v; }
};
