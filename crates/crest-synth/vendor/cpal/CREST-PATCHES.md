# CPAL provenance

This is the published `cpal` 0.18.1 crate, Apache-2.0, from
<https://github.com/RustAudio/cpal> commit
`94ecb6ec64546308885a59b38e29f938796e8100`. The crate archive SHA-256 is
`5f77b11176c37874be37e8d691c946e31b2b8c357abce9526f6a99eb469e1028`.
Upstream sources, platform implementations, and license are retained.

Crest changes only the ALSA `poll_for_period` error branch: `EINTR` returns
`Poll::Pending` (including alsa 0.11's negative poll errno), allowing the worker to check stream shutdown and retry its
wait. A handled signal does not mean the audio device failed. All other
errors retain upstream behavior. This applies to input and output streams.
`tests/alsa_signal_recovery.rs` in Crest exercises the actual output stream
under targeted signals; the unpatched crate reports a device error for each
interruption. Remove this adaptation when an upstream release includes it.
