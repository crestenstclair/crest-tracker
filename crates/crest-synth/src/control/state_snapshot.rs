/// Canonical JSON serialization of one accepted control state.
///
/// The state projector owns serialization and must pass the complete,
/// deterministic serde_json output to StateSnapshot::new. The snapshot then
/// provides one stable fingerprint for downstream text and real-time
/// projections without exposing any mutable state.
use std::sync::{Arc, OnceLock};

#[derive(Clone, Debug)]
pub struct StateSnapshot {
    json: StateJson,
    hash: String,
}

#[derive(Clone, Debug)]
enum StateJson {
    Ready(Arc<str>),
    Generation {
        template: Arc<GenerationTemplate>,
        generation: u64,
        rendered: Arc<OnceLock<String>>,
    },
}

#[derive(Debug)]
struct GenerationTemplate {
    suffix: Arc<str>,
    suffix_hash: u64,
    suffix_factor: u64,
}

impl StateSnapshot {
    /// Captures canonical JSON and computes its deterministic content hash.
    pub fn new(json: impl Into<String>) -> Self {
        let json = json.into();
        let hash = format_hash(hash_bytes(json.as_bytes()));

        Self {
            json: StateJson::Ready(Arc::from(json)),
            hash,
        }
    }

    /// Returns the complete canonical JSON document.
    pub fn json(&self) -> &str {
        match &self.json {
            StateJson::Ready(json) => json,
            StateJson::Generation {
                template,
                generation,
                rendered,
            } => {
                rendered.get_or_init(|| format!("{{\"generation\":{generation}{}", template.suffix))
            }
        }
    }

    /// Returns the stable lowercase hexadecimal fingerprint of the JSON.
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Consumes the snapshot and returns its canonical JSON document.
    pub fn into_json(self) -> String {
        self.json().to_owned()
    }

    /// Advances a snapshot whose accepted state changed only by generation.
    ///
    /// The canonical suffix and its hash contribution are shared across the
    /// MIDI stream. Full JSON is rendered only if an observer requests it.
    pub(crate) fn with_generation(&self, generation: u64) -> Option<Self> {
        let template = match &self.json {
            StateJson::Generation { template, .. } => Arc::clone(template),
            StateJson::Ready(json) => Arc::new(GenerationTemplate::from_json(json)?),
        };
        let prefix = format!("{{\"generation\":{generation}");
        let prefix_hash = hash_bytes(prefix.as_bytes());
        let hash = prefix_hash
            .wrapping_mul(template.suffix_factor)
            .wrapping_add(template.suffix_hash);

        Some(Self {
            json: StateJson::Generation {
                template,
                generation,
                rendered: Arc::new(OnceLock::new()),
            },
            hash: format_hash(hash),
        })
    }
}

impl PartialEq for StateSnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.json() == other.json()
    }
}

impl Eq for StateSnapshot {}

impl GenerationTemplate {
    fn from_json(json: &str) -> Option<Self> {
        const PREFIX: &str = "{\"generation\":";
        let after_prefix = json.strip_prefix(PREFIX)?;
        let suffix_offset = after_prefix.find(',')?;
        let suffix = &after_prefix[suffix_offset..];
        let (suffix_hash, suffix_factor) = suffix_hash(suffix.as_bytes());
        Some(Self {
            suffix: Arc::from(suffix),
            suffix_hash,
            suffix_factor,
        })
    }
}

/// A polynomial hash gives the snapshot a stable, platform-independent content identity
/// and lets an immutable JSON suffix contribute in constant time.
///
/// This is an identity fingerprint, not a security boundary.
fn hash_bytes(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    polynomial_hash(bytes, OFFSET_BASIS).0
}

fn suffix_hash(bytes: &[u8]) -> (u64, u64) {
    polynomial_hash(bytes, 0)
}

fn polynomial_hash(bytes: &[u8], mut hash: u64) -> (u64, u64) {
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    const POWERS: [u64; 9] = {
        let mut powers = [1_u64; 9];
        let mut i = 1;
        while i < powers.len() {
            powers[i] = powers[i - 1].wrapping_mul(PRIME);
            i += 1;
        }
        powers
    };
    let mut factor = 1_u64;
    let mut chunks = bytes.chunks_exact(8);
    for chunk in &mut chunks {
        // Expand eight steps of the same polynomial modulo 2^64. Independent
        // byte contributions avoid a multiply dependency for every JSON byte;
        // hashes and suffix composition remain exactly platform-independent.
        let contribution = chunk
            .iter()
            .zip(POWERS[..8].iter().rev())
            .fold(0_u64, |sum, (byte, power)| {
                sum.wrapping_add((u64::from(*byte) + 1).wrapping_mul(*power))
            });
        hash = hash.wrapping_mul(POWERS[8]).wrapping_add(contribution);
        factor = factor.wrapping_mul(POWERS[8]);
    }
    for byte in chunks.remainder() {
        hash = hash.wrapping_mul(PRIME).wrapping_add(u64::from(*byte) + 1);
        factor = factor.wrapping_mul(PRIME);
    }
    (hash, factor)
}

fn format_hash(hash: u64) -> String {
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::{hash_bytes, suffix_hash, StateSnapshot};

    #[test]
    fn batched_hash_and_suffix_match_scalar_at_every_block_boundary() {
        let bytes = (0..4096)
            .map(|i| ((i * 73 + 13) % 256) as u8)
            .collect::<Vec<_>>();
        for offset in 0..8 {
            let mut hash = 0xcbf2_9ce4_8422_2325_u64;
            let mut suffix = 0_u64;
            let mut factor = 1_u64;
            for length in 0..bytes.len() - offset {
                let slice = &bytes[offset..offset + length];
                assert_eq!(hash_bytes(slice), hash);
                assert_eq!(suffix_hash(slice), (suffix, factor));
                let value = u64::from(bytes[offset + length]) + 1;
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3).wrapping_add(value);
                suffix = suffix
                    .wrapping_mul(0x0000_0100_0000_01b3)
                    .wrapping_add(value);
                factor = factor.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }

    #[test]
    fn retains_the_complete_canonical_json() {
        let json = r#"{"global":{"masterGainDb":-3.0},"patches":[],"selection":{"patch":0}}"#;
        let snapshot = StateSnapshot::new(json);

        assert_eq!(snapshot.json(), json);
        assert_eq!(snapshot.clone().into_json(), json);
    }

    #[test]
    fn hash_is_deterministic_and_content_sensitive() {
        let first = StateSnapshot::new(r#"{"patches":[]}"#);
        let same = StateSnapshot::new(r#"{"patches":[]}"#);
        let different = StateSnapshot::new(r#"{"patches":[{}]}"#);

        assert_eq!(first.hash(), same.hash());
        assert_ne!(first.hash(), different.hash());
        assert_eq!(first.hash().len(), 16);
        assert!(first.hash().bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn value_equality_covers_json_and_derived_hash() {
        let left = StateSnapshot::new(r#"{"channels":[]}"#);
        let right = StateSnapshot::new(r#"{"channels":[]}"#);

        assert_eq!(left, right);
    }

    #[test]
    fn generation_successor_matches_eager_canonical_json_and_hash() {
        let first = StateSnapshot::new(
            r#"{"generation":41,"capabilities":{"descriptors":[]},"patches":[]}"#,
        );
        let successor = first.with_generation(42).unwrap();
        let eager = StateSnapshot::new(
            r#"{"generation":42,"capabilities":{"descriptors":[]},"patches":[]}"#,
        );

        assert_eq!(successor.hash(), eager.hash());
        assert_eq!(successor.json(), eager.json());
        assert_eq!(successor, eager);
    }
}
