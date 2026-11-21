//! Passphrase-protected, host-local secrets. The authenticated format is
//! magic/version, 16 random salt bytes, 12 random nonce bytes, then ciphertext.

use anyhow::{anyhow, bail, Context, Result};
use ring::{
    aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM},
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fs::{self, DirBuilder, File, Metadata, OpenOptions},
    io::{ErrorKind, Read, Write},
    num::NonZeroU32,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use uuid::Uuid;
use zeroize::Zeroizing;

const MAGIC: &[u8; 8] = b"TRLMVLT1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN;
const MAX_PAYLOAD: usize = 1024 * 1024;
const KDF_ROUNDS: NonZeroU32 = NonZeroU32::new(600_000).unwrap();

pub(crate) struct Vault<T> {
    dir: PathBuf,
    initialized: bool,
    unlocked: Option<Unlocked<T>>,
}

struct Unlocked<T> {
    salt: [u8; SALT_LEN],
    key: LessSafeKey,
    entries: T,
}

impl<T: Clone + Default + Serialize + DeserializeOwned> Vault<T> {
    pub(crate) fn open(root: &Path) -> Result<Self> {
        private_directory(root)?;
        let dir = root.join("credentials");
        private_directory(&dir)?;
        let initialized = match fs::symlink_metadata(vault_path(&dir)) {
            Ok(meta) => {
                check_private_file(&meta)?;
                true
            }
            Err(error) if error.kind() == ErrorKind::NotFound => false,
            Err(_) => bail!("Credential storage is unavailable."),
        };
        Ok(Self {
            dir,
            initialized,
            unlocked: None,
        })
    }

    pub(crate) fn initialized(&self) -> bool {
        self.initialized
    }

    pub(crate) fn unlock(&mut self, passphrase: &str) -> Result<()> {
        if passphrase.is_empty() || passphrase.len() > 1024 {
            bail!("Enter a passphrase of 1 to 1024 bytes.");
        }

        if self.initialized {
            let unlocked = self.read(passphrase).map_err(|_| {
                anyhow!("The passphrase is incorrect or the credential vault is damaged.")
            })?;
            self.unlocked = Some(unlocked);
            return Ok(());
        }

        let salt = random_array()?;
        let unlocked = Unlocked {
            key: key(passphrase, &salt)?,
            salt,
            entries: T::default(),
        };
        self.persist(&unlocked, &unlocked.entries)?;
        self.initialized = true;
        self.unlocked = Some(unlocked);
        self.sync_directory()
    }

    pub(crate) fn entries(&self) -> Result<&T> {
        self.unlocked
            .as_ref()
            .map(|unlocked| &unlocked.entries)
            .context("The credential vault is locked.")
    }

    pub(crate) fn update(&mut self, update: impl FnOnce(&mut T) -> Result<()>) -> Result<()> {
        let unlocked = self
            .unlocked
            .as_ref()
            .context("The credential vault is locked.")?;
        let mut entries = unlocked.entries.clone();
        update(&mut entries)?;
        self.persist(unlocked, &entries)?;
        // Rename committed this replacement. Keep memory consistent with the
        // visible file even if the directory cannot guarantee crash durability.
        self.unlocked.as_mut().expect("vault is unlocked").entries = entries;
        self.sync_directory()
    }

    fn read(&self, passphrase: &str) -> Result<Unlocked<T>> {
        check_private_directory(&self.dir)?;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(vault_path(&self.dir))?;
        check_private_file(&file.metadata()?)?;
        let max = HEADER_LEN + MAX_PAYLOAD + AES_256_GCM.tag_len();
        let mut file_data = Zeroizing::new(Vec::new());
        file.take(max as u64 + 1).read_to_end(&mut file_data)?;
        if file_data.len() < HEADER_LEN + AES_256_GCM.tag_len()
            || file_data.len() > max
            || !file_data.starts_with(MAGIC)
        {
            bail!("Invalid credential vault.");
        }
        let (header, ciphertext) = file_data.split_at_mut(HEADER_LEN);
        let salt = header[MAGIC.len()..MAGIC.len() + SALT_LEN].try_into()?;
        let nonce = header[HEADER_LEN - NONCE_LEN..].try_into()?;
        let key = key(passphrase, &salt)?;
        let plaintext = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(header),
                ciphertext,
            )
            .map_err(|_| anyhow!("Invalid credential vault."))?;
        let entries =
            serde_json::from_slice(plaintext).map_err(|_| anyhow!("Invalid credential vault."))?;
        Ok(Unlocked { salt, key, entries })
    }

    fn persist(&self, unlocked: &Unlocked<T>, entries: &T) -> Result<()> {
        check_private_directory(&self.dir)?;
        let nonce = random_array()?;
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&unlocked.salt);
        header.extend_from_slice(&nonce);

        let mut ciphertext = Zeroizing::new(
            serde_json::to_vec(entries)
                .map_err(|_| anyhow!("Credential storage is unavailable."))?,
        );
        if ciphertext.len() > MAX_PAYLOAD {
            bail!("The credential vault is full.");
        }
        unlocked
            .key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&header),
                &mut *ciphertext,
            )
            .map_err(|_| anyhow!("Credential storage is unavailable."))?;

        let path = self.dir.join(format!(".vault-{}.tmp", Uuid::new_v4()));
        let mut file = OpenOptions::new()
            .mode(0o600)
            .create_new(true)
            .write(true)
            .open(&path)
            .map_err(|_| anyhow!("Credential storage is unavailable."))?;
        let written = (|| -> std::io::Result<()> {
            file.write_all(&header)?;
            file.write_all(&ciphertext)?;
            file.sync_all()?;
            fs::rename(&path, vault_path(&self.dir))
        })();

        if written.is_err() {
            let _ = fs::remove_file(path);
            bail!("Credential storage is unavailable.");
        }
        Ok(())
    }

    fn sync_directory(&self) -> Result<()> {
        File::open(&self.dir)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| anyhow!("Credential storage could not guarantee durable saving."))
    }
}

fn vault_path(dir: &Path) -> PathBuf {
    dir.join("vault.bin")
}

fn key(passphrase: &str, salt: &[u8; SALT_LEN]) -> Result<LessSafeKey> {
    let mut bytes = Zeroizing::new([0_u8; 32]);
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        KDF_ROUNDS,
        salt,
        passphrase.as_bytes(),
        &mut *bytes,
    );
    UnboundKey::new(&AES_256_GCM, &*bytes)
        .map(LessSafeKey::new)
        .map_err(|_| anyhow!("Credential storage is unavailable."))
}

pub(crate) fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow!("Secure random generation is unavailable."))?;
    Ok(bytes)
}

pub(crate) fn private_directory(dir: &Path) -> Result<()> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    check_private_directory(dir)
}

fn check_private_directory(dir: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(dir)?;
    if !meta.is_dir() || !owner_only(&meta) {
        bail!("Storage must be a directory accessible only to the host user.");
    }
    Ok(())
}

pub(crate) fn check_private_file(meta: &Metadata) -> Result<()> {
    if !meta.is_file() || !owner_only(meta) || meta.nlink() != 1 {
        bail!("Storage must use regular files accessible only to the host user.");
    }
    Ok(())
}

fn owner_only(meta: &Metadata) -> bool {
    // geteuid has no arguments and reads only the effective process identity.
    meta.uid() == unsafe { libc::geteuid() } && meta.mode() & 0o077 == 0
}

#[cfg(test)]
pub(crate) fn private_tempdir() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, os::unix::fs::PermissionsExt};

    type TestVault = Vault<BTreeMap<String, String>>;

    #[test]
    fn encrypts_updates_and_rejects_wrong_password_and_tampering() {
        let root = private_tempdir();
        let mut vault = TestVault::open(root.path()).unwrap();
        assert!(!vault.initialized());
        assert!(vault.entries().is_err());
        vault.unlock("long example passphrase").unwrap();
        vault
            .update(|entries| {
                entries.insert("endpoint".into(), "example-bearer-secret".into());
                Ok(())
            })
            .unwrap();

        let path = vault_path(&root.path().join("credentials"));
        let mut bytes = fs::read(&path).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(!bytes
            .windows(b"example-bearer-secret".len())
            .any(|s| s == b"example-bearer-secret"));
        assert!(!bytes
            .windows(b"long example passphrase".len())
            .any(|s| s == b"long example passphrase"));

        let mut reopened = TestVault::open(root.path()).unwrap();
        assert!(reopened.initialized());
        assert!(reopened.unlock("wrong").is_err());
        assert!(reopened.entries().is_err());
        reopened.unlock("long example passphrase").unwrap();
        assert_eq!(
            reopened.entries().unwrap()["endpoint"],
            "example-bearer-secret"
        );

        bytes[HEADER_LEN - 1] ^= 1;
        fs::write(path, bytes).unwrap();
        let mut reopened = TestVault::open(root.path()).unwrap();
        assert!(reopened.unlock("long example passphrase").is_err());
        assert!(reopened.entries().is_err());
    }

    #[test]
    fn refuses_public_or_symlinked_storage() {
        let root = private_tempdir();
        let other = private_tempdir();
        let directory = root.path().join("credentials");
        std::os::unix::fs::symlink(other.path(), &directory).unwrap();
        assert!(TestVault::open(root.path()).is_err());
        fs::remove_file(&directory).unwrap();
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(TestVault::open(root.path()).is_err());
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink(other.path(), vault_path(&directory)).unwrap();
        assert!(TestVault::open(root.path()).is_err());
    }
}
