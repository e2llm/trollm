//! Bounded, text-only snapshots. Nothing is extracted into the mapped catalog.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path},
};
use trollm_core::plan::hex_sha256;

pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_MEMBER: usize = 32 * 1024 * 1024;
pub const MAX_FILES: usize = 1024;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceFile {
    pub path: String,
    pub content: String,
}
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceRequest {
    pub files: Vec<SourceFile>,
    pub entry_flow: Option<String>,
    pub task_registry: Option<String>,
}

pub fn safe_path(value: &str) -> Result<String> {
    if value.is_empty()
        || value.len() > 1024
        || value.contains(['\\', '\0', ':'])
        || value.chars().any(char::is_control)
        || value.starts_with('/')
    {
        bail!("unsafe package-relative path");
    }
    let parts: Vec<_> = value.split('/').collect();
    if parts.len() > 32
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
        || Path::new(value)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("unsafe package-relative path");
    }
    Ok(value.into())
}

pub fn normalize(files: Vec<SourceFile>) -> Result<BTreeMap<String, String>> {
    if files.len() > MAX_FILES {
        bail!("package exceeds 1024 files");
    }
    let mut out = BTreeMap::new();
    let mut total = 0usize;
    for file in files {
        let path = safe_path(&file.path)?;
        total = total
            .checked_add(file.content.len())
            .context("package too large")?;
        if file.content.len() > MAX_MEMBER || total > MAX_BYTES {
            bail!("package exceeds size limit");
        }
        if out.insert(path, file.content).is_some() {
            bail!("duplicate package path");
        }
    }
    Ok(out)
}

pub fn archive(bytes: &[u8]) -> Result<BTreeMap<String, String>> {
    if bytes.len() > MAX_BYTES {
        bail!("archive exceeds size limit");
    }
    let mut archive = tar::Archive::new(bytes);
    let mut files = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for entry in archive.entries().context("unreadable uncompressed tar")? {
        let mut entry = entry.context("unreadable archive member")?;
        let name = std::str::from_utf8(&entry.path_bytes())?
            .trim_end_matches('/')
            .to_owned();
        let path = safe_path(&name)?;
        if !seen.insert(path.clone()) {
            bail!("duplicate archive path");
        }
        if seen.len() > MAX_FILES {
            bail!("archive exceeds entry limit");
        }
        if entry.header().entry_type().is_dir() {
            continue;
        }
        if !entry.header().entry_type().is_file() {
            bail!("archive links and special entries are forbidden");
        }
        let size = usize::try_from(entry.size())?;
        total = total.checked_add(size).context("archive too large")?;
        if size > MAX_MEMBER || total > MAX_BYTES {
            bail!("archive exceeds size limit");
        }
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .context("package files must be UTF-8 text")?;
        files.push(SourceFile { path, content });
    }
    let files = normalize(files)?;
    // Keep the install reader's accepted archive formats and limits authoritative,
    // after strict raw-path/header/duplicate checks above.
    let canonical = trollm_uniffi::tar::extract(bytes)?;
    if canonical.len() != files.len()
        || canonical.iter().any(|entry| {
            files
                .get(&entry.path)
                .is_none_or(|text| text.as_bytes() != entry.data)
        })
    {
        bail!("archive interpretation differs from the bundle reader");
    }
    Ok(files)
}

/// Open a mapped leaf without following a symlink or blocking on a FIFO.
pub fn open_regular(path: &Path) -> Result<fs::File> {
    let fd = rustix::fs::open(
        path,
        rustix::fs::OFlags::RDONLY
            | rustix::fs::OFlags::NOFOLLOW
            | rustix::fs::OFlags::NONBLOCK
            | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )?;
    let file = fs::File::from(fd);
    if !file.metadata()?.is_file() {
        bail!("mapped archive must be a regular file");
    }
    Ok(file)
}

pub fn directory(root: &Path) -> Result<BTreeMap<String, String>> {
    use rustix::fs::{Dir, Mode, OFlags};
    fn visit(
        dir: &fs::File,
        relative: &Path,
        files: &mut Vec<SourceFile>,
        total: &mut usize,
        entries: &mut usize,
    ) -> Result<()> {
        // Directory enumeration and every open remain anchored to descriptors.
        // Renames and concurrent symlink swaps cannot redirect a package read.
        for entry in Dir::read_from(dir)? {
            let entry = entry?;
            let name = entry.file_name().to_str()?;
            if name == "." || name == ".." {
                continue;
            }
            *entries += 1;
            if *entries > MAX_FILES {
                bail!("mapped package exceeds entry limit");
            }
            let relative = relative.join(name);
            let path = safe_path(relative.to_str().context("path must be UTF-8")?)?;
            let fd = rustix::fs::openat(
                dir,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            let file = fs::File::from(fd);
            let meta = file.metadata()?;
            if meta.is_dir() {
                visit(&file, &relative, files, total, entries)?;
            } else if meta.is_file() {
                let size = usize::try_from(meta.len())?;
                if size > MAX_MEMBER {
                    bail!("mapped file exceeds size limit");
                }
                let mut content = String::new();
                file.take((MAX_MEMBER + 1) as u64)
                    .read_to_string(&mut content)?;
                *total = total
                    .checked_add(content.len())
                    .context("package too large")?;
                if content.len() > MAX_MEMBER || *total > MAX_BYTES {
                    bail!("mapped package exceeds size limit");
                }
                files.push(SourceFile { path, content });
            } else {
                bail!("mapped package contains a special file");
            }
        }
        Ok(())
    }
    let fd = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let mut files = Vec::new();
    visit(
        &fs::File::from(fd),
        Path::new(""),
        &mut files,
        &mut 0,
        &mut 0,
    )?;
    normalize(files)
}

pub fn identity(files: &BTreeMap<String, String>) -> String {
    // Length-delimited identities prevent path/content concatenation collisions.
    let mut bytes = Vec::new();
    for (path, content) in files {
        bytes.extend_from_slice(&(path.len() as u64).to_be_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(hex_sha256(content.as_bytes()).as_bytes());
    }
    hex_sha256(&bytes)
}
