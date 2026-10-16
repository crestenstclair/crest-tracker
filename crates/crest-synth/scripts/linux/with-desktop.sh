#!/usr/bin/env bash
# Isolated native Linux test desktop. Audio reaches a PulseAudio monitor sink;
# this proves the CPAL/ALSA path, not physical speakers or a connected gamepad.
set -euo pipefail

if [[ "$(uname -s)" != Linux ]]; then
  echo 'Run this script on Linux, or in the scripts/linux/Dockerfile image.' >&2
  exit 2
fi
if [[ "${1:-}" != --inside-desktop ]]; then
  screen_size="${CREST_LINUX_SCREEN_SIZE:-2560x1600}"
  if [[ ! "$screen_size" =~ ^[1-9][0-9]*x[1-9][0-9]*$ ]]; then
    echo 'CREST_LINUX_SCREEN_SIZE must be WIDTHxHEIGHT.' >&2
    exit 2
  fi
  window_system=x11
  if [[ "${1:-}" == --wayland ]]; then
    window_system=wayland
    shift
  fi
  exec dbus-run-session -- xvfb-run -a -s "-screen 0 ${screen_size}x24 -nolisten tcp" \
    "$0" --inside-desktop "$window_system" "$@"
fi
shift
window_system="$1"
shift
unset WAYLAND_DISPLAY
export GDK_BACKEND=x11
export XDG_CURRENT_DESKTOP=GNOME
# Xvfb has no hardware compositor. WebKit's accelerated path adds avoidable
# software-GPU round trips here; keep paint timing on its software path.
export WEBKIT_DISABLE_COMPOSITING_MODE="${WEBKIT_DISABLE_COMPOSITING_MODE:-1}"
if [[ "$#" -eq 0 ]]; then
  echo 'usage: with-desktop.sh [--wayland] <command> [arguments...]' >&2
  exit 2
fi

runtime="$(mktemp -d /tmp/crest-linux-desktop.XXXXXX)"
export XDG_RUNTIME_DIR="$runtime"
export CREST_LINUX_TEST_DESKTOP=1
export PULSE_SERVER="unix:$runtime/pulse/native"
export CREST_AUDIO_BUFFER_FRAMES="${CREST_AUDIO_BUFFER_FRAMES:-8192}"
export ALSA_CONFIG_PATH="$runtime/asound.conf"
# Portal backends activated by D-Bus need the display created by xvfb-run.
dbus-update-activation-environment DISPLAY XAUTHORITY XDG_CURRENT_DESKTOP XDG_RUNTIME_DIR
cat > "$ALSA_CONFIG_PATH" <<'EOF'
</usr/share/alsa/alsa.conf>
pcm.!default { type pulse }
ctl.!default { type pulse }
EOF
cleanup() {
  pulseaudio --kill >/dev/null 2>&1 || true
  if [[ -n "${compositor:-}" ]]; then
    kill "$compositor" 2>/dev/null || true
    # A failed client can leave Weston waiting for a surface during shutdown.
    # Bound teardown of this owned test compositor, including failure paths.
    for attempt in {1..20}; do
      kill -0 "$compositor" 2>/dev/null || break
      sleep .1
    done
    kill -KILL "$compositor" 2>/dev/null || true
    wait "$compositor" 2>/dev/null || true
  fi
  if [[ -n "${window_manager:-}" ]]; then
    kill "$window_manager" 2>/dev/null || true
    wait "$window_manager" 2>/dev/null || true
  fi
  rm -rf "$runtime"
}
trap cleanup EXIT
openbox > "$runtime/openbox.log" 2>&1 &
window_manager=$!
pulseaudio --daemonize=yes --exit-idle-time=-1 \
  --load='module-null-sink sink_name=crest_test_sink rate=48000 channels=2'
pactl set-default-sink crest_test_sink
if [[ "$window_system" == wayland ]]; then
  # The nested compositor supplies a real Wayland input seat without requiring
  # GPU or physical input devices. GTK clients cannot fall back to X11.
  export WAYLAND_DISPLAY=crest-test-wayland
  weston --backend=x11 --renderer=pixman --width=2304 --height=1440 \
    --idle-time=0 --socket="$WAYLAND_DISPLAY" --no-config \
    --log="$runtime/weston.log" &
  compositor=$!
  outer=
  for attempt in {1..100}; do
    if [[ -f "$runtime/weston.log" ]]; then
      outer="$(awk '/window id/ {print $NF; exit}' "$runtime/weston.log")"
    fi
    [[ -S "$runtime/$WAYLAND_DISPLAY" && "$outer" =~ ^[0-9]+$ ]] && break
    kill -0 "$compositor" 2>/dev/null || break
    sleep .1
  done
  if [[ ! -S "$runtime/$WAYLAND_DISPLAY" || ! "$outer" =~ ^[0-9]+$ ]]; then
    cat "$runtime/weston.log" >&2 || true
    echo 'Wayland test compositor did not start.' >&2
    exit 1
  fi
  export GDK_BACKEND=wayland
  dbus-update-activation-environment WAYLAND_DISPLAY GDK_BACKEND
  # XTest drives only the compositor's outer seat. The app remains a native
  # Wayland client; the log identifies the initially unmapped X11 seat window.
  timeout 5 xdotool windowmap --sync "$outer"
  sleep .5
  timeout 5 xdotool windowactivate --sync "$outer"
fi
"$@"
