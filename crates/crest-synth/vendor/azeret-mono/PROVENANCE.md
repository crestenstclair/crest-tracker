# Azeret Mono source provenance

This directory contains the Azeret Mono typeface required by the Crest Synth
visual system. `DESIGN.md` specifies Azeret Mono for every type style, in the
Regular, Medium, SemiBold, and Bold weights.

- Upstream: `https://github.com/google/fonts/tree/main/ofl/azeretmono`
- Original project: `https://github.com/displaay/azeret`
- License: SIL Open Font License 1.1, included verbatim as `OFL.txt`.
  Copyright 2021 The Azeret Project Authors.

`AzeretMono[wght].ttf` is the upstream variable font, retained byte-exact as the
provenance record. `ab_glyph` 0.2.32 does expose variation axes through its
`VariableFont` trait, but `epaint` 0.32.3 — the layer egui actually registers
fonts through — does not: `FontData` carries only the file bytes, a font-face
`index`, and a scale/offset `FontTweak`, and `epaint` never calls
`set_variation` anywhere. A variable font handed to egui therefore paints every
style at its default instance. The four static weights Crest uses are derived
from the variable source instead:

```
fonttools varLib.instancer "AzeretMono[wght].ttf" wght=<400|500|600|700> \
    -o AzeretMono-<Regular|Medium|SemiBold|Bold>.ttf
```

The derivation maps `DESIGN.md`'s named weights onto the upstream `wght` axis:
Regular 400, Medium 500, SemiBold 600, Bold 700. No outline, metric, or naming
data is edited by hand.

`SHA256SUMS` is the complete manifest of the files in this directory. Hashes are
computed over the byte-exact files as vendored.
