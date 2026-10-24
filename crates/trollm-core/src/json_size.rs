//! Compact JSON size accounting without allocating a serialized copy.

pub(crate) fn compact_json_bytes(value: &serde_json::Value) -> serde_json::Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(bytes.len())
                .ok_or_else(|| std::io::Error::other("compact JSON byte count overflow"))?;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_encoded_utf8_and_escaping() {
        for value in [
            serde_json::json!(null),
            serde_json::json!({
                "unicode": "עברית 🦀", "escaped": "\"\\\n\0", "nested": [null, 42, true]
            }),
        ] {
            assert_eq!(
                super::compact_json_bytes(&value).unwrap(),
                serde_json::to_vec(&value).unwrap().len()
            );
        }
    }
}
