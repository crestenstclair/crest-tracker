//! Interactive, local CoreMIDI source for the native recording acceptance path.
//! Select the printed source in Synth, arm/record in Tracker, then press Enter.
//! This exercises OS ingress but makes no physical latency claim.
#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use midir::os::unix::VirtualOutput;
    use std::io::{self, BufRead, Write};

    let name = format!("Crest Tracker Capture Fixture {}", std::process::id());
    let mut output = midir::MidiOutput::new(&name)?.create_virtual(&name)?;
    println!("READY: {name}");
    println!("Select this temporary input in Synth. Arm and Record in Tracker.");
    println!("Enter sends a complete take. h holds C4 with sustain; x removes the source without cleanup (unplug test). q releases notes and closes.");
    println!("m sends MPE: declare lower zone, 2 members, master ±2 / members ±48 semitones. Two C4 members plus channel reuse under sustain; use an MPE-ready destination.");
    println!("Clock: c starts 32 beats at 120 BPM; j sends 120/96/144 BPM with pulse jitter; r sends SPP beat 16 + Continue; l starts then loses pulses without Stop. Each pass includes a take.");
    io::stdout().flush()?;
    let mut unplug = false;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.trim() == "q" {
            break;
        }
        if line.trim() == "x" {
            unplug = true;
            break;
        }
        if line.trim() == "h" {
            for bytes in [&[0xe0, 1, 64][..], &[0xb0, 64, 127], &[0x90, 60, 101]] {
                output.send(bytes)?;
            }
            println!("HELD: C4 velocity 101, raw bend 8193, sustain 127. x unplugs without release packets.");
            io::stdout().flush()?;
            continue;
        }
        if matches!(line.trim(), "c" | "j" | "r" | "l") {
            send_clock_pass(&mut output, line.trim())?;
            continue;
        }
        if line.trim() == "m" {
            send_packets(&mut output, MPE_PACKETS)?;
            println!("SENT MPE: C4 on members 2/3, velocities 101/87, releases 45/33, distinct bend/pressure/CC74; member 2 reused for E4 velocity 95 release 27 under master sustain.");
            io::stdout().flush()?;
            continue;
        }
        if !line.trim().is_empty() {
            println!("Use Enter, m, h, c, j, r, l, x or q.");
            continue;
        }
        send_packets(&mut output, PACKETS)?;
        println!("SENT: key velocities 101/87; release velocities 45/33; raw bend 8193; pressure 73. End Take before closing this source.");
        io::stdout().flush()?;
    }
    if !unplug {
        output.send(&[0xfc])?;
        for channel in 0..=2 {
            output.send(&[0x80 | channel, 60, 0])?;
            output.send(&[0x80 | channel, 64, 0])?;
            output.send(&[0xb0 | channel, 64, 0])?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn send_packets(
    output: &mut midir::MidiOutputConnection,
    packets: &[(u64, &[u8])],
) -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    for (millis, bytes) in packets {
        std::thread::sleep(Duration::from_millis(*millis).saturating_sub(started.elapsed()));
        output.send(bytes)?;
        println!("{} us: {bytes:?}", started.elapsed().as_micros());
    }
    Ok(())
}

/// Exercise the real transport/performance ingress order with absolute deadlines.
/// This is a finite virtual-source witness, not a physical clock/latency benchmark.
#[cfg(target_os = "macos")]
fn send_clock_pass(
    output: &mut midir::MidiOutputConnection,
    command: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{self, Write};
    use std::time::{Duration, Instant};

    let segments: &[(u64, u64)] = if command == "j" {
        &[(8, 120), (8, 96), (8, 144)]
    } else {
        &[(32, 120)]
    };
    // One beat of clock precedes performance, leaving the delayed playback
    // mapping established before the first captured attack.
    let mut schedule: Vec<(u64, Vec<u8>)> = PACKETS
        .iter()
        .map(|(millis, bytes)| ((millis + 500) * 1000, bytes.to_vec()))
        .collect();
    let mut base = 0;
    let mut pulses = 0;
    for &(beats, bpm) in segments {
        schedule.extend(clock_ticks(base, beats, bpm, command == "j"));
        pulses += beats * 24;
        base += beats * 60_000_000 / bpm;
    }
    schedule.sort_by_key(|(micros, _)| *micros);
    if command == "r" {
        output.send(&[0xf2, 64, 0])?; // 64 sixteenths = beat 16.
        output.send(&[0xfb])?;
    } else {
        output.send(&[0xfa])?;
    }
    println!(
        "CLOCK RUNNING: {command}, {pulses} pulses, {} ms",
        base / 1000
    );
    io::stdout().flush()?;
    let started = Instant::now();
    for (micros, bytes) in schedule {
        std::thread::sleep(Duration::from_micros(micros).saturating_sub(started.elapsed()));
        output.send(&bytes)?;
    }
    std::thread::sleep(Duration::from_micros(base).saturating_sub(started.elapsed()));
    if command == "l" {
        println!("CLOCK LOST: source remains connected; no Stop sent. Tracker must stop and retain its take.");
    } else {
        output.send(&[0xfc])?;
        println!("CLOCK STOPPED: {pulses} pulses sent; source remains connected.");
    }
    io::stdout().flush()?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn clock_ticks(start: u64, beats: u64, bpm: u64, jitter: bool) -> Vec<(u64, Vec<u8>)> {
    (0..beats * 24)
        .map(|tick| {
            let delay = if jitter && tick % 2 == 1 { 1000 } else { 0 };
            (start + tick * 60_000_000 / (bpm * 24) + delay, vec![0xf8])
        })
        .collect()
}

#[cfg(target_os = "macos")]
const PACKETS: &[(u64, &[u8])] = &[
    (0, &[0xe0, 0, 64]),
    (0, &[0xd0, 0]),
    (0, &[0xb0, 64, 0]),
    (40, &[0x90, 60, 101]),
    (100, &[0xe0, 1, 64]),
    (160, &[0xd0, 73]),
    (180, &[0xb0, 64, 127]),
    (210, &[0x90, 64, 87]),
    (280, &[0x80, 60, 45]),
    (350, &[0x80, 64, 33]),
    (430, &[0xe0, 0, 64]),
    (430, &[0xd0, 0]),
    (500, &[0xb0, 64, 0]),
];

#[cfg(target_os = "macos")]
const MPE_PACKETS: &[(u64, &[u8])] = &[
    (0, &[0xe0, 0, 64]),
    (0, &[0xb0, 64, 0]),
    (0, &[0xe1, 0, 64]),
    (0, &[0xe2, 0, 64]),
    (40, &[0x91, 60, 101]),
    (80, &[0x92, 60, 87]),
    (100, &[0xe1, 1, 64]),
    (110, &[0xe2, 127, 63]),
    (140, &[0xd1, 73]),
    (150, &[0xd2, 41]),
    (160, &[0xb1, 74, 91]),
    (170, &[0xb2, 74, 33]),
    (180, &[0xe0, 2, 64]),
    (180, &[0xb0, 64, 127]),
    (240, &[0x81, 60, 45]),
    (300, &[0x91, 64, 95]),
    (360, &[0xe1, 3, 64]),
    (400, &[0x82, 60, 33]),
    (440, &[0x81, 64, 27]),
    (500, &[0xb0, 64, 0]),
    (540, &[0xe0, 0, 64]),
    (540, &[0xe1, 0, 64]),
    (540, &[0xe2, 0, 64]),
];

#[cfg(not(target_os = "macos"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("The native capture fixture requires macOS and CoreMIDI".into())
}
