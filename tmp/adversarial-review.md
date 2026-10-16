# Adversarial Review: Piano viewport and exact Timeline focus

**Date:** 2026-10-16 · **Mode:** Explicit files with partial-file boundaries, reviewed in the live working tree.
**Files:** 6 · **Raised:** 24 candidates (22 hunter findings and 2 directly reproduced defects) · **Merged review items:** 22.
**Survived skeptic:** 22 supported original items: 0 outstanding, 22 corrected during review. **Refuted:** 0. **Unresolved:** 0.
**Hunter coverage:** All nine required roles completed independently. Two separate skeptic agents verified bounded batches of 8 and 14 items. No required role was omitted.

## Scope and evidence handling

The review covered all of `webview-page/phrase-view.js` and `crates/crest-tracker-app/src/app/timeline_focus.rs`; only the specified Piano viewport, feedback, location, and Expanded-row functions in `webview-page/host.js`; only the specified timeline fields, projection, Locate validation, navigation dispatch, and feedback branch in `app.rs`; only Timeline Surface/Focus additions in `feedback.rs`; and only timeline validation and the per-view gate in `scripts/native_feedback.py`. Surrounding source and tests were read to establish contracts. The rest of the large host and application implementations, Synth, and unrelated dirty work were not independent review targets.

The source files were untracked, so the review used the explicit scope rather than treating every untracked application file as reviewable. No usable tracked history existed for the scoped source. Nine-role coverage applies to the supplied scope; subsequent owner corrections received targeted independent rechecks rather than another complete hunter pass.

The root agent corrected defects and some structural candidates while review was running. Original evidence is preserved under `/tmp/crest-viewport-timeline-review/baseline`; each verdict distinguishes that evidence from the final checked code. Corrected findings are not outstanding blockers, and a correction is not a refutation of the original evidence. Discovery quotations retain their original line references; the fresh-verification paragraph supplies current locations when code moved.

The existing report was preserved first at `tmp/adversarial-review.before-viewport-20261016T052156.391899.md`. Hunter reports, merged candidates, independent verdicts, and reproductions remain under `/tmp/crest-viewport-timeline-review`.

## Coverage

| Hunter | Completed | Candidates raised |
| --- | --- | ---: |
| Bloaters | Yes | 3 |
| OO Abusers | Yes | 0 |
| Change Preventers | Yes | 0 |
| Dispensables | Yes | 0 |
| Couplers | Yes | 1 |
| SOLID | Yes | 0 |
| Function Clarity | Yes | 11 |
| Module Architecture | Yes | 4 |
| Function Flow | Yes | 3 |

The split displayed-row transition was merged across Architecture and Function Flow (R02). The interval-query parameter and boundary-policy reports were merged across Bloaters and Function Clarity (R09); the final correction addresses both the parameter relationship and boundary-policy contract. Naming, state ownership, traversal, and representation findings were otherwise kept distinct.

## Verification

The independent checks reproduced the stale retained-row pointer conversion and the legacy-schema timeline validation bypass against the preserved baseline, then verified their current regressions. The checked JavaScript suites passed 14 phrase-view tests and 1 retained-row interaction test. The retained-row interaction regression was rerun after the final detached-viewport extraction and passed again. The Python native-feedback suite passed all 10 tests. The mutable-cache probe first reproduced loss of a sustained note without changing its authored duration; the new render-entry regression verified the corrected public result boundary. Parameter counts, current callers, and Python traversal depth were checked directly. Small standalone Rust probes checked selector wildcard and slice-index/empty-input contracts, not production rational arithmetic.

A direct `cargo test -p crest-tracker-app expanded_` invocation failed before tests because the default pinned Synth dependency lacks APIs already required by the application. It also caused Cargo to refresh three dependency resolutions in the untracked lockfile; the root was notified immediately and is reconciling resolution through the project's paired override. No further Cargo command was run by the review. That failed invocation is not passing verification. Separately, the root reported 85 application tests and 26 JavaScript tests passing with the intended paired setup; those results are owner-reported evidence, not an independently passing Rust run from this review.

No application or Synth source was edited by the review agents. Deliberate review outputs were the report, its backup, and scratch evidence. Native-window QA remained owned by the root agent and is not replaced by the mock-DOM regressions described here.

## Critical

No outstanding findings at this severity.

## Major

No outstanding findings at this severity.

## Minor

No outstanding findings at this severity.

## Corrected during review

These findings were independently substantiated in the original code and independently rechecked after the root agent corrected them. Historical severities describe the pre-correction impact.

### R01 — Timeline destination parameter lists

**Skeptic:** DOWNGRADED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The original signatures meet the catalog criterion: independently counted 5 explicit native parameters (excluding &self) and 4 browser parameters, with fields taken from the same Request::Locate or entry.row. The original observation is valid, but its bounded positional-argument maintenance risk warrants Minor rather than Major. Current code removes that risk: timeline_source_beat takes 2 explicit parameters at timeline_focus.rs:5-9; SourceLocation names occurrence/start/row/row_at at app.rs:508-516; locate accepts that record and view at app.rs:1954; browser locate has 3 parameters at host.js:413 and preserves entry.row at host.js:513,517. This is a confirmed historical smell corrected during review, not a refutation of its original evidence.

**Discovery evidence, harm, and proposed correction:**

#### 1. Timeline destination identity is decomposed into positional arguments

- smell: Long Parameter List (Bloaters)
- location: `crates/crest-tracker-app/src/app/timeline_focus.rs:5-12`; corresponding browser helper at `webview-page/host.js:413` and caller at `webview-page/host.js:512`.
- evidence: The native validator has **5 explicit parameters**, excluding `&self`:

  ```rust
    pub(super) fn validate_timeline_row_location(
        &self,
        occurrence: Occurrence,
        start: Beat,
        source_row: usize,
        at: Beat,
        view: View,
    ) -> Result<(), DocumentError> {
  ```

  All five come from the same `Request::Locate` variant in `app.rs:1081-1092`:

  ```rust
            Request::Locate {
                occurrence,
                at,
                row,
                view,
                row_at,
            } => {
                if let Some(row_at) = row_at {
                    self.validate_timeline_row_location(occurrence, at, row, row_at, view)?;
                }
                self.locate(occurrence, at, row, view)?;
                self.timeline_row_at = row_at;
            }
  ```

  The browser helper has **4 parameters**, and the source row and displayed beat are extracted from the same `entry.row`:

  ```javascript
  const locate=(span,view=model.view,row=0,row_at=null)=>emit({kind:"locate",occurrence:span.occurrence,at:span.start,row,view,row_at});
  ```

  ```javascript
      if(entry){const control=button(entry.row.cells[column],()=>locate(entry.span,"expanded",entry.row.source_row,entry.row.at));
  ```
- harm: An Expanded destination needs both its occurrence start and its displayed row beat, plus the source row. The native signature accepts the two beats in separate positional slots with the same `Beat` type, so swapping them compiles and rejects or resolves a different destination. The browser signature also allows a caller to supply a source row while accidentally omitting its distinct off-grid displayed beat. Adding another identity component requires changes to both argument lists and their callers.
- fix: Introduce Parameter Object for a timeline destination containing the occurrence/span start and the source-row/displayed-beat identity. Pass that destination to validation and navigation; preserve the complete `entry.row` when constructing the browser destination rather than separating its fields into positional arguments.
- confidence: 9/10.
- proposed severity: P2 (medium).

### R02 — Split exact-row focus transition

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Baseline app.rs:1088-1092 and timeline_focus.rs:71-78 both restore timeline_row_at after locate deliberately clears it at baseline app.rs:1988. The two callers therefore really own a required repair sequence. Current Request::Locate constructs SourceLocation at app.rs:1105-1113, and Expanded navigation passes Some(row.at) inside that record at timeline_focus.rs:89-98. Current locate validates and resolves before mutation at app.rs:1963-2005, then writes timeline_focus, timeline_row and timeline_row_at together at app.rs:2014-2016. No caller repair assignment remains. The ownership finding was valid and is now corrected; focused Rust runtime verification was blocked by unrelated pinned Synth API incompatibility.

**Discovery evidence, harm, and proposed correction:**

##### 1. Displayed-row navigation requires callers to finish a partial focus transition

- smell: Split state-transition ownership (Module architecture).
- location: `crates/crest-tracker-app/src/app/timeline_focus.rs:70-82`; `crates/crest-tracker-app/src/app.rs:1087-1092,1986-1988`.
- evidence:

  The raw request path must validate the displayed beat, perform source navigation, and then repair the displayed-row component:

  ```rust
                if let Some(row_at) = row_at {
                    self.validate_timeline_row_location(occurrence, at, row, row_at, view)?;
                }
                self.locate(occurrence, at, row, view)?;
                self.timeline_row_at = row_at;
  ```

  Keyboard navigation must independently remember the same final write:

  ```rust
        if self
            .locate(span.occurrence, span.start, row.source_row, View::Expanded)
            .is_err()
        {
            return false;
        }
        self.timeline_row_at = Some(row.at);
  ```

  The underlying operation deliberately erases that part of the identity:

  ```rust
        self.timeline_focus = Some(at);
        self.timeline_row = source_row;
        self.timeline_row_at = None;
  ```

  Downstream identity depends on the combination:

  ```rust
        row.source_row == self.timeline_row && self.timeline_row_at.is_none_or(|at| row.at == at)
  ```
- harm: The App owns one focus identity consisting of occurrence, occurrence start, source row, and optional exact displayed beat, but no operation owns its complete update. Both existing navigation paths must know that `locate` clears `timeline_row_at` and restore it afterward. Adding another displayed-row navigation path, or moving either assignment before `locate`, silently falls back to matching the first row with that source-row number and loses the distinction between off-grid rows. This is a concrete maintenance obligation even though both current callers perform the required sequence.
- fix: Encapsulate State Transition. Add one location operation that accepts the optional displayed beat and commits all focus fields together after the existing checks. Preserve the current generic `locate` behavior through an adapter passing `None`; pass `Some(row.at)` for keyboard navigation and the already-validated `row_at` for raw requests. A small named location record is sufficient if it makes these inputs clearer; no navigation framework is needed.
- confidence: 9.
- criterion: Clean Code at the Module Level, “Own state transitions explicitly” and “Encapsulate decisions and representations.”
- severity: Minor. Current callers are correct, but two entry points already depend on an undocumented repair sequence for a musically significant identity.



Independent Function Flow agreement:
- smell: Exact displayed-row focus requires callers to complete a partially applied location transition (Function flow)
- location: crates/crest-tracker-app/src/app.rs:1088–1092 and 1986–1988; crates/crest-tracker-app/src/app/timeline_focus.rs:71–77.
- evidence: The request path calls `self.locate(occurrence, at, row, view)?;` and then separately performs `self.timeline_row_at = row_at;`. The keyboard path likewise calls `.locate(span.occurrence, span.start, row.source_row, View::Expanded)` and, only after checking its result, performs `self.timeline_row_at = Some(row.at);`. Inside `locate`, the transition writes `self.timeline_focus = Some(at);`, `self.timeline_row = source_row;`, and `self.timeline_row_at = None;`.
- harm: One meaningful focus identity consists of occurrence, span start, source row, and optional displayed beat. The operation that sets the first parts explicitly clears the last, so both callers must know to repair it after success. Understanding or adding an Expanded navigation path requires reconstructing this hidden postcondition; omitting the repair loses the distinction between off-grid rows that share a source row. The existing two paths perform the repair correctly, so this is a distributed-invariant finding rather than a claim of an observed navigation failure.
- fix: Encapsulate State Transition by allowing the location operation to receive the optional displayed beat and install the final `timeline_row_at` alongside `timeline_focus` and `timeline_row`. Existing source-only callers pass `None`; the request and Expanded keyboard paths pass their exact beat. Preserve the request's existing row-location validation before mutation, all source-resolution errors, source-row clamping, and view-opening behavior. Remove the caller-side assignments once the transition owns them.
- confidence: 9
- criterion: Clean Code Across Function Boundaries — “Transitions remain complete,” “Keep each decision and invariant together,” and “Preserve behavior while improving the flow.”
- severity: Minor — the current callers are correct, but the required caller repair makes the off-grid focus invariant fragile.

### R03 — Partial focused-row predicate contract

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The original predicate contract finding remains CONFIRMED, Minor, confidence 9. Follow-up inspection verifies correction: timeline_focus.rs:101-105 now names matches_timeline_row_selector and explicitly states that callers restrict occurrence/span, None matches any displayed time for the source row, and a single selection uses the chronological first match. All three callers use the new name at lines 71, 78 and 125 and preserve their span restrictions. app.rs:568-569 now documents the None fallback on timeline_row_at. The predicate expression is unchanged, so its intentional selection behavior is preserved. Historical probe evidence remains valid; no new runtime test was needed or run for this rename/documentation correction.

**Discovery evidence, harm, and proposed correction:**

9. A focused-row predicate actually matches a partial row identity, with None as a wildcard.

- smell: Predicate overpromises focus identity and leaves absence semantics implicit (Function clarity)
- location: crates/crest-tracker-app/src/app/timeline_focus.rs:81-83; consumers at lines 48-60 and 98-104; matching webview fallback at webview-page/host.js:436
- evidence:
  ```rust
  fn is_focused_timeline_row(&self, row: &TimelineRow) -> bool {
      row.source_row == self.timeline_row && self.timeline_row_at.is_none_or(|at| row.at == at)
  }
  ```
  ```rust
  .find(|row| self.is_focused_timeline_row(row))
  ```
- harm: The predicate does not establish that a row belongs to the focused occurrence, and when timeline_row_at is None it accepts every displayed off-grid row sharing the source row. Current consumers correctly narrow the span first and use find, which makes the first matching row the effective fallback. A new consumer filtering an entire lane by is_focused_timeline_row can select multiple rows or a different occurrence. The module comment about exact identities does not explain this deliberately partial match or the None wildcard.
- fix: Rename Function to matches_timeline_row_selector and document that callers must already restrict the occurrence/span, that None matches any displayed time for the source row, and that callers selecting one row use the first chronological match. Add the same meaning to the timeline_row_at field contract; preserve the predicate and current fallback behavior.
- confidence: 9
- criterion: Names must describe the operation; A return value must fulfill the name's promise; Comments explain contracts and reasons.
- severity: Minor — the existing search context makes the predicate correct, but its name promises more identity checking than it performs.

### R04 — Expanded navigation handled boolean

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The original undocumented handled/fallback finding remains CONFIRMED, Minor, confidence 9. Follow-up inspection verifies correction: timeline_focus.rs:54 explicitly says True means handled, including clamped no-ops, and False permits grid fallback. The function at lines 55-99 still rejects an empty lane, clamps its index and returns locate(...).is_ok(). app.rs:2091-2092 forwards the result; app.rs:1409-1416 still uses it to suppress or allow grid fallback. The new documented boolean contract directly resolves the ambiguity without changing the control-flow protocol; a function rename or richer result type is not required.

**Discovery evidence, harm, and proposed correction:**

10. Expanded navigation returns an undocumented handled/fallback boolean.

- smell: Ambiguous command return contract (Function clarity)
- location: crates/crest-tracker-app/src/app/timeline_focus.rs:37-78; dispatch at crates/crest-tracker-app/src/app.rs:2063-2064 and consumer at lines 1386-1393
- evidence:
  ```rust
  pub(super) fn navigate_expanded_rows(
      &mut self,
      timeline: &Timeline,
      lane: usize,
      vertical: i32,
      changed_lane: bool,
  ) -> bool {
  ```
  ```rust
  let index = index
      .saturating_add_signed(vertical as isize)
      .min(rows.len() - 1);
  ```
  ```rust
  self.timeline_row_at = Some(row.at);
  true
  ```
  ```rust
  if matches!(self.view, View::Expanded | View::Minimap)
      && self.navigate_timeline(horizontal, vertical)
  {
      return;
  }
  self.column = self.column.saturating_add_signed(horizontal as isize);
  self.row = self.row.saturating_add_signed(vertical as isize);
  ```
- harm: true means the navigation request was handled, including a clamped move that leaves the same row focused; it does not mean focus changed. false permits the caller to fall back to ordinary grid navigation and covers both an empty lane and a failed locate. A maintainer using this boolean to count changed actions, or changing boundary no-ops to false, can accidentally count unchanged actions or invoke the fallback path. The command's name and signature do not expose that distinction.
- fix: Document the handled/fallback result explicitly and rename the local dispatch result to timeline_navigation_handled where consumed. Rename Function to try_handle_expanded_navigation if adopting that convention. Keep the boolean and all existing control flow; no new result framework is required.
- confidence: 9
- criterion: A return value must fulfill the name's promise; Comments explain contracts and reasons.
- severity: Minor — a meaningful control-flow protocol is passed as an unexplained bool, with existing callers currently interpreting it correctly.

### R05 — Nearest-row input and result contract

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The original missing input/index contract finding remains CONFIRMED, Minor, confidence 9. Follow-up inspection verifies correction: timeline_focus.rs:149-152 now documents a slice-index result, a nonempty input and ordering by displayed musical time, while retaining the existing tie and unrepresentable-distance policies. The function is renamed nearest_displayed_row_index; both calls at lines 81 and 83 use that name, and the empty-input guard remains at lines 62-65. The partition_point and nearest-choice algorithm at lines 153-163 is unchanged. Historical probe evidence remains valid as explanation of these preconditions; no new runtime test was needed or run for the rename/documentation correction.

**Discovery evidence, harm, and proposed correction:**

11. The nearest-row documentation omits its nonempty chronological-input requirement.

- smell: Missing input invariant and result-unit contract (Function clarity)
- location: crates/crest-tracker-app/src/app/timeline_focus.rs:127-140; caller guard at lines 44-46; row collection at lines 123-125
- evidence:
  ```rust
  /// Horizontal movement selects the nearest displayed musical row. Ties choose
  /// the earlier row; unrepresentable derived distances keep the later candidate.
  fn nearest_row(rows: &[(&PhraseSpan, &TimelineRow)], at: Beat) -> usize {
      let next = rows.partition_point(|(_, row)| row.at < at);
      if next == 0 || next == rows.len() {
          return next.min(rows.len() - 1);
      }
  ```
- harm: The existing documentation explains tie and overflow policy but not the required preconditions: rows must be nonempty and globally ordered by displayed at, and the usize result is an index into the supplied slice rather than source_row. partition_point depends on chronological order, and rows.len() - 1 cannot handle an empty slice. The current caller guards emptiness and the producer supplies ordered rows; another caller collecting by source row or using the returned number as a source row would silently choose the wrong displayed event or panic.
- fix: Rename Function to nearest_displayed_row_index and document the nonempty, chronological ordering requirements on the slice. Document that the return is its slice index. Preserve the current algorithm, tie policy and caller guard.
- confidence: 9
- criterion: Names must describe the operation; Comments explain contracts and reasons; Make input relationships and effects explicit.
- severity: Minor — a partially documented helper omits the input and output facts needed for safe reuse.

### R06 — Nested per-view timeline traversal

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The original depth-2 per-view traversal violation remains CONFIRMED, Minor, confidence 10. Follow-up source and Python AST inspection verify correction: the view loop at native_feedback.py:67 now calls timeline_playback_latencies(samples, view) at line 68 and contains zero nested source-level traversals. The helper at lines 87-91 owns a single list comprehension selecting native-keyboard, playing samples in the requested view. Gate depth is 1 and helper depth is 1. Filter order, input order, the fewer-than-50 short circuit, ceil(0.95*n)-1 percentile and strict 50,000,000 ns rejection threshold remain unchanged at lines 69-70. This is a complete extracted subproblem, not a helper carrying loop state. No new Python test run is claimed for this follow-up; the earlier 10-test run predates this extraction.

**Discovery evidence, harm, and proposed correction:**

- smell: Nested source-level traversal in the per-view timeline gate (Function flow)
- location: scripts/native_feedback.py:66–72, read_measurement.
- evidence: The outer traversal is `for view in ("expanded", "minimap"):`. Its body contains `measured = [sample["receipt_to_ack_ns"] for sample in samples` followed by `if sample["origin"] == "native_keyboard" and sample["state"]["playing"]` and `and sample["state"]["view"] == view]`. The measured source-level iteration depth is two: views, then samples. The enclosing `if surface == "timeline":` does not affect this count.
- harm: This violates the explicit catalog standard, “at most one level of iteration inside a function.” The reader must follow both the per-view policy and individual sample eligibility within the same traversal. This is a readability finding, not a performance claim, and does not require a failing runtime example.
- fix: Extract Function `timeline_playback_latencies(samples, view)` to own the complete selection of native-keyboard playback measurements for one view and return the latency list. Keep the outer loop responsible for the minimum count and p95 gate. Preserve input order, the exact origin/playing/view filters, short-circuiting when fewer than 50 samples exist, the percentile index, and the strict 50,000,000 ns threshold.
- confidence: 10
- criterion: Clean Code Across Function Boundaries — “Keep iteration one level deep within each function” and “Extract a complete subproblem.”
- severity: Minor — an explicit structural violation in a bounded diagnostic path, with no demonstrated incorrect result.

### R07 — Retained pointer closure — corrected during review

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Critical · **Current status:** corrected.

**Fresh verification:** The retained baseline has handlers capturing metrics at host.js:1580, viewport replacement at lines 1610-1619, and pointer conversion using the captured metrics at lines 1621-1625. Running repro-stale-pointer.cjs independently produced rendered section start 32, click beat 0/1, and double-click insertion beat 0/1. Current handlers at host.js:1581 no longer capture metrics, and pianoPointerAction reads pianoViewport.metrics at lines 1622-1626. The current retained-row regression passes across section starts 32, 64 and 0, asserting cursor and insertion beats while preserving the row node. The original wrong-time edit contract is confirmed; it is corrected and must not be presented as an outstanding blocker. Verification uses a minimal DOM harness, not a full native-window session.

**Discovery evidence, harm, and proposed correction:**

#### O1. Retained rows capture stale window coordinates

- smell: Incomplete viewport transition (Function flow)
- location: `webview-page/host.js:1558-1564`, `1578-1580`, `1607-1624` in the baseline snapshot. Root has since fixed this candidate; retain the original confirmed evidence and verify the replacement.
- evidence:

  ```javascript
      if(compareBeats(pianoViewport.metrics.start,metrics.start)!==0)updatePianoTimeWindow(pianoViewport,metrics);
  ```

  ```javascript
      const name=document.createElement("span");name.textContent=model.pitchLabels?.[pitch]||noteName(pitch);row.append(name);row.onclick=e=>pianoPointerAction(e,row,metrics,pitch,false);row.ondblclick=e=>pianoPointerAction(e,row,metrics,pitch,true);roll.append(row);}
  ```

  ```javascript
    view.metrics=metrics;view.visibleBounds=null;view.selectionKey=null;
  ```

  ```javascript
      const at=metrics.atPixel(event.clientX-row.getBoundingClientRect().left,model.pianoControls.grid);
  ```
- harm: A section change retains the pitch-row DOM and its event handlers. The rendered section uses the new window while the handlers retain the initial window. With a rendered section beginning at beat 32, a click and a double-click at its origin resolve to beat 0, moving the cursor or inserting a note at the wrong source position.
- fix: Encapsulate State Transition so pointer conversion reads the active viewport window when the event fires. Keep retained DOM and exact beat conversion, while removing captured obsolete geometry. Preserve intended interaction behavior and add a retained-row interaction regression.
- confidence: 10/10.
- criterion: `clean-code-function-flow.md`, “Keep each decision and invariant together” and “Transitions remain complete.”
- severity: Critical before correction, because the stale interaction inserts at the wrong musical time. Resolved during this review by the root agent; do not list as an outstanding blocker.
- execution: `node /tmp/crest-viewport-timeline-review/repro-stale-pointer.cjs` against the original source printed `Rendered section start: 32`, `Click beat: 0/1`, `Double-click insertion beat: 0/1`. The baseline source is retained under `/tmp/crest-viewport-timeline-review/baseline`. The root's new `node scripts/test_piano_interaction.cjs` passes after the fix, covering click and insertion across forward and backward retained sections.

### R08 — Legacy timeline schema bypass — corrected during review

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Major · **Current status:** corrected.

**Fresh verification:** The baseline accepts schema 1 with timeline surface while conditionally skipping validate_state and version-2 accounting. The independently inspected fixture contains exactly 50 expanded and 50 minimap samples, 0 focus objects, 0 revisions, and no attempts/no_changes fields. repro-legacy-timeline.py showed baseline acceptance and current rejection with Native feedback has no supported surface. Current native_feedback.py:36-39 rejects schema 1 unless surface is grid, so accepted timeline reports must pass schema-2 focus validation at lines 53-54 and accounting at lines 60-65. The earlier verification ran all 10 native-feedback tests successfully, including a valid schema-2 timeline, per-view rejection cases and six legacy non-grid surfaces. The original validation contract bypass is confirmed and corrected. The later R06 extraction was inspected statically; no new test run is claimed here.

**Discovery evidence, harm, and proposed correction:**

#### O2. Legacy schema bypasses all timeline focus validation

- smell: Validation contract bypass at the timeline measurement boundary (Module architecture)
- location: `scripts/native_feedback.py:35-54`, `64-70`, `112-129`.
- evidence:

  ```python
    if surface not in ("grid", "piano", "piano_selection", "piano_edit", "drum", "drum_edit", "timeline") or (version == 2 and "surface" not in report):
  ```

  ```python
        if version == 2:
            validate_state(state, surface)
  ```

  ```python
    if surface == "timeline":
        for view in ("expanded", "minimap"):
            measured = [sample["receipt_to_ack_ns"] for sample in samples
                        if sample["origin"] == "native_keyboard" and sample["state"]["playing"]
                        and sample["state"]["view"] == view]
  ```
- harm: A report can use schema 1 and the legacy grid scope while setting its surface to `timeline`. It then reaches the new per-view acceptance gate without carrying a revision, occurrence, source row, displayed beat, cells, or version-2 action accounting. The verifier reports a successful timeline witness without the exact focus evidence its new timeline validation requires. A fixture with 50 samples in each view and no focus object is accepted.
- fix: Separate schema parsing from timeline gate policy and make successful parsing establish a version-appropriate evidence contract. Keep schema-1 compatibility restricted to its grid surface; require schema 2 before admitting the newly supported timeline surface. Preserve all valid native timeline reports.
- confidence: 10/10.
- criterion: `clean-code-modules.md`, “Give validation an owner” and “Let downstream code consume the guarantee.”
- severity: Major before correction; the independent verifier could claim a timeline evidence gate passed while bypassing its defining identity validation. Resolved during review by the root agent, which restricted schema 1 to grid and added a six-surface regression; independently rerun Python suite passes 10/10. Verify before/after using /tmp/crest-viewport-timeline-review/repro-legacy-timeline.py.
- execution: `PYTHONPATH=scripts python3` importing `read_measurement` and reading `/tmp/crest-viewport-timeline-review/legacy-timeline-without-focus.json` returned the report successfully: `accepted schema 1 surface timeline with zero exact occurrence/row focus identities`.

### R09 — Split interval range and implicit boundary policy

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:138-175. The whole time interval remains intact, so the helper's four independent parameters no longer meet the catalog's Long Parameter List criterion. The remaining boundary policy is now named includeBoundaryPoints at :164 and used consistently in both equality comparisons at :170 and :174. The contract at :162-163 explicitly states the half-open range, why point controls include equality, and why note intervals ending there do not. Although the controls caller at :149 still supplies a boolean, the operation now names and documents its precise meaning; the catalog does not require a particular parameter-object spelling after the ambiguity is removed. Both merged components are corrected. Original CONFIRMED verdict, confidence, and Minor severity are preserved.

Original verification record (historical source state and line numbers, preserved as evidence):

The original Long Parameter List finding was correct, but that component is now corrected. I recounted five declared parameters, including the defaulted parameter, in baseline phrase-view.js:150. Both callers unpacked time.start and time.end from the same object, satisfying the catalog's 4+ parameters plus related-source-values threshold. Current webview-page/phrase-view.js:154 accepts four parameters, and callers at :134 and :142 preserve the complete time object. Those remaining arguments are independent; the catalog's long-parameter criterion no longer applies.

The independently merged boundary-policy finding remains at current phrase-view.js:142,154-164. The control caller still passes positional true; points changes equality handling both in the prefix search at :160 and final membership check at :164. There is no nearby contract explaining that controls at the left boundary are retained while durations ending there are excluded. A VM probe of the current helper returned zero entries with the default and one with points=true for the same interval ending at the query start. This is an intentional policy, not a reproduced production bug. The remaining Minor finding concerns naming that policy consistently at the callable boundary and in both comparisons. Introduce an explicitly named boundary-policy input or document and name the policy; do not restore the already corrected split range. Current status is outstanding only for this narrower clarity component.

**Discovery evidence, harm, and proposed correction:**

#### 2. Interval queries unpack one range into a five-parameter helper

- smell: Long Parameter List (Bloaters)
- location: `webview-page/phrase-view.js:150-161`; callers at `webview-page/phrase-view.js:127-140`.
- evidence: The helper has **5 parameters**, including the defaulted `points` argument:

  ```javascript
  function appendIntersections(visible, entries, startBeat, endBeat, points=false) {
  ```

  Both callers extract the two bounds from the same `time` object:

  ```javascript
        appendIntersections(visible,this.rows.get(pitch),time.start,time.end);
  ```

  ```javascript
      appendIntersections(visible,this.controls,time.start,time.end,true);
  ```

  The helper treats the ordered pair as a single valid interval:

  ```javascript
    if(!entries?.length||compareBeats(startBeat,endBeat)>=0)return;
  ```
- harm: Start and end are interchangeable rational-beat objects at the JavaScript boundary. A reversed call silently returns no intersections instead of exposing the argument mistake, which makes notes or controls disappear from the viewport. The two callers must separately remember the bounds order and the meaning of the final boolean.
- fix: Introduce Parameter Object for the interval query, preserving the existing `time` range and naming its point-versus-duration boundary policy. This reduces the helper to the output collection, indexed entries, and query object.
- confidence: 10/10.
- proposed severity: P3 (low).



Independent Function Clarity agreement:
5. Interval intersection hides its left-boundary policy behind a positional boolean.

- smell: Implicit boundary-policy input (Function clarity)
- location: webview-page/phrase-view.js:150-159; callers at webview-page/phrase-view.js:130,138
- evidence:
  ```js
  appendIntersections(visible,this.controls,time.start,time.end,true);
  function appendIntersections(visible, entries, startBeat, endBeat, points=false) {
    if(!entries?.length||compareBeats(startBeat,endBeat)>=0)return;
    let start=0,end=entries.length;
    while(start<end){
      const middle=Math.floor((start+end)/2),order=compareBeats(entries[middle].furthestEnd,startBeat);
      if(order<0||(!points&&order===0))start=middle+1;else end=middle;
    }
    for(let index=start;index<entries.length&&compareBeats(entries[index].start,endBeat)<0;index++){
      const order=compareBeats(entries[index].end,startBeat);
      if(order>0||(points&&order===0))visible.push(entries[index]);
    }
  }
  ```
- harm: The boolean does not select a different data shape; it changes two comparisons so controls at the left boundary are retained while notes released exactly there are excluded. That policy must agree in the prefix-index search and the final membership check. A maintainer changing only one equality case can lose zero-duration controls or reintroduce released notes, and a new caller cannot read the final true argument as that policy. There is no local contract comment explaining the distinction.
- fix: Introduce Parameter Object for the policy, such as { includeEndsAtStart: true }, with a default of false, and name the comparison predicates consistently in both phases. Document why point events include equality and sustained intervals do not. Preserve all comparison boundaries.
- confidence: 9
- criterion: Keep conditions at one level of meaning; Comments explain contracts and reasons; Make input relationships and effects explicit.
- severity: Minor — a subtle but intentional boundary rule is encoded twice without an explicit callable contract.

### R10 — DOM synchronization parameter list

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/host.js:1672,1677,1701-1717. The synchronizer now has exactly two declared parameters, target and entries. The cohesive target groups layer, node map, creation callback, and optional parent selection by name; both call sites construct the appropriate association. The helper destructures that target once and retains the prior default-parent behavior and explicit node-map mutation. This is a synchronization target rather than an application-context bag. The catalog's explicit parameter-count threshold no longer applies, and the target relationship is visible at both callers. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/host.js:1690 declares syncPianoElements(layer,nodes,entries,create,parentFor=()=>layer): five parameters, including the defaulted callback. I independently recounted them and inspected both callers at :1662 and :1667. Each extracts its layer and corresponding node map from the same view object, satisfying the catalog's explicit 4+ parameters plus common-source criterion. The create and optional parent callbacks also close over that view.

The helper removes nodes and deletes map entries at :1694, then creates or reparents retained nodes at :1698-1703. Pairing the wrong map, layer, or callbacks would therefore affect a different rendered surface. The default parent callback is useful, and the existing callers are correct, but neither fact is an exception to the catalog threshold. A cohesive synchronization-target parameter containing layer, node map, creation, and parent selection would express this association. Do not pass the entire application context merely to shorten the signature. The original severity maps from P3 to Minor.

**Discovery evidence, harm, and proposed correction:**

#### 3. DOM synchronization separately accepts related layer state and behavior

- smell: Long Parameter List (Bloaters)
- location: `webview-page/host.js:1689-1704`; callers at `webview-page/host.js:1661-1666`.
- evidence: The helper has **5 parameters**, including the defaulted `parentFor` callback:

  ```javascript
  function syncPianoElements(layer,nodes,entries,create,parentFor=()=>layer){
  ```

  Both callers extract the layer and corresponding node map from the same `view` object, then provide callbacks that close over that object:

  ```javascript
    syncPianoElements(view.noteLayer,view.noteNodes,view.index.notesIn(bounds,view.metrics),entry=>pianoNoteElement(entry,view),entry=>pianoNoteGroup(entry,view));
  ```

  ```javascript
    if(view.controlLayer)syncPianoElements(view.controlLayer,view.controlNodes,view.index.controlsIn(bounds,view.metrics),entry=>pianoControlElement(entry,view));
  ```

  The positional node-map argument is mutated and its nodes are removed from the DOM:

  ```javascript
      if(!visible.has(event)&&document.activeElement!==node){node.remove();nodes.delete(event);}
  ```
- harm: The layer, retained-node map, creation callback, and parent-selection callback form one synchronization target, but each call rebuilds that association manually. Copying a call for another rendered layer can pair the wrong node map or callback with a layer; the synchronizer then removes or reparents nodes owned by another surface. The five-argument calls provide no named boundary for that required association.
- fix: Introduce Parameter Object for the synchronization target containing its layer, node map, creation callback, and parent-selection callback. Pass that target with the visible entries, so each rendered surface defines its association once.
- confidence: 9/10.
- proposed severity: P3 (low).

### R11 — MPE provenance message chain

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/host.js:1647,1654-1656. updatePianoSelection now assigns the result of pianoNoteDetailText(selectedNote); it no longer navigates MPE provenance while updating DOM state. The extracted formatter owns the complete note-detail presentation, including the empty-selection message and source/member labels. It still reads the serialized provenance representation internally, which is appropriate at this explicit data-to-display conversion boundary: a representation change now belongs in the formatter instead of the selection updater. This implements the proposed presentation-accessor boundary rather than merely renaming an intermediate variable. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/host.js:1640 still reads selectedNote.note.mpe.profile.source and selectedNote.note.mpe.member while assembling detail text. I verified the native declarations at composition.rs:160-172 and :452-456, and mpe.rs:142-146 and :200-202. The source chain traverses three distinct object boundaries: Event to Note, Note to MpeProvenance, and MpeProvenance to MpeProfile, followed by the scalar source access. This meets the Message Chains catalog threshold. It is not a fluent chain returning the same type.

The optional MPE branch guards the provenance access, and validated native provenance always contains a profile. That disproves any claim of a current missing-profile failure; the candidate already limits itself to a structural dependency. The catalog says repeated sites are desirable evidence, not a prerequisite. Moving provenance's display source would still require changing the selection DOM updater's nested lookup. A note-detail presentation accessor or projection would keep that dependency at the conversion boundary. No current runtime failure is claimed. The original P3 severity maps to Minor.

**Discovery evidence, harm, and proposed correction:**

#### Finding C1

- smell: Message Chains (Couplers)
- location: `webview-page/host.js:1639`, in `updatePianoSelection`.
- evidence: The selected-note detail renderer reads `selectedNote.note.mpe.profile.source` and `selectedNote.note.mpe.member`. The first expression crosses three distinct object boundaries before accessing its scalar: `Event.note` is a `Note`, `Note.mpe` is `MpeProvenance`, and `MpeProvenance.profile` is `MpeProfile`. The corresponding native declarations are `pub note: Option<Note>` (`crates/crest-tracker/src/composition.rs:456`), `pub mpe: Option<crate::mpe::MpeProvenance>` (`composition.rs:172`), `profile: MpeProfile` (`crates/crest-tracker/src/mpe.rs:201`), and `source: String` (`mpe.rs:143`). These are serialized domain objects, not a fluent API or a collection transformation. The exact UI fragment is `selectedNote.note.mpe?" · source "+selectedNote.note.mpe.profile.source+" · member "+selectedNote.note.mpe.member:""`.
- harm: The DOM selection-update routine must know that source provenance is stored inside a profile inside MPE metadata inside a note. Moving the source label from the profile to provenance, or changing how a note records provenance, requires changing this selection-rendering routine even though its operation remains displaying a selected note's source and member. A missed update can throw while rendering note details because the intermediate `profile` access is unchecked.
- fix: Hide Delegate, using a note-detail presentation accessor or projection that exposes the source label and member directly. Extract Method + Move Method can place the provenance-to-display conversion beside the phrase-view or phrase-projection logic, leaving `updatePianoSelection` responsible for applying the resulting text.
- confidence: 8/10.
- severity: P3 (low). This is a structural dependency, not evidence of a current runtime failure.

### R12 — Event page/accessibility policy coupling

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The earlier correction remains intact in current webview-page/phrase-view.js:5-6,130,205-209. EVENT_PAGE_SIZE governs list pages; PIANO_NOTE_GROUP_SIZE governs stable accessibility groups. The subsequent naming and extraction changes do not recombine these policies. Current host.js:1693-1694 retains the WebKit grouping rationale. Prior independent policy-change evidence remains applicable; no new test run was needed for this status recheck. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

The original evidence was correct. Baseline phrase-view.js:5,119,177-180 uses EVENT_PAGE_SIZE both for stable note accessibility groups and for user-facing list page arithmetic. Host's accessibility explanation remains at current webview-page/host.js:1682-1683, while event chooser and raw editor pagination are distinct caller context. I checked whether the shared value represented a documented single policy; the source instead documents the WebKit crowded-parent constraint specifically at note grouping. Equal values do not establish one reason to change.

An in-memory experiment changing only EVENT_PAGE_SIZE from 64 to 128 changed baseline list-page and maximum note-group sizes to [128,128]. Current phrase-view.js:5-6 separates EVENT_PAGE_SIZE and PIANO_NOTE_GROUP_SIZE, uses the latter at :123, and retains list arithmetic at :182-186. The same isolated experiment on current source produced [128,64]. This verifies that list tuning no longer changes the accessibility cap. Existing dense-group and pagination tests pass. The original Minor architecture finding is confirmed and corrected.

**Discovery evidence, harm, and proposed correction:**

##### 2. List pagination and accessibility grouping share an unrelated policy constant

- smell: Accidental shared-policy coupling (Module architecture).
- location: `webview-page/phrase-view.js:5,119,177-184`; `webview-page/host.js:1679-1685`. The event chooser and raw-event pagination at `host.js:1215-1232,1291-1303,1764` are caller context.
- evidence:

  ```javascript
  const EVENT_PAGE_SIZE = 64;
  ```

  The interval index uses that page size to choose accessibility groups:

  ```javascript
          row.push({event,order,group:Math.floor(row.length/EVENT_PAGE_SIZE),start:event.at,end:add(event.at,event.note.duration)});
  ```

  The same constant defines user-facing list pagination:

  ```javascript
    const lastStart = Math.floor(Math.max(0, items.length - 1) / EVENT_PAGE_SIZE) * EVENT_PAGE_SIZE;
    const start = Math.max(0, Math.min(lastStart, Math.floor(requestedStart / EVENT_PAGE_SIZE) * EVENT_PAGE_SIZE));
    const end = Math.min(items.length, start + EVENT_PAGE_SIZE);
  ```

  The host explains the distinct reason for the index's grouping:

  ```javascript
    let page=group.pages.get(entry.group);
    if(!page){
      // WebKit's native accessibility truncates crowded parents even when
      // their children paint. Bound each group, never the source or viewport.
      page=document.createElement("div");page.setAttribute("role","group");
      page.setAttribute("aria-label",`Note group ${entry.group+1}`);
      group.pages.set(entry.group,page);group.node.append(page);
  ```
- harm: Choosing a larger event-list page is a pagination usability decision, while limiting the number of children under a Piano accessibility parent is a browser constraint. Changing `EVENT_PAGE_SIZE` to tune the list also changes the accessibility limit without touching the renderer that documents that limit. Conversely, adjusting the browser workaround changes every event-list page and page-number calculation. The independent reasons to change are currently represented as one policy.
- fix: Separate Policies by introducing a separately named accessibility-group limit, initially also 64, while retaining `EVENT_PAGE_SIZE` for list pagination. Keep the existing stable group assignment and DOM structure. This requires separating two constants, not moving the index into a new framework.
- confidence: 9.
- criterion: Clean Code at the Module Level, “Organize around responsibilities that change together” and “Encapsulate decisions and representations.”
- severity: Minor. Existing values work, but unrelated presentation changes can alter a documented accessibility safeguard.

### R13 — Mutable interval-cache representation leak

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The earlier correction remains intact in current webview-page/phrase-view.js:120-151. #rows and #controls remain private. Both public query projections explicitly construct render records at :143 and :150 and do not return start, end, or furthestEnd. The renamed sorting helper remains behind that private boundary. Source-event identity is intentionally retained, and the latest host extraction does not add an endpoint/cache consumer. Prior mutation-probe and regression evidence remains applicable. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

The original cache leak was independently reproduced early with the supplied repro-index-leak.cjs: it printed “Query results before mutation: 1 after mutation: 0 source duration unchanged: 20”. I also repeated it against the retained baseline. Mutating only the returned furthestEnd.numerator invalidated a later sustained-tail search; the musical source duration remained unchanged. Baseline rows/controls were public, and query result spreads retained references to internal start/end/prefix-maximum beats. Actual host consumers require source identity, ordering/group identity, and geometry, not those cache fields.

Current webview-page/phrase-view.js:114-115 makes the caches private. Query projections at :136 and :143 explicitly expose render fields and omit start, end, and furthestEnd. A fresh VM check found no public own properties on the index, no internal endpoint fields on render records, preserved source-event identity, and unchanged query membership after modifying returned geometry and adding an unrelated furthestEnd property. The new regression in scripts/test_phrase_view.cjs passes. Source-event identity remains intentionally shared under the existing immutable-source contract; changing that source would be a different operation. The original Minor representation-leak finding is confirmed and corrected.

**Discovery evidence, harm, and proposed correction:**

##### 3. Interval queries expose writable search-cache internals

- smell: Leaking mutable implementation representation (Module architecture).
- location: `webview-page/phrase-view.js:110-140,144-159`; renderer callers at `webview-page/host.js:1661,1671-1685,1705-1715`.
- evidence:

  The index stores sorted interval arrays in writable public fields:

  ```javascript
      this.rows = new Map();
  ```

  ```javascript
      for(const [pitch,entries] of this.rows)this.rows.set(pitch,indexIntervals(entries));
      this.controls=indexIntervals(controls);
  ```

  Prefix maxima are internal search metadata containing object references:

  ```javascript
    return entries.map(entry=>({...entry,furthestEnd:end=maximum(end,entry.end)}));
  ```

  Both query operations spread the entire cache entry into their public result:

  ```javascript
      return visible.map(entry=>({...entry,...window.noteGeometry(entry.event)}))
        .filter(entry=>entry.right>bounds.left&&entry.left<bounds.right).sort((a,b)=>a.order-b.order);
  ```

  ```javascript
      return visible.map(entry=>({...entry,left:window.x(entry.event.at),right:window.x(entry.event.at)+24}))
        .filter(entry=>entry.right>bounds.left&&entry.left<bounds.right).sort((a,b)=>a.order-b.order);
  ```

  Subsequent searches trust that exposed metadata:

  ```javascript
      const middle=Math.floor((start+end)/2),order=compareBeats(entries[middle].furthestEnd,startBeat);
  ```

  Actual renderer consumers require the source event, stable grouping, and geometry; they do not read `start`, `end`, or `furthestEnd`.
- harm: The exact index owns the invariant that each pitch row remains sorted and every `furthestEnd` is the correct prefix maximum, but its query contract exposes those internal mutable beat objects. The result is only shallowly copied. A read-only Node experiment on current source returned one sustained note for a tail viewport, changed only `notesIn(...)[0].furthestEnd.numerator`, and then returned zero notes for the same viewport while the source note duration remained 20. No production caller currently makes that mutation; the demonstrated issue is that a supposedly detached render result can invalidate the index and that replacing its search representation changes unnecessary public result fields.
- fix: Encapsulate Field and Narrow Public Result. Make the row map and control array private, and return explicit render records containing only the event reference, required ordering/group identity, and geometry. Keep cached interval bounds and prefix maxima private; preserve source-event object identity because existing draft/committed selection relies on it. Do not clone every musical event or remove the existing immutable-source contract.
- confidence: 9.
- criterion: Clean Code at the Module Level, “Guarantees survive use” guidance under “Give validation an owner,” “Encapsulate decisions and representations,” and “Use a small, intentional public surface.”
- severity: Minor. Present callers are read-only, but ordinary query results expose a direct path to corrupt the retained index without changing the musical source.

### R14 — Duplicated hit-target geometry policy

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** The earlier correction remains intact in current webview-page/phrase-view.js:7-8,81,139,148-150. Note geometry and query expansion share PIANO_NOTE_MIN_WIDTH; control expansion and returned width share PIANO_CONTROL_WIDTH. Current host.js:1733 still paints entry.width. The later viewport extraction and label-gutter constant do not restore independent hit-target dimensions. Prior isolated width-change evidence remains applicable. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Baseline phrase-view.js:72,128 duplicates the 4-pixel minimum note width, while :137,139 and baseline host.js:1720 independently encode the 24-pixel control width. I checked the actual path: culling expands the time range using these widths, geometry filtering computes the right edge, and the host paints the hit target. Those stages must agree. This is one shared geometry policy across calculation and presentation, not incidental duplicate numbers.

Current webview-page/phrase-view.js:7-8 gives note and control widths authoritative constants. Note geometry at :75 and culling at :132 share PIANO_NOTE_MIN_WIDTH. Control culling and render geometry at :141-143 share PIANO_CONTROL_WIDTH. Current host.js:1721 paints entry.width. An in-memory current-source change from note width 4 to 8 and control width 24 to 48 retained both newly extended tails in narrow viewports and returned the revised widths. Existing viewport tests pass. The original Minor finding is confirmed and corrected; there is no remaining independent host width literal at the cited paint boundary.

**Discovery evidence, harm, and proposed correction:**

##### 4. Culling and painting duplicate horizontal target-size policy

- smell: Shared geometry policy without one owner (Module architecture).
- location: `webview-page/phrase-view.js:69-73,127-140`; `webview-page/host.js:1718-1721`.
- evidence:

  The window defines the minimum note width:

  ```javascript
      const width=Math.max(4,this.x(end)-left);
  ```

  The index separately knows that width when widening its time search:

  ```javascript
      const visible=[],time=window.visibleTime(bounds,4);
  ```

  Control culling knows a separate fixed target width in two expressions:

  ```javascript
      const visible=[],time=window.visibleTime(bounds,24);
      appendIntersections(visible,this.controls,time.start,time.end,true);
      return visible.map(entry=>({...entry,left:window.x(entry.event.at),right:window.x(entry.event.at)+24}))
        .filter(entry=>entry.right>bounds.left&&entry.left<bounds.right).sort((a,b)=>a.order-b.order);
  ```

  The renderer independently paints that width:

  ```javascript
    item.className="expression-point";item.style.left=entry.left+"px";item.style.width="24px";
  ```
- harm: Visibility must be calculated from the same painted bounds used by the DOM. Enlarging a control's click target in `pianoControlElement` without finding the two numeric assumptions inside the index can make an item whose painted tail should remain visible disappear when its attack is left of the viewport. Increasing minimum note width has the same coordination requirement between `noteGeometry` and `notesIn`. These are shared geometry invariants spread across the calculation and rendering boundary, not merely repeated formatting numbers.
- fix: Consolidate Geometry Policy. Give minimum note width and control width named authoritative values in the view-calculation module. Return control width with the existing geometry record and paint `entry.width` in the host. Reuse the same values for time-search expansion and rectangle construction, retaining the present 4 px and 24 px behavior.
- confidence: 9.
- criterion: Clean Code at the Module Level, “Encapsulate decisions and representations”: shared policy should be authoritative, and a policy change should not require coordinated edits across consumers.
- severity: Minor. Current constants agree, but changing hit-target dimensions requires hidden cross-boundary edits to preserve visibility.

### R15 — Adjacent section anchor return contract

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:71-77 and host.js:1495. The method is now adjacentSectionAnchor. Its named targetSection and constructorAnchor intermediates show the extra stride explicitly, and the comments distinguish the returned constructor anchor from the neighboring section's start and assign final phrase-end clamping to the constructor. The only host caller uses the renamed method. The nonnegative-anchor calculation preserves the former arithmetic. The return contract that the finding required is now explicit. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/phrase-view.js:69-71 still names the operation adjacent and returns a Beat computed with section + direction + 1. The constructor at :56-60 subtracts a stride-equivalent section from the supplied anchor, while the sole host caller at webview-page/host.js:1495 correctly assigns the result to pianoAnchor. The class documentation at phrase-view.js:49-52 explains overlap and bounded geometry but does not explain this returned constructor anchor.

A VM probe matched the existing final-section test: adjacent(-1) returned beat 64, but constructing the requested previous window with it produced section start beat 32. Thus the result is neither the neighboring window nor its start. Current navigation correctness does not remove this name/result mismatch for a caller using the value as a page label or direct section start. Rename to adjacentSectionAnchor, name the target section and constructor anchor, and explain final constructor clamping. This remains a Minor contract-clarity finding, not a navigation defect.

**Discovery evidence, harm, and proposed correction:**

1. The adjacent-section operation returns a constructor anchor without naming that contract.

- smell: Ambiguous operation and return meaning (Function clarity)
- location: webview-page/phrase-view.js:66-68; webview-page/host.js:1494; constructor context at webview-page/phrase-view.js:53-57
- evidence:
  ```js
  adjacent(direction) {
    return { numerator: maximum(zero,{numerator:(this.section+BigInt(direction)+1n)*PIANO_SECTION_STRIDE,denominator:1n}).numerator, denominator: 1n };
  }
  const page=direction=>{pianoAnchor=metrics.adjacent(direction);pianoFollow=false;renderActions();renderPiano({scrollLeft:0,follow:false});};
  ```
- harm: The returned Beat is an anchor that compensates for the constructor's subtraction of one section stride. It is neither the adjacent section nor its start. For example, the existing final-section test expects adjacent(-1) to return beat 64 when the requested previous section starts at beat 32. A maintainer implementing a page preview or a direct jump from this result can label or navigate to the wrong section unless they reconstruct the constructor arithmetic. The existing class comment explains overlap but not this return contract.
- fix: Rename Function to adjacentSectionAnchor and introduce named intermediate values for the target section and its constructor anchor. Document that the result is intended for the constructor's anchor input and that the constructor performs final phrase-end clamping. Preserve the arithmetic and current callers.
- confidence: 10
- criterion: Names must describe the operation; A return value must fulfill the name's promise; Make returned structures readable where they are built.
- severity: Minor — current callers use the protocol correctly, but the public method conceals the meaning needed by its next caller.

### R16 — Signed approximate beat-offset naming

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:15-16,64-69,104. The helper is now beatOffsetAsNumber(at, origin), and its adjacent comment states signed quarter-note units, exact subtraction, and approximation only for drawing. All current consumers use that name. This exposes result representation, argument direction, and the intended numeric boundary without changing arithmetic or introducing another abstraction. The existing broader class rationale remains useful but is no longer carrying the helper's contract alone. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/phrase-view.js:14 still declares distance(a,b) as exact rational subtraction followed by Number conversion and division. Its consumers at :62-67 and :97 use its result for presentation geometry and tick density. A VM probe returned -1 for distance(1,2), 0.3333333333333333 for distance(1/3,0), and typeof number. This verifies signed offset and approximate presentation semantics rather than an exact, nonnegative distance.

The class comment at :49-50 already explains subtracting absolute beats before pixel conversion, so a claim that the presentation rationale is wholly undocumented would be wrong. It does not give the private helper's name or arguments a direction, unit, or approximate-result promise. Swapping two beat objects changes clipping sign, and reusing the result for exact membership would lose the existing numeric boundary. A name such as beatOffsetAsNumber with at/origin arguments would expose that contract without adding a parameter object or arithmetic change. The supported harm is local maintenance clarity, so Minor remains appropriate.

**Discovery evidence, harm, and proposed correction:**

2. A distance helper conceals signed, approximate beat-offset semantics.

- smell: Unnamed units and directional input relationship (Function clarity)
- location: webview-page/phrase-view.js:11,59-65,94
- evidence:
  ```js
  const distance = (a, b) => Number(BigInt(a.numerator)*BigInt(b.denominator)-BigInt(b.numerator)*BigInt(a.denominator))/Number(BigInt(a.denominator)*BigInt(b.denominator));
  x(at) { return 64+distance(at,this.start)*this.px; }
  ```
- harm: Despite the name distance, the helper returns the signed offset a minus b as an approximate Number measured in quarter-note beats. Its sign is necessary for locating events before the current section, and its approximation is safe only at the presentation boundary. Neither argument name nor operation name identifies those facts. Swapping the same-shaped arguments or treating this as a nonnegative exact musical distance changes clipping or loses exact membership decisions.
- fix: Rename Function to beatOffsetAsNumber, rename the arguments to at and origin, and document that exact subtraction precedes the approximate conversion used for drawing. A named input pair can be used if this operation gains more consumers; no domain arithmetic needs to change.
- confidence: 9
- criterion: Names must describe the operation; Give patterns and constants semantic names; Make input relationships and effects explicit.
- severity: Minor — a local naming contract obscures signedness and the intentional exact-to-presentation boundary.

### R17 — Pixel conversion guarantees

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:84-98 and host.js:1629-1634. presentationBeatAtPixel is explicitly documented as unsnapped presentation time. editGridBeatAtPixel has its own contract stating flooring, phrase-end clamping, representability rejection through scaleBeat, and unchanged source timing. visibleTime uses the presentation operation; pianoPointerAction uses the edit operation and preserves its catch boundary. Searches found no stale pixelBeat or atPixel invocation in these sources or the focused tests. The name/comment distinction now matches the actual guarantees. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/phrase-view.js:78-91 still exposes pixelBeat(pixel) and atPixel(pixel,grid). I traced both call paths: visibleTime uses pixelBeat at :85, and host's guarded editing path calls atPixel at webview-page/host.js:1624 and catches failure at :1627. The comment about choosing an edit-grid cell remains immediately above pixelBeat, although that method only converts a local pixel ratio and adds the section origin.

A fresh probe with pixel 239 produced presentation beat 1.75 and edit-grid beat 1. The current huge-time regression also rejects an unrepresentable derived edit with “Integer exceeds”, and tracing scaleBeat through time-values.js:34-35 and reducedBeat at :6-12 confirms the wire-validation path. Current callers choose correctly, so this is not an existing insertion defect. The names and misplaced comment do not distinguish unsnapped presentation conversion from floor/clamp/representability guarantees. Rename the two operations to identify those contracts and place the edit-grid documentation on the latter. Minor is retained.

**Discovery evidence, harm, and proposed correction:**

3. The two pixel-to-time operations do not name their different contracts.

- smell: Ambiguous conversion operation (Function clarity)
- location: webview-page/phrase-view.js:75-88; webview-page/host.js:1621-1626
- evidence:
  ```js
  pixelBeat(pixel) {
    const offset={numerator:BigInt(Math.round(Math.max(0,pixel-64)*1000000)),denominator:BigInt(Math.round(this.px*1000000))};
    return add(this.start,offset);
  }
  atPixel(pixel, grid) {
    const at=this.pixelBeat(pixel);
    const step=BigInt(at.numerator)*BigInt(grid.denominator)/(BigInt(at.denominator)*BigInt(grid.numerator));
    const last=(BigInt(this.total.numerator)*BigInt(grid.denominator)-1n)/(BigInt(this.total.denominator)*BigInt(grid.numerator));
    return scaleBeat(grid,step<last?step:last);
  }
  ```
- harm: pixelBeat derives an unsnapped presentation beat, whereas atPixel floors to the edit grid, clamps to the last editable cell, and can reject a derived time that cannot fit the wire's u64 representation. The nearby comment mentions choosing an edit-grid cell but sits above pixelBeat, which does not perform that operation. A new hit-testing or insertion caller must inspect both bodies to choose the correct conversion, and can accidentally use an unsnapped or beyond-end time. The existing huge-time test explicitly proves the rejection behavior, so this is not a speculative failure mode of the conversion.
- fix: Rename Function to presentationBeatAtPixel and editGridBeatAtPixel respectively. Move the edit-grid contract comment onto the latter, documenting floor/clamp behavior and representability failure. Preserve the existing calculations and catch behavior in pianoPointerAction.
- confidence: 8
- criterion: Names must describe the operation; Comments explain contracts and reasons.
- severity: Minor — both current consumers are correct, but the two public operations expose different edit guarantees under near-interchangeable names.

### R18 — Index builder hidden input reordering

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:134-135,155-160. Both constructor sites call sortAndIndexIntervals. The helper's comment explicitly says it sorts the supplied private temporary array in place and returns enriched entries. Its name identifies sorting, while the documented combined contract distinguishes mutation from the returned product. The underlying in-place sort and mapped enriched return are retained. A suffix exactly matching the originally proposed spelling is not required once the effect is clear. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/phrase-view.js:148-152 still sorts entries in place, then returns a distinct mapped array containing prefix maxima. I instrumented the helper only in a VM: input orders [1,0] became [0,1]; the returned array was different from the input; furthestEnd appeared on returned entries but not the original entry objects. This verifies the combined input-reordering and returned-transformation contract.

I checked both callers at :127-128. They supply constructor-owned temporary arrays and immediately replace the stored representation, so no current retained-source-order bug follows. That is already the candidate's limitation, not an exception to the function-clarity rule about predictable effects. There is no helper comment identifying consumption or mutation, and indexIntervals does not tell a future caller that its input array is reordered. An explicit sort-and-index-in-place name plus a short returned-array contract would preserve the current allocation behavior. The finding remains Minor and concerns an undocumented effect, not mutation by itself.

**Discovery evidence, harm, and proposed correction:**

4. Building an interval index silently reorders the input array.

- smell: Hidden mutable-input effect in a returned transformation (Function clarity)
- location: webview-page/phrase-view.js:144-148; callers at webview-page/phrase-view.js:123-124
- evidence:
  ```js
  function indexIntervals(entries) {
    entries.sort((a,b)=>compareBeats(a.start,b.start)||a.order-b.order);
    let end=zero;
    return entries.map(entry=>({...entry,furthestEnd:end=maximum(end,entry.end)}));
  }
  ```
- harm: The function both sorts entries in place and returns a different enriched array. A caller retaining entries for source-order enumeration receives a reordered input even though the returned array is the apparent product of indexing. The current constructor passes private temporary arrays, so it is safe, but neither the function name nor any local documentation tells a future caller that the input is consumed by an in-place sort.
- fix: Rename Function to sortAndIndexIntervalsInPlace and document that it reorders the supplied temporary array while returning enriched entries. This preserves the current allocation and mutation behavior. If a pure contract is preferred later, sort a copy under a separately named change.
- confidence: 9
- criterion: Make input relationships and effects explicit; Comments explain contracts and reasons.
- severity: Minor — a private helper has an unannounced effect that is currently masked by caller ownership.

### R19 — Piano group mutation/result naming

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/host.js:1672,1682-1700. The synchronizer's parent callback now calls ensurePianoNotePage. Its name identifies an ensure operation and the returned page, while the adjacent contract says it creates missing pitch/page parents and returns the bounded page for the note. The existing implementation still creates both levels when absent and returns the inner page. The accessibility rationale remains separate and intact. The command/result ambiguity is removed. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/host.js:1672-1689 still defines pianoNoteGroup(entry,view). It looks up a pitch group, creates and attaches one if absent, then looks up or creates an inner page and returns that page. Its caller at :1662 uses it as parentFor during synchronization, which is correct. The comment at :1682-1683 explains the accessibility need for bounded pages but not the mutation and return-level contract.

I executed the extracted current function against a minimal DOM stub. A single empty-state call created two nodes, attached a pitch group under noteLayer, populated both maps, and returned the page stored in group.pages rather than group.node. Thus the noun name hides a real ensure/create command and does not identify which grouping level is returned. Rename to ensurePianoNotePage and document that it ensures both levels and returns the page parent. No current caller failure is alleged. The observation is concrete and remains Minor.

**Discovery evidence, harm, and proposed correction:**

6. A noun-named group lookup creates and attaches DOM nodes.

- smell: Query-looking name conceals DOM mutation (Function clarity)
- location: webview-page/host.js:1671-1687; caller at webview-page/host.js:1661
- evidence:
  ```js
  function pianoNoteGroup(entry,view){
    const pitch=entry.event.note.pitch;
    let group=view.noteGroups.get(pitch);
    if(!group){
      const node=document.createElement("div");node.setAttribute("role","group");
      node.setAttribute("aria-label",`${model.pitchLabels?.[pitch]||noteName(pitch)} notes`);
      group={node,pages:new Map()};view.noteGroups.set(pitch,group);view.noteLayer.append(node);
    }
    let page=group.pages.get(entry.group);
    if(!page){
      // WebKit's native accessibility truncates crowded parents even when
      // their children paint. Bound each group, never the source or viewport.
      page=document.createElement("div");page.setAttribute("role","group");
      page.setAttribute("aria-label",`Note group ${entry.group+1}`);
      group.pages.set(entry.group,page);group.node.append(page);
    }
    return page;
  }
  ```
- harm: pianoNoteGroup sounds like a lookup, but it can create two levels of nodes, mutate two maps, attach them to the live roll, and return the inner page rather than the pitch group. A new caller asking for a group to inspect or test can change the DOM, and a consumer treating the returned node as the pitch group can attach notes outside the intended bounded page. The accessibility comment explains page size but not this command/result contract.
- fix: Rename Function to ensurePianoNotePage and document that it creates the pitch group and page when absent and returns the page parent. Update the parentFor callback to use the new name without changing behavior.
- confidence: 10
- criterion: Names must describe the operation; A return value must fulfill the name's promise; Make input relationships and effects explicit.
- severity: Minor — an active DOM operation is hidden behind a noun and returns a different level of grouping than the name suggests.

### R20 — Dense MIDI expression selection

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:178-189 and host.js:1536-1539. expressionLaneValue(control, kind) now owns an explicit switch for bend, pressure/poly_pressure, timbre, and sustain. TIMBRE_CC and SUSTAIN_CC name the MIDI mappings, and unsupported lanes return null directly instead of relying on an impossible -1 CC sentinel. The contract documents null as no sample and preserves zero as a real value. Host projection and raw-point counting still share the same adapter. This separates the domain decision from the rendering expression while preserving the supported representations. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/host.js:1536 still defines amount as one ternary chain covering bend, two pressure representations, CC 74 for timbre, CC 64 for sustain, and -1 as the no-supported-CC sentinel. Consumers at :1538-1539 use it both to project points and count raw points. The velocity branch returns immediately at :1535, so it would be wrong to claim this code currently misroutes the velocity lane.

I evaluated the extracted callback: pressure and poly_pressure both return their values, timbre accepts CC 74 and rejects CC 64, and sustain accepts CC 64. These correct results do not make the operation's filter/null contract or the two MIDI-number policies explicit. The supported concern is not ternary formatting alone: the name amount obscures selection and absence, and the sentinel and supported representation mapping must be reconstructed together when adding a lane. Extract expressionValueForLane with explicit cases and named control numbers. Keep projection and counting on the same selector. Minor remains appropriate.

**Discovery evidence, harm, and proposed correction:**

7. Expression-value selection compresses distinct MIDI representations into one ternary chain.

- smell: Dense domain decision and unnamed control-number policy (Function clarity)
- location: webview-page/host.js:1535; consumers at webview-page/host.js:1537-1538
- evidence:
  ```js
  const amount=control=>pianoExpressionKind==="bend"&&control.kind==="bend"?control.value:pianoExpressionKind==="pressure"&&["pressure","poly_pressure"].includes(control.kind)?control.value:control.kind==="cc"&&control.number===(pianoExpressionKind==="timbre"?74:pianoExpressionKind==="sustain"?64:-1)?control.value:null;
  ```
- harm: This expression encodes a direct bend representation, two pressure representations, two CC mappings, and an impossible -1 CC sentinel for every other lane. A maintainer adding another expression lane must understand the entire ternary and the sentinel to avoid accepting another lane's controls or dropping poly-pressure. The name amount does not identify that it filters the selected expression kind and returns null on a mismatch.
- fix: Extract Function named expressionValueForLane with explicit cases for bend, pressure, timbre and sustain, returning null for mismatches. Name the timbre and sustain CC numbers. Keep the same callback use for projected points and raw-point counting.
- confidence: 10
- criterion: Keep conditions at one level of meaning; Names must describe the operation; Give patterns and constants semantic names.
- severity: Minor — multiple supported MIDI representations and a no-match sentinel are compressed into a single decision expression.

### R21 — Unnamed label-gutter coordinate constant

**Skeptic:** CONFIRMED · **Confidence:** 10/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified in webview-page/phrase-view.js:9,64,69,86,198-199,212 and host.js:6,1485-1488,1524,1583,1596-1597,1603,1620-1622. PIANO_LABEL_WIDTH_PX is one exported pixel-origin constant, and the host imports and uses it for zoom anchoring, widths, bounds, and the extracted viewport construction. Forward conversion, inverse conversion, and expression clipping use the same value. A current source search leaves 64-valued page/group policies and named MIDI sustain policy separate; the host's expression SVG height of 64 is a vertical dimension, not a missed gutter origin. The originally cited coordinate invariant is now named and shared. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/phrase-view.js:62,67,81,175-176 still uses literal 64 for the pitch-label pixel gutter in scale calculation, forward coordinates, inverse coordinates, and expression clipping. Current webview-page/host.js:1485-1488 and :1575 independently use that origin for cursor visibility, zoom anchoring, bounds, and roll width. The same module now has separately named 64-valued event-page and note-group sizes at :5-6 and a 64-beat section size at :10; none of those constants names the pixel origin.

I traced forward x() and inverse pixelBeat() and the host zoom expression. These occurrences encode the same coordinate invariant, while the other 64 values have different units. Changing the label gutter in only one calculation would misalign painted and edited positions. Existing geometry tests establish the current value, not ownership of future layout changes. Introduce one PIANO_LABEL_WIDTH_PX value in the existing view/host boundary and reuse it only for the coordinate origin. This is a supported Minor shared-unit and naming concern.

**Discovery evidence, harm, and proposed correction:**

8. The Piano label gutter is an unnamed coordinate-system constant.

- smell: Unnamed unit-bearing geometry constant (Function clarity)
- location: webview-page/phrase-view.js:59,64,78,170-171; webview-page/host.js:1484-1487,1574
- evidence:
  ```js
  this.px = Math.max(320,viewportWidth-64)/Math.min(16,distance(total,zero))*zoom;
  x(at) { return 64+distance(at,this.start)*this.px; }
  const offset={numerator:BigInt(Math.round(Math.max(0,pixel-64)*1000000)),denominator:BigInt(Math.round(this.px*1000000))};
  if(x<64){points.length=0;points.push({x:64,value});}
  else if(x<=window.width+64)points.push({x,value});
  const cursorVisible=old.contains(model.pianoControls.at)&&cursorX>=64&&cursorX<=viewport;
  const screenX=cursorVisible?cursorX:viewport/2,anchorPixels=container.scrollLeft+screenX-64;
  ```
- harm: These 64 values are pixels reserved for pitch labels, while the same module separately has a 64-beat section size and a 64-event page size. The repeated unlabelled literal defines the origin used by drawing, pointer inversion, expression clipping and zoom anchoring. Changing the label gutter at only one occurrence misaligns visual notes and edited beats; changing unrelated 64 values can instead alter musical-window or accessibility behavior. The names and comments do not identify this shared unit-bearing invariant.
- fix: Replace Magic Number with PIANO_LABEL_WIDTH_PX and use it in the corresponding coordinate calculations in the existing view/host interface. Leave PIANO_SECTION_BEATS and EVENT_PAGE_SIZE separate, as they already represent different units and policies.
- confidence: 10
- criterion: Give patterns and constants semantic names; Make input relationships and effects explicit.
- severity: Minor — a cross-function coordinate invariant has no semantic name, making routine layout changes difficult to preserve.

### R22 — Piano render mixed abstraction

**Skeptic:** CONFIRMED · **Confidence:** 9/10 · **Severity:** Minor · **Current status:** corrected.

**Fresh verification:** Current correction verified by reading the complete current renderPiano at webview-page/host.js:1547-1579 and createPianoViewport at :1580-1607. The coordinator retains section selection, cache reuse, pitch-window extension, mounting, scroll restoration, capture/follow updates, and final cursor/viewport updates. The builder receives phrase, source, and a cohesive layout description; it constructs detached content, the complete viewport reference record, and returns {content,view}. It does not assign pianoViewport or mount content. The coordinator installs view before mounting and preserves the subsequent update sequence. I checked the builder's direct dependencies: updatePianoTicks writes the supplied detached roll; renderPianoEventList retains previous-list state; listeners reference the established pointer operation rather than mutating coordinator locals. Existing global presentation data is still read where needed, but no partially installed viewport is required to complete construction. The original inline DOM-construction subproblem is now separated from viewport policy. Corrected.

Original verification record (historical source state and line numbers, preserved as evidence):

Current webview-page/host.js:1547-1600 still contains both viewport orchestration and detailed DOM construction. I read the whole 54-physical-line function, counting declaration through closing brace inclusively. It is below the Long Method limit; this candidate is instead the function-flow catalog's mixed-abstraction criterion. Section/cursor/playback ownership is decided at :1550-1554, retained-content reuse and update order at :1560-1566, and pitch-window extension at :1571-1574. Between those decisions and final mounting/follow/cursor updates at :1597-1599, the function creates and styles pitch rows, handlers, cursor elements, playhead, expression layers, and content at :1575-1596.

Existing helpers such as updatePianoTicks and renderPianoEventList already hide some detail, but they do not remove the remaining complete viewport-construction subproblem. Reviewing cache/follow sequencing still requires crossing that DOM detail, and changing node construction shares the coordinator's locals and ordering. Extract a complete createPianoViewport result containing detached content and required references; keep cache selection, mounting, scroll restoration, and updates in the coordinator. The extraction must preserve listener closure behavior, event-list retention, accessibility order, and update sequence without requiring a partially installed global viewport. This is a Minor maintenance finding, not a length or runtime failure claim.

**Discovery evidence, harm, and proposed correction:**

- smell: The Piano render coordinator mixes viewport policy with detailed DOM construction (Function flow)
- location: webview-page/host.js:1546–1598, renderPiano.
- evidence: The function first chooses the section with `if(cursorKey!==lastPianoCursor&&!metrics.contains(model.pianoControls.at))pianoAnchor=model.pianoControls.at;` and `else if(options.follow!==false&&pianoFollow&&playAt&&!metrics.contains(playAt))pianoAnchor=playAt;`. It then selects a retained-content branch with `if(sameContent&&previousRoll&&pitch>=Number(previousRoll.dataset.low)&&pitch<=Number(previousRoll.dataset.high)){`. The rebuild branch directly creates pitch rows and lays them out using `row.style.top=(36+(high-pitch)*22)+"px"`, binds pointer actions, constructs cursor elements, and sets `cursorPoint.style.cssText="position:absolute;left:0;width:7px;height:20px;border:2px solid var(--color-accent-warning);box-sizing:border-box";`. It finally assembles the viewport record with `pianoViewport={roll,noteLayer,controlLayer,rawControlLayer:controlLayer,channelExpression:expression,expressionSurface:expression,noteDetail,detail,eventList,noteCount:notes.length,selectionKey:null,metrics,index,storedEvents,selected,phrase:p,high,total,noteNodes:new Map(),noteGroups:new Map(),controlNodes:new Map()};` and resumes orchestration with `updatePianoSelection`, `followPianoPlayback`, `updatePianoCursor`, and `updatePianoViewport`.
- harm: Cache reuse, cursor-versus-playback ownership, pitch-window growth, and final scroll/reveal ordering are coordinator decisions. Inline node styling, accessibility decoration, and event binding interrupt those decisions with a second abstraction level. Reviewing a change to the viewport policy requires carrying the DOM-construction details through the same function, and changing how the roll is built requires understanding unrelated cache and follow sequencing. The finding is about the mixed responsibilities, not the function's line count.
- fix: Extract Function `createPianoViewport` as a complete construction operation that receives the phrase/source content, metrics, pitch bounds, selection, playback beat, and prior event list as meaningful inputs and returns the complete detached content plus its viewport references. Keep section selection, cache reuse, mounting, scroll restoration, and follow/cursor/viewport updates in `renderPiano`. The builder must not mutate the coordinator's locals or depend on partially installed `pianoViewport`; preserve node order, retained event-list behavior, accessibility attributes, listener closures, and the existing update order.
- confidence: 8
- criterion: Clean Code Across Function Boundaries — “Compose functions at one level of abstraction” and “Extract a complete subproblem.”
- severity: Minor — the present rendering can work correctly, but the mixed abstraction increases the reasoning burden for both viewport-policy and DOM changes.

## Unresolved

None. Every merged candidate received a verdict, and no required hunter coverage remains incomplete.

## Refuted (appendix)

None. The independent checks did not establish an evidence-, scope-, or catalog-based disproof of an original candidate. Severity was calibrated separately from confidence and correction status.

## Summary

The review completed all nine hunter roles and independent verification of all 22 merged items. 22 supported items were corrected during the review, including wrong-time Piano insertion and acceptance of timeline measurements without exact focus evidence. No supported finding remains outstanding in the reviewed scope after the targeted correction checks. There are 0 unresolved and 0 refuted items. Current statuses reflect targeted rechecks after the final owner corrections. The incompatible plain-pin Rust test attempt is disclosed separately and is not evidence of a passing build.
