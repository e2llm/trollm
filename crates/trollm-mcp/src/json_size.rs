//! Count canonical compact UTF-8 JSON without retaining serialized payloads.
use anyhow::Result;
use std::io::{self, Write};

#[derive(Default)]
struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("compact JSON byte count overflow"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn compact_bytes(value: &impl serde::Serialize) -> Result<u64> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn counts_compact_utf8_escapes_and_empty_arrays_exactly() {
        for value in [json!([]), json!(["é🦀\n\"\\", {"a": [null, false, 1.25]}])] {
            assert_eq!(
                compact_bytes(&value).unwrap(),
                serde_json::to_vec(&value).unwrap().len() as u64
            );
        }
    }

    #[test]
    fn serialization_failure_is_not_reported_as_zero() {
        struct Invalid;
        impl serde::Serialize for Invalid {
            fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom(
                    "intentional serialization failure",
                ))
            }
        }
        assert!(compact_bytes(&Invalid).is_err());
    }
}
