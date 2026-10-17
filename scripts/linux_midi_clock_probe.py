#!/usr/bin/env python3
"""Measure ALSA queue/monotonic timing; never certify live capture from a probe.

Linux prerequisites: a C compiler, libasound2-dev and accessible /dev/snd/seq.
The caller loads snd-seq/snd-hrtimer and grants device access if needed. This
script does not change host configuration. Only private virtual ports are used.
"""
import argparse
import csv
import json
import platform
from pathlib import Path
import subprocess
import tempfile


def summarize(trace):
    rows = list(csv.reader(trace.splitlines()))
    samples = [tuple(map(int, row[1:])) for row in rows if row[0] == 'sample']
    events = [tuple(map(int, row[1:])) for row in rows if row[0] == 'event']
    timers = [tuple(map(int, row[1:])) for row in rows if row[0] == 'timer']
    ports = [tuple(map(int, row[1:])) for row in rows if row[0] == 'port']
    if len(samples) != 203 or len(events) != 1 or len(timers) != 1 or len(ports) != 1:
        raise ValueError('Incomplete ALSA clock probe trace')
    if any(after < before or queue < 0 or state != 1
           for before, queue, after, state in samples):
        raise ValueError('Clock query failed or queue was not running')
    timestamping, real_time, queue_id = ports[0]
    if (timestamping, real_time) != (1, 1):
        raise ValueError('Input port did not retain real queue timestamps')

    before, event, after, receipt, flags, event_queue = events[0]
    if not before <= event <= after or flags & 1 != 1 or event_queue != queue_id:
        raise ValueError('Event timestamp escaped independently sampled send interval')
    # The consumer deliberately waits 30 ms. Queue timestamp must remain at
    # send time rather than being reassigned when userspace reads the event.
    if before != samples[-3][1] or after != samples[-2][1]:
        raise ValueError('Event send bounds do not match queue observations')
    if receipt - samples[-2][2] < 30_000_000:
        raise ValueError('Delayed-read scenario did not wait for 30 ms')
    if not 0 <= samples[-1][1] < samples[-2][1]:
        raise ValueError('Queue restart did not reset its time origin')

    running = samples[:-1]
    offsets = [(before + after) // 2 - queue for before, queue, after, _ in running]
    return {
        'probe_passed': True,
        'live_capture_qualified': False,
        'physical_workflow_qualified': False,
        'timer': dict(zip(('type', 'class', 'device', 'subdevice', 'resolution'), timers[0])),
        'samples': len(running),
        'elapsed_ns': running[-1][2] - running[0][0],
        'observed_offset_span_ns': max(offsets) - min(offsets),
        'max_query_span_ns': max(after - before for before, _, after, _ in running),
        'delayed_receive_ns': receipt - running[-1][2],
        'event_in_send_interval': True,
        'restart_resets_origin': True,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('target/linux-midi-clock-evidence'))
    args = parser.parse_args()
    if platform.system() != 'Linux':
        parser.error('Run this probe on Linux with the ALSA sequencer available')
    args.output.mkdir(parents=True, exist_ok=True)
    source = Path(__file__).parent / 'linux' / 'alsa_clock_probe.c'
    with tempfile.TemporaryDirectory(prefix='crest-alsa-clock-') as directory:
        executable = Path(directory) / 'probe'
        subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                        str(source), '-lasound', '-o', str(executable)], check=True)
        result = subprocess.run([str(executable)], text=True, capture_output=True, timeout=15)
    (args.output / 'trace.csv').write_text(result.stdout)
    (args.output / 'stderr.txt').write_text(result.stderr)
    result.check_returncode()
    report = summarize(result.stdout)
    report['system'] = platform.platform()
    encoded = json.dumps(report, indent=2) + '\n'
    (args.output / 'result.json').write_text(encoded)
    print(encoded, end='')


if __name__ == '__main__':
    main()
