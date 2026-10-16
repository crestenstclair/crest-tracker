# Adversarial Review: Exact editing and bounded grid windows

**Date:** 2026-10-15 · **Mode:** Explicit symbol scope in working files
**Files:** 13 · **Raised:** 8 candidates, 6 unique findings · **Survived skeptic:** 6
**Hunter coverage:** All nine completed: Bloaters, OO Abusers, Change Preventers, Dispensables, Couplers, SOLID, Function Clarity, Module Architecture, Function Flow. One independent skeptic verified every merged finding. No unresolved or refuted findings.

The repository contains pre-existing untracked implementation, so HEAD was not used as the review baseline. Evidence and line numbers below describe the frozen pre-fix sources. The review did not cover the complete application or certify native/physical acceptance.

## Critical

None.

## Major

### Exact Drum step membership disagrees with native editing

The hit renderer used beat-distance tolerance `<1e-9`; off-grid rendering used step-distance tolerance `>1e-9`. Native editing compares rational beats exactly. `1/10000000000` appeared as an active step-zero hit and disappeared from the off-grid list; `500000001/2000000000` appeared both on and off a sixteenth-note grid. A toggle inserted a new exact-grid note instead of removing the displayed near-grid note.

**Fix:** One exact rational step-window index now feeds both hit cells and off-grid entries. Original timing, pitch/target grouping and the native edit contract remain intact. **Skeptic:** CONFIRMED (10/10). Node regression covers both reproductions and tuplets; native-domain regression proves toggle/Undo preserve the precise off-grid event.

## Minor

- **Initializer length:** New editing namespace had 124 physical lines against the 80-line criterion. Exact time parsing/conversion now has its own 55-line namespace; the FX namespace has 75 lines. **Skeptic:** CONFIRMED (10/10).
- **Coupled window parameters:** Drum helpers separately accepted `(lane,start,end,phrase)`. They now consume one exact window result and no longer repeat matching predicates. **Skeptic:** CONFIRMED (9/10).
- **Repeated FX dispatch:** Arity, parse and format switches required coordinated changes. A per-kind ordered codec definition now owns field count, parsing and formatting; round-trip fixtures still cover all 19 variants and reordered JSON keys. **Skeptic:** CONFIRMED (10/10).
- **Tempo output unit:** `parseTempo` returned unlabelled milli-BPM. `parseTempoMilliBpm` now documents its integer wire representation; consumers and existing boundary tests use the explicit name. **Skeptic:** CONFIRMED (9/10).
- **Hidden parser contracts:** `integer` and `fields` concealed strict syntax/field validation. Renamed to `parseSafeDecimalInteger` and `parseNonemptyCommaFields`, with the optional count contract documented. **Skeptic:** CONFIRMED (9/10).

## Summary

All six findings were addressed. Parser behavior is bracketed by the same four passing regression tests. Additional view tests cover exact Drum membership, absolute tuplet windows, a 150,000-note Piano Roll summary and complete event-page coverage. Native UI proof and optimized packaging passed and are recorded separately in README. No Synth ownership or pure-core dependency boundary changed.

## Independent verification details

# Independent skeptic verification

Reviewed the six merged candidates against the supplied scope, all five hunter reports, and `smells-bloaters.md`, `smells-oo-abusers.md`, `solid-violations.md`, `clean-code-functions.md`, and `clean-code-modules.md`. Reopened the cited source and consumers. No source files were changed. The explicit symbol scope governs this review: Git reports the reviewed JavaScript files as untracked, so HEAD is not a valid baseline. No different repository length limit was found.

All six candidates are substantiated. One is Major; five are Minor. Current parser tests pass (4/4), which does not refute the structural findings. The tempo-documentation claim is narrowed below to acknowledge the existing test documentation.

## 1. Namespace initializer physical length

- finding: Long Method at `webview-page/edit-values.js:3-126`.
- verdict: CONFIRMED (confidence 10/10).
- severity: Minor.
- basis: Reopened the complete file and independently counted declaration through closing body line, including nested declarations, comments, and blank lines. The Node measurement was `{"iifeStart":3,"iifeEnd":126,"physicalLines":124}`. The declaration is `const CrestTrackerEditValues = (() => {`; the final body line is `})();`. The bloaters catalog explicitly limits functions to 80 physical lines and supplies no namespace-wrapper exception. This is a new function under the supplied scope. Its nested helpers being short and cohesive does not disprove the explicit limit violation.
- harm and fix: Keep the claim structural, not a runtime failure. Beat parsing, tempo conversion, and FX codecs all occupy the same over-limit initializer. Extract named private helper/codec construction units and compose the same five-operation public API in a short initializer, preserving accepted syntax, errors, and namespace isolation. Avoid replacing one long wrapper with another.

## 2. Coupled Drum row-window parameters

- finding: Long Parameter List at `webview-page/host.js:1265-1274,1285-1290`; callers at `1242,1255-1257`.
- verdict: CONFIRMED (confidence 9/10).
- severity: Minor.
- basis: Signature extraction independently returned four parameters for each helper: `["lane","start","end","phrase"]`. The caller derives both bounds from the model: `start=model.rowStart,end=start+model.rows.length`; both calls forward the same `start,end` group. This meets the catalog's 4+ threshold and shared-source/forwarded-group condition. The values are not four independent inputs: both helpers interpret one inclusive-start/exclusive-end window. Current correct call order is not an exception.
- harm and fix: A row-window representation change requires matching edits to both signatures and callers; exchanging the adjacent bounds silently produces empty cells or off-grid results. Introduce one `{start, end}` window parameter and preserve the boundary comparisons. The finding does not claim that current pagination is wrong.

## 3. Repeated FX dispatch

- finding: Switch Statements / Open-Closed violation at `webview-page/edit-values.js:72-94,100-121`.
- verdict: CONFIRMED (confidence 10/10).
- severity: Minor.
- basis: Reopened the arity lookup, parser switch, formatter switch, and `host.js:938,949,953` consumers. The source contains `const count = { retrigger: 2, cc: 2, poly_pressure: 2, voicing: 2, send: 3 }[kind];`, `switch (kind)`, and `switch (fx.kind)`. Send, retrigger, voicing, tempo, and other variants appear in both dispatches; both switches throw `Unknown FX` by default. The catalogs' repeated-type-dispatch condition is met. The exception for one isolated stable boundary switch does not apply to this repeated bidirectional contract.
- execution: Evaluated modified strings in memory only to test the stated omission hazards. Removing only `send: 3` left its parser branch intact but changed valid `0,65,1/2` input to `Enter exactly 1 nonempty value`. Removing only the Send formatter case caused `Unknown FX`. Unmodified Send parsed successfully; all four shipped parser tests passed.
- harm and fix: Adding a multi-field editable variant or changing its field order requires synchronized arity, parse, and format updates. Keep the finding at Minor: no current variant failure was found. A per-kind codec registry owning these three decisions removes the repeated obligation while preserving special control normalization, saved chords, null effects, exact numbers, error behavior, and native domain validation.

## 4. Inexact Drum step classification

- finding: Divergent ownership / incompatible tolerance units at `webview-page/host.js:1275-1289`; native comparison at `crates/crest-tracker-app/src/app.rs:2522-2547`.
- verdict: CONFIRMED (confidence 10/10).
- severity: Major.
- basis: Reopened `drumEventsAt`, `drumOffgrid`, `drumCell`, lane grouping, native toggle, and rational time construction. Hit selection uses `Math.abs(value(event.at)-grid(phrase)*step)<1e-9`; off-grid selection uses `Math.abs(step-Math.round(step))>1e-9`. Their tolerances use beats and steps respectively. Native toggle computes the exact division multiple and requires `event.at == at`, along with matching target and pitch. The click callback sends step, pitch, and target, not the rendered event ID. `Beat` construction and decoding accept these positive-denominator u64 fractions; the sixteenth division is exactly `1/4`.
- execution: Evaluated the actual extracted shipped `drumEventsAt` and `drumOffgrid` functions with their actual `value`, `divisionOf`, and `grid` helpers; stubbed only label/button construction. Used the shipped text parser and `compareBeats` for independent exact comparison. With grid `0.25` and window `[0,64)`, results were:
  - `0.0000000001` parsed as `1/10000000000`: step 0 had one grid match, zero off-grid entries, and no exact grid match.
  - `500000001/2000000000`: step 1 had one grid match, one off-grid entry, and no exact grid match.
  - Exact control `1/4`: step 1 had one grid match, zero off-grid entries, and an exact grid match.
- harm and fix: Both hunter reproductions hold. A displayed active step represents an event the native toggle cannot remove at that step; absent another exact match, the native `else` branch adds an exact-grid note. The second reproduction also lists the same note as both a grid hit and off-grid. Consolidate exact rational step classification and consume it for both views, preserving native toggle semantics and original timing. Add a near-grid regression. This is a correctness repair preserving the intended exact-time contract, not a claim that existing incorrect rendering stays unchanged. No new native UI interaction was executed for this review; the JS reproduction plus the inspected native branch establishes the mismatch.

## 5. Tempo parser return-unit contract

- finding: Hidden output unit at `webview-page/edit-values.js:53-63`; consumers at `host.js:497,1054` and formatter at `edit-values.js:111`.
- verdict: CONFIRMED (confidence 9/10), with documentation claim narrowed.
- severity: Minor.
- basis: The exported `parseTempo` accepts displayed BPM text, multiplies by `1000n`, and returns a primitive number. Direct execution yielded `parseTempo("108.125") === 108125`; the formatter returned `"108.125"`. Existing consumers serialize the result correctly. The source has no function-facing unit contract and uses `beats` for the intermediate BPM rational. However, `scripts/test_edit_values.cjs:63-68` explicitly names milli-BPM and verifies the conversion, so a blanket claim that the unit is undocumented anywhere would be inaccurate.
- harm and fix: Coexisting displayed BPM and wire milli-BPM remain indistinguishable in the exported operation's name/result type. A new display/calculation consumer must inspect implementation or tests to avoid a 1000-fold mistake. The catalog explicitly calls for units where callers cannot infer them. Rename to `parseTempoMilliBpm`, use `bpm` for the intermediate, and state the positive integer wire-result contract. Preserve all values and errors. Existing correct consumers and explanatory tests mitigate severity but do not remove the API-contract issue.

## 6. Strict parser helper names

- finding: Noun names conceal parsing and validation at `webview-page/edit-values.js:37-50`; callers at `68,73,76-93`.
- verdict: CONFIRMED (confidence 9/10).
- severity: Minor.
- basis: Reopened both complete bodies and their callers. `integer` performs signed decimal syntax validation plus safe-integer checking; `fields` parses comma-separated text, rejects empty fields, and optionally enforces exact cardinality. The module comment says the boundary rejects ambiguous syntax but does not identify these specific helper operations. The function-clarity catalog explicitly rejects noun names hiding parsing/validation; these are ordinary functions, not constructors.
- execution: Actual public parser calls rejected `bend: "0x2000"` with `FX values must be exact integers` and Send inputs `0,,1/2` and `0,65,1/2,99` with `Enter exactly 3 nonempty values`; valid Send still parsed correctly. These checks establish the extra guarantees beyond plain `Number` or `split` conversion.
- harm and fix: Refactoring the apparent conversions without inspecting the bodies can discard syntax, safe-number, or cardinality guarantees central to this boundary. Rename to `parseSafeDecimalInteger` and `parseNonemptyCommaFields`, with `expectedCount` and a short optional-cardinality contract. No current malformed-input acceptance or caller failure is alleged.

## Unresolved or refuted findings

None. Confidence describes evidence strength, not impact. The Major Drum defect should lead the final report; the five Minor findings remain concrete catalog-supported maintenance concerns.

## Subsequent native regression

A stricter page-boundary measurement reproduced first-row focus hidden by the sticky header. The fix reserves measured sticky-header/lane-label space through native scroll margins. The same 34 page crossings now pass, within a 102-action native playback run: p95 34.533 ms, zero lost/mismatched acknowledgments. This was found after the six-finding review and is not attributed to the hunters. See README and `target/workload/native-feedback-visible-windows-20261016.json`.
