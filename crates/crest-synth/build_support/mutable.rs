//! Keep upstream modal equations while batching independent filters for SIMD.
use std::path::{Path, PathBuf};

fn replace(text: &mut String, before: &str, after: &str) {
    assert_eq!(
        text.matches(before).count(),
        1,
        "Mutable adaptation no longer matches: {before}"
    );
    *text = text.replace(before, after);
}

pub fn stage(output: &Path) -> PathBuf {
    let destination = output.join("mutable-source");
    let mut filter = std::fs::read_to_string("vendor/audio/stmlib/dsp/filter.h").unwrap();
    replace(
        &mut filter,
        "class Svf {\n public:",
        "class Svf {\n  template<size_t> friend class SvfBatch;\n public:",
    );
    write(&output.join("stmlib/dsp/filter.h"), &filter);
    let relative = "rings/dsp/resonator.cc";
    let mut source = std::fs::read_to_string(format!("vendor/audio/mutable/{relative}")).unwrap();
    source.insert_str(0, "#include \"svf_batch.h\"\n");
    replace(
        &mut source,
        "  ParameterInterpolator position(&previous_position_, position_, size);",
        r###"  const int32_t rendered_modes = (num_modes + 1) & ~1;
  SvfBatch<kMaxModes> bank(f_, rendered_modes);
  float response[kMaxModes], gains[kMaxModes];
  // Also recognize a nonzero interpolation increment that rounds away: every
  // upstream Next() then returns the same position, even before exact equality.
  const bool fixed = size && previous_position_ +
      (position_ - previous_position_) / size == previous_position_;
  if (fixed) {
    CosineOscillator amplitudes;
    amplitudes.Init<COSINE_OSCILLATOR_APPROXIMATE>(previous_position_);
    for (int32_t i=0; i<rendered_modes; ++i) gains[i]=amplitudes.Next();
  }
  ParameterInterpolator position(&previous_position_, position_, size);"###,
    );
    replace(
        &mut source,
        r###"    for (int32_t i = 0; i < num_modes;) {
      odd += amplitudes.Next() * f_[i++].Process<FILTER_MODE_BAND_PASS>(input);
      even += amplitudes.Next() * f_[i++].Process<FILTER_MODE_BAND_PASS>(input);
    }"###,
        r###"    if (bank.Process(input, response)) {
      if (fixed) {
        for (int32_t i=0; i<num_modes;) {
          odd += gains[i] * response[i]; ++i;
          even += gains[i] * response[i]; ++i;
        }
      } else {
        for (int32_t i=0; i<num_modes;) {
          odd += amplitudes.Next() * response[i++];
          even += amplitudes.Next() * response[i++];
        }
      }
    }"###,
    );
    replace(
        &mut source,
        "\n}\n\n}  // namespace rings",
        "\n  bank.Store();\n}\n\n}  // namespace rings",
    );
    write(&destination.join(relative), &source);
    stage_elements(&destination);
    destination
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).unwrap();
    }
}

pub fn add_sources(build: &mut cc::Build, directory: &Path, overrides: &Path) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            add_sources(build, &path, overrides);
        } else if path.extension().and_then(|p| p.to_str()) == Some("cc") {
            let relative = path.strip_prefix("vendor/audio/mutable").unwrap();
            build.file(
                if matches!(
                    relative.to_str(),
                    Some("rings/dsp/resonator.cc" | "elements/dsp/resonator.cc")
                ) {
                    overrides.join(relative)
                } else {
                    path
                },
            );
        }
    }
}

fn stage_elements(destination: &Path) {
    let relative = "elements/dsp/resonator";
    let mut header = std::fs::read_to_string(format!("vendor/audio/mutable/{relative}.h")).unwrap();
    replace(
        &mut header,
        r###"  size_t clock_divider_;"###,
        r###"  size_t clock_divider_;
  unsigned cached_parities_;
  float cached_frequency_, cached_geometry_, cached_brightness_, cached_damping_;
  size_t cached_resolution_, cached_modes_;"###,
    );
    write(&destination.join(format!("{relative}.h")), &header);
    let mut source =
        std::fs::read_to_string(format!("vendor/audio/mutable/{relative}.cc")).unwrap();
    source.insert_str(
        0,
        r#"#include "svf_batch.h"
#include <cstring>
"#,
    );
    replace(
        &mut source,
        r###"void Resonator::Init() {"###,
        r###"void Resonator::Init() {
  cached_parities_ = 0;"###,
    );
    replace(
        &mut source,
        r###"  ++clock_divider_;"###,
        r###"  ++clock_divider_;
  // Upstream alternates higher-mode coefficient updates. Both parities must
  // observe unchanged inputs before their calculations can be reused.
  if(cached_parities_ && cached_frequency_==frequency_ && cached_geometry_==geometry_
      && cached_brightness_==brightness_ && cached_damping_==damping_
      && cached_resolution_==resolution_) {
    if(cached_parities_==3) return cached_modes_;
  } else {
    cached_frequency_=frequency_; cached_geometry_=geometry_;
    cached_brightness_=brightness_; cached_damping_=damping_;
    cached_resolution_=resolution_; cached_parities_=0;
  }
  cached_parities_|=1u<<(clock_divider_&1);"###,
    );
    replace(
        &mut source,
        r###"  return num_modes;"###,
        r###"  cached_modes_=num_modes;
  return num_modes;"###,
    );
    let start = source.find("  // Linearly interpolate position.").unwrap();
    let end = source[start..]
        .find("\n}\n\n}  // namespace elements")
        .unwrap()
        + start;
    source.replace_range(
        start..end,
        r###"  // Keep each oscillator's upstream recurrence. A fixed center pickup has
  // identical weights at every sample; its LFO-driven side pickup still advances.
  const float position_increment=(position_-previous_position_)/size;
  SvfBatch<kMaxModes> bank(f_,num_modes);
  float response[kMaxModes],fixed_center[kMaxModes];
  const bool fixed=previous_position_+position_increment==previous_position_;
  if(fixed) {
    CosineOscillator osc;
    osc.Init<COSINE_OSCILLATOR_APPROXIMATE>(previous_position_);
    for(size_t i=0;i<num_modes;++i) fixed_center[i]=osc.Next();
  }
  while(size) {
    const size_t take=std::min(size,kMaxBlockSize);
    CosineOscillator center_osc[kMaxBlockSize],side_osc[kMaxBlockSize];
    float center_gain[kMaxBlockSize][kMaxModes],side_gain[kMaxBlockSize][kMaxModes];
    for(size_t j=0;j<take;++j) {
      lfo_phase_+=modulation_frequency_;
      if(lfo_phase_>=1.0f)lfo_phase_-=1.0f;
      previous_position_+=position_increment;
      const float lfo=lfo_phase_>0.5f?1.0f-lfo_phase_:lfo_phase_;
      if(!fixed) center_osc[j].Init<COSINE_OSCILLATOR_APPROXIMATE>(previous_position_);
      side_osc[j].Init<COSINE_OSCILLATOR_APPROXIMATE>(modulation_offset_+lfo);
    }
    if(!fixed) for(size_t mode=0;mode<num_modes;++mode) {
      for(size_t j=0;j<take;++j) {
        center_gain[j][mode]=center_osc[j].Next();
      }
    }
    for(size_t mode=0;mode<num_modes;++mode) {
      for(size_t j=0;j<take;++j) {
        side_gain[j][mode]=side_osc[j].Next();
      }
    }
    for(size_t j=0;j<take;++j) {
      const float* center_weights=fixed?fixed_center:center_gain[j];
      float input=*in++*0.125f;
      float sum_center=0.0f,sum_side=0.0f;
      if(bank.Process(input,response)) {
        // Four independent partial sums remove the scalar accumulation chain.
        // Filter/oscillator recurrences remain upstream; only sum association
        // changes, covered by the unchanged numerical reference tolerance.
#if defined(__GNUC__) || defined(__clang__)
        using Float4 = float __attribute__((vector_size(16)));
        Float4 c{},s{};
#else
        float c[4]{},s[4]{};
#endif
        size_t i=0;
        for(;i+4<=num_modes;i+=4) {
#if defined(__GNUC__) || defined(__clang__)
          Float4 value,cg,sg;
          std::memcpy(&value,response+i,sizeof(value));
          std::memcpy(&cg,center_weights+i,sizeof(cg));
          std::memcpy(&sg,side_gain[j]+i,sizeof(sg));
          c+=value*cg; s+=value*sg;
#else
          for(size_t lane=0;lane<4;++lane) {
            c[lane]+=response[i+lane]*center_weights[i+lane];
            s[lane]+=response[i+lane]*side_gain[j][i+lane];
          }
#endif
        }
        sum_center=((c[0]+c[1])+c[2])+c[3];
        sum_side=((s[0]+s[1])+s[2])+s[3];
        for(;i<num_modes;++i) {
          sum_center+=response[i]*center_weights[i];
          sum_side+=response[i]*side_gain[j][i];
        }
      }
      *sides++=sum_side-sum_center;
      float bow_signal=0.0f;
      input+=bow_signal_;
      for(size_t i=0;i<num_banded_wg;++i) {
        float s=0.99f*d_bow_[i].Read();
        bow_signal+=s;
        s=f_bow_[i].Process<FILTER_MODE_BAND_PASS_NORMALIZED>(input+s);
        d_bow_[i].Write(s);
        sum_center+=s*center_weights[i]*8.0f;
      }
      bow_signal_=BowTable(bow_signal,*bow_strength++);
      *center++=sum_center;
    }
    size-=take;
  }
  bank.Store();"###,
    );
    write(&destination.join(format!("{relative}.cc")), &source);
}
