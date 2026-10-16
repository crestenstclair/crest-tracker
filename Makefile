ARGS ?=
.DEFAULT_GOAL := dev
.PHONY: demo-corridors test-corridors
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

dev:
	cargo run -p crest-tracker-app -- $(ARGS)
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
	cargo test --release -p crest-tracker-app synth::witness:: -- --ignored --test-threads=1
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
