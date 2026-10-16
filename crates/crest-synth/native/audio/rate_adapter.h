#pragma once
#include "processor.h"
#include "CDSPResampler.h"
#include <algorithm>
#include <cmath>

// Streaming, preallocated rate/block adapter around the upstream r8brain
// resampler. A constant prepared delay absorbs converter and block batching.
class RateAdapter final : public CrestProcessor {
    inline static thread_local unsigned preparation_phase_ = 0;
public:
    enum class Signal { StereoEffect, StereoGenerator, MonoGenerator };
    // Factory context is used only during off-thread construction, including
    // Darwin's first TLS access. Callback/reset never accesses shared context.
    class PreparationScope {
        unsigned previous_;
    public:
        explicit PreparationScope(unsigned phase): previous_(preparation_phase_) { preparation_phase_=phase; }
        ~PreparationScope() { preparation_phase_=previous_; }
        PreparationScope(const PreparationScope&) = delete;
        PreparationScope& operator=(const PreparationScope&) = delete;
    };
private:
    std::unique_ptr<CrestProcessor> source_;
    const Signal signal_;
    r8b::CDSPResampler24 input_l_, input_r_, output_l_, output_r_;
    std::vector<double> input_left_, input_right_, output_left_, output_right_;
    std::vector<float> native_left_, native_right_, queued_left_, queued_right_;
    size_t native_count_ = 0, read_ = 0, write_ = 0, queued_ = 0, latency_;
public:
    RateAdapter(CrestProcessor* source, double source_rate, double host_rate, size_t block, size_t frames,
        Signal signal = Signal::StereoEffect, unsigned phase = preparation_phase_):
        source_(source), signal_(signal), input_l_(host_rate,source_rate,frames,2.0,phase), input_r_(host_rate,source_rate,frames,2.0,phase+0x9e3779b9u),
        output_l_(source_rate,host_rate,block,2.0,phase+0x3c6ef372u), output_r_(source_rate,host_rate,block,2.0,phase+0xdaa66d2bu),
        input_left_(frames), input_right_(frames), output_left_(block), output_right_(block),
        native_left_(block), native_right_(block) {
        latency_ = input_l_.getInputRequiredForOutput(output_l_.getInputRequiredForOutput(frames)+block)+frames;
        const size_t capacity = latency_ + frames + output_l_.getMaxOutLen(block) + 1;
        queued_left_.resize(capacity); queued_right_.resize(capacity);
        queued_ = latency_; write_ = latency_;
    }
    size_t count() const override { return source_->count(); }
    const char* label(size_t i) const override { return source_->label(i); }
    float initial(size_t i) const override { return source_->initial(i); }
    float minimum(size_t i) const override { return source_->minimum(i); }
    float maximum(size_t i) const override { return source_->maximum(i); }
    bool stepped(size_t i) const override { return source_->stepped(i); }
    size_t latency() const override { return latency_; }
    void set(size_t i, float v) override { source_->set(i,v); }
    void note(int status, int a, int b) override { source_->note(status,a,b); }
    bool load(const uint8_t* bytes,size_t size) override { return source_->load(bytes,size); }
    void reset() override {
        source_->reset(); input_l_.clear(); input_r_.clear(); output_l_.clear(); output_r_.clear();
        std::fill(queued_left_.begin(),queued_left_.end(),0);
        std::fill(queued_right_.begin(),queued_right_.end(),0);
        failed_=false; read_=native_count_=0; write_=queued_=latency_;
    }
    void process(float* left, float* right, size_t frames) override {
        if (signal_ == Signal::StereoEffect) {
            for (size_t i=0; i<frames; ++i) { input_left_[i]=left[i]; input_right_[i]=right[i]; }
        }
        // A generator's zero input still passes through the original converter
        // clock. Removing that latency would change native note/control timing.
        double *l, *r;
        const int n=input_l_.process(input_left_.data(),frames,l);
        if (signal_ == Signal::StereoEffect) input_r_.process(input_right_.data(),frames,r);
        else r=l;
        for (int i=0; i<n;) {
            const size_t take=std::min<size_t>(n-i,native_left_.size()-native_count_);
            for(size_t j=0;j<take;++j) {
                native_left_[native_count_+j]=l[i+j]; native_right_[native_count_+j]=r[i+j];
            }
            native_count_+=take; i+=take;
            if (native_count_ != native_left_.size()) continue;
            source_->process(native_left_.data(),native_right_.data(),native_count_);
            for (size_t j=0; j<native_count_; ++j) { output_left_[j]=native_left_[j]; output_right_[j]=native_right_[j]; }
            double *ol,*oright;
            const int count=output_l_.process(output_left_.data(),native_count_,ol);
            if (signal_ == Signal::MonoGenerator) oright=ol;
            else output_r_.process(output_right_.data(),native_count_,oright);
            if (size_t(count)>queued_left_.size()-queued_) { failed_=true; return; }
            for(size_t offset=0;offset<size_t(count);) {
                const size_t take=std::min<size_t>(count-offset,queued_left_.size()-write_);
                for(size_t j=0;j<take;++j) {
                    queued_left_[write_+j]=ol[offset+j]; queued_right_[write_+j]=oright[offset+j];
                }
                write_+=take; offset+=take;
                if(write_==queued_left_.size()) write_=0;
            }
            queued_+=count;
            native_count_=0;
        }
        if(frames>queued_) { failed_=true; return; }
        for(size_t offset=0;offset<frames;) {
            const size_t take=std::min(frames-offset,queued_left_.size()-read_);
            std::copy_n(queued_left_.data()+read_,take,left+offset);
            std::copy_n(queued_right_.data()+read_,take,right+offset);
            read_+=take;offset+=take;
            if(read_==queued_left_.size()) read_=0;
        }
        queued_-=frames;
    }
    bool healthy() const override { return !failed_ && source_->healthy(); }
private:
    bool failed_ = false;
};
