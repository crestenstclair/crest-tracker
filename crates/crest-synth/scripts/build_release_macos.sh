#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
export MACOSX_DEPLOYMENT_TARGET=11.0
target="$(rustc -vV | sed -n 's/^host: //p')"
case "$target" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *) echo "Release packaging requires a native macOS host: $target" >&2; exit 1 ;;
esac
version="$(python3 -c 'import json; print(json.load(open("tauri.conf.json"))["version"])')"
target_directory="$(scripts/cargo_target_dir.sh)"
output="$target_directory/release-assets"
mkdir -p "$output"

# LFS pointers must never be packaged as playable SoundFonts.
python3 - <<'PY'
from pathlib import Path
with Path('sf2/SGM-V2.01.sf2').open('rb') as source:
    header = source.read(12)
assert header[:4] == b'RIFF' and header[8:] == b'sfbk', 'Run git lfs pull before packaging'
PY

cargo build --locked --release --bins
# Unlike Apple's ARM linker, the Intel linker leaves executables unsigned.
# Tauri verifies the main executable before signing its bundled sibling, so
# every executable needs an initial signature before it enters the app bundle.
for executable in crest-synth crest-synth-witness; do
  codesign --force --sign - "$target_directory/release/$executable"
done
npx --yes @tauri-apps/cli@2.11.4 bundle --ci --bundles app,dmg
app="$target_directory/release/bundle/macos/crest-synth.app"
codesign --verify --deep --strict "$app"

# Exercise the installed production binary outside the source checkout.
# No resource lookup may depend on the invoking shell's working directory.
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
(
  cd "$scratch"
  "$app/Contents/MacOS/crest-synth" --smoke --observe
) > "$output/smoke-$target.json"

python3 - "$app" "$target" "$version" <<'PY'
from pathlib import Path
import plistlib, subprocess, sys
app, target = Path(sys.argv[1]), sys.argv[2]
info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
assert info['CFBundleShortVersionString'] == sys.argv[3]
assert info['LSMinimumSystemVersion'] == '11.0'
binary = app / 'Contents/MacOS/crest-synth'
expected = 'arm64' if target.startswith('aarch64') else 'x86_64'
assert subprocess.check_output(['lipo', '-archs', str(binary)], text=True).strip() == expected
for line in subprocess.check_output(['otool', '-L', str(binary)], text=True).splitlines()[1:]:
    dependency = line.strip().split(' (', 1)[0]
    assert dependency.startswith(('/usr/lib/', '/System/Library/')), dependency
for resource in ['sf2/HiDef.sf2', 'licenses/UPSTREAM_AUDIO.txt',
                 'licenses/SDL3.txt', 'licenses/AZERET_MONO.txt',
                 'licenses/EIGEN_SOURCE.tar.gz']:
    assert (app / 'Contents/Resources' / resource).stat().st_size > 0, resource
PY

ditto -c -k --sequesterRsrc --keepParent "$app" \
  "$output/crest-synth-v$version-$target.app.zip"
for dmg in "$target_directory"/release/bundle/dmg/*.dmg; do
  cp "$dmg" "$output/crest-synth-v$version-$target.dmg"
done
echo "Verified release assets: $output"
