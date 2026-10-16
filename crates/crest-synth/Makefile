.DEFAULT_GOAL := help
CARGO_OUTPUT_DIR = $(shell scripts/cargo_target_dir.sh)

.PHONY: help cache-guard cache-status cache-prune-preview cache-prune build check test test-session-lifecycle test-engine-post-fx-options test-midi-devices test-midi-host midi-device-handoff test-webview-detail-native test-webview-sample-native test-webview-options-native test-webview-empty-patch-native lint fmt fmt-check run play ui smoke observe demo demo-live demo-live-detail-and-assets demo-live-mixer demo-live-effects-and-buses demo-live-patch-editor demo-live-sixteen-track-mixer-routing demo-live-semantic-view-model demo-live-graphical-shell demo-live-component-library semantic-graphical-view-model-acceptance webview-tokens clean

help: ## Show the available project commands
	@grep -E '^[a-zA-Z_-]+:.*##' $(MAKEFILE_LIST) | sort | awk 'BEGIN {FS = ":.*##"}; {printf "  %-12s %s\n", $$1, $$2}'

cache-guard:
	@scripts/check_build_cache_size.sh --guard

cache-status: ## Report target/ usage and its configured limits
	@scripts/check_build_cache_size.sh --status

cache-prune-preview: ## Preview removal of debug and test build artifacts
	cargo clean --dry-run --profile dev

cache-prune: ## Remove debug and test artifacts while retaining release output
	cargo clean --profile dev

build: cache-guard ## Build the library and crest-synth binary
	cargo build

check: cache-guard ## Type-check all targets
	cargo check --all-targets

test: cache-guard ## Run all tests
	cargo test --all-targets

.PHONY: linux-shell linux-image linux-desktop test-linux
linux-shell: ## Open this worktree's Linux development shell through Docker
	scripts/linux/dev.sh

linux-image: ## Build or refresh the native-architecture Linux development image
	scripts/linux/dev.sh --build

linux-desktop: ## Open a visible Linux desktop with the synth, terminal, and browser audio
	scripts/linux/dev.sh --desktop

test-linux: cache-guard ## Run all Linux tests and native witnesses in an isolated virtual desktop
	scripts/linux/with-desktop.sh scripts/linux/check.sh

.PHONY: test-linux-wayland
test-linux-wayland: cache-guard ## Verify Detail, Mixer and Sample paint/resize on native Wayland
	env -u CREST_SOUNDFONT_EVIDENCE_DIR -u CREST_WEBVIEW_CONTROLLER_WITNESS \
	  -u CREST_WEBVIEW_PAGE_NAVIGATION_WITNESS -u CREST_WEBVIEW_OPTION_WITNESS \
	  -u CREST_WEBVIEW_EMPTY_PATCH_WITNESS -u CREST_WEBVIEW_PERFORMANCE_WITNESS \
	  CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_DETAIL_WITNESS=1 CREST_WEBVIEW_SAMPLE_WITNESS=1 \
	  scripts/linux/with-desktop.sh --wayland cargo test --locked --test webview_projection_shell

.PHONY: test-linux-session-native
test-linux-session-native: cache-guard ## Verify shipping audio and session dialogs on X11 and Wayland
	cargo build --locked --release --bin crest-synth
	scripts/linux/with-desktop.sh python3 scripts/linux/session-smoke.py "$(CARGO_OUTPUT_DIR)/release/crest-synth"
	scripts/linux/with-desktop.sh --wayland python3 scripts/linux/session-smoke.py "$(CARGO_OUTPUT_DIR)/release/crest-synth"

test-session-lifecycle: cache-guard ## Run the automated New/Open/Save/Save As/close acceptance suite
	cargo test shell::session_lifecycle --lib
	cargo test shell::standalone_application::tests::normal_ --lib
	cargo test --test production_runtime_contracts

test-engine-post-fx-options: cache-guard ## Run focused deterministic Engine/Post FX option witnesses
	cargo test --test engine_post_fx_option_states -- --nocapture

test-midi-devices: cache-guard ## Run deterministic physical-MIDI contracts, reducer/orchestrator, projection, and renderer guards
	cargo test --test midi_device_contracts -- --nocapture
	cargo test midi_ --lib

test-midi-host: cache-guard ## Enumerate the real host MIDI backend (zero ports is valid; typed init failure is reported)
	cargo test adapter::midir_input_device::tests::real_host_seam_reports_zero_or_more_ports_or_one_typed_initialization_failure --lib -- --exact --nocapture

midi-device-handoff: cache-guard ## Launch the production app for the interactive physical-MIDI Settings/audio checklist
	@scripts/run_midi_device_handoff.sh

test-webview-detail-native: cache-guard ## Run the real-window Detail/Mixer witness without unrelated soak/fault scenes
	CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_DETAIL_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

test-webview-sample-native: cache-guard ## Run Sample Detail lifecycle, waveform, and responsive native checks
	CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_DETAIL_WITNESS=1 CREST_WEBVIEW_SAMPLE_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

test-webview-options-native: cache-guard ## Run the real-window Engine/Post FX option witness and retained Detail/Mixer scenes
	CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_OPTION_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

test-webview-empty-patch-native: cache-guard ## Run the native Shift handoff and empty/pending/capacity/newly-created PATCH witness
	CREST_REQUIRE_KEY_WITNESS=1 cargo test --test input_capture_witness -- --nocapture
	CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_EMPTY_PATCH_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

lint: cache-guard ## Run Clippy with warnings denied
	cargo clippy --all-targets -- -D warnings

fmt: ## Format all Rust sources
	cargo fmt --all

fmt-check: ## Verify Rust formatting without changing files
	cargo fmt --all -- --check

run: cache-guard ## Launch crest-synth with the clean canonical INIT session
	cargo run --release --bin crest-synth

play: cache-guard ## Launch the user-playable canonical INIT session
	cargo run --release --bin crest-synth

ui: cache-guard ## Launch the synth graphical window
	cargo run --release --bin crest-synth

smoke: cache-guard ## Run the complete headless synth path
	cargo run --bin crest-synth -- --smoke

observe: cache-guard ## Print the structured headless behavioral observation
	cargo run --bin crest-synth -- --smoke --observe

demo: cache-guard ## Run the exhaustive GUI demo and structured trace
	cargo run --bin crest-synth -- --smoke --observe --demo-scene

# The five retained live targets below (and the demo-live alias) run on the
# webview shell: every --demo-live-* mode composes its own TauriWebviewWindow
# inside StandaloneApplication::run_live_demo_scene. The webview shell is the product's
# only shell — the interactive and headless targets compose the same
# TauriWebviewWindow directly and no renderer flag exists. Target names are
# retained as stable operator-facing compatibility targets.
demo-live: demo-live-detail-and-assets ## Run the newest optimized graphical live demo

demo-live-detail-and-assets: cache-guard ## Run the cumulative Phase 7 detail, choice, browser, preview, and Sample assignment demo
	cargo run --release --bin crest-synth -- --demo-live-detail-and-assets

# Controlled negative (must exit non-zero on preview reach/audio predicates):
#   cargo run --release --bin crest-synth -- --demo-live-detail-and-assets --defeat-detail-and-assets-preview

demo-live-mixer: cache-guard ## Run the dedicated sixteen-track Mixer demo with a real window, MIDI probes, and physical audio
	cargo run --release --bin crest-synth -- --demo-live-mixer

demo-live-effects-and-buses: cache-guard ## Run the cumulative effects-and-buses demo with a real window and physical audio
	cargo run --release --bin crest-synth -- --demo-live-effects-and-buses

# Additive: the demo-live alias above keeps pointing at the cumulative
# detail-and-assets scene, and this scene does not subsume it. Its subject is
# the SECOND installed Patch, reached through the on-screen SelectPatch
# gesture. Its declared controlled negative removes that gesture:
#   cargo run --release --bin crest-synth -- --demo-live-patch-editor --defeat-patch-selection
# which must exit 1 on the reach predicates.
demo-live-patch-editor: cache-guard ## Run the functional Patch editor demo on the second Patch with a real window and physical audio
	cargo run --release --bin crest-synth -- --demo-live-patch-editor

demo-live-sixteen-track-mixer-routing: cache-guard ## Run the cumulative sixteen-track mixer-routing demo with a real window and physical audio
	cargo run --release --bin crest-synth -- --demo-live-sixteen-track-mixer-routing

demo-live-semantic-view-model: cache-guard ## Run the Phase Two semantic view model with a real window and physical audio
	cargo run --release --bin crest-synth -- --demo-live-semantic-view-model

demo-live-graphical-shell: cache-guard ## Run the Phase One shell with a real window and physical audio
	cargo run --release --bin crest-synth -- --demo-live-graphical-shell

# Browsable by hand, not autonomous: it waits for the operator and is
# deliberately not part of the demo-live alias group above.
demo-live-component-library: cache-guard ## Browse the component gallery by hand — digits 1-9 and 0 select the first ten pages, [ and ] step through all fifteen, closing the window finishes
	cargo run --release --bin crest-synth -- --demo-live-component-library

semantic-graphical-view-model-acceptance: cache-guard ## Prove the deterministic Phase Two semantic view model
	cargo test --test semantic_graphical_view_model -- --nocapture

webview-tokens: cache-guard ## Regenerate webview-page/tokens.css from the authored Rust vocabulary
	cargo test --lib token_export::tests::write_tokens_css -- --ignored

clean: ## Remove Cargo build output
	cargo clean

.PHONY: test-soundfont-loading test-webview-soundfont-native

test-soundfont-loading: cache-guard ## Prove SoundFont import, independent presets/audio, and exact session restore
	cargo test --test soundfont_file_loading --test soundfont_preset_selection

test-webview-soundfont-native: cache-guard ## Prove SoundFont file, preset, loading and failure pages in WKWebView
	CREST_SOUNDFONT_EVIDENCE_DIR=/tmp/crest-soundfont-evidence cargo test --test soundfont_file_loading
	CREST_SOUNDFONT_EVIDENCE_DIR=/tmp/crest-soundfont-evidence CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_DETAIL_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

.PHONY: test-controller test-controller-native test-controller-sdl
test-controller: cache-guard ## Verify gamepad gestures, saved mappings, and reducer-owned Settings
	cargo test --lib controller
	cargo test --lib gamepad
	cargo test --test controller_settings

test-controller-native: cache-guard ## Verify Controller Settings rendering and focus in WKWebView
	CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_CONTROLLER_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

test-controller-sdl: cache-guard ## Verify SDL3 virtual gamepads through the production reducer and projector
	cargo test --lib sdl_gamepads_reach_reducer_and_preserve_input_lifetimes -- --nocapture

.PHONY: test-page-navigation test-webview-page-navigation-native
test-page-navigation: cache-guard ## Run focused page-routing, identity, input, and audio checks
	cargo test --test page_navigation
	cargo test --lib page_

test-webview-page-navigation-native: cache-guard ## Drive bounded Q/E and Shift-arrow navigation through AppKit and native paint
	CREST_WEBVIEW_TESTS=1 CREST_WEBVIEW_PAGE_NAVIGATION_WITNESS=1 cargo test --test webview_projection_shell -- --nocapture

.PHONY: test-braids-controls test-performance test-performance-native test-performance-physical performance-report performance-tools test-performance-runner
test-braids-controls: cache-guard ## Prove keyboard editing of every Braids control through the live graph
	cargo test --test braids_controls

performance-tools: ## Install the pinned Samply profiler under target/performance/tools
	python3 scripts/performance_suite.py install-profiler

test-performance: cache-guard ## Run repeated headless scenes, stress gates, resource measurements, and Samply profiles
	python3 scripts/performance_suite.py run $(PERFORMANCE_ARGS)

performance-report: ## Rank bottlenecks from the latest performance run
	python3 scripts/report_performance.py $(PERFORMANCE_REPORT_ARGS)

test-performance-native: cache-guard ## Profile native WKWebView paint and meter witnesses (unlocked interactive session)
	python3 scripts/performance_suite.py run --group native $(PERFORMANCE_ARGS)

test-performance-physical: cache-guard ## Profile existing live scenes with a real window and audio device
	python3 scripts/performance_suite.py run --group physical $(PERFORMANCE_ARGS)

test-performance-runner: ## Prove suite failures, watchdogs, profile validation, and regression comparisons
	python3 -m unittest discover -s scripts -p 'test_performance_suite.py' -v

.PHONY: test-upstream-audio
test-upstream-audio: cache-guard ## Verify upstream catalog, import/restore, and native real-time operations
	cargo test --test upstream_catalog
	cargo test --lib adapter::upstream_audio
	python3 scripts/check_native_audio.py

# New catalog listening tour: instruments dry, then effects on unchanged Braids.
.PHONY: full-instrument-effect-demo
full-instrument-effect-demo: cache-guard ## Audition new instruments and effects on Braids; eight bars per entry, one voice
	cargo run --release --bin crest-synth -- --full-instrument-effect-demo

.PHONY: demo-live-drum-rack
demo-live-drum-rack: cache-guard ## Audition all sixteen Drum Rack pads, then play an eight-bar layered groove
	cargo run --release --bin crest-synth -- --demo-live-drum-rack
