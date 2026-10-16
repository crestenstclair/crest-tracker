# Chorus source provenance

This directory contains only the audited source subset needed to build the
Rings `rings::Chorus` processor: Chorus, its `FxEngine`, the generated Rings
lookup resources it references, and the directly included stmlib headers. It
excludes firmware, drivers, bootloaders, hardware support, module UI code, and
runtime downloads.

- Eurorack upstream: `https://github.com/pichenettes/eurorack`
- Eurorack revision: `08460a69a7e1f7a81c5a2abcc7189c9a6b7208d4`
- stmlib upstream: `https://github.com/pichenettes/stmlib`
- stmlib revision: `e3bd7c9cc00e4364166f9905c0509b6ffd0535ec`
- License: MIT for the Chorus/FxEngine/resources source used by Crest. Those
  files retain their upstream MIT notices. The upstream stmlib license bundle
  is included verbatim as `LICENSE`.

`SHA256SUMS` is the complete manifest of vendored upstream files. Hashes are
computed over the byte-exact files at the revisions above.

