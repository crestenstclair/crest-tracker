ARGS ?=
.DEFAULT_GOAL := dev
.PHONY: demo demo-mixer list-demos test-demos midi-fixtures midi-import
SCENE ?= drums
demo:
ifeq ($(shell uname -s),Darwin)
	python3 scripts/bundle_macos.py --release
	"target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app" --demo "$(SCENE)"
else
	cargo run --release -p crest-tracker-app -- --demo "$(SCENE)"
endif
list-demos:
	cargo run --release -p crest-tracker-app -- --list-demos
demo-mixer:
	@test -n "$(SONG)" || (echo 'Use make demo-mixer SONG="/path/to/song.crest-song"'; exit 2)
ifeq ($(shell uname -s),Darwin)
	python3 scripts/bundle_macos.py --release
	"target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app" --demo-mixer "$(SONG)"
else
	cargo run --release -p crest-tracker-app -- --demo-mixer "$(SONG)"
endif
test-demos:
	cargo test --release -p crest-tracker-app feature_event_scene -- --ignored --test-threads=1 --nocapture
midi-fixtures:
	python3 scripts/midi_fixtures.py $(ARGS)
midi-import:
	cargo build --release -p crest-tracker-app
	python3 scripts/midi_import.py $(ARGS)

.PHONY: demo-corridors test-corridors demo-cursors test-cursors demo-piano test-piano
demo-piano:
ifeq ($(shell uname -s),Darwin)
	python3 scripts/bundle_macos.py --release
	"target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app" --demo-piano
else
	cargo run --release -p crest-tracker-app -- --demo-piano
endif
test-piano:
	cargo test --release -p crest-tracker-app piano_event_scene -- --ignored --test-threads=1 --nocapture

demo-cursors:
ifeq ($(shell uname -s),Darwin)
	python3 scripts/bundle_macos.py --release
	"target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app" --demo-cursors
else
	cargo run --release -p crest-tracker-app -- --demo-cursors
endif
test-cursors:
	cargo test --release -p crest-tracker-app independent_cursor_event_scene -- --ignored --test-threads=1 --nocapture

demo-corridors:
ifeq ($(shell uname -s),Darwin)
	python3 scripts/bundle_macos.py --release
	"target/release/Crest Tracker.app/Contents/MacOS/crest-tracker-app" --demo-corridors
else
	cargo run --release -p crest-tracker-app -- --demo-corridors
endif
test-corridors:
	cargo test --release -p crest-tracker-app corridors_event_scene -- --ignored --test-threads=1 --nocapture

.PHONY: dev synth test test-synth test-host check lint fmt-check bundle verify-bundle synth-inventory workload audio-workload

PERF_DIR ?= .dev/performance
dev:
	CREST_PERF_DIR="$(PERF_DIR)" cargo run --profile profiling -p crest-tracker-app -- $(ARGS)
synth:
	cargo run -p crest-synth --bin crest-synth -- $(ARGS)
check:
	cargo check -p crest-tracker -p crest-tracker-app -p crest-synth --all-targets
test:
	cargo test -p crest-tracker -p crest-tracker-app
	python3 -B -m unittest discover -s scripts -p 'test_*.py'
test-synth:
	cd crates/crest-synth && cargo test -p crest-synth --lib

test-host:
	cargo test --release -p crest-tracker-app synth::witness:: -- --ignored --test-threads=1 --skip imported_song_
lint:
	cargo clippy -p crest-tracker -p crest-tracker-app -p crest-synth --all-targets --no-deps -- -D warnings
fmt-check:
	cargo fmt --all -- --check
bundle:
	python3 scripts/bundle_macos.py $(ARGS)
verify-bundle:
	python3 scripts/bundle_macos.py --verify-only "$(if $(BUNDLE),$(BUNDLE),target/debug/Crest Tracker.app)"
synth-inventory:
	python3 scripts/synth_inventory.py $(ARGS)
workload:
	python3 scripts/workload.py $(ARGS)
audio-workload:
	python3 scripts/audio_workload.py $(ARGS)

.PHONY: stress-corpus
stress-corpus:
	@test -n "$(CORPUS)" || (echo 'Use make stress-corpus CORPUS="/path/to/organized songs"'; exit 2)
	python3 scripts/stress_corpus.py "$(CORPUS)" $(ARGS)
