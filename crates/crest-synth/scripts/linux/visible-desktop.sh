#!/usr/bin/env bash
# Share the existing Linux test desktop through Xpra's packaged HTML5 client.
set -euo pipefail
if [[ "${CREST_LINUX_TEST_DESKTOP:-}" != 1 || -z "${CREST_DESKTOP_PASSWORD:-}" ]]; then
  echo 'Start with scripts/linux/dev.sh --desktop.' >&2
  exit 2
fi

# Keep manual settings across disposable desktop containers, apart from the
# automated witnesses' isolated configuration. Save sessions in /workspace.
export XDG_CONFIG_HOME="${CARGO_TARGET_DIR:-/workspace/target}/linux-desktop/config"
mkdir -p "$XDG_CONFIG_HOME"
pactl set-default-source crest_test_sink.monitor
# Native icons are already included in the desktop capture. Disabling their
# duplicate transport also avoids Xpra 3's obsolete Pillow icon-resize API.
export XPRA_PNG_ICONS=0

# Build visibly so the first launch and build failures never look like a hung
# desktop. Closing the app returns to a shell for the normal development loop.
xterm -title 'Crest Synth Linux terminal' -fa 'DejaVu Sans Mono' -fs 11 \
  -geometry 96x28+10+10 -e bash -c '
  cargo build --locked --release --bin crest-synth && "$CARGO_TARGET_DIR/release/crest-synth"
  printf "\nLinux shell: edit on your Mac, then run cargo run --release --bin crest-synth.\n"
  exec bash
' &
terminal=$!
trap 'kill "$terminal" 2>/dev/null || true' EXIT

xpra shadow "$DISPLAY" --daemon=no --html=on \
  --socket-dirs="$XDG_RUNTIME_DIR/xpra" --ssh-upgrade=no --dbus-control=no \
  --bind-tcp=0.0.0.0:14500 --tcp-auth=env:name=CREST_DESKTOP_PASSWORD \
  --video-encoders=none \
  --pulseaudio=no --speaker=on --microphone=disabled --sound-source=pulse \
  --webcam=no --printing=no --file-transfer=no --open-files=no \
  --start-new-commands=no --notifications=no --mdns=no --dbus-proxy=no
