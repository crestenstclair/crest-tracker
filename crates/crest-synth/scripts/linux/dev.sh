#!/usr/bin/env bash
# Run this worktree's commands in a reproducible, non-root Linux environment.
set -euo pipefail

usage() {
  cat <<'EOF'
usage: scripts/linux/dev.sh [--arch arm64|amd64] [--build] [--desktop] [--] [command ...]

With no command, open a Linux shell. The image is built automatically if absent.
--build rebuilds the development image without starting a container.
--desktop opens a browser-accessible Linux desktop with the synth and a terminal.
Keep the launcher running; Ctrl+C stops its desktop. No custom command is accepted.

Examples:
  scripts/linux/dev.sh cargo check --locked --all-targets
  scripts/linux/dev.sh make test-linux
  scripts/linux/dev.sh make test-linux-wayland
  scripts/linux/dev.sh make test-linux-session-native
  scripts/linux/dev.sh --arch amd64 cargo test --locked --lib
  scripts/linux/dev.sh --desktop

Optional host environment:
  CREST_LINUX_TARGET_DIR  Existing/alternate build-cache directory
  CREST_LINUX_CPUS        Container CPU allowance (default: 4)
  CREST_LINUX_MEMORY      Container memory allowance (default: 6g)
  CARGO_BUILD_JOBS        Parallel compiler jobs (default: 2)
  RUST_TEST_THREADS       Parallel test threads (default: 2)
  CREST_LINUX_SCREEN_SIZE Desktop resolution (default: 1024x768)
EOF
}

case "$(uname -m)" in
  arm64|aarch64) architecture=arm64 ;;
  x86_64|amd64) architecture=amd64 ;;
  *) architecture= ;;
esac
build_only=false
desktop=false
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --arch)
      [[ "$#" -ge 2 ]] || { usage >&2; exit 2; }
      architecture="$2"
      shift 2
      ;;
    --build) build_only=true; shift ;;
    --desktop) desktop=true; shift ;;
    --help|-h) usage; exit 0 ;;
    --) shift; break ;;
    --*) echo "Unknown launcher option: $1" >&2; usage >&2; exit 2 ;;
    *) break ;;
  esac
done
case "$architecture" in
  arm64|amd64) ;;
  *) echo 'Choose --arch arm64 or --arch amd64.' >&2; exit 2 ;;
esac
if "$build_only" && [[ "$#" -ne 0 ]]; then
  echo '--build does not accept a command.' >&2
  exit 2
fi
if "$desktop" && [[ "$#" -ne 0 ]]; then
  echo '--desktop does not accept a command; use its Linux terminal.' >&2
  exit 2
fi

workspace="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
cargo_manifest="$(cargo locate-project --workspace --message-format plain --manifest-path "$workspace/Cargo.toml")"
cargo_workspace="$(dirname "$cargo_manifest")"
package_relative="${workspace#"$cargo_workspace"}"
container_package="/workspace$package_relative"
owner_uid="$(id -u)"
if [[ "$owner_uid" == 0 ]]; then
  echo 'Run the development launcher as your normal user, without sudo.' >&2
  exit 2
fi
command -v docker >/dev/null || { echo 'Install and start Docker Desktop first.' >&2; exit 1; }
if ! docker info >/dev/null 2>&1; then
  echo 'Docker is unavailable. Start Docker Desktop, then retry.' >&2
  exit 1
fi

# Different worktrees share only downloaded Cargo sources, never target output.
# The image key also prevents reuse of an outdated toolchain/dependency image.
image_key="$(git -C "$workspace" hash-object scripts/linux/Dockerfile)"
build_target=development
"$desktop" && build_target=desktop
image="crest-linux-dev:${architecture}-${owner_uid}-${image_key:0:12}-$build_target"
if "$build_only" || ! docker image inspect "$image" >/dev/null 2>&1; then
  docker build --platform "linux/$architecture" --target "$build_target" --build-arg "CREST_UID=$owner_uid" \
    -f "$workspace/scripts/linux/Dockerfile" -t "$image" "$workspace/scripts/linux"
fi
"$build_only" && exit 0

target_directory="${CREST_LINUX_TARGET_DIR:-$workspace/target/linux-$architecture}"
if [[ "$target_directory" != /* ]]; then
  target_directory="$workspace/$target_directory"
fi
mkdir -p "$target_directory"
target_directory="$(cd "$target_directory" && pwd -P)"
# Build targets own the cache guard; keep the shell and cargo clean available
# even when a cache has reached its limit.

registry_volume="crest-linux-registry-$owner_uid"
git_volume="crest-linux-cargo-git-$owner_uid"
arguments=(run --rm --init --interactive --platform "linux/$architecture"
  --security-opt seccomp=unconfined
  --cpus "${CREST_LINUX_CPUS:-4}" --memory "${CREST_LINUX_MEMORY:-6g}"
  --workdir "$container_package"
  --mount "type=bind,source=$cargo_workspace,target=/workspace"
  --mount "type=bind,source=$target_directory,target=/workspace/target"
  --mount "type=volume,source=$registry_volume,target=/opt/cargo/registry"
  --mount "type=volume,source=$git_volume,target=/opt/cargo/git"
  --env CARGO_TARGET_DIR=/workspace/target
  --env "CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-2}"
  --env "RUST_TEST_THREADS=${RUST_TEST_THREADS:-2}")
if [[ -f "$cargo_workspace/.git" ]]; then
  # A linked worktree's .git file names metadata outside its source directory.
  # Keep that path valid without mounting the other worktrees' source trees.
  git_metadata="$(git -C "$cargo_workspace" rev-parse --path-format=absolute --git-common-dir)"
  arguments+=(--mount "type=bind,source=$git_metadata,target=$git_metadata")
fi
if "$desktop"; then
  # Docker assigns the host port atomically, so separate worktrees can each
  # own a desktop. The server also authenticates connections from other
  # containers and unrelated browser origins.
  CREST_DESKTOP_PASSWORD="$(openssl rand -hex 16)"
  export CREST_DESKTOP_PASSWORD
  session_directory="$(mktemp -d)"
  container=
  logger=
  waiter=
  cleanup() {
    trap - EXIT INT TERM
    [[ -z "$logger" ]] || kill "$logger" 2>/dev/null || true
    [[ -z "$waiter" ]] || kill "$waiter" 2>/dev/null || true
    if [[ -z "$container" && -s "$session_directory/container" ]]; then
      container="$(cat "$session_directory/container")"
    fi
    [[ -z "$container" ]] || docker stop --time 5 "$container" >/dev/null 2>&1 || true
    rm -rf "$session_directory"
  }
  trap cleanup EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  container="$(docker "${arguments[@]}" --detach --cidfile "$session_directory/container" \
    --publish 127.0.0.1::14500 \
    --env "CREST_LINUX_SCREEN_SIZE=${CREST_LINUX_SCREEN_SIZE:-1024x768}" \
    --env CREST_DESKTOP_PASSWORD "$image" \
    scripts/linux/with-desktop.sh scripts/linux/visible-desktop.sh)"
  binding="$(docker port "$container" 14500/tcp)"
  url="http://127.0.0.1:${binding##*:}/"
  ready=false
  for attempt in {1..60}; do
    if curl --fail --silent --max-time 1 "$url" >/dev/null; then
      ready=true
      break
    fi
    [[ "$(docker inspect --format '{{.State.Running}}' "$container" 2>/dev/null)" == true ]] || break
    sleep .5
  done
  if ! "$ready"; then
    docker logs "$container" >&2 || true
    echo 'Linux desktop did not start.' >&2
    exit 1
  fi
  printf 'Linux desktop: %s?username=crest&password=%s&sound=true&swap_keys=false\n' "$url" "$CREST_DESKTOP_PASSWORD"
  printf 'Click the desktop for Linux keyboard focus. If silent, toggle Xpra Audio off/on once.\n'
  printf 'Close browser to disconnect; Ctrl+C stops this desktop.\nStop from another terminal: docker stop %s\n' "$container"
  docker logs --follow "$container" &
  logger=$!
  docker wait "$container" > "$session_directory/status" &
  waiter=$!
  wait "$waiter"
  exit "$(cat "$session_directory/status")"
fi
if [[ -t 0 && -t 1 ]]; then
  arguments+=(--tty)
fi
if [[ "$#" -eq 0 ]]; then
  set -- bash
fi
printf 'Linux %s · worktree %s · build cache %s\n' "$architecture" "$workspace" "$target_directory" >&2
exec docker "${arguments[@]}" "$image" "$@"
