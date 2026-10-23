#!/usr/bin/env python3
"""Write small, original MIDI performances for deterministic multi-song import QA."""
import argparse
from pathlib import Path
import struct


def variable(value):
    output = [value & 127]
    while value := value >> 7:
        output.insert(0, (value & 127) | 128)
    return bytes(output)


def track(events):
    result = bytearray()
    previous = 0
    for tick, event in sorted(events, key=lambda event: event[0]):
        result.extend(variable(tick - previous))
        result.extend(event)
        previous = tick
    result.extend(b"\x00\xff\x2f\x00")
    return b"MTrk" + struct.pack(">I", len(result)) + result


def smf(tracks, format=1, ppq=96):
    return b"MThd" + struct.pack(">IHHH", 6, format, len(tracks), ppq) + b"".join(track(events) for events in tracks)


def note(channel, pitch, at, duration, velocity=90, release=27):
    return [(at, bytes((0x90 | channel, pitch, velocity))),
            (at + duration, bytes((0x80 | channel, pitch, release)))]


def fixtures():
    conductor = [(0, b"\xff\x51\x03\x07\xa1\x20"), (0, b"\xff\x58\x04\x04\x02\x18\x08")]
    melody = [(0, b"\xc0\x00")]
    for index, pitch in enumerate([60, 64, 67, 71, 72, 67, 64, 60]):
        melody += note(0, pitch, index * 48, 36, 82 + index)
    multichannel = list(conductor) + melody
    for index, pitch in enumerate([48, 55, 52, 55]):
        multichannel += note(1, pitch, index * 96, 84, 75)
    for index in range(8):
        multichannel += note(9, 36 if index % 2 == 0 else 38, index * 48, 12, 80)
    changing = list(conductor) + [(0, b"\xc0\x00")]
    changing += note(0, 60, 0, 120) + [(96, b"\xc0\x05")] + note(0, 67, 96, 96)
    changing += [(384, b"\xff\x51\x03\x09\x27\xc0"), (384, b"\xff\x58\x04\x03\x02\x18\x08")]
    changing += note(0, 72, 384, 144)
    expression = list(conductor) + note(0, 60, 0, 192)
    expression += [(0, b"\xb0\x07\x64"), (48, b"\xe0\x00\x50"), (72, b"\xd0\x35"),
                   (96, b"\xa0\x3c\x45"), (120, b"\xb0\x01\x37"), (160, b"\xe0\x00\x40")]
    gates = list(conductor) + note(0, 60, 0, 0) + note(0, 64, 1, 32)
    gates += [(48, b"\x90\x43\x50"), (96, b"\x90\x43\x00"),
              (120, b"\x80\x40\x00"), (144, b"\x90\x48\x50"), (192, b"\xff\x01\x03end")]
    return {
        "melody-type1.mid": smf([conductor, melody]),
        "ensemble-type0.mid": smf([multichannel], format=0),
        "program-tempo-meter.mid": smf([changing], format=0),
        "channel-expression.mid": smf([expression], format=0),
        "gate-repairs.mid": smf([gates], format=0),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    options = parser.parse_args()
    options.output.mkdir(parents=True, exist_ok=False)
    for name, data in fixtures().items():
        with (options.output / name).open("xb") as stream:
            stream.write(data)
    print(options.output)


if __name__ == "__main__":
    main()
