//! Preserve pinned DaisySP sources while caching unchanged filter coefficients.
use std::path::{Path, PathBuf};

pub fn stage(output: &Path) -> PathBuf {
    let destination = output.join("daisy-source");
    copy_tree(Path::new("vendor/audio/daisysp/Source"), &destination);
    inline_svf(&destination);
    split_modal_preparation(&destination);
    destination
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let source = entry.unwrap().path();
        let target = destination.join(source.file_name().unwrap());
        if source.is_dir() {
            copy_tree(&source, &target);
            continue;
        }
        let mut bytes = std::fs::read(&source).unwrap();
        let relative = source.strip_prefix("vendor/audio/daisysp/Source").unwrap();
        for (path, before, after) in EDITS {
            if relative == Path::new(path) {
                let text = String::from_utf8(bytes).unwrap();
                assert_eq!(
                    text.matches(before).count(),
                    1,
                    "DaisySP coefficient adaptation no longer matches {}",
                    source.display()
                );
                bytes = text.replace(before, after).into_bytes();
            }
        }
        if std::fs::read(&target).ok().as_deref() != Some(&bytes) {
            std::fs::write(&target, bytes).unwrap();
        }
    }
}

// Setters retain their original calculations, including the first call after
// Init. Changes of frequency, resonance, or drive still update dependent state.
// ResonatorSvf caches only coefficients; gain and filter history always advance.
const EDITS: &[(&str, &str, &str)] = &[
    (
        "Drums/analogbassdrum.h",
        "    float accent_, f0_, tone_, decay_;",
        "    float accent_, f0_, tone_, decay_;\n    float cached_q_, cached_tone_;",
    ),
    (
        "Drums/analogbassdrum.cpp",
        "1500.0f * powf(2.f, kOneTwelfth * decay_ * 80.0f)",
        "cached_q_",
    ),
    (
        "Drums/analogbassdrum.cpp",
        "powf(2.f, kOneTwelfth * tone_ * 108.0f)",
        "cached_tone_",
    ),
    (
        "Drums/analogbassdrum.cpp",
        "    tone_ = fclamp(tone, 0.f, 1.f);",
        "    tone_ = fclamp(tone, 0.f, 1.f);\n    cached_tone_ = powf(2.f, kOneTwelfth * tone_ * 108.0f);",
    ),
    (
        "Drums/analogbassdrum.cpp",
        "    decay_ -= .1f;",
        "    decay_ -= .1f;\n    cached_q_ = 1500.0f * powf(2.f, kOneTwelfth * decay_ * 80.0f);",
    ),
    (
        "PhysicalModeling/resonator.h",
        "    float                        mode_amplitude_[kMaxNumModes];",
        r###"    bool modes_ready_;
    float cached_frequency_, cached_structure_, cached_brightness_, cached_damping_;
    float prepared_f_[kMaxNumModes], prepared_q_[kMaxNumModes], prepared_a_[kMaxNumModes];
    float                        mode_amplitude_[kMaxNumModes];"###,
    ),
    (
        "PhysicalModeling/resonator.cpp",
        "    sample_rate_ = sample_rate;",
        "    sample_rate_ = sample_rate;\n    modes_ready_ = false;",
    ),
    (
        "PhysicalModeling/resonator.cpp",
        "    float stiffness  = CalcStiff(structure_);",
        r###"    if(!modes_ready_ || frequency_ != cached_frequency_ || structure_ != cached_structure_
       || brightness_ != cached_brightness_ || damping_ != cached_damping_)
    {
    modes_ready_ = true;
    cached_frequency_ = frequency_; cached_structure_ = structure_;
    cached_brightness_ = brightness_; cached_damping_ = damping_;
    float stiffness  = CalcStiff(structure_);"###,
    ),
    (
        "PhysicalModeling/resonator.cpp",
        r###"    float mode_q[kModeBatchSize];
    float mode_f[kModeBatchSize];
    float mode_a[kModeBatchSize];
    int   batch_counter = 0;

    ResonatorSvf<kModeBatchSize>* batch_processor = &mode_filters_[0];
"###,
        "",
    ),
    (
        "PhysicalModeling/resonator.cpp",
        r###"        mode_f[batch_counter] = mode_frequency;
        mode_q[batch_counter] = 1.0f + mode_frequency * q;
        mode_a[batch_counter] = mode_amplitude_[i] * mode_attenuation;
        ++batch_counter;

        if(batch_counter == kModeBatchSize)
        {
            batch_counter = 0;
            batch_processor
                ->Process<ResonatorSvf<kModeBatchSize>::BAND_PASS, true>(
                    mode_f, mode_q, mode_a, in, &out);
            ++batch_processor;
        }"###,
        r###"        prepared_f_[i] = mode_frequency;
        prepared_q_[i] = 1.0f + mode_frequency * q;
        prepared_a_[i] = mode_amplitude_[i] * mode_attenuation;"###,
    ),
    (
        "PhysicalModeling/resonator.cpp",
        "    return out;",
        r###"    }
    for(int batch = 0; batch < resolution_ / kModeBatchSize; ++batch)
    {
        const int offset = batch * kModeBatchSize;
        mode_filters_[batch].Process<ResonatorSvf<kModeBatchSize>::BAND_PASS, true>(
            prepared_f_ + offset, prepared_q_ + offset, prepared_a_ + offset, in, &out);
    }
    return out;"###,
    ),
    (
        "Filters/svf.h",
        r###"    float sr_, fc_, res_, drive_, freq_, damp_;"###,
        r###"    bool frequency_ready_, resonance_ready_;
    float sr_, fc_, res_, drive_, freq_, damp_;"###,
    ),
    (
        "Filters/svf.cpp",
        r###"    sr_        = sample_rate;"###,
        r###"    frequency_ready_ = resonance_ready_ = false;
    sr_        = sample_rate;"###,
    ),
    (
        "Filters/svf.cpp",
        r###"    fc_ = fclamp(f, 1.0e-6, fc_max_);"###,
        r###"    const float next = fclamp(f, 1.0e-6, fc_max_);
    if(frequency_ready_ && next == fc_) return;
    frequency_ready_ = true;
    fc_ = next;"###,
    ),
    (
        "Filters/svf.cpp",
        r###"    res_      = res;"###,
        r###"    if(resonance_ready_ && res == res_) return;
    resonance_ready_ = true;
    res_      = res;"###,
    ),
    (
        "PhysicalModeling/resonator.h",
        r###"    void Init()
    {"###,
        r###"    void Init()
    {
        coefficients_ready_ = false;"###,
    ),
    (
        "PhysicalModeling/resonator.h",
        r###"            g[i]        = fasttan(f[i]);
            r[i]        = 1.0f / q[i];
            h[i]        = 1.0f / (1.0f + r[i] * g[i] + g[i] * g[i]);
            r_plus_g[i] = r[i] + g[i];"###,
        r###"            if(!coefficients_ready_ || f[i] != cached_f_[i] || q[i] != cached_q_[i])
            {
                cached_f_[i] = f[i];
                cached_q_[i] = q[i];
                const float g = fasttan(f[i]);
                const float r = 1.0f / q[i];
                cached_g_[i] = g;
                cached_h_[i] = 1.0f / (1.0f + r * g + g * g);
                cached_r_plus_g_[i] = r + g;
            }
            g[i] = cached_g_[i];
            h[i] = cached_h_[i];
            r_plus_g[i] = cached_r_plus_g_[i];"###,
    ),
    (
        "PhysicalModeling/resonator.h",
        r###"        float r[batch_size];
"###,
        r###""###,
    ),
    (
        "PhysicalModeling/resonator.h",
        r###"        float s_in  = in;"###,
        r###"        coefficients_ready_ = true;
        float s_in  = in;"###,
    ),
    (
        "PhysicalModeling/resonator.h",
        r###"    float state_1_[batch_size];"###,
        r###"    bool coefficients_ready_;
    float cached_f_[batch_size], cached_q_[batch_size];
    float cached_g_[batch_size], cached_h_[batch_size], cached_r_plus_g_[batch_size];
    float state_1_[batch_size];"###,
    ),
    (
        "Filters/svf.h",
        "    float sr_, fc_, res_, drive_, freq_, damp_;",
        "    float resonance_limit_;\n    float sr_, fc_, res_, drive_, freq_, damp_;",
    ),
    (
        "Filters/svf.cpp",
        "    res_       = 0.5f;",
        "    res_       = 0.5f;\n    resonance_limit_ = 2.0f * (1.0f - powf(res_, 0.25f));",
    ),
    (
        "Filters/svf.cpp",
        "    res_      = res;",
        "    res_      = res;\n    resonance_limit_ = 2.0f * (1.0f - powf(res_, 0.25f));",
    ),
    (
        "Filters/svf.cpp",
        "damp_ = MIN(2.0f * (1.0f - powf(res_, 0.25f)),",
        "damp_ = MIN(resonance_limit_,",
    ),
    (
        "Filters/svf.cpp",
        "damp_  = MIN(2.0f * (1.0f - powf(res_, 0.25f)),",
        "damp_  = MIN(resonance_limit_,",
    ),
    (
        "Drums/synthbassdrum.h",
        "    float accent_, new_f0_, tone_, decay_;",
        "    float cached_decay_, cached_tone_;\n    float accent_, new_f0_, tone_, decay_;",
    ),
    (
        "Drums/synthbassdrum.cpp",
        "powf(2.f, (-decay_ * 60.0f) * kOneTwelfth)",
        "cached_decay_",
    ),
    (
        "Drums/synthbassdrum.cpp",
        "powf(2.f, (tone_ * 108.0f) * kOneTwelfth)",
        "cached_tone_",
    ),
    (
        "Drums/synthbassdrum.cpp",
        "    tone_ = fclamp(tone, 0.f, 1.f);",
        "    tone_ = fclamp(tone, 0.f, 1.f);\n    cached_tone_ = powf(2.f, (tone_ * 108.0f) * kOneTwelfth);",
    ),
    (
        "Drums/synthbassdrum.cpp",
        "    decay_ = decay * decay;",
        "    decay_ = decay * decay;\n    cached_decay_ = powf(2.f, (-decay_ * 60.0f) * kOneTwelfth);",
    ),
    (
        "PhysicalModeling/KarplusString.h",
        r###"    float sample_rate_;"###,
        r###"    bool coefficients_ready_;
    float cached_frequency_, cached_non_linearity_, cached_brightness_, cached_damping_;
    float prepared_delay_, prepared_src_ratio_, prepared_damping_compensation_;
    float prepared_stretch_point_, prepared_stretch_correction_, prepared_noise_amount_;
    float prepared_noise_filter_, prepared_bridge_curving_, prepared_ap_gain_;
    float sample_rate_;"###,
    ),
    (
        "PhysicalModeling/KarplusString.cpp",
        r###"void String::Reset()
{"###,
        r###"void String::Reset()
{
    coefficients_ready_ = false;"###,
    ),
    (
        "PhysicalModeling/KarplusString.cpp",
        r###"    float brightness = brightness_;

    float delay = 1.0f / frequency_;
    delay       = fclamp(delay, 4.f, kDelayLineSize - 4.0f);

    // If there is not enough delay time in the delay line, we play at the
    // lowest possible note and we upsample on the fly with a shitty linear
    // interpolator. We don't care because it's a corner case (frequency_ < 11.7Hz)
    float src_ratio = delay * frequency_;
    if(src_ratio >= 0.9999f)
    {
        // When we are above 11.7 Hz, we make sure that the linear interpolator
        // does not get in the way.
        src_phase_ = 1.0f;
        src_ratio  = 1.0f;
    }

    float damping_cutoff
        = fmin(12.0f + damping_ * damping_ * 60.0f + brightness * 24.0f, 84.0f);
    float damping_f
        = fmin(frequency_ * powf(2.f, damping_cutoff * kOneTwelfth), 0.499f);

    // Crossfade to infinite decay.
    if(damping_ >= 0.95f)
    {
        float to_infinite = 20.0f * (damping_ - 0.95f);
        brightness += to_infinite * (1.0f - brightness);
        damping_f += to_infinite * (0.4999f - damping_f);
        damping_cutoff += to_infinite * (128.0f - damping_cutoff);
    }

    float temp_f = damping_f;
    iir_damping_filter_.SetFrequency(temp_f);

    float ratio                = powf(2.f, damping_cutoff * kOneTwelfth);
    float damping_compensation = 1.f - 2.f * atanf(1.f / ratio) / (TWOPI_F);

    float stretch_point
        = non_linearity_amount_ * (2.0f - non_linearity_amount_) * 0.225f;
    float stretch_correction = (160.0f / sample_rate_) * delay;
    stretch_correction       = fclamp(stretch_correction, 1.f, 2.1f);

    float noise_amount_sqrt = non_linearity_amount_ > 0.75f
                                  ? 4.0f * (non_linearity_amount_ - 0.75f)
                                  : 0.0f;
    float noise_amount = noise_amount_sqrt * noise_amount_sqrt * 0.1f;
    float noise_filter = 0.06f + 0.94f * brightness * brightness;

    float bridge_curving_sqrt = non_linearity_amount_;
    float bridge_curving = bridge_curving_sqrt * bridge_curving_sqrt * 0.01f;

    float ap_gain = -0.618f * non_linearity_amount_
                    / (0.15f + fabsf(non_linearity_amount_));

"###,
        r###"    // Cache parameter-only calculations. Delay/filter histories and the
    // low-frequency interpolation clock still advance for every sample.
    if(!coefficients_ready_ || cached_frequency_ != frequency_
       || cached_non_linearity_ != non_linearity_amount_
       || cached_brightness_ != brightness_ || cached_damping_ != damping_)
    {
        coefficients_ready_ = true;
        cached_frequency_ = frequency_;
        cached_non_linearity_ = non_linearity_amount_;
        cached_brightness_ = brightness_;
        cached_damping_ = damping_;
        float brightness = brightness_;

        float delay = 1.0f / frequency_;
        delay       = fclamp(delay, 4.f, kDelayLineSize - 4.0f);

        // If there is not enough delay time in the delay line, we play at the
        // lowest possible note and we upsample on the fly with a shitty linear
        // interpolator. We don't care because it's a corner case (frequency_ < 11.7Hz)
        float src_ratio = delay * frequency_;

        float damping_cutoff
            = fmin(12.0f + damping_ * damping_ * 60.0f + brightness * 24.0f, 84.0f);
        float damping_f
            = fmin(frequency_ * powf(2.f, damping_cutoff * kOneTwelfth), 0.499f);

        // Crossfade to infinite decay.
        if(damping_ >= 0.95f)
        {
            float to_infinite = 20.0f * (damping_ - 0.95f);
            brightness += to_infinite * (1.0f - brightness);
            damping_f += to_infinite * (0.4999f - damping_f);
            damping_cutoff += to_infinite * (128.0f - damping_cutoff);
        }

        float temp_f = damping_f;
        iir_damping_filter_.SetFrequency(temp_f);

        float ratio                = powf(2.f, damping_cutoff * kOneTwelfth);
        float damping_compensation = 1.f - 2.f * atanf(1.f / ratio) / (TWOPI_F);

        float stretch_point
            = non_linearity_amount_ * (2.0f - non_linearity_amount_) * 0.225f;
        float stretch_correction = (160.0f / sample_rate_) * delay;
        stretch_correction       = fclamp(stretch_correction, 1.f, 2.1f);

        float noise_amount_sqrt = non_linearity_amount_ > 0.75f
                                      ? 4.0f * (non_linearity_amount_ - 0.75f)
                                      : 0.0f;
        float noise_amount = noise_amount_sqrt * noise_amount_sqrt * 0.1f;
        float noise_filter = 0.06f + 0.94f * brightness * brightness;

        float bridge_curving_sqrt = non_linearity_amount_;
        float bridge_curving = bridge_curving_sqrt * bridge_curving_sqrt * 0.01f;

        float ap_gain = -0.618f * non_linearity_amount_
                        / (0.15f + fabsf(non_linearity_amount_));

        prepared_delay_ = delay;
        prepared_src_ratio_ = src_ratio;
        prepared_damping_compensation_ = damping_compensation;
        prepared_stretch_point_ = stretch_point;
        prepared_stretch_correction_ = stretch_correction;
        prepared_noise_amount_ = noise_amount;
        prepared_noise_filter_ = noise_filter;
        prepared_bridge_curving_ = bridge_curving;
        prepared_ap_gain_ = ap_gain;
    }
    float delay = prepared_delay_;
    float src_ratio = prepared_src_ratio_;
    float damping_compensation = prepared_damping_compensation_;
    float stretch_point = prepared_stretch_point_;
    float stretch_correction = prepared_stretch_correction_;
    float noise_amount = prepared_noise_amount_;
    float noise_filter = prepared_noise_filter_;
    float bridge_curving = prepared_bridge_curving_;
    float ap_gain = prepared_ap_gain_;

    if(src_ratio >= 0.9999f)
    {
        // When we are above 11.7 Hz, we make sure that the linear interpolator
        // does not get in the way.
        src_phase_ = 1.0f;
        src_ratio  = 1.0f;
    }

"###,
    ),
    (
        "PhysicalModeling/modalvoice.h",
        r###"    float sample_rate_;"###,
        r###"    bool coefficients_ready_, cached_sustain_;
    float cached_f0_, cached_brightness_, cached_damping_, cached_accent_;
    float prepared_brightness_, prepared_damping_, prepared_cutoff_, prepared_q_;
    float sample_rate_;"###,
    ),
    (
        "PhysicalModeling/modalvoice.cpp",
        r###"    sample_rate_ = sample_rate;"###,
        r###"    sample_rate_ = sample_rate;
    coefficients_ready_ = false;"###,
    ),
    (
        "PhysicalModeling/modalvoice.cpp",
        r###"    float brightness = brightness_ + 0.25f * accent_ * (1.0f - brightness_);
    float damping    = damping_ + 0.25f * accent_ * (1.0f - damping_);

    const float range  = sustain_ ? 36.0f : 60.0f;
    const float f      = sustain_ ? 4.0f * f0_ : 2.0f * f0_;
    const float cutoff = fmin(
        f
            * powf(2.f,
                   kOneTwelfth
                       * ((brightness * (2.0f - brightness) - 0.5f) * range)),
        0.499f);
    const float q = sustain_ ? 0.7f : 1.5f;

"###,
        r###"    if(!coefficients_ready_ || cached_sustain_ != sustain_ || cached_f0_ != f0_
       || cached_brightness_ != brightness_ || cached_damping_ != damping_
       || cached_accent_ != accent_)
    {
        coefficients_ready_ = true;
        cached_sustain_ = sustain_; cached_f0_ = f0_;
        cached_brightness_ = brightness_; cached_damping_ = damping_;
        cached_accent_ = accent_;
        float brightness = brightness_ + 0.25f * accent_ * (1.0f - brightness_);
        float damping    = damping_ + 0.25f * accent_ * (1.0f - damping_);

        const float range  = sustain_ ? 36.0f : 60.0f;
        const float f      = sustain_ ? 4.0f * f0_ : 2.0f * f0_;
        const float cutoff = fmin(
            f
                * powf(2.f,
                       kOneTwelfth
                           * ((brightness * (2.0f - brightness) - 0.5f) * range)),
            0.499f);
        const float q = sustain_ ? 0.7f : 1.5f;

        prepared_brightness_ = brightness;
        prepared_damping_ = damping;
        prepared_cutoff_ = cutoff;
        prepared_q_ = q;
    }
    const float brightness = prepared_brightness_;
    const float damping = prepared_damping_;
    const float cutoff = prepared_cutoff_;
    const float q = prepared_q_;

"###,
    ),
];

// Keep the pinned implementation visible at its sample-by-sample call sites.
// This avoids repeated native calls and allows unused SVF outputs to disappear.
fn inline_svf(destination: &Path) {
    let source_path = destination.join("Filters/svf.cpp");
    let source = std::fs::read_to_string(&source_path).unwrap();
    let methods = source[source.find("void Svf::Init").unwrap()..]
        .replace("void Svf::", "inline void Svf::")
        .replace("MIN(", "std::min(");
    let header_path = destination.join("Filters/svf.h");
    let header = std::fs::read_to_string(&header_path)
        .unwrap()
        .replace(
            "#define DSY_SVF_H",
            "#define DSY_SVF_H\n#include <algorithm>\n#include <cmath>\n#include \"Utility/dsp.h\"",
        )
        .replace(
            "} // namespace daisysp",
            &format!("{methods}\n}} // namespace daisysp"),
        );
    std::fs::write(header_path, header).unwrap();
    std::fs::write(source_path, "#include \"svf.h\"\n").unwrap();
}

fn split_modal_preparation(destination: &Path) {
    let header_path = destination.join("PhysicalModeling/resonator.h");
    let mut header = std::fs::read_to_string(&header_path).unwrap();
    let start = header
        .find("    template <FilterMode mode, bool add>")
        .unwrap();
    let end = header[start..].find("  private:").unwrap() + start;
    header.replace_range(
        start..end,
        r###"    void Prepare(const float* f, const float* q)
    {
        for(int i=0; i<batch_size; ++i)
        {
            if(!coefficients_ready_ || f[i] != cached_f_[i] || q[i] != cached_q_[i])
            {
                cached_f_[i] = f[i];
                cached_q_[i] = q[i];
                const float g = fasttan(f[i]);
                const float r = 1.0f / q[i];
                cached_g_[i] = g;
                cached_h_[i] = 1.0f / (1.0f + r * g + g * g);
                cached_r_plus_g_[i] = r + g;
            }
        }
        coefficients_ready_ = true;
    }

    template <FilterMode mode, bool add>
    void Process(const float* f, const float* q, const float* gain,
                 const float in, float* out)
    {
        Prepare(f, q);
        ProcessPrepared<mode, add>(gain, in, out);
    }

    template <FilterMode mode, bool add>
    void ProcessPrepared(const float* gain, const float in, float* out)
    {
        float sum = 0.0f;
        for(int i=0; i<batch_size; ++i)
        {
            const float g = cached_g_[i];
            const float hp = (in - cached_r_plus_g_[i] * state_1_[i] - state_2_[i]) * cached_h_[i];
            const float bp = g * hp + state_1_[i];
            state_1_[i] = g * hp + bp;
            const float lp = g * bp + state_2_[i];
            state_2_[i] = g * bp + lp;
            sum += gain[i] * ((mode == LOW_PASS) ? lp : bp);
        }
        if(add) *out += sum;
        else *out = sum;
    }

"###,
    );
    std::fs::write(header_path, header).unwrap();
    let source_path = destination.join("PhysicalModeling/resonator.cpp");
    let source = std::fs::read_to_string(&source_path).unwrap();
    let before = r###"    }
    for(int batch = 0; batch < resolution_ / kModeBatchSize; ++batch)
    {
        const int offset = batch * kModeBatchSize;
        mode_filters_[batch].Process<ResonatorSvf<kModeBatchSize>::BAND_PASS, true>(
            prepared_f_ + offset, prepared_q_ + offset, prepared_a_ + offset, in, &out);
    }"###;
    let after = r###"        // Coefficients only change with the modal controls above. Keep their
        // comparison/preparation out of the per-sample filter-history path.
        for(int batch = 0; batch < resolution_ / kModeBatchSize; ++batch)
        {
            const int offset = batch * kModeBatchSize;
            mode_filters_[batch].Prepare(prepared_f_ + offset, prepared_q_ + offset);
        }
    }
    for(int batch = 0; batch < resolution_ / kModeBatchSize; ++batch)
    {
        const int offset = batch * kModeBatchSize;
        mode_filters_[batch].ProcessPrepared<ResonatorSvf<kModeBatchSize>::BAND_PASS, true>(
            prepared_a_ + offset, in, &out);
    }"###;
    assert_eq!(
        source.matches(before).count(),
        1,
        "DaisySP modal preparation no longer matches"
    );
    std::fs::write(source_path, source.replace(before, after)).unwrap();
}
