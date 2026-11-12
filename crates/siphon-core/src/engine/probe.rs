use anyhow::{Context, Result};
use reqwest::header::{ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG, RANGE, USER_AGENT};
use std::time::Duration;
use url::Url;

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub effective_url: String,
    pub file_name: String,
    pub total_bytes: Option<u64>,
    pub supports_range: bool,
    pub etag: Option<String>,
    pub content_type: Option<String>,
}

pub struct HttpProbe {
    client: reqwest::Client,
}

impl Default for HttpProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpProbe {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("Failed to build HTTP client");
        Self { client }
    }

    pub async fn probe(&self, raw_url: &str) -> Result<ProbeResult> {
        let parsed_url = Url::parse(raw_url).context("Invalid URL format")?;
        let user_agent = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36 Siphon/0.1.0";

        // Step 1: Attempt HEAD request
        let head_res = self
            .client
            .head(parsed_url.clone())
            .header(USER_AGENT, user_agent)
            .send()
            .await;

        if let Ok(res) = head_res {
            if res.status().is_success() {
                let effective_url = res.url().to_string();
                let headers = res.headers();

                let total_bytes = headers
                    .get(CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok());

                let supports_range = headers
                    .get(ACCEPT_RANGES)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.to_lowercase().contains("bytes"))
                    .unwrap_or(false);

                let etag = headers
                    .get(ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.to_string());

                let content_type = headers
                    .get(CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.to_string());

                let cd_header = headers
                    .get(CONTENT_DISPOSITION)
                    .and_then(|v| v.to_str().ok());

                let file_name = Self::extract_filename(cd_header, &effective_url);

                // If supports_range wasn't explicitly advertised in HEAD, probe with GET Range 0-0
                if !supports_range && total_bytes.is_some() {
                    let range_check = self.probe_range_with_get(&effective_url, user_agent).await;
                    if let Ok(Some((verified_range, verified_len))) = range_check {
                        return Ok(ProbeResult {
                            effective_url,
                            file_name,
                            total_bytes: verified_len.or(total_bytes),
                            supports_range: verified_range,
                            etag,
                            content_type,
                        });
                    }
                }

                return Ok(ProbeResult {
                    effective_url,
                    file_name,
                    total_bytes,
                    supports_range,
                    etag,
                    content_type,
                });
            }
        }

        // Step 2: Fallback to GET with Range: bytes=0-0 (handles servers blocking HEAD)
        let get_res = self
            .client
            .get(parsed_url.clone())
            .header(USER_AGENT, user_agent)
            .header(RANGE, "bytes=0-0")
            .send()
            .await
            .context("Failed to probe URL with GET request")?;

        let effective_url = get_res.url().to_string();
        let headers = get_res.headers();
        let status = get_res.status();

        let mut supports_range = status == reqwest::StatusCode::PARTIAL_CONTENT;
        if !supports_range {
            if let Some(ar) = headers.get(ACCEPT_RANGES).and_then(|v| v.to_str().ok()) {
                if ar.to_lowercase().contains("bytes") {
                    supports_range = true;
                }
            }
        }

        let mut total_bytes = None;
        if let Some(cr) = headers.get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) {
            // Format: bytes 0-0/123456
            if let Some(pos) = cr.rfind('/') {
                let len_str = &cr[pos + 1..];
                if let Ok(len) = len_str.trim().parse::<u64>() {
                    total_bytes = Some(len);
                }
            }
        }

        if total_bytes.is_none() {
            total_bytes = headers
                .get(CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
        }

        let etag = headers
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());

        let cd_header = headers
            .get(CONTENT_DISPOSITION)
            .and_then(|v| v.to_str().ok());

        let file_name = Self::extract_filename(cd_header, &effective_url);

        Ok(ProbeResult {
            effective_url,
            file_name,
            total_bytes,
            supports_range,
            etag,
            content_type,
        })
    }

    async fn probe_range_with_get(
        &self,
        url: &str,
        user_agent: &str,
    ) -> Result<Option<(bool, Option<u64>)>> {
        let res = self
            .client
            .get(url)
            .header(USER_AGENT, user_agent)
            .header(RANGE, "bytes=0-0")
            .send()
            .await?;

        if res.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let mut total_bytes = None;
            if let Some(cr) = res.headers().get(CONTENT_RANGE).and_then(|v| v.to_str().ok()) {
                if let Some(pos) = cr.rfind('/') {
                    let len_str = &cr[pos + 1..];
                    if let Ok(len) = len_str.trim().parse::<u64>() {
                        total_bytes = Some(len);
                    }
                }
            }
            Ok(Some((true, total_bytes)))
        } else {
            Ok(Some((false, None)))
        }
    }

    fn extract_filename(cd_header: Option<&str>, url_str: &str) -> String {
        if let Some(cd) = cd_header {
            // Check filename*=UTF-8''foo.ext
            if let Some(idx) = cd.to_lowercase().find("filename*=") {
                let val = &cd[idx + 10..];
                let raw_name = val.split(';').next().unwrap_or("").trim().trim_matches('"');
                if let Some(enc_idx) = raw_name.find("''") {
                    let encoded = &raw_name[enc_idx + 2..];
                    if let Ok(decoded) = percent_encoding::percent_decode_str(encoded).decode_utf8() {
                        let name = decoded.trim().trim_matches('"');
                        if !name.is_empty() {
                            return sanitize_filename(name);
                        }
                    }
                }
            }

            // Check filename="foo.ext"
            if let Some(idx) = cd.to_lowercase().find("filename=") {
                let val = &cd[idx + 9..];
                let name = val.split(';').next().unwrap_or("").trim().trim_matches('"');
                if !name.is_empty() {
                    return sanitize_filename(name);
                }
            }
        }

        // Fallback: extract last path segment from URL
        if let Ok(parsed) = Url::parse(url_str) {
            let path_segments: Vec<&str> = parsed
                .path_segments()
                .map(|s| s.filter(|seg| !seg.is_empty()).collect())
                .unwrap_or_default();

            if let Some(last) = path_segments.last() {
                if let Ok(decoded) = percent_encoding::percent_decode_str(last).decode_utf8() {
                    let name = sanitize_filename(decoded.trim());
                    if !name.is_empty() {
                        return name;
                    }
                }
            }
        }

        "download.bin".to_string()
    }
}

fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '?' | '%' | '*' | ':' | '|' | '"' | '<' | '>' => '_',
            other => other,
        })
        .collect();
    if sanitized.is_empty() {
        "download.bin".to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_filename_from_url() {
        assert_eq!(
            HttpProbe::extract_filename(None, "https://example.com/files/archive.tar.gz"),
            "archive.tar.gz"
        );
        assert_eq!(
            HttpProbe::extract_filename(None, "https://example.com/path/"),
            "path"
        );
        assert_eq!(
            HttpProbe::extract_filename(None, "https://example.com/"),
            "download.bin"
        );
    }

    #[test]
    fn test_extract_filename_from_content_disposition() {
        assert_eq!(
            HttpProbe::extract_filename(
                Some("attachment; filename=\"custom-archive.zip\""),
                "https://example.com/download"
            ),
            "custom-archive.zip"
        );
        assert_eq!(
            HttpProbe::extract_filename(
                Some("attachment; filename*=UTF-8''my%20file.pdf"),
                "https://example.com/download"
            ),
            "my file.pdf"
        );
    }
}
