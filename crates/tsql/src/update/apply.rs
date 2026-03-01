use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::Archive;

use super::types::UpdateInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyResult {
    pub from: semver::Version,
    pub to: semver::Version,
    pub backup_path: PathBuf,
}

pub fn apply_update(info: &UpdateInfo) -> Result<ApplyResult> {
    let current = std::env::current_exe().context("Failed to locate current executable")?;
    apply_update_to_path(info, &current)
}

fn apply_update_to_path(info: &UpdateInfo, current_executable: &Path) -> Result<ApplyResult> {
    if cfg!(windows) {
        bail!("In-app apply is not supported on Windows yet");
    }

    let asset_url = info
        .asset_url
        .as_deref()
        .context("No release asset URL available for this update")?;
    let checksum_url = info
        .checksum_url
        .as_deref()
        .context("No checksum URL available for this update")?;
    let asset_filename = filename_from_url(asset_url)
        .context("Could not determine archive filename from asset URL")?;

    let temp_dir = tempfile::tempdir().context("Failed to create temporary update directory")?;
    let archive_path = temp_dir.path().join(&asset_filename);

    download_to_path(asset_url, &archive_path)?;

    let checksum_text = download_text(checksum_url)?;
    verify_archive_checksum(&archive_path, &asset_filename, &checksum_text)?;

    let extracted_binary = extract_binary_from_archive(&archive_path, temp_dir.path())?;
    let backup_path = replace_executable_at_path(current_executable, &extracted_binary)?;

    Ok(ApplyResult {
        from: info.current.clone(),
        to: info.latest.clone(),
        backup_path,
    })
}

fn download_to_path(url: &str, destination: &Path) -> Result<()> {
    let agent = build_agent();
    let response = agent
        .get(url)
        .set("User-Agent", "tsql-updater")
        .call()
        .with_context(|| format!("Failed to download update archive from {}", url))?;

    let mut reader = response.into_reader();
    let mut output = fs::File::create(destination)
        .with_context(|| format!("Failed to create {}", destination.display()))?;

    io::copy(&mut reader, &mut output)
        .with_context(|| format!("Failed to write archive to {}", destination.display()))?;
    output
        .flush()
        .with_context(|| format!("Failed to flush {}", destination.display()))?;
    Ok(())
}

fn download_text(url: &str) -> Result<String> {
    let agent = build_agent();
    let response = agent
        .get(url)
        .set("User-Agent", "tsql-updater")
        .call()
        .with_context(|| format!("Failed to download checksums from {}", url))?;

    response
        .into_string()
        .with_context(|| format!("Failed to read checksum response from {}", url))
}

fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout_read(Duration::from_secs(15))
        .timeout_write(Duration::from_secs(15))
        .build()
}

fn verify_archive_checksum(archive_path: &Path, filename: &str, checksums: &str) -> Result<()> {
    let expected = checksum_for_asset(checksums, filename)
        .ok_or_else(|| anyhow!("Checksum for '{}' not found in checksum file", filename))?;
    let actual = sha256_hex(archive_path)?;

    if expected != actual {
        bail!(
            "Checksum mismatch for '{}': expected {}, got {}",
            filename,
            expected,
            actual
        );
    }
    Ok(())
}

fn sha256_hex(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Failed to open {} for checksum", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];

    loop {
        let n = file
            .read(&mut buffer)
            .with_context(|| format!("Failed reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_binary_from_archive(archive_path: &Path, out_dir: &Path) -> Result<PathBuf> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let mut tar = Archive::new(GzDecoder::new(file));

    let output_binary = out_dir.join("tsql.new");

    for entry in tar.entries().context("Failed reading archive entries")? {
        let mut entry = entry.context("Failed to read archive entry")?;
        let path = entry.path().context("Failed to read archive entry path")?;

        if path.file_name().is_some_and(|name| name == "tsql") {
            entry.unpack(&output_binary).with_context(|| {
                format!("Failed to unpack binary to {}", output_binary.display())
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&output_binary)
                    .with_context(|| format!("Failed to stat {}", output_binary.display()))?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&output_binary, perms).with_context(|| {
                    format!(
                        "Failed to set executable permissions on {}",
                        output_binary.display()
                    )
                })?;
            }

            return Ok(output_binary);
        }
    }

    bail!("Could not find 'tsql' binary in release archive")
}

fn replace_executable_at_path(current: &Path, new_binary: &Path) -> Result<PathBuf> {
    let parent = current
        .parent()
        .ok_or_else(|| anyhow!("Current executable has no parent directory"))?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_path = parent.join(format!("tsql.backup.{}", timestamp));

    fs::rename(current, &backup_path).with_context(|| {
        format!(
            "Failed to move current binary from {} to {}",
            current.display(),
            backup_path.display()
        )
    })?;

    if let Err(error) = fs::copy(new_binary, current).with_context(|| {
        format!(
            "Failed to install new binary from {} to {}",
            new_binary.display(),
            current.display()
        )
    }) {
        if let Err(rollback_error) = rollback_to_backup(current, &backup_path) {
            return Err(error.context(format!(
                "Additionally, rollback failed while restoring {} from {}: {}",
                current.display(),
                backup_path.display(),
                rollback_error
            )));
        }
        return Err(error);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(current)
            .with_context(|| format!("Failed to stat {}", current.display()))?
            .permissions();
        perms.set_mode(0o755);
        if let Err(error) = fs::set_permissions(current, perms)
            .with_context(|| format!("Failed to set permissions on {}", current.display()))
        {
            if let Err(rollback_error) = rollback_to_backup(current, &backup_path) {
                return Err(error.context(format!(
                    "Additionally, rollback after permission failure failed while restoring {} from {}: {}",
                    current.display(),
                    backup_path.display(),
                    rollback_error
                )));
            }
            return Err(error);
        }
    }

    Ok(backup_path)
}

fn rollback_to_backup(current: &Path, backup_path: &Path) -> Result<()> {
    if current.exists() {
        fs::remove_file(current).with_context(|| {
            format!(
                "Failed to remove partially installed binary {}",
                current.display()
            )
        })?;
    }

    fs::rename(backup_path, current).with_context(|| {
        format!(
            "Failed to restore backup from {} to {}",
            backup_path.display(),
            current.display()
        )
    })?;

    Ok(())
}

fn checksum_for_asset(checksums: &str, filename: &str) -> Option<String> {
    for line in checksums.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let raw_name = parts.next()?;
        let raw_name = raw_name.trim_start_matches('*');

        let matches = raw_name == filename
            || Path::new(raw_name)
                .file_name()
                .is_some_and(|name| name == filename);

        if matches {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

fn filename_from_url(url: &str) -> Option<String> {
    let path = url::Url::parse(url).ok()?.path().to_string();
    Path::new(&path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::thread;

    use semver::Version;

    use super::*;
    use tempfile::tempdir;

    struct TestHttpServer {
        base_url: String,
        handle: Option<thread::JoinHandle<()>>,
    }

    impl Drop for TestHttpServer {
        fn drop(&mut self) {
            if let Some(handle) = self.handle.take() {
                let _ = handle.join();
            }
        }
    }

    fn start_test_http_server(routes: HashMap<String, Vec<u8>>) -> TestHttpServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        listener
            .set_nonblocking(true)
            .expect("set nonblocking listener");
        let address = listener.local_addr().expect("read bound address");

        let handle = thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut served = 0_usize;

            while served < routes.len() && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _peer)) => {
                        served += 1;

                        let mut request_buffer = [0_u8; 8192];
                        let bytes_read = stream.read(&mut request_buffer).unwrap_or(0);
                        let request = String::from_utf8_lossy(&request_buffer[..bytes_read]);
                        let path = request
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .unwrap_or("/");

                        let (status, body): (&str, &[u8]) = if let Some(body) = routes.get(path) {
                            ("200 OK", body)
                        } else {
                            ("404 Not Found", b"not found")
                        };

                        let response_headers = format!(
                            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );

                        let _ = stream.write_all(response_headers.as_bytes());
                        let _ = stream.write_all(body);
                        let _ = stream.flush();
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        TestHttpServer {
            base_url: format!("http://{}", address),
            handle: Some(handle),
        }
    }

    fn build_test_tar_gz(binary_contents: &[u8]) -> Vec<u8> {
        let gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut archive = tar::Builder::new(gzip);

        let mut header = tar::Header::new_gnu();
        header
            .set_path("tsql")
            .expect("set tar entry path for test archive");
        header.set_size(binary_contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append(&header, binary_contents)
            .expect("append binary to test tar archive");

        let gzip = archive.into_inner().expect("finish test tar archive");
        gzip.finish().expect("finish test gzip encoding")
    }

    fn sha256_hex_bytes(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    #[test]
    fn test_filename_from_url_extracts_last_segment() {
        let filename =
            filename_from_url("https://example.com/releases/tsql-x86_64-unknown-linux-gnu.tar.gz")
                .expect("filename should be parsed");
        assert_eq!(filename, "tsql-x86_64-unknown-linux-gnu.tar.gz");
    }

    #[test]
    fn test_checksum_for_asset_matches_standard_sha256sums_format() {
        let checksums = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  tsql-aarch64-apple-darwin.tar.gz\nbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  tsql-x86_64-unknown-linux-gnu.tar.gz\n";
        let checksum = checksum_for_asset(checksums, "tsql-x86_64-unknown-linux-gnu.tar.gz")
            .expect("checksum should be found");
        assert_eq!(
            checksum,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
    }

    #[test]
    fn test_checksum_for_asset_supports_asterisk_prefixed_names() {
        let checksums = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc *tsql-x86_64-unknown-linux-gnu.tar.gz\n";
        let checksum = checksum_for_asset(checksums, "tsql-x86_64-unknown-linux-gnu.tar.gz")
            .expect("checksum should be found");
        assert_eq!(
            checksum,
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_apply_update_end_to_end_download_verify_extract_and_replace() {
        let dir = tempdir().expect("tempdir");
        let current = dir.path().join("tsql-current");
        fs::write(&current, b"old-binary").expect("write current binary");

        let archive_filename = "tsql-x86_64-unknown-linux-gnu.tar.gz";
        let archive_bytes = build_test_tar_gz(b"new-binary");
        let checksums = format!(
            "{}  {}\n",
            sha256_hex_bytes(&archive_bytes),
            archive_filename
        );

        let mut routes = HashMap::new();
        routes.insert(format!("/{}", archive_filename), archive_bytes);
        routes.insert("/SHA256SUMS".to_string(), checksums.into_bytes());
        let server = start_test_http_server(routes);

        let info = UpdateInfo {
            current: Version::new(0, 4, 2),
            latest: Version::new(0, 4, 3),
            notes_url: None,
            asset_url: Some(format!("{}/{}", server.base_url, archive_filename)),
            checksum_url: Some(format!("{}/SHA256SUMS", server.base_url)),
        };

        let result = apply_update_to_path(&info, &current).expect("apply should succeed");

        assert_eq!(result.from, Version::new(0, 4, 2));
        assert_eq!(result.to, Version::new(0, 4, 3));
        assert_eq!(
            fs::read_to_string(&current).expect("read installed binary"),
            "new-binary"
        );
        assert_eq!(
            fs::read_to_string(&result.backup_path).expect("read backup binary"),
            "old-binary"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn test_apply_update_end_to_end_rejects_bad_checksum_without_replacing_binary() {
        let dir = tempdir().expect("tempdir");
        let current = dir.path().join("tsql-current");
        fs::write(&current, b"old-binary").expect("write current binary");

        let archive_filename = "tsql-x86_64-unknown-linux-gnu.tar.gz";
        let archive_bytes = build_test_tar_gz(b"new-binary");
        let checksums = format!("{:064x}  {}\n", 0_u8, archive_filename);

        let mut routes = HashMap::new();
        routes.insert(format!("/{}", archive_filename), archive_bytes);
        routes.insert("/SHA256SUMS".to_string(), checksums.into_bytes());
        let server = start_test_http_server(routes);

        let info = UpdateInfo {
            current: Version::new(0, 4, 2),
            latest: Version::new(0, 4, 3),
            notes_url: None,
            asset_url: Some(format!("{}/{}", server.base_url, archive_filename)),
            checksum_url: Some(format!("{}/SHA256SUMS", server.base_url)),
        };

        let error = apply_update_to_path(&info, &current).expect_err("checksum mismatch expected");
        let error_message = format!("{error:#}");
        assert!(
            error_message.contains("Checksum mismatch"),
            "unexpected error: {error_message}"
        );
        assert_eq!(
            fs::read_to_string(&current).expect("read unchanged current binary"),
            "old-binary"
        );
        let backup_count = fs::read_dir(dir.path())
            .expect("read directory")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("tsql.backup.")
            })
            .count();
        assert_eq!(
            backup_count, 0,
            "backup should not be created on checksum failure"
        );
    }

    #[test]
    fn test_replace_executable_at_path_swaps_binary_and_creates_backup() {
        let dir = tempdir().expect("tempdir");
        let current = dir.path().join("tsql-current");
        let new_binary = dir.path().join("tsql-new");

        fs::write(&current, b"old-binary").expect("write current binary");
        fs::write(&new_binary, b"new-binary").expect("write new binary");

        let backup_path =
            replace_executable_at_path(&current, &new_binary).expect("replace should succeed");

        let installed = fs::read_to_string(&current).expect("read current binary");
        assert_eq!(installed, "new-binary");

        let backup = fs::read_to_string(&backup_path).expect("read backup binary");
        assert_eq!(backup, "old-binary");
    }

    #[test]
    fn test_replace_executable_at_path_rolls_back_when_copy_fails() {
        let dir = tempdir().expect("tempdir");
        let current = dir.path().join("tsql-current");
        let missing_binary = dir.path().join("missing-binary");

        fs::write(&current, b"old-binary").expect("write current binary");

        let before_backup_count = fs::read_dir(dir.path())
            .expect("read dir before")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("tsql.backup.")
            })
            .count();

        let error = replace_executable_at_path(&current, &missing_binary)
            .expect_err("replace should fail when source binary is missing");
        let error_message = format!("{error:#}");
        assert!(
            error_message.contains("Failed to install new binary"),
            "unexpected error: {error_message}"
        );

        let restored = fs::read_to_string(&current).expect("read restored binary");
        assert_eq!(restored, "old-binary");

        let after_backup_count = fs::read_dir(dir.path())
            .expect("read dir after")
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("tsql.backup.")
            })
            .count();
        assert_eq!(before_backup_count, after_backup_count);
    }
}
