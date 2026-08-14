use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::transcriber::TranscriberMetadata;

const DATASET_SCHEMA_VERSION: u32 = 1;
const SAMPLE_SCHEMA_VERSION: u32 = 1;

type Result<T> = std::result::Result<T, DatasetError>;

#[derive(Debug)]
pub(crate) enum DatasetError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Wav(hound::Error),
    Invalid(String),
}

impl fmt::Display for DatasetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "dataset I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "invalid dataset JSON: {error}"),
            Self::Wav(error) => write!(formatter, "invalid dataset WAV: {error}"),
            Self::Invalid(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for DatasetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::Wav(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

impl From<std::io::Error> for DatasetError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for DatasetError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl From<hound::Error> for DatasetError {
    fn from(error: hound::Error) -> Self {
        Self::Wav(error)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DatasetManifest {
    schema_version: u32,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SttSnapshot {
    pub(crate) endpoint: String,
    pub(crate) model: String,
    pub(crate) language: Option<String>,
    pub(crate) temperature: f32,
    pub(crate) prompt: Option<String>,
}

impl From<TranscriberMetadata> for SttSnapshot {
    fn from(metadata: TranscriberMetadata) -> Self {
        Self {
            endpoint: metadata.endpoint,
            model: metadata.model,
            language: metadata.language,
            temperature: metadata.temperature,
            prompt: metadata.prompt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureStatus {
    Success,
    Partial,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureTranscription {
    pub(crate) status: CaptureStatus,
    pub(crate) text: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) stt: SttSnapshot,
}

impl CaptureTranscription {
    pub(crate) fn success(text: impl Into<String>, stt: SttSnapshot) -> Self {
        Self {
            status: CaptureStatus::Success,
            text: Some(text.into()),
            error: None,
            stt,
        }
    }

    pub(crate) fn partial(
        text: impl Into<String>,
        error: impl Into<String>,
        stt: SttSnapshot,
    ) -> Self {
        Self {
            status: CaptureStatus::Partial,
            text: Some(text.into()),
            error: Some(error.into()),
            stt,
        }
    }

    pub(crate) fn failed(error: impl Into<String>, stt: SttSnapshot) -> Self {
        Self {
            status: CaptureStatus::Failed,
            text: None,
            error: Some(error.into()),
            stt,
        }
    }

    fn validate(&self) -> Result<()> {
        if !self.stt.temperature.is_finite() {
            return Err(DatasetError::Invalid(
                "capture STT temperature must be finite".to_string(),
            ));
        }
        if self.stt.endpoint.trim().is_empty() || self.stt.model.trim().is_empty() {
            return Err(DatasetError::Invalid(
                "capture STT endpoint and model must not be empty".to_string(),
            ));
        }
        let has_text = self
            .text
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty());
        let has_error = self
            .error
            .as_deref()
            .is_some_and(|error| !error.trim().is_empty());
        let valid = match self.status {
            CaptureStatus::Success => has_text && self.error.is_none(),
            CaptureStatus::Partial => has_text && has_error,
            CaptureStatus::Failed | CaptureStatus::Interrupted => self.text.is_none() && has_error,
        };
        if valid {
            Ok(())
        } else {
            Err(DatasetError::Invalid(format!(
                "capture fields do not match {:?} status",
                self.status
            )))
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProperNounAnnotation {
    pub(crate) canonical: String,
    #[serde(default)]
    pub(crate) accepted: Vec<String>,
    pub(crate) case_sensitive: bool,
    pub(crate) expected_occurrences: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReferenceStatus {
    Pending,
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReferenceRecord {
    pub(crate) status: ReferenceStatus,
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) proper_nouns: Vec<ProperNounAnnotation>,
}

impl ReferenceRecord {
    fn pending() -> Self {
        Self {
            status: ReferenceStatus::Pending,
            text: None,
            proper_nouns: Vec::new(),
        }
    }

    fn ready(text: String, proper_nouns: Vec<ProperNounAnnotation>) -> Result<Self> {
        let candidate = Self {
            status: ReferenceStatus::Ready,
            text: Some(text),
            proper_nouns,
        };
        candidate.validate()?;
        Ok(candidate)
    }

    fn validate(&self) -> Result<()> {
        match self.status {
            ReferenceStatus::Pending => {
                if self.text.is_none() && self.proper_nouns.is_empty() {
                    Ok(())
                } else {
                    Err(DatasetError::Invalid(
                        "pending reference must not contain text or proper nouns".to_string(),
                    ))
                }
            }
            ReferenceStatus::Ready => {
                let text = self.text.as_deref().ok_or_else(|| {
                    DatasetError::Invalid("ready reference is missing text".to_string())
                })?;
                if text.trim().is_empty() {
                    return Err(DatasetError::Invalid(
                        "ready reference text must not be empty".to_string(),
                    ));
                }
                validate_annotations(text, &self.proper_nouns)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AudioRecord {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureRecord {
    pub(crate) created_at_unix_ms: u64,
    pub(crate) transcription: CaptureTranscription,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SampleRecord {
    schema_version: u32,
    pub(crate) id: String,
    pub(crate) audio: AudioRecord,
    pub(crate) capture: CaptureRecord,
    pub(crate) reference: ReferenceRecord,
}

#[derive(Debug)]
pub(crate) struct SampleReservation {
    pub(crate) id: String,
    pub(crate) audio_path: PathBuf,
    sample_path: PathBuf,
    created_at_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DatasetIssue {
    pub(crate) code: String,
    pub(crate) path: PathBuf,
    pub(crate) message: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct DatasetValidation {
    pub(crate) ready_count: usize,
    pub(crate) pending_count: usize,
    pub(crate) issues: Vec<DatasetIssue>,
}

#[derive(Debug)]
pub(crate) struct DatasetStore {
    root: PathBuf,
    next_id: AtomicU64,
}

impl DatasetStore {
    pub(crate) fn open_or_create(root: PathBuf) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(root)?;
        for name in ["audio", "samples", "runs"] {
            ensure_owned_directory(&root, name)?;
        }
        let manifest_path = root.join("dataset.json");
        match fs::symlink_metadata(&manifest_path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(DatasetError::Invalid(format!(
                        "dataset manifest must be a regular file: {}",
                        manifest_path.display()
                    )));
                }
                let manifest: DatasetManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
                if manifest.schema_version != DATASET_SCHEMA_VERSION {
                    return Err(DatasetError::Invalid(format!(
                        "unsupported dataset schema_version {}; expected {DATASET_SCHEMA_VERSION}",
                        manifest.schema_version
                    )));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_json(
                    &manifest_path,
                    &DatasetManifest {
                        schema_version: DATASET_SCHEMA_VERSION,
                        created_at_unix_ms: unix_time_ms()?,
                    },
                )?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(Self {
            root,
            next_id: AtomicU64::new(1),
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn audio_dir(&self) -> PathBuf {
        self.root.join("audio")
    }

    pub(crate) fn samples_dir(&self) -> PathBuf {
        self.root.join("samples")
    }

    pub(crate) fn reserve_sample(&self, created_at_unix_ms: u64) -> Result<SampleReservation> {
        loop {
            let sequence = self.next_id.fetch_add(1, Ordering::Relaxed);
            let id = format!("sample-{created_at_unix_ms}-{sequence}");
            let audio_path = self.audio_dir().join(format!("{id}.wav"));
            let sample_path = self.samples_dir().join(format!("{id}.json"));
            if !audio_path.exists() && !sample_path.exists() {
                return Ok(SampleReservation {
                    id,
                    audio_path,
                    sample_path,
                    created_at_unix_ms,
                });
            }
        }
    }

    pub(crate) fn complete_capture(
        &self,
        reservation: SampleReservation,
        transcription: CaptureTranscription,
    ) -> Result<SampleRecord> {
        transcription.validate()?;
        let expected_audio = self.audio_dir().join(format!("{}.wav", reservation.id));
        let expected_sample = self.samples_dir().join(format!("{}.json", reservation.id));
        if reservation.audio_path != expected_audio || reservation.sample_path != expected_sample {
            return Err(DatasetError::Invalid(
                "sample reservation does not belong to this dataset".to_string(),
            ));
        }
        if expected_sample.exists() {
            return Err(DatasetError::Invalid(format!(
                "sample {} already exists",
                reservation.id
            )));
        }
        validate_wav(&expected_audio)?;
        let sha256 = sha256_file(&expected_audio)?;
        let sample = SampleRecord {
            schema_version: SAMPLE_SCHEMA_VERSION,
            id: reservation.id.clone(),
            audio: AudioRecord {
                path: format!("audio/{}.wav", reservation.id),
                sha256,
            },
            capture: CaptureRecord {
                created_at_unix_ms: reservation.created_at_unix_ms,
                transcription,
            },
            reference: ReferenceRecord::pending(),
        };
        write_json(&expected_sample, &sample)?;
        Ok(sample)
    }

    pub(crate) fn correct_sample(
        &self,
        id: &str,
        text: &str,
        proper_nouns: Vec<ProperNounAnnotation>,
    ) -> Result<SampleRecord> {
        let mut sample = self.load_sample(id)?;
        let reference = ReferenceRecord::ready(text.to_string(), proper_nouns)?;
        self.validate_audio(&sample)?;
        sample.reference = reference;
        write_json(&self.sample_path(id)?, &sample)?;
        Ok(sample)
    }

    pub(crate) fn load_sample(&self, id: &str) -> Result<SampleRecord> {
        validate_id(id)?;
        let path = self.sample_path(id)?;
        let sample: SampleRecord = serde_json::from_slice(&fs::read(path)?)?;
        self.validate_sample_identity(&sample, id)?;
        sample.capture.transcription.validate()?;
        sample.reference.validate()?;
        Ok(sample)
    }

    pub(crate) fn list_samples(
        &self,
        status: Option<ReferenceStatus>,
    ) -> Result<Vec<SampleRecord>> {
        let mut entries = fs::read_dir(self.samples_dir())?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        let mut samples = Vec::new();
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| {
                    DatasetError::Invalid(format!("invalid sample file name: {}", path.display()))
                })?;
            let sample = self.load_sample(id)?;
            if status.is_none_or(|status| sample.reference.status == status) {
                samples.push(sample);
            }
        }
        Ok(samples)
    }

    pub(crate) fn sample_path(&self, id: &str) -> Result<PathBuf> {
        validate_id(id)?;
        Ok(self.samples_dir().join(format!("{id}.json")))
    }

    pub(crate) fn validate(&self) -> Result<DatasetValidation> {
        let mut report = DatasetValidation {
            ready_count: 0,
            pending_count: 0,
            issues: Vec::new(),
        };
        let mut referenced_audio = HashSet::new();
        let mut entries = fs::read_dir(self.samples_dir())?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type()?.is_symlink()
                || path.extension().and_then(|ext| ext.to_str()) != Some("json")
            {
                report.issues.push(issue(
                    "sample_file",
                    &path,
                    "sample entry must be a regular JSON file",
                ));
                continue;
            }
            let sample: SampleRecord = match fs::read(&path)
                .map_err(DatasetError::from)
                .and_then(|bytes| serde_json::from_slice(&bytes).map_err(DatasetError::from))
            {
                Ok(sample) => sample,
                Err(error) => {
                    report
                        .issues
                        .push(issue("sample_json", &path, &error.to_string()));
                    continue;
                }
            };
            referenced_audio.insert(sample.audio.path.clone());
            let expected_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("");
            let validation = self
                .validate_sample_identity(&sample, expected_id)
                .and_then(|()| sample.capture.transcription.validate())
                .and_then(|()| sample.reference.validate())
                .and_then(|()| self.validate_audio(&sample));
            match validation {
                Ok(()) => match sample.reference.status {
                    ReferenceStatus::Ready => report.ready_count += 1,
                    ReferenceStatus::Pending => report.pending_count += 1,
                },
                Err(error) => {
                    report
                        .issues
                        .push(issue("sample_invalid", &path, &error.to_string()))
                }
            }
        }

        let mut audio_entries =
            fs::read_dir(self.audio_dir())?.collect::<std::io::Result<Vec<_>>>()?;
        audio_entries.sort_by_key(|entry| entry.file_name());
        for entry in audio_entries {
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if !referenced_audio.contains(&relative) {
                report.issues.push(issue(
                    "unreferenced_audio",
                    &path,
                    "audio file is not owned by a sample sidecar",
                ));
            }
        }
        Ok(report)
    }

    fn validate_sample_identity(&self, sample: &SampleRecord, expected_id: &str) -> Result<()> {
        if sample.schema_version != SAMPLE_SCHEMA_VERSION {
            return Err(DatasetError::Invalid(format!(
                "unsupported sample schema_version {}; expected {SAMPLE_SCHEMA_VERSION}",
                sample.schema_version
            )));
        }
        validate_id(&sample.id)?;
        if sample.id != expected_id {
            return Err(DatasetError::Invalid(format!(
                "sample ID {} does not match file name {expected_id}",
                sample.id
            )));
        }
        let expected = format!("audio/{}.wav", sample.id);
        if sample.audio.path != expected {
            return Err(DatasetError::Invalid(format!(
                "sample audio path must be {expected}"
            )));
        }
        Ok(())
    }

    fn validate_audio(&self, sample: &SampleRecord) -> Result<()> {
        let path = self.root.join(&sample.audio.path);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DatasetError::Invalid(format!(
                "sample audio is not a regular file: {}",
                path.display()
            )));
        }
        let canonical = fs::canonicalize(&path)?;
        if !canonical.starts_with(&self.root) {
            return Err(DatasetError::Invalid(
                "sample audio escapes the dataset root".to_string(),
            ));
        }
        validate_wav(&canonical)?;
        let actual = sha256_file(&canonical)?;
        if actual != sample.audio.sha256 {
            return Err(DatasetError::Invalid(format!(
                "sample audio digest mismatch for {}",
                sample.id
            )));
        }
        Ok(())
    }
}

fn ensure_owned_directory(root: &Path, name: &str) -> Result<()> {
    let path = root.join(name);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&path)?;
            fs::symlink_metadata(&path)?
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DatasetError::Invalid(format!(
            "dataset {name} directory must not be a symlink: {}",
            path.display()
        )));
    }
    let canonical = fs::canonicalize(&path)?;
    if !canonical.starts_with(root) {
        return Err(DatasetError::Invalid(format!(
            "dataset {name} directory escapes the dataset root"
        )));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<()> {
    let valid = id.starts_with("sample-")
        && id.len() > "sample-".len()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(DatasetError::Invalid(format!("invalid sample ID: {id}")))
    }
}

fn validate_annotations(text: &str, annotations: &[ProperNounAnnotation]) -> Result<()> {
    let mut all_forms = HashSet::new();
    for annotation in annotations {
        if annotation.canonical.trim().is_empty() {
            return Err(DatasetError::Invalid(
                "proper noun canonical form must not be empty".to_string(),
            ));
        }
        if annotation.expected_occurrences == 0 {
            return Err(DatasetError::Invalid(
                "proper noun expected_occurrences must be greater than zero".to_string(),
            ));
        }
        let mut forms = Vec::with_capacity(annotation.accepted.len() + 1);
        forms.push(annotation.canonical.as_str());
        forms.extend(annotation.accepted.iter().map(String::as_str));
        let mut local_forms = HashSet::new();
        for form in &forms {
            if form.trim().is_empty() {
                return Err(DatasetError::Invalid(
                    "proper noun accepted forms must not be empty".to_string(),
                ));
            }
            let normalized = normalize_case(form, annotation.case_sensitive);
            if !local_forms.insert(normalized.clone()) {
                return Err(DatasetError::Invalid(format!(
                    "duplicate accepted form for {}",
                    annotation.canonical
                )));
            }
            if !all_forms.insert(normalized) {
                return Err(DatasetError::Invalid(format!(
                    "ambiguous proper noun form: {form}"
                )));
            }
        }
        let count = count_non_overlapping_forms(text, &forms, annotation.case_sensitive);
        if count != annotation.expected_occurrences as usize {
            return Err(DatasetError::Invalid(format!(
                "proper noun {} expected_occurrences is {}, but reference contains {count}",
                annotation.canonical, annotation.expected_occurrences
            )));
        }
    }
    Ok(())
}

fn count_non_overlapping_forms(text: &str, forms: &[&str], case_sensitive: bool) -> usize {
    let text = normalize_case(text, case_sensitive);
    let mut forms = forms
        .iter()
        .map(|form| normalize_case(form, case_sensitive))
        .collect::<Vec<_>>();
    forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
    let mut occupied = vec![false; text.len()];
    let mut count = 0;
    for form in forms {
        for (start, _) in text.match_indices(&form) {
            let end = start + form.len();
            if !occupied[start..end].iter().any(|value| *value) {
                occupied[start..end].fill(true);
                count += 1;
            }
        }
    }
    count
}

fn normalize_case(value: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        value.to_string()
    } else {
        value.to_lowercase()
    }
}

fn validate_wav(path: &Path) -> Result<()> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 || spec.bits_per_sample == 0 || reader.len() == 0
    {
        return Err(DatasetError::Invalid(format!(
            "sample WAV is empty or has an invalid format: {}",
            path.display()
        )));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes)?;
    Ok(())
}

fn unix_time_ms() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            DatasetError::Invalid(format!("system clock is before UNIX epoch: {error}"))
        })?
        .as_millis()
        .try_into()
        .map_err(|_| DatasetError::Invalid("current timestamp does not fit u64".to_string()))
}

fn issue(code: &str, path: &Path, message: &str) -> DatasetIssue {
    DatasetIssue {
        code: code.to_string(),
        path: path.to_path_buf(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use hound::{SampleFormat, WavSpec, WavWriter};
    use tempfile::tempdir;

    use super::*;

    fn write_test_wav(path: &Path) {
        let mut writer = WavWriter::create(
            path,
            WavSpec {
                channels: 1,
                sample_rate: 16_000,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )
        .unwrap();
        for sample in [0_i16, 1, -1, 2] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn snapshot() -> SttSnapshot {
        SttSnapshot {
            endpoint: "https://api.example.test/v1/audio/transcriptions".to_string(),
            model: "whisper-test".to_string(),
            language: Some("zh".to_string()),
            temperature: 0.0,
            prompt: Some("技术词汇提示".to_string()),
        }
    }

    fn captured_sample(store: &DatasetStore, timestamp: u64) -> String {
        let reservation = store.reserve_sample(timestamp).unwrap();
        write_test_wav(&reservation.audio_path);
        let id = reservation.id.clone();
        store
            .complete_capture(
                reservation,
                CaptureTranscription::success("使用 ViberWhisper", snapshot()),
            )
            .unwrap();
        id
    }

    #[test]
    fn capture_and_correction_build_a_ready_sample() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        let id = captured_sample(&store, 42);

        let pending = store.validate().unwrap();
        assert_eq!(pending.ready_count, 0);
        assert_eq!(pending.pending_count, 1);
        assert!(pending.issues.is_empty());

        store
            .correct_sample(
                &id,
                "使用 ViberWhisper",
                vec![ProperNounAnnotation {
                    canonical: "ViberWhisper".to_string(),
                    accepted: vec!["Viber Whisper".to_string()],
                    case_sensitive: false,
                    expected_occurrences: 1,
                }],
            )
            .unwrap();

        let sample = store.load_sample(&id).unwrap();
        assert_eq!(sample.reference.status, ReferenceStatus::Ready);
        assert_eq!(sample.reference.text.as_deref(), Some("使用 ViberWhisper"));
        assert!(!sample.audio.sha256.is_empty());
        let ready = store.validate().unwrap();
        assert_eq!(ready.ready_count, 1);
        assert_eq!(ready.pending_count, 0);
        assert!(ready.issues.is_empty());
    }

    #[test]
    fn invalid_correction_does_not_rewrite_the_sidecar() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        let id = captured_sample(&store, 42);
        let path = store.sample_path(&id).unwrap();
        let before = fs::read(&path).unwrap();

        let error = store
            .correct_sample(
                &id,
                "只出现一次 Example",
                vec![ProperNounAnnotation {
                    canonical: "Example".to_string(),
                    accepted: Vec::new(),
                    case_sensitive: true,
                    expected_occurrences: 2,
                }],
            )
            .unwrap_err();

        assert!(error.to_string().contains("expected_occurrences"));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[test]
    fn validation_reports_malformed_and_unreferenced_files() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();
        fs::write(store.samples_dir().join("broken.json"), b"{broken").unwrap();
        write_test_wav(&store.audio_dir().join("orphan.wav"));

        let report = store.validate().unwrap();

        assert_eq!(report.ready_count, 0);
        assert_eq!(report.pending_count, 0);
        assert_eq!(report.issues.len(), 2);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "sample_json")
        );
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "unreferenced_audio")
        );
    }

    #[test]
    fn strict_manifest_rejects_unknown_fields() {
        let directory = tempdir().unwrap();
        let root = directory.path().join("dataset");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("dataset.json"),
            r#"{"schema_version":1,"created_at_unix_ms":1,"unknown":true}"#,
        )
        .unwrap();

        let error = DatasetStore::open_or_create(root).unwrap_err();

        assert!(error.to_string().contains("unknown field"));
    }

    #[cfg(unix)]
    #[test]
    fn opening_dataset_rejects_symlinked_storage_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let root = directory.path().join("dataset");
        let outside = directory.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("audio")).unwrap();

        let error = DatasetStore::open_or_create(root).unwrap_err();

        assert!(error.to_string().contains("audio"));
        assert!(error.to_string().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn opening_dataset_rejects_dangling_manifest_symlink_without_writing_target() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let root = directory.path().join("dataset");
        let outside_manifest = directory.path().join("outside-dataset.json");
        fs::create_dir_all(&root).unwrap();
        symlink(&outside_manifest, root.join("dataset.json")).unwrap();

        let error = DatasetStore::open_or_create(root).unwrap_err();

        assert!(error.to_string().contains("manifest"));
        assert!(!outside_manifest.exists());
    }

    #[test]
    fn reservations_remain_unique_for_the_same_timestamp() {
        let directory = tempdir().unwrap();
        let store = DatasetStore::open_or_create(directory.path().join("dataset")).unwrap();

        let first = store.reserve_sample(42).unwrap();
        let second = store.reserve_sample(42).unwrap();

        assert_ne!(first.id, second.id);
        assert!(first.audio_path.starts_with(store.root()));
        assert!(second.audio_path.starts_with(store.root()));
    }
}
