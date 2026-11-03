use ring::digest::{Context, SHA256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, BufReader, Read, Seek};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const LIMIT: usize = 6;
const PREFIX: &str = "dictto-tts-v1-";

pub(crate) struct Cache {
    dir: PathBuf,
    verified: HashMap<String, (String, Fingerprint)>,
}

#[derive(Debug)]
struct Entry {
    order: u64,
    key: String,
    checksum: String,
    path: PathBuf,
}

fn hex(s: &str, length: usize) -> bool {
    s.len() == length
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

impl Entry {
    fn parse(name: &str, path: &Path) -> Option<Self> {
        let rest = name.strip_prefix(PREFIX)?.strip_suffix(".lru")?;
        let (order, rest) = rest.split_once('-')?;
        let (key, checksum) = rest.split_once('-')?;
        if !hex(order, 16) || !hex(key, 64) || !hex(checksum, 64) {
            return None;
        }
        Some(Self {
            order: u64::from_str_radix(order, 16).ok()?,
            key: key.into(),
            checksum: checksum.into(),
            path: path.into(),
        })
    }
}

fn render_pid(name: &str) -> Option<u32> {
    let (pid, serial) = name
        .strip_prefix("dictto-tts-render-")?
        .strip_suffix(".partial")?
        .split_once('-')?;
    if ![pid, serial]
        .iter()
        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    serial.parse::<u64>().ok()?;
    pid.parse().ok()
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

// Validate the PCM payload length without allocating it. Hashing also detects
// unrelated valid WAVs copied over an existing cache entry.
fn wav_checksum(path: &Path) -> io::Result<String> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid("Expected a regular WAV file"));
    }
    let mut file = BufReader::new(File::open(path)?);
    let reader =
        hound::WavReader::new(&mut file).map_err(|_| invalid("Invalid cached WAV header"))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Int
        || spec.bits_per_sample != 16
        || reader.duration() == 0
    {
        return Err(invalid("Expected nonempty 16-bit PCM WAV"));
    }
    let size = u64::from(reader.len()) * 2;
    if file.stream_position()? + size > metadata.len() {
        return Err(invalid("Truncated cached WAV"));
    }
    file.rewind()?;
    let mut digest = Context::new(&SHA256);
    let mut chunk = [0_u8; 32768];
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        digest.update(&chunk[..n]);
    }
    Ok(digest
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[derive(PartialEq, Eq)]
struct Fingerprint {
    length: u64,
    modified: std::time::SystemTime,
}

fn fingerprint(path: &Path) -> io::Result<Fingerprint> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_file() {
        return Err(invalid("Expected regular speech audio"));
    }
    Ok(Fingerprint {
        length: meta.len(),
        modified: meta.modified()?,
    })
}

fn shared(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33)) || error.kind() == io::ErrorKind::PermissionDenied
}

fn cleanup(path: &Path) -> io::Result<()> {
    match remove_if_present(path) {
        Err(error) if shared(&error) => Ok(()),
        result => result,
    }
}

fn owned_wav(name: &str) -> Option<&str> {
    name.strip_prefix(PREFIX)?
        .strip_suffix(".wav")
        .filter(|key| hex(key, 64))
}

impl Cache {
    pub(crate) fn new(dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let mut cache = Self {
            dir: std::path::absolute(dir)?,
            verified: HashMap::new(),
        };
        let mut keep = HashSet::new();
        for entry in cache.entries()?.into_iter().rev() {
            if keep.len() == LIMIT || keep.contains(&entry.key) {
                cleanup(&entry.path)?;
                continue;
            }
            match cache.intact(&entry) {
                Ok(false) => cleanup(&entry.path)?,
                Ok(true) => {
                    keep.insert(entry.key);
                }
                Err(e) if shared(&e) => {
                    keep.insert(entry.key);
                }
                Err(e) => return Err(e),
            }
        }
        // Startup precedes recording in the single-instance native app. An old
        // render remains stale even when Windows reused its process ID.
        for item in fs::read_dir(&cache.dir)? {
            let item = item?;
            if !item.file_type()?.is_file() {
                continue;
            }
            let name = item.file_name();
            let Some(name) = name.to_str() else { continue };
            if render_pid(name).is_some()
                || owned_wav(name).is_some_and(|k| !keep.contains(k))
                || name
                    .strip_suffix(".tmp")
                    .is_some_and(|n| owned_wav(n).is_some())
            {
                cleanup(&item.path())?;
            }
        }
        Ok(cache)
    }

    pub(crate) fn directory(&self) -> &Path {
        &self.dir
    }

    fn wav(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{PREFIX}{key}.wav"))
    }

    fn entries(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for item in fs::read_dir(&self.dir)? {
            let item = item?;
            if !item.file_type()?.is_file() {
                continue;
            }
            let name = item.file_name();
            if let Some(entry) = name.to_str().and_then(|n| Entry::parse(n, &item.path())) {
                entries.push(entry);
            }
        }
        entries.sort_by_key(|e| e.order);
        Ok(entries)
    }

    fn intact(&mut self, entry: &Entry) -> io::Result<bool> {
        let path = self.wav(&entry.key);
        let validate = || -> io::Result<Option<Fingerprint>> {
            let stamp = fingerprint(&path)?;
            let known = self
                .verified
                .get(&entry.key)
                .is_some_and(|(sum, old)| sum == &entry.checksum && old == &stamp);
            if known || wav_checksum(&path)? == entry.checksum {
                Ok(Some(stamp))
            } else {
                Ok(None)
            }
        };
        match validate() {
            Ok(Some(stamp)) => {
                self.verified
                    .insert(entry.key.clone(), (entry.checksum.clone(), stamp));
                Ok(true)
            }
            Ok(None) => Ok(false),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::InvalidData | io::ErrorKind::NotFound
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(e),
        }
    }

    fn destination(&self, entries: &[Entry], key: &str, checksum: &str) -> io::Result<PathBuf> {
        let order = entries
            .last()
            .map(|e| e.order)
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| invalid("Cache access counter overflow"))?;
        Ok(self
            .dir
            .join(format!("{PREFIX}{order:016x}-{key}-{checksum}.lru")))
    }

    pub(crate) fn get_path(&mut self, key: &str) -> io::Result<Option<PathBuf>> {
        if !hex(key, 64) {
            return Err(invalid("Invalid speech cache key"));
        }
        let entries = self.entries()?;
        let path = self.wav(key);
        let mut rejected = false;
        for entry in entries.iter().rev().filter(|e| e.key == key) {
            if !self.intact(entry)? {
                cleanup(&entry.path)?;
                rejected = true;
                continue;
            }
            if entries.last().is_none_or(|last| last.path != entry.path) {
                let dest = self.destination(&entries, key, &entry.checksum)?;
                match fs::rename(&entry.path, dest) {
                    Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                    Err(e) if shared(&e) => (),
                    result => result?,
                }
            }
            return Ok(Some(path));
        }
        if rejected {
            self.verified.remove(key);
            cleanup(&path)?;
        }
        Ok(None)
    }

    pub(crate) fn put_file(&mut self, key: &str, path: &Path) -> io::Result<PathBuf> {
        if !hex(key, 64) {
            return Err(invalid("Invalid speech cache key"));
        }
        if path.parent() != Some(self.directory())
            || path
                .file_name()
                .and_then(|s| s.to_str())
                .and_then(render_pid)
                != Some(std::process::id())
        {
            return Err(invalid(
                "Expected a Dictto render file inside the cache directory",
            ));
        }
        let stamp = fingerprint(path)?;
        let checksum = wav_checksum(path)?;
        let entries = self.entries()?;
        let marker = self.destination(&entries, key, &checksum)?;
        let target = self.wav(key);
        // If interrupted before replacement, startup can still match the old
        // WAV against its previous marker. The WAV itself keeps a stable path.
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        File::options()
            .write(true)
            .create_new(true)
            .attributes(FILE_ATTRIBUTE_HIDDEN)
            .open(&marker)?;
        if let Err(e) = fs::rename(path, &target) {
            cleanup(&marker)?;
            return Err(e);
        }
        self.verified.insert(key.into(), (checksum, stamp));
        let mut keep = HashSet::from([key]);
        for entry in entries.iter().rev() {
            if keep.contains(entry.key.as_str()) {
                cleanup(&entry.path)?;
            } else if keep.len() < LIMIT {
                keep.insert(&entry.key);
            } else {
                self.verified.remove(&entry.key);
                cleanup(&entry.path)?;
                cleanup(&self.wav(&entry.key))?;
            }
        }
        // An editor may have released a WAV that was locked at the preceding eviction.
        for item in fs::read_dir(&self.dir)? {
            let item = item?;
            if item.file_type()?.is_file()
                && item
                    .file_name()
                    .to_str()
                    .and_then(owned_wav)
                    .is_some_and(|key| !keep.contains(key))
            {
                cleanup(&item.path())?;
            }
        }
        Ok(target)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SERIAL: AtomicU64 = AtomicU64::new(1);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "dictto-cache-test-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn key(i: u64) -> String {
        format!("{i:064x}")
    }
    fn render(dir: &Path, sample: i16) -> PathBuf {
        let path = dir.join(format!(
            "dictto-tts-render-{}-{}.partial",
            std::process::id(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        let mut wav = hound::WavWriter::create(
            &path,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        wav.write_sample(sample).unwrap();
        wav.finalize().unwrap();
        path
    }

    #[test]
    fn six_result_lru_promotes_hits_and_updates_across_restart() {
        let dir = Temp::new();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        for i in 1..=6 {
            cache.put_file(&key(i), &render(&dir.0, i as i16)).unwrap();
        }
        cache.get_path(&key(1)).unwrap().unwrap();
        drop(cache);
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        cache.put_file(&key(7), &render(&dir.0, 7)).unwrap();
        assert!(cache.get_path(&key(2)).unwrap().is_none());
        let path = cache.put_file(&key(3), &render(&dir.0, 33)).unwrap();
        assert_eq!(
            hound::WavReader::open(path)
                .unwrap()
                .samples::<i16>()
                .next()
                .unwrap()
                .unwrap(),
            33
        );
        assert_eq!(cache.entries().unwrap().len(), 6);
        cache.put_file(&key(8), &render(&dir.0, 8)).unwrap();
        assert!(cache.get_path(&key(4)).unwrap().is_none());
        for i in [1, 3, 5, 6, 7, 8] {
            assert!(cache.get_path(&key(i)).unwrap().is_some());
        }
    }

    #[test]
    fn cleans_owned_partial_and_corrupt_files_only() {
        let dir = Temp::new();
        let retained = [
            "recording.wav",
            "dictto-tts-render-x.partial",
            "todo.wav.tmp",
        ];
        for name in retained {
            fs::write(dir.0.join(name), b"keep").unwrap();
        }
        let stale = dir.0.join("dictto-tts-render-0-42.partial");
        fs::write(&stale, b"discard").unwrap();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        assert!(!stale.exists());
        let path = cache.put_file(&key(1), &render(&dir.0, 1)).unwrap();
        let old_tmp = path.with_extension("wav.tmp");
        fs::write(&old_tmp, b"discard").unwrap();
        fs::write(&path, b"broken").unwrap();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        assert!(!path.exists() && !old_tmp.exists());
        assert!(cache.get_path(&key(1)).unwrap().is_none());
        let path = cache.put_file(&key(2), &render(&dir.0, 2)).unwrap();
        let other = render(&dir.0, 55);
        fs::copy(other, path).unwrap();
        assert!(cache.get_path(&key(2)).unwrap().is_none());
        let path = cache.put_file(&key(3), &render(&dir.0, 3)).unwrap();
        fs::remove_file(path).unwrap();
        assert!(cache.get_path(&key(3)).unwrap().is_none());
        for name in retained {
            assert_eq!(fs::read(dir.0.join(name)).unwrap(), b"keep");
        }
    }

    #[test]
    fn rejects_invalid_keys_external_inputs_and_truncated_samples() {
        let dir = Temp::new();
        let outside = Temp::new();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        let file = render(&outside.0, 1);
        assert!(cache.put_file(&key(1), &file).is_err());
        assert!(file.exists());
        let file = render(&dir.0, 1);
        assert!(cache.put_file("../unrelated", &file).is_err());
        let size = fs::metadata(&file).unwrap().len();
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(size - 1)
            .unwrap();
        assert!(cache.put_file(&key(1), &file).is_err());
        assert!(file.exists());
    }
    #[test]
    fn stable_path_survives_touch_replacement_and_interrupted_publish() {
        let dir = Temp::new();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        let key1 = key(1);
        let path = cache.put_file(&key1, &render(&dir.0, 1)).unwrap();
        cache.put_file(&key(2), &render(&dir.0, 2)).unwrap();
        assert_eq!(cache.get_path(&key1).unwrap().unwrap(), path);
        assert_eq!(cache.put_file(&key1, &render(&dir.0, 11)).unwrap(), path);
        let stale = render(&dir.0, 99);
        let incomplete = cache
            .destination(
                &cache.entries().unwrap(),
                &key1,
                &wav_checksum(&stale).unwrap(),
            )
            .unwrap();
        File::create_new(&incomplete).unwrap();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        assert!(!stale.exists() && !incomplete.exists());
        assert_eq!(cache.get_path(&key1).unwrap().unwrap(), path);
    }

    #[cfg(windows)]
    #[test]
    fn unchanged_cache_hit_needs_no_audio_read_and_locked_eviction_keeps_working() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = Temp::new();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        let path = cache.put_file(&key(1), &render(&dir.0, 1)).unwrap();
        cache.put_file(&key(2), &render(&dir.0, 2)).unwrap();
        let mut cache = Cache::new(dir.0.clone()).unwrap();
        let lock = File::options()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(File::open(&path).is_err());
        assert_eq!(cache.get_path(&key(1)).unwrap().unwrap(), path);
        for i in 3..=8 {
            cache.put_file(&key(i), &render(&dir.0, i as i16)).unwrap();
        }
        assert!(cache.get_path(&key(1)).unwrap().is_none());
        assert!(path.exists());
        drop(lock);
        cache.put_file(&key(9), &render(&dir.0, 9)).unwrap();
        assert!(!path.exists());
    }
}
