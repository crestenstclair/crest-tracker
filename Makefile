SYNTH_PATH ?= ../crest-synth
ARGS ?=

.PHONY: dev synth synth-inventory check-pair status-pair diff-pair pin-synth check-pinned test test-host bundle verify-bundle workload audio-workload native-feedback midi-fixture lint fmt-check

dev:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" dev -- $(ARGS)
synth:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" synth -- $(ARGS)
synth-inventory:
	python3 scripts/synth_inventory.py --synth-path "$(SYNTH_PATH)" $(ARGS)
check-pair:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" check-pair $(SYNTH_TESTS)
status-pair:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" status-pair
diff-pair:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" diff-pair
pin-synth:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" pin-synth $(REV)
check-pinned:
	python3 scripts/develop.py check-pinned
bundle:
	python3 scripts/bundle_macos.py --synth-path "$(SYNTH_PATH)" $(ARGS)
verify-bundle:
	python3 scripts/bundle_macos.py --synth-path "$(SYNTH_PATH)" --verify-only "$(if $(BUNDLE),$(BUNDLE),target/release/Crest Tracker.app)"
test:
	cargo --config 'patch."https://github.com/crestenstclair/crest-synth.git".crest-synth.path="$(abspath $(SYNTH_PATH))"' test --workspace
	python3 -B -m unittest discover -s scripts -p 'test_*.py'
	node --test scripts/test_*.cjs

test-host:
	cargo --config 'patch."https://github.com/crestenstclair/crest-synth.git".crest-synth.path="$(abspath $(SYNTH_PATH))"' test --release -p crest-tracker-app synth::witness:: -- --ignored --test-threads=1

workload:
	python3 scripts/workload.py --synth-path "$(SYNTH_PATH)" $(ARGS)

audio-workload:
	python3 scripts/audio_workload.py --synth-path "$(SYNTH_PATH)" $(ARGS)

native-feedback:
	python3 scripts/native_feedback.py --synth-path "$(SYNTH_PATH)" $(ARGS)

midi-fixture:
	cargo --config 'patch."https://github.com/crestenstclair/crest-synth.git".crest-synth.path="$(abspath $(SYNTH_PATH))"' run --release -p crest-tracker-app --example midi_capture_fixture

lint:
	python3 scripts/develop.py --synth-path "$(SYNTH_PATH)" lint
fmt-check:
	cargo fmt --all -- --check
