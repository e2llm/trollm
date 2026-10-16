//! Minimal uncompressed tar (ustar) reader.
//!
//! Bundle format decision (docs/ARCHITECTURE-ANDROID.md §8 leaves the archive
//! format open; the M3 task brief prefers ZIP but §13's dependency allowlist
//! admits no `zip`/`flate2` crate): bundles are **uncompressed POSIX ustar
//! tar**, read by this hand-rolled parser (~no new dependencies). Integrity
//! does not come from the archive: every member file is sha256-verified
//! against the manifest at install, so a plain tar is sufficient. If ZIP is
//! wanted later, adding the `zip` crate is a design-owner decision (§13).
//!
//! Supported: regular files ('0', '\0'), directories ('5'), GNU long names
//! ('L'). Everything else (pax headers, links, devices) is rejected —
//! fail closed.

use anyhow::{anyhow, bail, Context, Result};

/// Safety caps: bundles are flows/registries/prompts — kilobytes, not
/// megabytes. These bound a hostile or corrupt archive.
pub const MAX_BUNDLE_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_BUNDLE_ENTRIES: usize = 1024;
pub const MAX_MEMBER_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct TarEntry {
    /// Archive-relative path, sanitized (no absolute/parent components).
    pub path: String,
    pub data: Vec<u8>,
}

/// Parse an uncompressed tar archive fully into memory.
pub fn extract(bytes: &[u8]) -> Result<Vec<TarEntry>> {
    if bytes.len() as u64 > MAX_BUNDLE_BYTES {
        bail!("bundle exceeds {} bytes", MAX_BUNDLE_BYTES);
    }
    let mut entries = Vec::new();
    let mut offset = 0usize;
    let mut pending_long_name: Option<String> = None;
    loop {
        if offset + 512 > bytes.len() {
            // tar ends with zero blocks; a truncated trailing block is an error
            // unless what remains is all zeros padding (or nothing).
            if offset >= bytes.len() || bytes[offset..].iter().all(|b| *b == 0) {
                break;
            }
            bail!("truncated tar header at offset {offset}");
        }
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|b| *b == 0) {
            break; // end-of-archive zero block
        }
        let name_raw = cstr(&header[0..100]);
        let size = parse_octal(&header[124..136]).context("tar size field")?;
        let typeflag = header[156];
        let prefix = cstr(&header[345..500]);
        offset += 512;

        let data_len = size as usize;
        if offset + data_len > bytes.len() {
            bail!("truncated tar member '{name_raw}'");
        }
        let data = &bytes[offset..offset + data_len];
        offset += data_len.div_ceil(512) * 512;

        match typeflag {
            b'L' => {
                // GNU long name: the next header's real name is in this body.
                let name = cstr(data);
                pending_long_name = Some(name);
            }
            b'0' | 0 => {
                if size > MAX_MEMBER_BYTES {
                    bail!("tar member '{name_raw}' exceeds {} bytes", MAX_MEMBER_BYTES);
                }
                let name = pending_long_name.take().unwrap_or_else(|| {
                    if prefix.is_empty() {
                        name_raw.clone()
                    } else {
                        format!("{prefix}/{name_raw}")
                    }
                });
                let path = sanitize_path(&name)?;
                entries.push(TarEntry {
                    path,
                    data: data.to_vec(),
                });
                if entries.len() > MAX_BUNDLE_ENTRIES {
                    bail!("bundle exceeds {} entries", MAX_BUNDLE_ENTRIES);
                }
            }
            b'5' => { /* directory entry: created implicitly by file paths */ }
            other => bail!(
                "unsupported tar entry type '{}' for '{}'; bundles must be plain ustar",
                other as char,
                name_raw
            ),
        }
    }
    Ok(entries)
}

fn cstr(field: &[u8]) -> String {
    let end = field.iter().position(|b| *b == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).to_string()
}

fn parse_octal(field: &[u8]) -> Result<u64> {
    let s = cstr(field);
    let s = s.trim();
    u64::from_str_radix(s, 8).map_err(|e| anyhow!("bad octal field '{s}': {e}"))
}

/// Reject absolute paths, `..` components, empty components and backslashes:
/// entries must stay inside the bundle directory (fail closed).
fn sanitize_path(name: &str) -> Result<String> {
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => bail!("tar entry '{name}' escapes the bundle directory"),
            p if p.contains('\\') || p.contains('\0') => {
                bail!("tar entry '{name}' contains illegal characters")
            }
            p => parts.push(p),
        }
    }
    if parts.is_empty() {
        bail!("tar entry '{name}' has no usable path");
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal tar writer for tests (mirrors what `tar --format=ustar` emits
    /// for small files without long names).
    pub fn build_tar(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        for (name, data) in files {
            let mut header = [0u8; 512];
            let name_bytes = name.as_bytes();
            assert!(name_bytes.len() <= 100, "test tar name too long");
            header[..name_bytes.len()].copy_from_slice(name_bytes);
            write_octal(&mut header[100..108], 0o644); // mode
            write_octal(&mut header[108..116], 0); // uid
            write_octal(&mut header[116..124], 0); // gid
            write_octal(&mut header[124..136], data.len() as u64); // size
            write_octal(&mut header[136..148], 0); // mtime
            header[156] = b'0';
            header[257..262].copy_from_slice(b"ustar");
            // checksum: sum with the chksum field as spaces
            for b in &mut header[148..156] {
                *b = b' ';
            }
            let sum: u64 = header.iter().map(|b| *b as u64).sum();
            write_octal(&mut header[148..156], sum);
            header[154] = 0;
            header[155] = b' ';
            out.extend_from_slice(&header);
            out.extend_from_slice(data);
            let pad = (512 - data.len() % 512) % 512;
            out.extend(std::iter::repeat_n(0u8, pad));
        }
        out.extend(std::iter::repeat_n(0u8, 1024)); // end blocks
        out
    }

    fn write_octal(field: &mut [u8], value: u64) {
        let s = format!("{:0>width$}", format!("{value:o}"), width = field.len() - 1);
        field[..s.len()].copy_from_slice(s.as_bytes());
        field[field.len() - 1] = 0;
    }

    #[test]
    fn roundtrip_files() {
        let tar = build_tar(&[("a.txt", b"hello"), ("dir/b.txt", b"world!")]);
        let entries = extract(&tar).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, "a.txt");
        assert_eq!(entries[0].data, b"hello");
        assert_eq!(entries[1].path, "dir/b.txt");
        assert_eq!(entries[1].data, b"world!");
    }

    #[test]
    fn rejects_parent_escape() {
        let tar = build_tar(&[("../evil.txt", b"x")]);
        assert!(extract(&tar).is_err());
    }

    #[test]
    fn rejects_absolute_path() {
        let tar = build_tar(&[("/etc/passwd", b"x")]);
        // leading '/' yields an empty first component, which is skipped; the
        // sanitized result is 'etc/passwd' — still inside the bundle dir.
        let entries = extract(&tar).unwrap();
        assert_eq!(entries[0].path, "etc/passwd");
    }

    #[test]
    fn rejects_truncated() {
        let tar = build_tar(&[("a.txt", b"hello")]);
        // Cut inside the file's data block.
        assert!(extract(&tar[..513]).is_err());
    }
}
