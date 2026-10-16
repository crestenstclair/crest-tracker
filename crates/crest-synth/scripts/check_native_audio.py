#!/usr/bin/env python3
"""Exercise the same native archives as Cargo, with C++ allocation counters."""
from pathlib import Path
import argparse, json, subprocess, sys
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--release', action='store_true', help='Check the optimized release archives.')
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
# Cargo owns profile/feature/target selection. A newest-file heuristic can link
# debug or partially rebuilt archives instead of the optimized test artifacts.
build = ['cargo', 'test', '--locked', '--lib', '--no-run', '--message-format=json']
if args.release:
    build.append('--release')
result = subprocess.run(build, cwd=root, text=True, stdout=subprocess.PIPE)
if result.returncode:
    print(result.stdout, file=sys.stderr)
    sys.exit(result.returncode)
outputs = []
for line in result.stdout.splitlines():
    message = json.loads(line)
    if message.get('reason') == 'build-script-executed':
        candidate = Path(message['out_dir'])
        if (candidate / 'libcrest_audio.a').is_file():
            outputs.append(candidate)
if len(outputs) != 1:
    sys.exit(f'Cargo must identify exactly one Crest native build, found {outputs}')
output = outputs[0]
print(f'Native witness archives: {output}', flush=True)
binary = output / 'native-audio-witness'
# Both Cargo test and release profiles compile DSP at O3. Build the upstream
# numerical references the same way (notably floating-point contraction).
command = ['c++', '-std=c++20', '-O3', '-g', '-I'+str(output), str(root/'native/audio/tests/realtime.cpp'), '-o', str(binary)]
command += ['-I'+str(output/'sfizz-source/src'),
            '-I'+str(root/'vendor/audio/sfizz/external/abseil-cpp'),
            '-I'+str(root/'vendor/audio/sfizz/external/simde'),
            str(root/'native/audio/tests/sfizz_midi.cpp')]
command += ['-I'+str(output/'daisy-source'),
            '-I'+str(output/'daisy-source/Utility'),
            str(root/'native/audio/tests/daisy_reference.cpp'),
            str(root/'native/audio/tests/daisy_coefficients.cpp')]
command += ['-I'+str(output/'r8brain-source'),
            '-I'+str(root/'native/audio'),
            str(root/'native/audio/tests/rate_adapter.cpp'),
            str(root/'native/audio/tests/r8brain_reference.cpp'),
            str(root/'native/audio/tests/r8brain_scheduling.cpp')]
if subprocess.run(['c++', '-fno-lifetime-dse', '-x', 'c++', '-fsyntax-only', '-'],
                  input='', text=True, capture_output=True).returncode == 0:
    command.append('-fno-lifetime-dse')
command += ['-DTEST', '-I'+str(output/'mutable-source'), '-I'+str(root/'vendor/audio'), '-I'+str(root/'vendor/audio/mutable'),
            str(root/'native/audio/tests/mutable_reference.cpp'),
            str(root/'native/audio/tests/mutable_resonators.cpp')]
command += ['-I'+str(output/'stk-source'), '-I'+str(root/'vendor/audio/stk/include'),
            str(root/'native/audio/tests/stk_reference.cpp'),
            str(root/'native/audio/tests/stk_models.cpp'),
            str(root/'native/audio/tests/stk_sample_rate.cpp')]
if sys.platform == 'darwin':
    interposer = output / 'libcrest_witness_interpose.dylib'
    subprocess.run(['c++', '-std=c++20', '-O1', '-dynamiclib', '-mmacosx-version-min=11.0',
                    str(root/'native/audio/tests/interpose.cpp'), '-o', str(interposer),
                    '-Wl,-install_name,@rpath/'+interposer.name], check=True)
    command += ['-mmacosx-version-min=11.0', str(interposer), '-Wl,-rpath,'+str(output)]
elif sys.platform.startswith('linux'):
    command.append(str(root/'native/audio/tests/interpose.cpp'))
    command += ['-Wl,--wrap='+symbol for symbol in (
        'malloc', 'calloc', 'realloc', 'free', 'pthread_mutex_lock',
        'pthread_rwlock_rdlock', 'pthread_rwlock_wrlock', 'time')]
for name in ['audio','mutable','daisy','stk','msfa','nam','convolution','sfizz']:
    command.append(str(output/f'libcrest_{name}.a'))
command += ['-pthread']
subprocess.run(command, check=True)
subprocess.run([str(binary)], check=True)
