//! The on-disk capture store.
//!
//! Layout under the store root, one folder per session key:
//!
//! - `calls.jsonl.gz`: one gzip member per record, so an append never rewrites
//!   the file and a crash can only damage the last member.
//! - `blobs/<sha256>.json.gz`: each distinct system prompt and tool set, stored
//!   once. In a stored record these two body fields are replaced in place by
//!   `{"snitchcraft_blob": "<sha256>"}` and listed in the line's `blobs` map.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use capture_core::{Body, CaptureRecord, SYSTEM_KEY, TOOLS_KEY, content_hash};
use flate2::Compression;
use flate2::bufread::GzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Session key used for calls that carry no session id.
pub const UNKNOWN_SESSION: &str = "unknown";

const CALLS_FILE: &str = "calls.jsonl.gz";
const BLOBS_DIR: &str = "blobs";
const BLOB_MARKER: &str = "snitchcraft_blob";
const BLOB_FIELDS: [&str; 2] = [SYSTEM_KEY, TOOLS_KEY];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Why a store operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// The session key is not 1 to 128 characters of letters, digits, `_` or `-`.
    #[error("invalid session key")]
    InvalidKey,
    /// A file system operation failed.
    #[error("could not {what}: {source}")]
    Io {
        /// What the store was doing.
        what: &'static str,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A record could not be turned into JSON.
    #[error("could not encode a capture: {0}")]
    Encode(serde_json::Error),
}

fn io(what: &'static str) -> impl FnOnce(std::io::Error) -> StoreError {
    move |source| StoreError::Io { what, source }
}

/// True when `key` is 1 to 128 characters of `[A-Za-z0-9_-]`.
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Records read back from a session, plus why some data was left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// The records that could be read, in the order they were stored.
    pub records: Vec<CaptureRecord>,
    /// One reason per line or blob that was skipped or could not be restored.
    pub skipped: Vec<String>,
}

/// A stored session, for listings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionCaptures {
    /// The session key.
    pub key: String,
    /// Total bytes stored for the session.
    pub bytes: u64,
    /// Last modification time, milliseconds since 1970-01-01 UTC.
    pub modified_ms: i64,
}

#[derive(Serialize)]
struct StoredOut<'a> {
    blobs: Map<String, Value>,
    record: &'a CaptureRecord,
}

#[derive(Deserialize)]
struct StoredIn {
    #[serde(default)]
    blobs: Map<String, Value>,
    record: CaptureRecord,
}

/// Reads and writes captures under one root folder.
pub struct CaptureStore {
    root: PathBuf,
    write_lock: Mutex<()>,
}

impl CaptureStore {
    /// A store rooted at `root`. Nothing is created until the first append.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            write_lock: Mutex::new(()),
        }
    }

    /// The root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn session_dir(&self, key: &str) -> Result<PathBuf, StoreError> {
        if is_valid_key(key) {
            Ok(self.root.join(key))
        } else {
            Err(StoreError::InvalidKey)
        }
    }

    /// Append one record to the session, storing its system prompt and tools as blobs.
    pub fn append(&self, key: &str, record: &CaptureRecord) -> Result<(), StoreError> {
        let dir = self.session_dir(key)?;
        let blobs_dir = dir.join(BLOBS_DIR);
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fs::create_dir_all(&blobs_dir).map_err(io("create the session folder"))?;

        let mut stored = record.clone();
        let mut blobs = Map::new();
        if let Body::Json(Value::Object(map)) = &mut stored.request.body {
            for field in BLOB_FIELDS {
                let Some(value) = map.get_mut(field) else {
                    continue;
                };
                let hash = content_hash(value);
                write_blob(&blobs_dir, &hash, value)?;
                let mut placeholder = Map::new();
                placeholder.insert(BLOB_MARKER.to_string(), Value::String(hash.clone()));
                *value = Value::Object(placeholder);
                blobs.insert(field.to_string(), Value::String(hash));
            }
        }

        let mut line = serde_json::to_vec(&StoredOut {
            blobs,
            record: &stored,
        })
        .map_err(StoreError::Encode)?;
        line.push(b'\n');
        let member = gzip(&line).map_err(io("compress a capture"))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(CALLS_FILE))
            .map_err(io("open the capture file"))?;
        file.write_all(&member).map_err(io("write a capture"))
    }

    /// Read every record of a session. A missing session gives an empty result.
    pub fn load(&self, key: &str) -> Result<Loaded, StoreError> {
        let dir = self.session_dir(key)?;
        let mut loaded = Loaded {
            records: Vec::new(),
            skipped: Vec::new(),
        };
        let bytes = match fs::read(dir.join(CALLS_FILE)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(loaded),
            Err(e) => return Err(io("read the capture file")(e)),
        };

        // Each record is its own gzip member, so decode member by member. A
        // damaged member (for example a cut-off last one) is dropped whole.
        let mut rest: &[u8] = &bytes;
        while !rest.is_empty() {
            let mut text = String::new();
            let mut decoder = GzDecoder::new(&mut rest);
            if let Err(e) = decoder.read_to_string(&mut text) {
                loaded
                    .skipped
                    .push(format!("stopped at a damaged capture: {e}"));
                break;
            }
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                match serde_json::from_str::<StoredIn>(line) {
                    Ok(stored) => {
                        let record = restore(&dir, stored, &mut loaded.skipped);
                        loaded.records.push(record);
                    }
                    Err(e) => loaded
                        .skipped
                        .push(format!("skipped a capture line that did not parse: {e}")),
                }
            }
        }
        Ok(loaded)
    }

    /// Total bytes stored for a session, 0 if it does not exist.
    pub fn size(&self, key: &str) -> Result<u64, StoreError> {
        let dir = self.session_dir(key)?;
        dir_size(&dir).map_err(io("measure the session folder"))
    }

    /// Delete a session. A missing session is fine.
    pub fn delete(&self, key: &str) -> Result<(), StoreError> {
        let dir = self.session_dir(key)?;
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io("delete the session folder")(e)),
        }
    }
}

fn gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn write_blob(blobs_dir: &Path, hash: &str, value: &Value) -> Result<(), StoreError> {
    let path = blobs_dir.join(format!("{hash}.json.gz"));
    if path.exists() {
        return Ok(());
    }
    let json = serde_json::to_vec(value).map_err(StoreError::Encode)?;
    let data = gzip(&json).map_err(io("compress a blob"))?;
    let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp = blobs_dir.join(format!("{hash}.{}.{n}.tmp", std::process::id()));
    fs::write(&temp, &data).map_err(io("write a blob"))?;
    if let Err(e) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        // Another writer may have created the blob in the meantime.
        if !path.exists() {
            return Err(io("store a blob")(e));
        }
    }
    Ok(())
}

fn read_blob(dir: &Path, hash: &str) -> Result<Value, String> {
    if !is_hash(hash) {
        return Err(format!("blob name {hash:?} is not a content hash"));
    }
    let path = dir.join(BLOBS_DIR).join(format!("{hash}.json.gz"));
    let file = File::open(&path).map_err(|e| format!("blob {hash} could not be opened: {e}"))?;
    let mut text = String::new();
    GzDecoder::new(std::io::BufReader::new(file))
        .read_to_string(&mut text)
        .map_err(|e| format!("blob {hash} could not be read: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("blob {hash} is not JSON: {e}"))
}

fn restore(dir: &Path, stored: StoredIn, skipped: &mut Vec<String>) -> CaptureRecord {
    let mut record = stored.record;
    if let Body::Json(Value::Object(map)) = &mut record.request.body {
        for (field, hash) in &stored.blobs {
            let (Some(slot), Some(hash)) = (map.get_mut(field), hash.as_str()) else {
                continue;
            };
            match read_blob(dir, hash) {
                Ok(value) => *slot = value,
                Err(reason) => skipped.push(format!("{field}: {reason}")),
            }
        }
    }
    record
}

fn dir_size(dir: &Path) -> std::io::Result<u64> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let mut total = 0;
    for entry in entries {
        let entry = entry?;
        let meta = entry.metadata()?;
        total += if meta.is_dir() {
            dir_size(&entry.path())?
        } else {
            meta.len()
        };
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use capture_core::{CapturedRequest, CapturedResponse, Header};
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("snitchcraft-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn system_text() -> String {
        "You are a careful invented assistant. ".repeat(400)
    }

    fn record(id: &str, user: &str) -> CaptureRecord {
        let body = json!({
            "model": "invented-model",
            "system": [{"type": "text", "text": system_text()}],
            "tools": [{"name": "read_thing", "input_schema": {"type": "object", "properties": {"zeta": {}, "alpha": {}}}}],
            "messages": [{"role": "user", "content": user}],
            "stream": true
        });
        CaptureRecord {
            id: id.to_string(),
            started_at_ms: 1_000,
            first_byte_at_ms: Some(1_100),
            ended_at_ms: Some(1_200),
            request: CapturedRequest {
                method: "POST".to_string(),
                path: "/v1/messages".to_string(),
                headers: vec![Header {
                    name: "content-type".to_string(),
                    value: "application/json".to_string(),
                }],
                body: Body::Json(body),
            },
            response: Some(CapturedResponse {
                status: 200,
                headers: vec![],
                stream: None,
                message: Some(json!({"id": "msg_1", "type": "message"})),
            }),
            message_id: Some("msg_1".to_string()),
            error: None,
        }
    }

    fn body_text(r: &CaptureRecord) -> String {
        serde_json::to_string(&r.request.body).unwrap()
    }

    #[test]
    fn round_trip_restores_system_and_tools_in_place() {
        let store = CaptureStore::new(temp_root("round-trip"));
        let a = record("a", "first question");
        let b = record("b", "second question");
        store.append("s1", &a).unwrap();
        store.append("s1", &b).unwrap();
        let loaded = store.load("s1").unwrap();
        assert!(loaded.skipped.is_empty(), "{:?}", loaded.skipped);
        assert_eq!(loaded.records, vec![a.clone(), b.clone()]);
        assert_eq!(body_text(&loaded.records[0]), body_text(&a));
        assert_eq!(body_text(&loaded.records[1]), body_text(&b));
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn identical_system_prompts_are_stored_once() {
        let store = CaptureStore::new(temp_root("dedup"));
        store.append("s1", &record("a", "one")).unwrap();
        store.append("s1", &record("b", "two")).unwrap();
        store.append("s1", &record("c", "one")).unwrap();
        let blobs = fs::read_dir(store.root().join("s1").join(BLOBS_DIR))
            .unwrap()
            .count();
        assert_eq!(blobs, 2);
        let size = store.size("s1").unwrap();
        assert!(size < 3 * system_text().len() as u64, "size was {size}");
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn truncated_last_record_is_skipped() {
        let store = CaptureStore::new(temp_root("truncated"));
        for id in ["a", "b", "c"] {
            store.append("s1", &record(id, id)).unwrap();
        }
        let path = store.root().join("s1").join(CALLS_FILE);
        let len = fs::metadata(&path).unwrap().len();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_len(len - 10).unwrap();
        drop(file);
        let loaded = store.load("s1").unwrap();
        assert_eq!(loaded.records.len(), 2);
        assert_eq!(loaded.records[1].id, "b");
        assert_eq!(loaded.skipped.len(), 1);
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn invalid_keys_are_rejected_and_nothing_is_written() {
        let store = CaptureStore::new(temp_root("invalid"));
        let r = record("a", "x");
        let long = "x".repeat(129);
        for key in ["", "..", "a/b", "a\\b", "C:", long.as_str()] {
            assert!(matches!(store.append(key, &r), Err(StoreError::InvalidKey)));
            assert!(matches!(store.load(key), Err(StoreError::InvalidKey)));
            assert!(matches!(store.size(key), Err(StoreError::InvalidKey)));
            assert!(matches!(store.delete(key), Err(StoreError::InvalidKey)));
        }
        let entries = fs::read_dir(store.root()).map(|d| d.count()).unwrap_or(0);
        assert_eq!(entries, 0);
        assert!(is_valid_key("abc-DEF_123"));
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn delete_removes_only_that_session() {
        let store = CaptureStore::new(temp_root("delete"));
        store.append("s1", &record("a", "x")).unwrap();
        store.append("s2", &record("b", "y")).unwrap();
        store.delete("s1").unwrap();
        assert!(!store.root().join("s1").exists());
        assert_eq!(store.load("s2").unwrap().records.len(), 1);
        store.delete("s1").unwrap();
        let _ = fs::remove_dir_all(store.root());
    }

    #[test]
    fn missing_session_loads_empty() {
        let store = CaptureStore::new(temp_root("missing"));
        let loaded = store.load("nope").unwrap();
        assert!(loaded.records.is_empty());
        assert!(loaded.skipped.is_empty());
        assert_eq!(store.size("nope").unwrap(), 0);
    }

    #[test]
    fn missing_blob_leaves_placeholder_and_a_reason() {
        let store = CaptureStore::new(temp_root("missing-blob"));
        store.append("s1", &record("a", "x")).unwrap();
        let blobs = store.root().join("s1").join(BLOBS_DIR);
        for entry in fs::read_dir(&blobs).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let loaded = store.load("s1").unwrap();
        assert_eq!(loaded.records.len(), 1);
        assert_eq!(loaded.skipped.len(), 2);
        let body = loaded.records[0].request.body.json().unwrap();
        assert!(body["system"].get(BLOB_MARKER).is_some());
        let _ = fs::remove_dir_all(store.root());
    }
}
