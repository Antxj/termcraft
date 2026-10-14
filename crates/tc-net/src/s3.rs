//! "Aws S3" session backend: browse buckets and objects like folders and
//! files, with the same [`FsEvent`]s as the SFTP / FTP browsers.
//!
//! Paths: `/` lists the buckets, `/bucket/a/b` is the key `a/b` (a
//! "folder" is a common prefix). Requests are signed with AWS Signature
//! Version 4 over HTTPS (virtual-hosted style; path style for bucket names
//! with dots). Uploads are single PUTs (up to 5 GB).

use crate::Notify;
use crossbeam_channel::{Receiver, Sender};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use tc_core::remote_fs::{FsEvent, FsRequest, RemoteEntry};

#[derive(Clone)]
pub struct S3Params {
    pub key_id: String,
    pub secret: zeroize::Zeroizing<String>,
    /// Where `ListBuckets` goes; buckets then use their own region.
    pub region: String,
    pub start_path: Option<String>,
    /// An S3-compatible endpoint (`http://127.0.0.1:9000`) used with path
    /// style requests; `None` = Amazon S3.
    pub endpoint: Option<String>,
}

impl std::fmt::Debug for S3Params {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Params")
            .field("key_id", &self.key_id)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

// ------------------------------------------------------------- signing

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding as SigV4 wants it (`/` kept in paths).
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (keep_slash && b == b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `YYYYMMDD'T'HHMMSS'Z'` for a Unix time (UTC).
pub fn amz_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Unix time of an ISO 8601 UTC date (`2024-05-01T10:20:30.000Z`).
pub fn parse_iso8601(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        std::str::from_utf8(b.get(r)?).ok()?.parse().ok()
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Days from civil (inverse of the above).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// A request to sign.
pub struct Signing<'a> {
    pub method: &'a str,
    pub host: &'a str,
    /// Already URI-encoded path (`/bucket/key`).
    pub path: &'a str,
    /// Query parameters, not encoded.
    pub query: &'a [(&'a str, String)],
    /// Extra headers to sign (lower-case names).
    pub headers: &'a [(&'a str, String)],
    pub payload_hash: &'a str,
    pub region: &'a str,
    pub service: &'a str,
    pub date: &'a str,
}

/// The `Authorization` header value (AWS Signature Version 4).
pub fn authorization(key_id: &str, secret: &str, r: &Signing<'_>) -> String {
    let mut query: Vec<(String, String)> = r
        .query
        .iter()
        .map(|(k, v)| (uri_encode(k, false), uri_encode(v, false)))
        .collect();
    query.sort();
    let canonical_query = query
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let mut headers: Vec<(String, String)> = vec![
        ("host".into(), r.host.to_string()),
        ("x-amz-date".into(), r.date.to_string()),
    ];
    headers.extend(
        r.headers
            .iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string())),
    );
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed: Vec<&str> = headers.iter().map(|(k, _)| k.as_str()).collect();
    let signed = signed.join(";");
    let canonical = format!(
        "{}\n{}\n{canonical_query}\n{canonical_headers}\n{signed}\n{}",
        r.method, r.path, r.payload_hash
    );
    let day = &r.date[..8];
    let scope = format!("{day}/{}/{}/aws4_request", r.region, r.service);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        r.date,
        sha256_hex(canonical.as_bytes())
    );
    let k_date = hmac(format!("AWS4{secret}").as_bytes(), day);
    let k_region = hmac(&k_date, r.region);
    let k_service = hmac(&k_region, r.service);
    let k_signing = hmac(&k_service, "aws4_request");
    let signature = hex(&hmac(&k_signing, &to_sign));
    format!(
        "AWS4-HMAC-SHA256 Credential={key_id}/{scope}, SignedHeaders={signed}, Signature={signature}"
    )
}

// ------------------------------------------------------------------ XML

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Inner texts of every `<tag>…</tag>` (not nested in themselves).
pub fn elements<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        out.push(&after[..end]);
        rest = &after[end + close.len()..];
    }
    out
}

fn element(xml: &str, tag: &str) -> Option<String> {
    elements(xml, tag).first().map(|s| unescape(s))
}

/// `ListAllMyBucketsResult` → bucket names.
pub fn parse_buckets(xml: &str) -> Vec<RemoteEntry> {
    elements(xml, "Bucket")
        .iter()
        .filter_map(|b| {
            Some(RemoteEntry {
                name: element(b, "Name")?,
                is_dir: true,
                size: 0,
                modified: element(b, "CreationDate").and_then(|d| parse_iso8601(&d)),
                permissions: None,
                owner: None,
                group: None,
            })
        })
        .collect()
}

/// One `ListObjectsV2` page under `prefix`: folders, files and the next
/// continuation token.
pub fn parse_objects(xml: &str, prefix: &str) -> (Vec<RemoteEntry>, Option<String>) {
    let mut entries: Vec<RemoteEntry> = elements(xml, "CommonPrefixes")
        .iter()
        .filter_map(|p| element(p, "Prefix"))
        .filter_map(|p| {
            let name = p.strip_prefix(prefix)?.trim_end_matches('/').to_string();
            (!name.is_empty()).then_some(RemoteEntry {
                name,
                is_dir: true,
                size: 0,
                modified: None,
                permissions: None,
                owner: None,
                group: None,
            })
        })
        .collect();
    entries.extend(elements(xml, "Contents").iter().filter_map(|c| {
        let key = element(c, "Key")?;
        let name = key.strip_prefix(prefix)?.to_string();
        // The folder marker itself ("prefix/") is not listed.
        (!name.is_empty() && !name.contains('/')).then(|| RemoteEntry {
            name,
            is_dir: false,
            size: element(c, "Size").and_then(|s| s.parse().ok()).unwrap_or(0),
            modified: element(c, "LastModified").and_then(|d| parse_iso8601(&d)),
            permissions: None,
            owner: element(c, "DisplayName"),
            group: element(c, "StorageClass"),
        })
    }));
    let next = (element(xml, "IsTruncated").as_deref() == Some("true"))
        .then(|| element(xml, "NextContinuationToken"))
        .flatten();
    (entries, next)
}

/// `/bucket/a/b` → (`bucket`, `a/b`); `/` → None.
pub fn split_path(path: &str) -> Option<(String, String)> {
    let p = path.trim_start_matches('/');
    if p.is_empty() {
        return None;
    }
    let (bucket, key) = p.split_once('/').unwrap_or((p, ""));
    Some((bucket.to_string(), key.to_string()))
}

// --------------------------------------------------------------- client

const UNSIGNED: &str = "UNSIGNED-PAYLOAD";
/// Objects listed per folder at most.
const MAX_LISTED: usize = 20_000;

struct Client {
    agent: ureq::Agent,
    params: S3Params,
    /// Bucket → region (from `GetBucketLocation`).
    regions: HashMap<String, String>,
}

struct Reply {
    status: u16,
    body: ureq::Body,
}

impl Client {
    fn new(params: S3Params) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .build()
            .into();
        Self {
            agent,
            params,
            regions: HashMap::new(),
        }
    }

    fn now() -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        amz_date(secs)
    }

    /// Scheme, host and encoded path of a bucket / key request.
    fn location_for(
        &self,
        bucket: Option<&str>,
        key: &str,
        region: &str,
    ) -> (&'static str, String, String) {
        if let Some(base) = &self.params.endpoint {
            let (scheme, host) = match base.split_once("://") {
                Some(("http", h)) => ("http", h),
                Some((_, h)) => ("https", h),
                None => ("https", base.as_str()),
            };
            let path = match bucket {
                None => "/".to_string(),
                Some(b) => format!("/{}/{}", uri_encode(b, false), uri_encode(key, true)),
            };
            return (scheme, host.trim_end_matches('/').to_string(), path);
        }
        let (host, path) = Self::location(bucket, key, region);
        ("https", host, path)
    }

    /// Host and encoded path of a bucket / key request.
    fn location(bucket: Option<&str>, key: &str, region: &str) -> (String, String) {
        let regional = if region == "us-east-1" {
            "s3.amazonaws.com".to_string()
        } else {
            format!("s3.{region}.amazonaws.com")
        };
        match bucket {
            None => (regional, "/".into()),
            // Dots break the TLS wildcard certificate: path style then.
            Some(b) if b.contains('.') => (
                regional,
                format!("/{}/{}", uri_encode(b, false), uri_encode(key, true)),
            ),
            Some(b) => (
                format!("{b}.{regional}"),
                format!("/{}", uri_encode(key, true)),
            ),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn send(
        &self,
        method: &str,
        bucket: Option<&str>,
        key: &str,
        region: &str,
        query: &[(&str, String)],
        extra: &[(&str, String)],
        body: Option<(&mut dyn Read, u64)>,
    ) -> Result<Reply, String> {
        let (scheme, host, path) = self.location_for(bucket, key, region);
        let date = Self::now();
        let mut headers: Vec<(&str, String)> = vec![("x-amz-content-sha256", UNSIGNED.into())];
        headers.extend(extra.iter().cloned());
        let auth = authorization(
            &self.params.key_id,
            &self.params.secret,
            &Signing {
                method,
                host: &host,
                path: &path,
                query,
                headers: &headers,
                payload_hash: UNSIGNED,
                region,
                service: "s3",
                date: &date,
            },
        );
        let mut url = format!("{scheme}://{host}{path}");
        if !query.is_empty() {
            let q: Vec<String> = query
                .iter()
                .map(|(k, v)| format!("{}={}", uri_encode(k, false), uri_encode(v, false)))
                .collect();
            url.push('?');
            url.push_str(&q.join("&"));
        }
        let builder = http_request(method, &url)
            .header("x-amz-date", &date)
            .header("authorization", &auth);
        let builder = headers
            .iter()
            .fold(builder, |b, (k, v)| b.header(*k, v.as_str()));
        let response = match body {
            Some((reader, len)) => {
                let request = builder
                    .header("content-length", len.to_string())
                    .body(ureq::SendBody::from_reader(reader))
                    .map_err(|e| e.to_string())?;
                self.agent.run(request)
            }
            None => {
                let request = builder.body(()).map_err(|e| e.to_string())?;
                self.agent.run(request)
            }
        }
        .map_err(|e| e.to_string())?;
        let status = response.status().as_u16();
        Ok(Reply {
            status,
            body: response.into_body(),
        })
    }

    /// Text of an answer; an S3 error becomes its message.
    fn text(reply: Reply) -> Result<String, String> {
        let mut text = String::new();
        reply
            .body
            .into_reader()
            .take(32 * 1024 * 1024)
            .read_to_string(&mut text)
            .map_err(|e| e.to_string())?;
        if (200..300).contains(&reply.status) {
            Ok(text)
        } else {
            Err(match (element(&text, "Code"), element(&text, "Message")) {
                (Some(c), Some(m)) => format!("{c}: {m}"),
                _ => format!("HTTP {}", reply.status),
            })
        }
    }

    fn region(&mut self, bucket: &str) -> String {
        if let Some(r) = self.regions.get(bucket) {
            return r.clone();
        }
        let base = self.params.region.clone();
        let region = self
            .send(
                "GET",
                Some(bucket),
                "",
                &base,
                &[("location", String::new())],
                &[],
                None,
            )
            .and_then(Self::text)
            .ok()
            .map(|xml| element(&xml, "LocationConstraint").unwrap_or_default())
            .map(|r| if r.is_empty() { "us-east-1".into() } else { r })
            .unwrap_or(base);
        self.regions.insert(bucket.to_string(), region.clone());
        region
    }

    fn list(&mut self, path: &str) -> Result<Vec<RemoteEntry>, String> {
        let Some((bucket, key)) = split_path(path) else {
            let region = self.params.region.clone();
            let xml = Self::text(self.send("GET", None, "", &region, &[], &[], None)?)?;
            return Ok(parse_buckets(&xml));
        };
        let region = self.region(&bucket);
        let prefix = if key.is_empty() || key.ends_with('/') {
            key
        } else {
            format!("{key}/")
        };
        let mut entries = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![
                ("list-type", "2".to_string()),
                ("delimiter", "/".to_string()),
                ("prefix", prefix.clone()),
            ];
            if let Some(t) = &token {
                query.push(("continuation-token", t.clone()));
            }
            let xml =
                Self::text(self.send("GET", Some(&bucket), "", &region, &query, &[], None)?)?;
            let (page, next) = parse_objects(&xml, &prefix);
            entries.extend(page);
            match next {
                Some(t) if entries.len() < MAX_LISTED => token = Some(t),
                _ => break,
            }
        }
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(entries)
    }

    fn object(&mut self, path: &str) -> Result<(String, String, String), String> {
        let (bucket, key) = split_path(path).ok_or("choose a bucket first")?;
        if key.is_empty() {
            return Err("this is a bucket".into());
        }
        let region = self.region(&bucket);
        Ok((bucket, key, region))
    }

    fn put_empty(&mut self, path: &str) -> Result<(), String> {
        let (bucket, key, region) = self.object(path)?;
        let mut empty: &[u8] = &[];
        Self::text(self.send(
            "PUT",
            Some(&bucket),
            &key,
            &region,
            &[],
            &[],
            Some((&mut empty, 0)),
        )?)
        .map(|_| ())
    }

    fn delete(&mut self, path: &str) -> Result<(), String> {
        let (bucket, key, region) = self.object(path)?;
        Self::text(self.send("DELETE", Some(&bucket), &key, &region, &[], &[], None)?).map(|_| ())
    }

    fn execute(
        &mut self,
        id: u64,
        request: FsRequest,
        events: &Sender<FsEvent>,
    ) -> Result<Option<FsEvent>, String> {
        match request {
            FsRequest::List(path) => {
                let entries = self.list(&path)?;
                Ok(Some(FsEvent::Listing { id, path, entries }))
            }
            FsRequest::Download { remote, local, .. } => {
                let (bucket, key, region) = self.object(&remote)?;
                let reply = self.send("GET", Some(&bucket), &key, &region, &[], &[], None)?;
                if !(200..300).contains(&reply.status) {
                    return Self::text(reply).map(|_| None);
                }
                let total = reply.body.content_length();
                let mut reader = reply.body.into_reader();
                let mut file = std::fs::File::create(&local)
                    .map_err(|e| format!("{}: {e}", local.display()))?;
                let mut buf = vec![0u8; 64 * 1024];
                let mut done = 0u64;
                loop {
                    let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
                    done += n as u64;
                    let _ = events.send(FsEvent::Progress { id, done, total });
                }
                Ok(None)
            }
            FsRequest::Upload { local, remote, .. } => {
                let (bucket, key, region) = self.object(&remote)?;
                let file =
                    std::fs::File::open(&local).map_err(|e| format!("{}: {e}", local.display()))?;
                let len = file.metadata().map_err(|e| e.to_string())?.len();
                let mut counting = Counting {
                    inner: file,
                    done: 0,
                    report: |done| {
                        let _ = events.send(FsEvent::Progress {
                            id,
                            done,
                            total: Some(len),
                        });
                    },
                };
                Self::text(self.send(
                    "PUT",
                    Some(&bucket),
                    &key,
                    &region,
                    &[],
                    &[],
                    Some((&mut counting, len)),
                )?)
                .map(|_| None)
            }
            FsRequest::Mkdir(path) => self
                .put_empty(&format!("{}/", path.trim_end_matches('/')))
                .map(|()| None),
            FsRequest::CreateFile(path) => self.put_empty(&path).map(|()| None),
            FsRequest::Remove { path, is_dir } => {
                if is_dir {
                    let inside = self.list(&path)?;
                    if !inside.is_empty() {
                        return Err("this folder is not empty".into());
                    }
                    self.delete(&format!("{}/", path.trim_end_matches('/')))
                        .map(|()| None)
                } else {
                    self.delete(&path).map(|()| None)
                }
            }
            FsRequest::Rename { from, to } => {
                let (src_bucket, src_key, _) = self.object(&from)?;
                let (bucket, key, region) = self.object(&to)?;
                let source = format!("/{src_bucket}/{}", uri_encode(&src_key, true));
                Self::text(self.send(
                    "PUT",
                    Some(&bucket),
                    &key,
                    &region,
                    &[],
                    &[("x-amz-copy-source", source)],
                    Some((&mut (&[] as &[u8]), 0)),
                )?)?;
                self.delete(&from).map(|()| None)
            }
            FsRequest::SetPermissions { .. } => {
                Err("permissions do not apply to S3 objects".into())
            }
        }
    }
}

fn http_request(method: &str, url: &str) -> ureq::http::request::Builder {
    ureq::http::Request::builder().method(method).uri(url)
}

struct Counting<R, F: FnMut(u64)> {
    inner: R,
    done: u64,
    report: F,
}

impl<R: Read, F: FnMut(u64)> Read for Counting<R, F> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        (self.report)(self.done);
        Ok(n)
    }
}

/// An S3 browser connection: one worker thread runs the requests in order.
pub struct S3Session {
    requests: Sender<(u64, FsRequest)>,
    events: Receiver<FsEvent>,
    next: AtomicU64,
}

impl S3Session {
    pub fn connect(params: S3Params, notify: Notify) -> Self {
        let (req_tx, req_rx) = crossbeam_channel::unbounded::<(u64, FsRequest)>();
        let (ev_tx, events) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("s3".into())
            .spawn(move || {
                let start = params.start_path.clone().unwrap_or_else(|| "/".into());
                let mut client = Client::new(params);
                // Check the keys by listing the buckets.
                match client.list("/") {
                    Ok(_) => {
                        let _ = ev_tx.send(FsEvent::Ready { home: start });
                    }
                    Err(e) => {
                        let _ = ev_tx.send(FsEvent::Unavailable(e));
                        notify();
                        return;
                    }
                }
                notify();
                for (id, request) in req_rx {
                    let event = match client.execute(id, request, &ev_tx) {
                        Ok(Some(e)) => e,
                        Ok(None) => FsEvent::Done { id },
                        Err(error) => FsEvent::Failed { id, error },
                    };
                    let _ = ev_tx.send(event);
                    notify();
                }
            })
            .expect("spawn s3 thread");
        Self {
            requests: req_tx,
            events,
            next: AtomicU64::new(1),
        }
    }
}

impl crate::FsBackend for S3Session {
    fn request(&self, request: FsRequest) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let _ = self.requests.send((id, request));
        id
    }

    fn events(&self) -> &Receiver<FsEvent> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny S3 imitation on loopback (plain HTTP): every request must be
    /// signed with the key id; objects live in memory.
    type Objects = std::sync::Arc<std::sync::Mutex<HashMap<String, Vec<u8>>>>;

    fn mock_s3() -> (String, Objects) {
        use std::io::{BufRead, BufReader, Write};
        let store = std::sync::Arc::new(std::sync::Mutex::new(HashMap::<String, Vec<u8>>::new()));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let objects = store.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let mut len = 0usize;
                let mut signed = false;
                loop {
                    let mut h = String::new();
                    reader.read_line(&mut h).unwrap();
                    let lower = h.to_ascii_lowercase();
                    if let Some(v) = lower.strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if lower.starts_with("authorization: aws4-hmac-sha256 credential=akidtest/") {
                        signed = true;
                    }
                    if h == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0u8; len];
                std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                let mut parts = line.split_whitespace();
                let (method, target) = (parts.next().unwrap(), parts.next().unwrap());
                let (path, query) = target.split_once('?').unwrap_or((target, ""));
                let key = path.trim_start_matches("/bucket/").to_string();
                let mut store = objects.lock().unwrap();
                let (status, reply): (&str, Vec<u8>) = if !signed {
                    (
                        "403 Forbidden",
                        b"<Error><Code>AccessDenied</Code><Message>unsigned</Message></Error>"
                            .to_vec(),
                    )
                } else if path == "/" {
                    ("200 OK", b"<ListAllMyBucketsResult><Buckets><Bucket><Name>bucket</Name></Bucket></Buckets></ListAllMyBucketsResult>".to_vec())
                } else if query.starts_with("location") {
                    (
                        "200 OK",
                        b"<LocationConstraint>eu-west-1</LocationConstraint>".to_vec(),
                    )
                } else if query.contains("list-type=2") {
                    let mut xml =
                        String::from("<ListBucketResult><IsTruncated>false</IsTruncated>");
                    for (k, v) in store.iter() {
                        xml.push_str(&format!(
                            "<Contents><Key>{k}</Key><Size>{}</Size></Contents>",
                            v.len()
                        ));
                    }
                    xml.push_str("</ListBucketResult>");
                    ("200 OK", xml.into_bytes())
                } else if method == "PUT" {
                    store.insert(key, body);
                    ("200 OK", Vec::new())
                } else if method == "GET" {
                    match store.get(&key) {
                        Some(v) => ("200 OK", v.clone()),
                        None => (
                            "404 Not Found",
                            b"<Error><Code>NoSuchKey</Code><Message>missing</Message></Error>"
                                .to_vec(),
                        ),
                    }
                } else if method == "DELETE" {
                    store.remove(&key);
                    ("204 No Content", Vec::new())
                } else {
                    ("400 Bad Request", Vec::new())
                };
                let mut out = stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    reply.len()
                );
                let _ = out.write_all(&reply);
            }
        });
        (base, store)
    }

    #[test]
    fn browse_upload_download_against_a_mock() {
        use crate::FsBackend as _;
        let (endpoint, store) = mock_s3();
        let s = S3Session::connect(
            S3Params {
                key_id: "AKIDTEST".into(),
                secret: zeroize::Zeroizing::new("secret".into()),
                region: "us-east-1".into(),
                start_path: None,
                endpoint: Some(endpoint),
            },
            std::sync::Arc::new(|| {}),
        );
        let next = |s: &S3Session| {
            s.events()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
        };
        assert_eq!(next(&s), FsEvent::Ready { home: "/".into() });
        let id = s.request(FsRequest::List("/".into()));
        let FsEvent::Listing {
            id: got, entries, ..
        } = next(&s)
        else {
            panic!()
        };
        assert_eq!((got, entries[0].name.as_str()), (id, "bucket"));

        let dir = tempfile::tempdir().unwrap();
        let up = dir.path().join("up.bin");
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&up, &data).unwrap();
        let id = s.request(FsRequest::Upload {
            local: up,
            remote: "/bucket/up.bin".into(),
            ascii: false,
        });
        let done = loop {
            match next(&s) {
                FsEvent::Progress { .. } => {}
                other => break other,
            }
        };
        assert_eq!(done, FsEvent::Done { id });
        assert_eq!(store.lock().unwrap()["up.bin"], data);

        let id = s.request(FsRequest::List("/bucket".into()));
        let FsEvent::Listing { entries, .. } = next(&s) else {
            panic!()
        };
        assert_eq!(
            (entries[0].name.as_str(), entries[0].size),
            ("up.bin", 100_000)
        );
        let _ = id;

        let back = dir.path().join("back.bin");
        let id = s.request(FsRequest::Download {
            remote: "/bucket/up.bin".into(),
            local: back.clone(),
            ascii: false,
        });
        let done = loop {
            match next(&s) {
                FsEvent::Progress { .. } => {}
                other => break other,
            }
        };
        assert_eq!(done, FsEvent::Done { id });
        assert_eq!(std::fs::read(&back).unwrap(), data);

        let id = s.request(FsRequest::Download {
            remote: "/bucket/missing".into(),
            local: dir.path().join("x"),
            ascii: false,
        });
        assert_eq!(
            next(&s),
            FsEvent::Failed {
                id,
                error: "NoSuchKey: missing".into()
            }
        );
    }

    /// AWS Signature Version 4 test suite, "get-vanilla".
    #[test]
    fn signature_v4_test_vector() {
        let auth = authorization(
            "AKIDEXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            &Signing {
                method: "GET",
                host: "example.amazonaws.com",
                path: "/",
                query: &[],
                headers: &[],
                payload_hash: &sha256_hex(b""),
                region: "us-east-1",
                service: "service",
                date: "20150830T123600Z",
            },
        );
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn dates() {
        assert_eq!(amz_date(1_440_938_160), "20150830T123600Z");
        assert_eq!(amz_date(951_782_400), "20000229T000000Z");
        assert_eq!(
            parse_iso8601("2015-08-30T12:36:00.000Z"),
            Some(1_440_938_160)
        );
        assert_eq!(parse_iso8601("2000-02-29T00:00:00Z"), Some(951_782_400));
        assert_eq!(parse_iso8601("nope"), None);
    }

    #[test]
    fn listings_and_paths() {
        let buckets = "<ListAllMyBucketsResult><Buckets><Bucket><Name>logs</Name>\
            <CreationDate>2020-01-01T00:00:00.000Z</CreationDate></Bucket>\
            <Bucket><Name>a&amp;b</Name></Bucket></Buckets></ListAllMyBucketsResult>";
        let b = parse_buckets(buckets);
        assert_eq!((b[0].name.as_str(), b[0].is_dir), ("logs", true));
        assert_eq!(b[1].name, "a&b");
        let objects = "<ListBucketResult><IsTruncated>true</IsTruncated>\
            <NextContinuationToken>tok</NextContinuationToken>\
            <Contents><Key>dir/</Key><Size>0</Size></Contents>\
            <Contents><Key>dir/a.txt</Key><Size>12</Size>\
            <LastModified>2015-08-30T12:36:00.000Z</LastModified></Contents>\
            <CommonPrefixes><Prefix>dir/sub/</Prefix></CommonPrefixes></ListBucketResult>";
        let (e, next) = parse_objects(objects, "dir/");
        assert_eq!(next.as_deref(), Some("tok"));
        assert_eq!(e.len(), 2, "{e:?}");
        assert_eq!((e[0].name.as_str(), e[0].is_dir), ("sub", true));
        assert_eq!(
            (e[1].name.as_str(), e[1].size, e[1].modified),
            ("a.txt", 12, Some(1_440_938_160))
        );
        assert_eq!(split_path("/"), None);
        assert_eq!(split_path("/b"), Some(("b".into(), String::new())));
        assert_eq!(
            split_path("/b/x/y.txt"),
            Some(("b".into(), "x/y.txt".into()))
        );
        assert_eq!(uri_encode("a b/ü", true), "a%20b/%C3%BC");
        assert_eq!(
            Client::location(Some("my.bucket"), "k 1", "eu-west-1"),
            (
                "s3.eu-west-1.amazonaws.com".into(),
                "/my.bucket/k%201".into()
            )
        );
        assert_eq!(
            Client::location(Some("plain"), "k", "us-east-1"),
            ("plain.s3.amazonaws.com".into(), "/k".into())
        );
    }
}
