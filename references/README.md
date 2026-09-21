# Reference material

These sources explain design intent. They do not override Crest's explicit requirements. In particular, older screenshots use hexadecimal numbering, an older visual style and controls that have since been corrected.

| File | Intended use |
|---|---|
| 6988270f-b552-4f0c-955e-7e2084d22c27.png | Previous Crest/Questing Tracker Chord Designer bank overview, intervals and keyboard-style preview. |
| d0f43de9-9b4a-4bef-b560-197b157fa04c.png | Previous chord-slot editor and interval toggles. |
| a420e0c0-9933-49a6-b3e2-2d430f4faffe.png | Previous contextual FX command picker over the phrase grid. |
| f080979c-a345-41e5-9b1c-e70196948bbc.png | Previous contextual chord-choice picker over the phrase grid. |
| aaaccc19-5482-46f7-b721-2cb2552f808c.png | User-supplied Stria Android tracker Song minimap inspiration; activity in narrow track lanes. Final Crest display remains open. |
| oxi-one-mkii-manual.pdf | Official OXI ONE MKII manual v2.0a downloaded during the chord behavior review; retained for implementation reference. |

## OXI behaviors reviewed

The [official OXI ONE MKII manual](https://drive.google.com/file/d/1LdJvG-GqzzKI2qw92CyZKklE7kSoZLFE/view), printed pp. 150 and 153–154 (physical PDF pages 158 and 161–162), separates chord type, voicing and spread. Voicing offers inversions/open choices; spread adds upper-octave tones. Auto voicing supports smoother transitions, and user slots retain chord settings.

Crest uses those behaviors as inspiration while retaining explicit chord quality and its own three-slot FX model. Exact open-voicing pitch tables and a numeric spread mapping are not established by the feature-level review. The current Crest proposal is SPR as a count of added octave-doubled voices, not the older idea of alternately shifting existing voices apart. Manual voicing is the current proposed default. See A14 and the decision log.

## Other technical references

- [MIDI 1.0 message summary](https://midi.org/summary-of-midi-1-0-messages): channel voice and control-message semantics. Musical divisions are independent of MIDI channel counts.
- [Clean Architecture](https://blog.cleancoder.com/uncle-bob/2012/08/13/the-clean-architecture.html): inward dependencies and use-case ports.
- [Dirtywave support](https://dirtywave.com/pages/support): obtain the relevant official M8 manual/version when defining the exact supported FX catalog. The user requested M8-inspired effects; exhaustive command parity has not been audited here.

No dedicated drum-machine reference was found among these five user images. Euclidean and Randomizer visuals were explicitly left for further design. Crest Synth source code, its native API specification and a native Figma `.fig` export are not included in this package.
