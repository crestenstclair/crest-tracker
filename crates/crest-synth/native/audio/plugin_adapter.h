#pragma once
#include "audioeffectx.h"
#include "processor.h"
#include <array>
#include "random.h"

class CrestPlugin final : public CrestProcessor {
    std::unique_ptr<AudioEffectX> plugin_;
    std::vector<std::array<char, 128>> labels_;
    std::vector<float> defaults_;
    std::vector<size_t> indices_;
    VstMidiEvent pending_[2];
    int pending_count_ = 0;
public:
    CrestPlugin(AudioEffectX* plugin, float rate, size_t frames, bool voice = false): plugin_(plugin) {
        plugin_->setSampleRate(rate);
        plugin_->setBlockSize(static_cast<int>(frames));
        plugin_->resume();
        for (int i = 0; i < plugin_->numParams; ++i) {
            std::array<char, 128> label{};
            plugin_->getParameterName(i, label.data());
            // Each instance represents one independently enveloped host voice.
            // Native voice-pool controls have no user-visible effect here.
            if (voice && std::string(label.data()).find("Poly") != std::string::npos) continue;
            indices_.push_back(i);
            labels_.push_back(label);
            defaults_.push_back(plugin_->getParameter(i));
        }
    }
    size_t count() const override { return indices_.size(); }
    const char* label(size_t i) const override { return labels_[i].data(); }
    float initial(size_t i) const override { return defaults_[i]; }
    void set(size_t i, float value) override { plugin_->setParameter(indices_[i], value); }
    void note(int status, int a, int b) override {
        VstMidiEvent message; message.midiData[0] = status;
        message.midiData[1] = a; message.midiData[2] = b;
        if (status == 0x90 || status == 0x80) {
            // One host voice receives at most one onset and release before
            // processing; reuse first resets the old voice and this mailbox.
            if (pending_count_ < 2) pending_[pending_count_++] = message;
        } else {
            VstEvents events; events.numEvents = 1; events.events[0] = &message;
            plugin_->processEvents(&events);
        }
    }
    void reset() override {
        pending_count_ = 0;
        plugin_->suspend();
        note(0xb0, 123, 0);
    }
    void process(float* left, float* right, size_t frames) override {
        if (pending_count_) {
            VstEvents events; events.numEvents = pending_count_;
            for (int i=0; i<pending_count_; ++i) events.events[i] = &pending_[i];
            plugin_->processEvents(&events);
            pending_count_ = 0;
        }
        float* channels[] = {left, right};
        plugin_->processReplacing(channels, channels, static_cast<int>(frames));
    }
};
