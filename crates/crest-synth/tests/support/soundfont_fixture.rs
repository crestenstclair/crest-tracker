/// Tiny, locally generated SF2 with two authored presets and a looped test tone.
/// No third-party samples or bank files are embedded in the test suite.
pub fn bank(label: &str, bank: u16, period: usize) -> Vec<u8> {
    fn chunk(id: &[u8; 4], mut data: Vec<u8>) -> Vec<u8> {
        if !data.len().is_multiple_of(2) {
            data.push(0);
        }
        [
            id.to_vec(),
            (data.len() as u32).to_le_bytes().to_vec(),
            data,
        ]
        .concat()
    }
    fn name(value: &str) -> Vec<u8> {
        let mut bytes = vec![0; 20];
        let count = value.len().min(19);
        bytes[..count].copy_from_slice(&value.as_bytes()[..count]);
        bytes
    }
    fn words(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn header(label: &str, program: u16, bank: u16, bag: u16) -> Vec<u8> {
        [name(label), words(&[program, bank, bag]), vec![0; 12]].concat()
    }
    let info = chunk(
        b"LIST",
        [
            b"INFO".to_vec(),
            chunk(b"ifil", words(&[2, 1])),
            chunk(b"INAM", b"Crest test bank\0".to_vec()),
        ]
        .concat(),
    );
    let length = period * 32;
    let mut pcm = (0..length)
        .flat_map(|i| {
            (((std::f64::consts::TAU * i as f64 / period as f64).sin() * 12000.0) as i16)
                .to_le_bytes()
        })
        .collect::<Vec<_>>();
    pcm.extend([0; 92]);
    let sample_data = chunk(b"LIST", [b"sdta".to_vec(), chunk(b"smpl", pcm)].concat());
    let sample = [
        name("Test tone"),
        [0u32, length as u32, 0, length as u32, 48_000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect(),
        vec![60, 0],
        words(&[0, 1]),
    ]
    .concat();
    let parameters = chunk(
        b"LIST",
        [
            b"pdta".to_vec(),
            chunk(
                b"phdr",
                [
                    header(&format!("{label} One"), 3, bank, 0),
                    header(&format!("{label} Two"), 11, bank, 1),
                    header("EOP", 0, 0, 2),
                ]
                .concat(),
            ),
            chunk(b"pbag", words(&[0, 0, 1, 0, 3, 0])),
            chunk(b"pmod", vec![0; 10]),
            chunk(b"pgen", words(&[41, 0, 51, 12, 41, 0, 0, 0])),
            chunk(
                b"inst",
                [name("Tone"), words(&[0]), name("EOI"), words(&[1])].concat(),
            ),
            chunk(b"ibag", words(&[0, 0, 2, 0])),
            chunk(b"imod", vec![0; 10]),
            chunk(b"igen", words(&[54, 1, 53, 0, 0, 0])),
            chunk(b"shdr", [sample, name("EOS"), vec![0; 26]].concat()),
        ]
        .concat(),
    );
    chunk(
        b"RIFF",
        [b"sfbk".to_vec(), info, sample_data, parameters].concat(),
    )
}
