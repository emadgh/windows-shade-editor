use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inverse_lut_identity::InverseLutIdentityRecord;
use crate::safe_fs;

pub const INVERSE_LUT_ARTIFACT_FORMAT_VERSION: u32 = 1;
pub const MAX_INVERSE_LUT_ARTIFACT_HEADER_BYTES: usize = 1024 * 1024;
pub const MAX_INVERSE_LUT_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;
const ARTIFACT_MAGIC: [u8; 8] = *b"SHDLUT01";
const FIXED_PREFIX_BYTES: u64 = 16;
static STAGE_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct InverseLutArtifactHeader {
    format_version: u32,
    identity: InverseLutIdentityRecord,
    identity_content_id: String,
    payload_sha256: String,
    node_count: u64,
    channel_count: u16,
    validity_bytes: u64,
    coverage_values: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedInverseLutArtifact {
    pub identity: InverseLutIdentityRecord,
    pub identity_content_id: String,
    pub payload_sha256: String,
    pub validity: Vec<bool>,
    /// Node-major normalized coverages in authoritative identity channel order.
    pub coverages: Vec<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InverseLutPublishOutcome {
    Published,
    ReusedExisting,
}

pub fn write_inverse_lut_artifact(
    path: &Path,
    identity: &InverseLutIdentityRecord,
    validity: &[bool],
    coverages: &[f32],
) -> Result<(), String> {
    let prepared = prepare_header(identity, validity, coverages)?;
    write_prepared(path, &prepared, validity, coverages)
}

pub fn publish_inverse_lut_artifact_if_absent(
    destination: &Path,
    identity: &InverseLutIdentityRecord,
    validity: &[bool],
    coverages: &[f32],
) -> Result<InverseLutPublishOutcome, String> {
    let prepared = prepare_header(identity, validity, coverages)?;

    if destination.exists() {
        verify_existing_matches(destination, &prepared)?;
        return Ok(InverseLutPublishOutcome::ReusedExisting);
    }

    let staged = unique_staged_path(destination)?;
    let result = (|| {
        write_prepared(&staged, &prepared, validity, coverages)?;
        let staged_artifact = load_inverse_lut_artifact(&staged)?;
        verify_loaded_matches(&staged_artifact, &prepared)?;

        match safe_fs::commit_staged_file_if_absent(&staged, destination) {
            Ok(()) => Ok(InverseLutPublishOutcome::Published),
            Err(commit_error) if destination.exists() => {
                let _ = fs::remove_file(&staged);
                verify_existing_matches(destination, &prepared)?;
                Ok(InverseLutPublishOutcome::ReusedExisting)
            }
            Err(commit_error) => Err(commit_error),
        }
    })();
    if result.is_err() && staged.exists() {
        let _ = fs::remove_file(&staged);
    }
    result
}

pub fn load_inverse_lut_artifact(path: &Path) -> Result<VerifiedInverseLutArtifact, String> {
    let metadata = fs::metadata(path)
        .map_err(|err| format!("Cannot inspect inverse LUT artifact {}: {err}", path.display()))?;
    if metadata.len() < FIXED_PREFIX_BYTES || metadata.len() > MAX_INVERSE_LUT_ARTIFACT_BYTES {
        return Err(format!(
            "Inverse LUT artifact {} has invalid bounded size {} bytes.",
            path.display(), metadata.len()
        ));
    }

    let file = File::open(path)
        .map_err(|err| format!("Cannot open inverse LUT artifact {}: {err}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut magic = [0u8; 8];
    reader.read_exact(&mut magic).map_err(|err| err.to_string())?;
    if magic != ARTIFACT_MAGIC {
        return Err("Invalid inverse LUT artifact magic.".to_owned());
    }
    let format_version = read_u32_le(&mut reader)?;
    if format_version != INVERSE_LUT_ARTIFACT_FORMAT_VERSION {
        return Err(format!(
            "Unsupported inverse LUT artifact format {format_version} (expected {INVERSE_LUT_ARTIFACT_FORMAT_VERSION})."
        ));
    }
    let header_len = read_u32_le(&mut reader)? as usize;
    if header_len == 0 || header_len > MAX_INVERSE_LUT_ARTIFACT_HEADER_BYTES {
        return Err(format!("Invalid inverse LUT artifact header length {header_len}."));
    }

    let mut header_bytes = vec![0u8; header_len];
    reader.read_exact(&mut header_bytes).map_err(|err| {
        format!("Cannot read inverse LUT artifact header: {err}")
    })?;
    let header: InverseLutArtifactHeader = serde_json::from_slice(&header_bytes)
        .map_err(|err| format!("Cannot parse inverse LUT artifact header: {err}"))?;
    validate_header(&header, header_len, metadata.len())?;

    let node_count = usize::try_from(header.node_count)
        .map_err(|_| "Inverse LUT node count does not fit usize.".to_owned())?;
    let coverage_values = usize::try_from(header.coverage_values)
        .map_err(|_| "Inverse LUT coverage count does not fit usize.".to_owned())?;

    let mut hasher = Sha256::new();
    let mut validity = Vec::with_capacity(node_count);
    for node in 0..node_count {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte)
            .map_err(|err| format!("Cannot read inverse LUT validity node {node}: {err}"))?;
        if byte[0] > 1 {
            return Err(format!("Inverse LUT validity node {node} is not canonical 0/1."));
        }
        hasher.update(byte);
        validity.push(byte[0] == 1);
    }

    let mut coverages = Vec::with_capacity(coverage_values);
    for index in 0..coverage_values {
        let mut bytes = [0u8; 4];
        reader.read_exact(&mut bytes)
            .map_err(|err| format!("Cannot read inverse LUT coverage {index}: {err}"))?;
        hasher.update(bytes);
        let value = f32::from_bits(u32::from_le_bytes(bytes));
        validate_stored_coverage(value, bytes, index)?;
        coverages.push(value);
    }

    let actual_payload_sha256 = format!("{:x}", hasher.finalize());
    if actual_payload_sha256 != header.payload_sha256 {
        return Err(format!(
            "Inverse LUT payload digest mismatch: header {}, payload {}.",
            header.payload_sha256, actual_payload_sha256
        ));
    }

    Ok(VerifiedInverseLutArtifact {
        identity: header.identity,
        identity_content_id: header.identity_content_id,
        payload_sha256: header.payload_sha256,
        validity,
        coverages,
    })
}

fn prepare_header(
    identity: &InverseLutIdentityRecord,
    validity: &[bool],
    coverages: &[f32],
) -> Result<InverseLutArtifactHeader, String> {
    identity.validate().map_err(|errors| errors.join("\n"))?;
    let identity_content_id = identity.content_id()?;
    let node_count = identity
        .build_policy
        .grid
        .node_count()
        .ok_or_else(|| "Inverse LUT identity grid node count overflowed.".to_owned())?;
    let channel_count = u16::try_from(identity.channel_names.len())
        .map_err(|_| "Inverse LUT channel count does not fit u16.".to_owned())?;
    if validity.len() as u64 != node_count {
        return Err(format!(
            "Inverse LUT validity length mismatch: expected {node_count}, got {}.", validity.len()
        ));
    }
    let coverage_values = node_count
        .checked_mul(u64::from(channel_count))
        .ok_or_else(|| "Inverse LUT coverage count overflowed u64.".to_owned())?;
    if coverages.len() as u64 != coverage_values {
        return Err(format!(
            "Inverse LUT coverage length mismatch: expected {coverage_values}, got {}.", coverages.len()
        ));
    }

    let mut hasher = Sha256::new();
    for valid in validity {
        hasher.update([u8::from(*valid)]);
    }
    for (index, value) in coverages.iter().copied().enumerate() {
        let bytes = canonical_coverage_bytes(value, index)?;
        hasher.update(bytes);
    }

    Ok(InverseLutArtifactHeader {
        format_version: INVERSE_LUT_ARTIFACT_FORMAT_VERSION,
        identity: identity.clone(),
        identity_content_id,
        payload_sha256: format!("{:x}", hasher.finalize()),
        node_count,
        channel_count,
        validity_bytes: node_count,
        coverage_values,
    })
}

fn write_prepared(
    path: &Path,
    header: &InverseLutArtifactHeader,
    validity: &[bool],
    coverages: &[f32],
) -> Result<(), String> {
    let header_bytes = serde_json::to_vec(header)
        .map_err(|err| format!("Cannot serialize inverse LUT artifact header: {err}"))?;
    if header_bytes.is_empty() || header_bytes.len() > MAX_INVERSE_LUT_ARTIFACT_HEADER_BYTES {
        return Err("Serialized inverse LUT artifact header exceeds bounded size.".to_owned());
    }
    let expected_len = expected_file_len(header, header_bytes.len())?;
    if expected_len > MAX_INVERSE_LUT_ARTIFACT_BYTES {
        return Err(format!("Inverse LUT artifact would be {expected_len} bytes; bounded maximum is {MAX_INVERSE_LUT_ARTIFACT_BYTES}."));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("Cannot create inverse LUT folder {}: {err}", parent.display()))?;
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|err| format!("Cannot create inverse LUT artifact {}: {err}", path.display()))?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&ARTIFACT_MAGIC).map_err(|err| err.to_string())?;
    writer.write_all(&INVERSE_LUT_ARTIFACT_FORMAT_VERSION.to_le_bytes()).map_err(|err| err.to_string())?;
    writer.write_all(&(header_bytes.len() as u32).to_le_bytes()).map_err(|err| err.to_string())?;
    writer.write_all(&header_bytes).map_err(|err| err.to_string())?;
    for valid in validity {
        writer.write_all(&[u8::from(*valid)]).map_err(|err| err.to_string())?;
    }
    for (index, value) in coverages.iter().copied().enumerate() {
        writer.write_all(&canonical_coverage_bytes(value, index)?)
            .map_err(|err| err.to_string())?;
    }
    writer.flush().map_err(|err| err.to_string())?;
    let file = writer.into_inner().map_err(|err| err.to_string())?;
    file.sync_all().map_err(|err| err.to_string())?;
    let persisted_len = file.metadata().map_err(|err| err.to_string())?.len();
    if persisted_len != expected_len {
        return Err(format!(
            "Inverse LUT persisted length mismatch: expected {expected_len}, got {persisted_len}."
        ));
    }
    Ok(())
}

fn validate_header(header: &InverseLutArtifactHeader, header_len: usize, file_len: u64) -> Result<(), String> {
    if header.format_version != INVERSE_LUT_ARTIFACT_FORMAT_VERSION {
        return Err("Inverse LUT header format version does not match binary prefix.".to_owned());
    }
    header.identity.validate().map_err(|errors| errors.join("\n"))?;
    let expected_identity_id = header.identity.content_id()?;
    if header.identity_content_id != expected_identity_id {
        return Err(format!(
            "Inverse LUT identity content-id mismatch: header {}, identity {}.",
            header.identity_content_id, expected_identity_id
        ));
    }
    if !is_bare_sha256(&header.payload_sha256) {
        return Err("Inverse LUT payload_sha256 must be canonical lowercase 64-character hex.".to_owned());
    }
    let expected_nodes = header.identity.build_policy.grid.node_count()
        .ok_or_else(|| "Inverse LUT identity node count overflowed.".to_owned())?;
    if header.node_count != expected_nodes || header.validity_bytes != expected_nodes {
        return Err("Inverse LUT header node/validity counts do not match identity grid.".to_owned());
    }
    if usize::from(header.channel_count) != header.identity.channel_names.len() {
        return Err("Inverse LUT header channel count does not match identity topology.".to_owned());
    }
    let expected_values = expected_nodes
        .checked_mul(u64::from(header.channel_count))
        .ok_or_else(|| "Inverse LUT header coverage count overflowed.".to_owned())?;
    if header.coverage_values != expected_values {
        return Err("Inverse LUT header coverage count does not match grid topology.".to_owned());
    }
    let expected_len = expected_file_len(header, header_len)?;
    if expected_len != file_len {
        return Err(format!(
            "Inverse LUT file length mismatch: expected {expected_len}, got {file_len}; trailing or truncated data is rejected."
        ));
    }
    Ok(())
}

fn expected_file_len(header: &InverseLutArtifactHeader, header_len: usize) -> Result<u64, String> {
    FIXED_PREFIX_BYTES
        .checked_add(header_len as u64)
        .and_then(|value| value.checked_add(header.validity_bytes))
        .and_then(|value| value.checked_add(header.coverage_values.checked_mul(4)?))
        .ok_or_else(|| "Inverse LUT artifact length overflowed u64.".to_owned())
}

fn canonical_coverage_bytes(value: f32, index: usize) -> Result<[u8; 4], String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("Inverse LUT coverage {index} must be finite and in 0..=1."));
    }
    let canonical = if value == 0.0 { 0.0 } else { value };
    Ok(canonical.to_bits().to_le_bytes())
}

fn validate_stored_coverage(value: f32, bytes: [u8; 4], index: usize) -> Result<(), String> {
    if !value.is_finite() || !(0.0..=1.0).contains(&value) {
        return Err(format!("Stored inverse LUT coverage {index} is not finite normalized data."));
    }
    if value == 0.0 && u32::from_le_bytes(bytes) != 0 {
        return Err(format!("Stored inverse LUT coverage {index} uses non-canonical negative zero."));
    }
    Ok(())
}

fn verify_existing_matches(path: &Path, expected: &InverseLutArtifactHeader) -> Result<(), String> {
    let loaded = load_inverse_lut_artifact(path)?;
    verify_loaded_matches(&loaded, expected)
}

fn verify_loaded_matches(loaded: &VerifiedInverseLutArtifact, expected: &InverseLutArtifactHeader) -> Result<(), String> {
    if loaded.identity_content_id != expected.identity_content_id
        || loaded.payload_sha256 != expected.payload_sha256
        || loaded.identity != expected.identity
    {
        return Err("Existing inverse LUT cache object does not exactly match requested identity/payload.".to_owned());
    }
    Ok(())
}

fn unique_staged_path(destination: &Path) -> Result<PathBuf, String> {
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|err| format!("Cannot create inverse LUT cache folder {}: {err}", parent.display()))?;
    let file_name = destination.file_name().and_then(|value| value.to_str()).unwrap_or("inverse-lut");
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for _ in 0..64 {
        let sequence = STAGE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{file_name}.{}.{}.{}.tmp", std::process::id(), timestamp, sequence));
        if !path.exists() {
            return Ok(path);
        }
    }
    Err("Cannot allocate a unique inverse LUT staging path.".to_owned())
}

fn read_u32_le<R: Read>(reader: &mut R) -> Result<u32, String> {
    let mut bytes = [0u8; 4];
    reader.read_exact(&mut bytes).map_err(|err| err.to_string())?;
    Ok(u32::from_le_bytes(bytes))
}

fn is_bare_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
