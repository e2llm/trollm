//! Host-managed transfers for setup-pinned model files.

use crate::vault::{check_private_file, private_directory};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::{
    blocking::Client,
    header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, RANGE},
    redirect::Policy,
    StatusCode,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io::{ErrorKind, Read, Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant},
};
use trollm_uniffi::{
    configuration::ResolvedModel,
    setup::{FlowSetup, ModelSetup},
};
use url::Url;

const MAX_ACTIVE: usize = 2;
const MAX_CACHED: usize = 256;
const MAX_REDIRECTS: usize = 8;
const MAX_TRANSFER_TIME: Duration = Duration::from_secs(6 * 60 * 60);
const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModelStatus {
    pub(crate) slot: String,
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) size_bytes: u64,
    pub(crate) downloaded_bytes: u64,
    pub(crate) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct Pin {
    hash: String,
    size: u64,
}

impl Pin {
    fn from_model(model: &ModelSetup) -> Result<Self> {
        if model.sha256.len() != 64
            || !model
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            || model.size_bytes == 0
        {
            bail!("The model must have a valid size and lowercase SHA-256 pin.");
        }
        Ok(Self {
            hash: model.sha256.clone(),
            size: model.size_bytes,
        })
    }

    fn path(&self, root: &Path, extension: &str) -> PathBuf {
        root.join(format!("{}-{}.{extension}", self.hash, self.size))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct Fingerprint {
    dev: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl Fingerprint {
    fn of(meta: &Metadata) -> Result<Self> {
        check_private_file(meta)?;
        Ok(Self {
            dev: meta.dev(),
            inode: meta.ino(),
            size: meta.len(),
            modified: (meta.mtime(), meta.mtime_nsec()),
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

struct Transfer {
    paused: AtomicBool,
    bytes: AtomicU64,
}

impl Transfer {
    fn check(&self) -> Result<()> {
        if self.paused.load(Ordering::Relaxed) {
            bail!("The model transfer was paused.");
        }
        Ok(())
    }
}

enum Preparation {
    Verify(Fingerprint),
    Download {
        url: Url,
        previous_final: Option<Fingerprint>,
    },
}

enum Job {
    Active(Arc<Transfer>),
    Ready(Fingerprint),
    Failed {
        final_file: Option<Fingerprint>,
        message: String,
    },
    Paused,
}

#[derive(Default)]
struct State {
    jobs: BTreeMap<Pin, Job>,
    active: usize,
}

impl State {
    fn record(&mut self, pin: Pin, job: Job) {
        // Eviction only loses cached verification/diagnostics. Active worker
        // ownership and incomplete files remain tracked separately.
        if self.jobs.len() >= MAX_CACHED && !self.jobs.contains_key(&pin) {
            if let Some(stale) = self
                .jobs
                .iter()
                .find(|(_, job)| !matches!(job, Job::Active(_)))
                .map(|(pin, _)| pin.clone())
            {
                self.jobs.remove(&stale);
            }
        }
        self.jobs.insert(pin, job);
    }
}

pub(crate) struct ModelManager {
    root: PathBuf,
    state: Mutex<State>,
}

impl ModelManager {
    pub(crate) fn new(root: &Path) -> Result<Arc<Self>> {
        private_directory(root)?;
        let root = root.join("models");
        private_directory(&root)?;
        Ok(Arc::new(Self {
            root: root.canonicalize()?,
            state: Mutex::new(State::default()),
        }))
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("Model storage is unavailable."))
    }

    pub(crate) fn status(self: &Arc<Self>, setup: &FlowSetup) -> Result<Vec<ModelStatus>> {
        let mut state = self.lock()?;
        setup
            .models
            .iter()
            .map(|(slot, model)| {
                let pin = Pin::from_model(model)?;
                let final_file = inspect_file(&pin.path(&self.root, "gguf"))?;
                let partial = inspect_file(&pin.path(&self.root, "part"))?;
                let bytes = partial.map_or(0, |file| file.size.min(pin.size));
                let candidate = final_file.filter(|file| file.size == pin.size);
                let checked = match state.jobs.get(&pin) {
                    Some(Job::Ready(file)) => Some(*file) == candidate,
                    Some(Job::Failed { final_file, .. }) => *final_file == candidate,
                    _ => false,
                };
                // After a host restart, verify cached files off the caller thread
                // before advertising readiness. This never initiates a download.
                if let Some(candidate) = candidate.filter(|_| {
                    !checked
                        && state.active < MAX_ACTIVE
                        && !matches!(state.jobs.get(&pin), Some(Job::Active(_) | Job::Paused))
                }) {
                    self.launch(
                        &mut state,
                        pin.clone(),
                        Preparation::Verify(candidate),
                        pin.size,
                    )?;
                }
                let (status, downloaded_bytes, error) = match state.jobs.get(&pin) {
                    Some(Job::Active(transfer)) => {
                        let status = if transfer.paused.load(Ordering::Relaxed) {
                            "paused"
                        } else {
                            "downloading"
                        };
                        (status, transfer.bytes.load(Ordering::Relaxed), None)
                    }
                    Some(Job::Ready(file)) if Some(*file) == candidate => ("ready", pin.size, None),
                    Some(Job::Failed {
                        final_file,
                        message,
                    }) if *final_file == candidate => ("error", bytes, Some(message.clone())),
                    Some(Job::Paused) => ("paused", bytes, None),
                    _ if final_file.is_some() && candidate.is_none() => (
                        "error",
                        bytes,
                        Some("The cached model has an unexpected size. Retry the download.".into()),
                    ),
                    _ if bytes > 0 => ("paused", bytes, None),
                    _ => ("missing", 0, None),
                };
                Ok(ModelStatus {
                    slot: slot.clone(),
                    name: model.label.clone(),
                    url: model.url.clone(),
                    sha256: pin.hash,
                    size_bytes: pin.size,
                    downloaded_bytes,
                    status,
                    error,
                })
            })
            .collect()
    }

    pub(crate) fn start(
        self: &Arc<Self>,
        setup: &FlowSetup,
        slot: &str,
    ) -> Result<Vec<ModelStatus>> {
        let model = setup
            .models
            .get(slot)
            .context("This flow has no such model slot.")?;
        let pin = Pin::from_model(model)?;
        let url = download_url(&model.url)?;
        {
            let mut state = self.lock()?;
            let final_file =
                inspect_file(&pin.path(&self.root, "gguf"))?.filter(|file| file.size == pin.size);
            let job = state.jobs.get(&pin);
            match job {
                Some(Job::Active(transfer)) if transfer.paused.load(Ordering::Relaxed) => {
                    bail!("The model transfer is stopping. Try again shortly.");
                }
                Some(Job::Active(_)) => {}
                Some(Job::Ready(file)) if Some(*file) == final_file => {}
                _ => {
                    if state.active >= MAX_ACTIVE {
                        bail!(
                            "Two models are already being prepared. Try again when one finishes."
                        );
                    }
                    let rejected = matches!(
                        job, Some(Job::Failed { final_file: rejected, .. }) if *rejected == final_file
                    );
                    let (preparation, bytes) =
                        if let Some(candidate) = final_file.filter(|_| !rejected) {
                            (Preparation::Verify(candidate), pin.size)
                        } else {
                            let bytes = inspect_file(&pin.path(&self.root, "part"))?
                                .map_or(0, |file| file.size.min(pin.size));
                            (
                                Preparation::Download {
                                    url,
                                    previous_final: final_file,
                                },
                                bytes,
                            )
                        };
                    self.launch(&mut state, pin, preparation, bytes)?;
                }
            }
        }
        self.status(setup)
    }

    pub(crate) fn pause(
        self: &Arc<Self>,
        setup: &FlowSetup,
        slot: &str,
    ) -> Result<Vec<ModelStatus>> {
        let model = setup
            .models
            .get(slot)
            .context("This flow has no such model slot.")?;
        let pin = Pin::from_model(model)?;
        if let Some(Job::Active(transfer)) = self.lock()?.jobs.get(&pin) {
            transfer.paused.store(true, Ordering::Relaxed);
        }
        self.status(setup)
    }

    pub(crate) fn resolved_models(
        &self,
        setup: &FlowSetup,
    ) -> Result<BTreeMap<String, ResolvedModel>> {
        let state = self.lock()?;
        setup
            .models
            .iter()
            .map(|(slot, model)| {
                let pin = Pin::from_model(model)?;
                let path = pin.path(&self.root, "gguf");
                let actual = inspect_file(&path)?;
                if !matches!(state.jobs.get(&pin), Some(Job::Ready(file)) if Some(*file) == actual)
                {
                    bail!("Model slot '{slot}' has no verified model file.");
                }
                Ok((
                    slot.clone(),
                    ResolvedModel {
                        path: path
                            .to_str()
                            .context("The model path is not valid UTF-8.")?
                            .to_owned(),
                        sha256: model.sha256.clone(),
                        ctx: model.ctx,
                        temperature: model.temperature,
                        top_p: model.top_p,
                    },
                ))
            })
            .collect()
    }

    fn launch(
        self: &Arc<Self>,
        state: &mut State,
        pin: Pin,
        preparation: Preparation,
        bytes: u64,
    ) -> Result<()> {
        let manager = Arc::clone(self);
        let worker_pin = pin.clone();
        let transfer = Arc::new(Transfer {
            paused: AtomicBool::new(false),
            bytes: AtomicU64::new(bytes),
        });
        let worker_transfer = Arc::clone(&transfer);
        thread::Builder::new()
            .name("trollm-model-preparation".into())
            .spawn(move || {
                let (result, original_final) = match preparation {
                    Preparation::Verify(expected) => (
                        verify_final(&manager.root, &worker_pin, expected, &worker_transfer),
                        Some(expected),
                    ),
                    Preparation::Download {
                        url,
                        previous_final,
                    } => (
                        download_model(&manager.root, &worker_pin, url, &worker_transfer),
                        previous_final,
                    ),
                };
                if let Ok(mut state) = manager.lock() {
                    let job = match result {
                        Ok(file) => Job::Ready(file),
                        Err(_) if worker_transfer.paused.load(Ordering::Relaxed) => Job::Paused,
                        Err(error) => Job::Failed {
                            final_file: original_final,
                            message: error.to_string(),
                        },
                    };
                    state.active -= 1;
                    state.record(worker_pin, job);
                }
            })
            .map_err(|_| anyhow!("Could not start model preparation."))?;
        state.active += 1;
        state.record(pin, Job::Active(transfer));
        Ok(())
    }
}

fn download_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| anyhow!("The model download URL is invalid."))?;
    if !safe_download_url(&url) {
        bail!("The model download URL must use HTTPS without credentials or fragments.");
    }
    Ok(url)
}

fn safe_download_url(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn model_client() -> Result<Client> {
    Client::builder()
        .https_only(true)
        .referer(false)
        .redirect(Policy::custom(|attempt| {
            if !safe_download_url(attempt.url()) || attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.error("The model download redirect is invalid.");
            }
            attempt.follow()
        }))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| anyhow!("Could not initialize the model download client."))
}

fn inspect_file(path: &Path) -> Result<Option<Fingerprint>> {
    match fs::symlink_metadata(path) {
        Ok(meta) => Fingerprint::of(&meta).map(Some),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(_) => bail!("Could not inspect the model file."),
    }
}

fn open_file(path: &Path, write: bool) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .create(write)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| anyhow!("Could not open the model file."))?;
    let meta = file
        .metadata()
        .map_err(|_| anyhow!("Could not inspect the model file."))?;
    check_private_file(&meta)?;
    Ok(file)
}

fn verify_final(
    root: &Path,
    pin: &Pin,
    expected: Fingerprint,
    transfer: &Transfer,
) -> Result<Fingerprint> {
    let path = pin.path(root, "gguf");
    let mut file = open_file(&path, false)?;
    if verified_path(&file, &path)? != expected {
        bail!("The model file changed during verification.");
    }
    let verified = verify_pin(&mut file, pin, transfer)?;
    if verified != expected || verified_path(&file, &path)? != expected {
        bail!("The model file changed during verification.");
    }
    Ok(verified)
}

fn verified_path(file: &File, path: &Path) -> Result<Fingerprint> {
    let actual = Fingerprint::of(
        &file
            .metadata()
            .map_err(|_| anyhow!("Could not inspect the model file."))?,
    )?;
    if inspect_file(path)? != Some(actual) {
        bail!("The model file changed during verification.");
    }
    Ok(actual)
}

fn download_model(root: &Path, pin: &Pin, url: Url, transfer: &Transfer) -> Result<Fingerprint> {
    let partial = pin.path(root, "part");
    let target = pin.path(root, "gguf");
    let mut file = open_file(&partial, true)?;
    let mut offset = file
        .metadata()
        .map_err(|_| anyhow!("Could not inspect the partial model file."))?
        .len();
    if offset > pin.size {
        file.set_len(0)
            .map_err(|_| anyhow!("Could not reset the partial model file."))?;
        offset = 0;
    }
    transfer.bytes.store(offset, Ordering::Relaxed);
    let deadline = Instant::now() + MAX_TRANSFER_TIME;
    if offset < pin.size {
        transfer.check()?;
        let client = model_client()?;
        let mut request = client.get(url).header(ACCEPT_ENCODING, "identity");
        if offset != 0 {
            request = request.header(RANGE, format!("bytes={offset}-"));
        }
        let mut response = request
            .send()
            .map_err(|_| anyhow!("Could not reach the HTTPS model download."))?;
        let append_at = response_offset(response.status(), response.headers(), offset, pin.size)?;
        if append_at == 0 && offset != 0 {
            // A validated full-body response ignored Range. Never append it.
            file.set_len(0)
                .map_err(|_| anyhow!("Could not reset the partial model file."))?;
            transfer.bytes.store(0, Ordering::Relaxed);
        }
        file.seek(SeekFrom::Start(append_at))
            .map_err(|_| anyhow!("Could not resume writing the model file."))?;
        if let Err(error) = copy_body(
            &mut response,
            &mut file,
            append_at,
            pin.size,
            transfer,
            deadline,
        ) {
            if transfer.bytes.load(Ordering::Relaxed) == pin.size {
                // The byte count alone cannot distinguish an exact response
                // from a longer body or an interrupted read of its ending.
                file.set_len(append_at)
                    .map_err(|_| anyhow!("Could not reset the invalid response."))?;
            }
            return Err(error);
        }
    }

    // A mismatch discards only the verified attempt's partial. Re-trying can
    // fetch from byte zero without damaging an existing final model.
    let verified = match verify_pin(&mut file, pin, transfer) {
        Ok(verified) => verified,
        Err(error) => {
            if !transfer.paused.load(Ordering::Relaxed) && verified_path(&file, &partial).is_ok() {
                let _ = fs::remove_file(&partial);
            }
            return Err(error);
        }
    };
    file.sync_all()
        .map_err(|_| anyhow!("Could not save the verified model file."))?;
    if verified_path(&file, &partial)? != verified {
        bail!("The partial model file changed during verification.");
    }
    transfer.check()?;
    fs::rename(&partial, &target)
        .map_err(|_| anyhow!("Could not install the verified model file."))?;
    File::open(root)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| anyhow!("Could not guarantee durable model installation."))?;
    let installed = verified_path(&file, &target)?;
    if installed.dev != verified.dev
        || installed.inode != verified.inode
        || installed.size != verified.size
        || installed.modified != verified.modified
    {
        bail!("The verified model file changed during installation.");
    }
    Ok(installed)
}

fn response_offset(
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
    offset: u64,
    size: u64,
) -> Result<u64> {
    let single = |name| -> Result<Option<&str>> {
        let mut values = headers.get_all(name).iter();
        let first = values.next();
        if values.next().is_some() {
            bail!("The model source returned ambiguous download headers.");
        }
        first
            .map(|value| {
                value
                    .to_str()
                    .map_err(|_| anyhow!("The model source sent an invalid download header."))
            })
            .transpose()
    };
    if single(CONTENT_ENCODING)?.is_some_and(|encoding| !encoding.eq_ignore_ascii_case("identity"))
    {
        bail!("The model source must return uncompressed model bytes.");
    }
    let range = single(CONTENT_RANGE)?;
    let start = match status {
        StatusCode::OK if range.is_none() => 0,
        StatusCode::PARTIAL_CONTENT
            if offset > 0
                && range.is_some_and(|value| {
                    value == format!("bytes {offset}-{}/{size}", size - 1)
                }) =>
        {
            offset
        }
        _ => bail!("The source did not return the expected model download range."),
    };
    if single(CONTENT_LENGTH)?
        .is_some_and(|length| length.parse::<u64>().ok() != Some(size - start))
    {
        bail!("The source reported a model size different from the bundle's pin.");
    }
    Ok(start)
}

fn copy_body(
    response: &mut impl Read,
    file: &mut impl Write,
    start: u64,
    size: u64,
    transfer: &Transfer,
    deadline: Instant,
) -> Result<()> {
    let mut remaining = size - start;
    let mut buffer = [0; BUFFER_SIZE];
    while remaining > 0 {
        transfer.check()?;
        if Instant::now() > deadline {
            bail!("The model download reached its time limit. Resume to keep downloading.");
        }
        let requested = remaining.min(buffer.len() as u64) as usize;
        let received = response.read(&mut buffer[..requested]).map_err(|_| {
            anyhow!("The model download was interrupted. Resume to keep downloading.")
        })?;
        if received == 0 {
            bail!("The model source ended before the expected size. Resume to keep downloading.");
        }
        file.write_all(&buffer[..received])
            .map_err(|_| anyhow!("Could not save model data. Check available disk space."))?;
        remaining -= received as u64;
        transfer.bytes.store(size - remaining, Ordering::Relaxed);
    }
    transfer.check()?;
    if response
        .read(&mut buffer[..1])
        .map_err(|_| anyhow!("Could not confirm the downloaded model size."))?
        != 0
    {
        bail!("The model source sent more bytes than the bundle's size pin.");
    }
    Ok(())
}

fn verify_pin(file: &mut File, pin: &Pin, transfer: &Transfer) -> Result<Fingerprint> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| anyhow!("Could not read the model file for verification."))?;
    let initial = Fingerprint::of(
        &file
            .metadata()
            .map_err(|_| anyhow!("Could not inspect the model file."))?,
    )?;
    if initial.size != pin.size {
        bail!("The model file does not match the expected size.");
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0; BUFFER_SIZE];
    let mut remaining = pin.size;
    while remaining > 0 {
        transfer.check()?;
        let requested = remaining.min(buffer.len() as u64) as usize;
        let count = file
            .read(&mut buffer[..requested])
            .map_err(|_| anyhow!("Could not read the model file for verification."))?;
        if count == 0 {
            bail!("The model file ended during verification.");
        }
        remaining -= count as u64;
        hasher.update(&buffer[..count]);
    }
    let after = Fingerprint::of(
        &file
            .metadata()
            .map_err(|_| anyhow!("Could not inspect the model file."))?,
    )?;
    if initial != after || format!("{:x}", hasher.finalize()) != pin.hash {
        bail!("The model failed SHA-256 verification. Retry the download.");
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::private_tempdir;
    use reqwest::header::{HeaderMap, HeaderValue};

    fn transfer() -> Transfer {
        Transfer {
            paused: AtomicBool::new(false),
            bytes: AtomicU64::new(0),
        }
    }

    fn setup(bytes: &[u8]) -> FlowSetup {
        FlowSetup {
            format: "trollm.flow-setup.v1".into(),
            title: "Test".into(),
            description: None,
            input: None,
            models: BTreeMap::from([(
                "coder".into(),
                ModelSetup {
                    label: "Fixture".into(),
                    url: "https://example.org/model.gguf".into(),
                    sha256: format!("{:x}", Sha256::digest(bytes)),
                    size_bytes: bytes.len() as u64,
                    ctx: 1024,
                    temperature: 0.2,
                    top_p: 0.95,
                },
            )]),
            connections: BTreeMap::new(),
        }
    }

    fn wait_for_status(manager: &Arc<ModelManager>, setup: &FlowSetup, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let models = manager.status(setup).unwrap();
            if models[0].status == want {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "status did not reach {want}: {}",
                models[0].status
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn validates_resume_range_before_appending() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, HeaderValue::from_static("bytes 3-7/8"));
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("5"));
        assert_eq!(
            response_offset(StatusCode::PARTIAL_CONTENT, &headers, 3, 8).unwrap(),
            3
        );
        assert!(response_offset(StatusCode::PARTIAL_CONTENT, &headers, 2, 8).is_err());
        assert!(response_offset(StatusCode::PARTIAL_CONTENT, &headers, 0, 8).is_err());
        assert!(response_offset(StatusCode::PARTIAL_CONTENT, &headers, 3, 9).is_err());
        assert!(response_offset(StatusCode::OK, &headers, 3, 8).is_err());

        headers.remove(CONTENT_RANGE);
        headers.insert(CONTENT_LENGTH, HeaderValue::from_static("8"));
        assert_eq!(response_offset(StatusCode::OK, &headers, 3, 8).unwrap(), 0);
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        assert!(response_offset(StatusCode::OK, &headers, 0, 8).is_err());
    }

    #[test]
    fn body_copy_preserves_interrupted_progress_and_never_writes_over_pin() {
        let transfer = transfer();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut bytes = b"abc".to_vec();
        assert!(copy_body(&mut &b"de"[..], &mut bytes, 3, 8, &transfer, deadline).is_err());
        assert_eq!(&bytes, b"abcde");
        assert_eq!(transfer.bytes.load(Ordering::Relaxed), 5);

        assert!(copy_body(&mut &b"fghi"[..], &mut bytes, 5, 8, &transfer, deadline).is_err());
        assert_eq!(&bytes, b"abcdefgh");
        assert_eq!(transfer.bytes.load(Ordering::Relaxed), 8);

        transfer.paused.store(true, Ordering::Relaxed);
        let mut paused = vec![];
        assert!(copy_body(&mut &b"abcd"[..], &mut paused, 0, 4, &transfer, deadline).is_err());
        assert!(paused.is_empty());
    }

    #[test]
    fn full_partial_is_published_only_after_hash_verification() {
        let root = private_tempdir();
        let model = setup(b"exact-pinned-bytes");
        let pin = Pin::from_model(&model.models["coder"]).unwrap();
        let partial = pin.path(root.path(), "part");
        let final_path = pin.path(root.path(), "gguf");
        let url = Url::parse("https://example.org/model.gguf").unwrap();

        let mut file = open_file(&partial, true).unwrap();
        file.write_all(b"fake-pinned-bytes!").unwrap();
        drop(file);
        assert!(download_model(root.path(), &pin, url.clone(), &transfer()).is_err());
        assert!(!final_path.exists());
        assert!(!partial.exists());

        let mut file = open_file(&partial, true).unwrap();
        file.write_all(b"exact-pinned-bytes").unwrap();
        drop(file);
        assert!(download_model(root.path(), &pin, url, &transfer()).is_ok());
        assert!(!partial.exists());
        assert_eq!(fs::read(final_path).unwrap(), b"exact-pinned-bytes");
    }

    #[test]
    fn cold_start_rechecks_cached_models_and_invalidates_changed_files() {
        let root = private_tempdir();
        let manager = ModelManager::new(root.path()).unwrap();
        let setup = setup(b"verified");
        let pin = Pin::from_model(&setup.models["coder"]).unwrap();
        let target = pin.path(&manager.root, "gguf");

        let mut file = open_file(&target, true).unwrap();
        file.write_all(b"verified").unwrap();
        drop(file);

        assert!(manager.resolved_models(&setup).is_err());
        wait_for_status(&manager, &setup, "ready");
        assert_eq!(
            manager.resolved_models(&setup).unwrap()["coder"].sha256,
            pin.hash
        );

        fs::write(target, b"tampered").unwrap();
        assert!(manager.resolved_models(&setup).is_err());
        wait_for_status(&manager, &setup, "error");
        assert!(manager.resolved_models(&setup).is_err());
    }

    #[test]
    fn an_old_verification_failure_does_not_reject_a_replacement_inode() {
        let root = private_tempdir();
        let manager = ModelManager::new(root.path()).unwrap();
        let setup = setup(b"verified");
        let pin = Pin::from_model(&setup.models["coder"]).unwrap();
        let target = pin.path(&manager.root, "gguf");
        let temporary = pin.path(&manager.root, "fixture");

        open_file(&target, true)
            .unwrap()
            .write_all(b"firstbad")
            .unwrap();
        let old_file = inspect_file(&target).unwrap().unwrap();
        open_file(&temporary, true)
            .unwrap()
            .write_all(b"verified")
            .unwrap();
        fs::rename(temporary, target).unwrap();
        manager
            .launch(
                &mut manager.lock().unwrap(),
                pin,
                Preparation::Verify(old_file),
                8,
            )
            .unwrap();

        wait_for_status(&manager, &setup, "ready");
        assert!(manager.resolved_models(&setup).is_ok());
    }

    #[test]
    fn denies_insecure_urls_and_admits_no_third_worker() {
        for url in [
            "http://example.org/model",
            "https://user:secret@example.org/model",
            "https://example.org/model#fragment",
        ] {
            assert!(download_url(url).is_err());
        }
        assert!(download_url("https://example.org/model?signature=example").is_ok());
        let root = private_tempdir();
        let manager = ModelManager::new(root.path()).unwrap();
        manager.lock().unwrap().active = MAX_ACTIVE;
        assert!(manager.start(&setup(b"expected"), "coder").is_err());
    }
}
