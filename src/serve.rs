//! The HTTP kernel and route table for `grind serve` (U5/U6). GET-only, bounded,
//! `Connection: close`; the accept loop is this module's essence and the sole home of
//! `std::net` (KTD3, ADR-0014). Parsing and framing are pure functions over bytes and
//! values, so every decision here is testable with no socket.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::job;
use crate::observe;
use crate::page;
use crate::view;
use crate::world;

/// One parsed request. Segments are percent-decoded individually, after splitting, so
/// `%2F` can never become a separator and `%2e%2e` can never become a dot-segment.
#[derive(Debug, PartialEq)]
pub struct Request {
    pub method: String,
    pub segments: Vec<String>,
    pub query: Option<String>,
}

/// Response status. Variants map to R7's register.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Ok,
    NotFound,
    BadRequest,
    MethodNotAllowed,
    NotImplemented,
}

/// Cache posture for a response body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cache {
    NoStore,
    Assets,
}

/// A fully framed response waiting to be written to the wire.
pub struct Response {
    pub status: Status,
    pub content_type: &'static str,
    pub cache: Cache,
    pub body: Vec<u8>,
    pub extra: Vec<(&'static str, String)>,
}

/// Total budget for the request line plus headers (R7).
const HEAD_LIMIT: usize = 8 * 1024;

/// Per-connection read cap; anything past it cannot be a legal request (bounded ≤ 8 KiB).
const READ_CAP: usize = 16 * 1024;

/// Methods the router knows. Non-GET ones get 405 + `Allow: GET`; unknown tokens get 501.
const KNOWN_METHODS: [&str; 7] = ["GET", "POST", "PUT", "DELETE", "HEAD", "OPTIONS", "PATCH"];

/// Per-poll cap on a served log delta (KTD9).
const LOG_CAP: usize = 1024 * 1024;

/// How much of the tail a truncated log is re-served with (KTD9).
const LOG_TAIL: usize = 64 * 1024;

/// Parse one request from buffered bytes. Pure; see plan U5 for the checklist.
///
/// Malformed framing, an illegal request line, or an HTTP/1.1 Host violation is
/// `BadRequest`; anything that smells like a traversal or a character outside the
/// filesystem-safe set is `NotFound` (existence register, R7) so probes learn nothing.
pub fn parse_request(bytes: &[u8]) -> Result<Request, Status> {
    // One tolerated leading CRLF (clients sometimes send a stray one after pipelining).
    let bytes = bytes.strip_prefix(b"\r\n").unwrap_or(bytes);

    let head_end = find(bytes, b"\r\n\r\n").ok_or(Status::BadRequest)?;
    if head_end + 4 > HEAD_LIMIT {
        return Err(Status::BadRequest);
    }
    let head = std::str::from_utf8(&bytes[..head_end]).map_err(|_| Status::BadRequest)?;

    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() => (m, t, v),
        _ => return Err(Status::BadRequest),
    };
    if version != "HTTP/1.0" && version != "HTTP/1.1" {
        return Err(Status::BadRequest);
    }

    let mut hosts = 0usize;
    for line in lines {
        let (name, _) = line.split_once(':').ok_or(Status::BadRequest)?;
        if name.trim().eq_ignore_ascii_case("host") {
            hosts += 1;
        }
    }
    if version == "HTTP/1.1" && hosts != 1 {
        return Err(Status::BadRequest);
    }

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p, Some(q.to_string())),
        None => (target, None),
    };

    let mut segments = Vec::new();
    if path != "/" {
        let rest = path.strip_prefix('/').ok_or(Status::BadRequest)?;
        for raw in rest.split('/') {
            let decoded = percent_decode(raw).ok_or(Status::NotFound)?;
            if !fs_safe(&decoded) {
                return Err(Status::NotFound);
            }
            segments.push(decoded);
        }
    }

    Ok(Request {
        method: method.to_string(),
        segments,
        query,
    })
}

/// Route a parsed request against the fixed table (KTD7). Every handler reads through
/// `view` and renders through `page`; none of them names a path from unparsed input and
/// none of them writes anything (ADR-0013).
pub fn route(req: &Request, home: &Path) -> Response {
    if req.method != "GET" {
        if KNOWN_METHODS.contains(&req.method.as_str()) {
            return Response {
                status: Status::MethodNotAllowed,
                content_type: "text/plain; charset=utf-8",
                cache: Cache::NoStore,
                body: b"get only\n".to_vec(),
                extra: vec![("Allow", "GET".to_string())],
            };
        }
        return plain(Status::NotImplemented);
    }
    let segments: Vec<&str> = req.segments.iter().map(String::as_str).collect();
    match segments.as_slice() {
        [] => html(page::roster_page(
            &sorted_roster(home, req.query.as_deref()),
            &view::proposal_queue(home),
        )),
        ["f", "roster"] => html(page::roster_fragment(&sorted_roster(
            home,
            req.query.as_deref(),
        ))),
        ["runs", id] => run_response(home, id, false),
        ["f", "runs", id] => run_response(home, id, true),
        ["f", "runs", id, "log"] => log_delta(home, id, req.query.as_deref()),
        ["raw", "runs", id, file] => raw_file(home, id, file),
        ["raw", "runs", id, "stages", "reflect", kind, name]
            if matches!(*kind, "jobs" | "diffs") =>
        {
            raw_reflect(home, id, kind, name)
        }
        ["style.css"] => asset("text/css; charset=utf-8", crate::style::CSS),
        ["script.js"] => asset("text/javascript; charset=utf-8", crate::script::JS),
        _ => plain(Status::NotFound),
    }
}

/// The roster, freshly read, with the server-side sort applied (KTD11): `sort=id|state|
/// activity` and `dir=asc|desc`. Unknown values are ignored, and an absent `sort` keeps
/// `view::roster`'s own order.
fn sorted_roster(home: &Path, query: Option<&str>) -> Vec<view::RosterRow> {
    let mut rows = view::roster(home);
    let (mut key, mut desc) = (None, false);
    for pair in query.unwrap_or_default().split('&') {
        match pair.split_once('=') {
            Some(("sort", value)) => key = Some(value),
            Some(("dir", "desc")) => desc = true,
            _ => {}
        }
    }
    let ordered: Option<fn(&view::RosterRow, &view::RosterRow) -> std::cmp::Ordering> = match key {
        Some("id") => Some(|a, b| a.run_id.cmp(&b.run_id)),
        Some("state") => Some(|a, b| a.recorded_state.cmp(&b.recorded_state)),
        Some("activity") => Some(|a, b| a.last_activity.cmp(&b.last_activity)),
        _ => None,
    };
    if let Some(ordered) = ordered {
        if desc {
            rows.sort_by(|a, b| ordered(a, b).reverse());
        } else {
            rows.sort_by(ordered);
        }
    }
    rows
}

/// One Run's page or fragment. `NotHere` and `Unreadable` are the same 404 — whether a Run
/// exists elsewhere or its record is damaged is not the browser's business (R7).
fn run_response(home: &Path, id: &str, fragment: bool) -> Response {
    let view::Lookup::Here(found) = view::load(home, id) else {
        return html_not_found();
    };
    let Some(facts) = view::gather(home, id) else {
        return html_not_found();
    };
    let here = view::supervisor_here(
        found.supervisor_identity.as_deref(),
        &observe::process_start_stamp(&world::ps_start_stamp(found.supervisor_pid)),
    );
    let live = view::live(
        &view::transcript_path(home, &found.worktree, &found.session_id),
        world::now_epoch(),
    );
    let body = if fragment {
        page::run_fragment(id, &facts, &live, &here)
    } else {
        page::run_page(id, &facts, &live, &here)
    };
    html(body)
}

/// The `supervisor.log` delta (KTD9): `o` bytes in, at most [`LOG_CAP`] bytes out, the
/// served end named in `X-New-Offset` because UTF-8 makes client-computed lengths wrong.
/// An offset past the end means the log was truncated under the reader: the last
/// [`LOG_TAIL`] bytes go back with `X-Log-Reset: 1` so the client replaces, not appends.
/// A Run that has not narrated yet is not an error — an empty delta from zero.
fn log_delta(home: &Path, id: &str, query: Option<&str>) -> Response {
    let Some(offset) = log_offset(query) else {
        return plain(Status::BadRequest);
    };
    let path = job::runs_dir(home).join(id).join("supervisor.log");
    let Ok(bytes) = world::read_bytes(&path) else {
        return text(Vec::new(), vec![("X-New-Offset", "0".to_string())]);
    };
    let len = bytes.len();
    if offset <= len as u64 {
        let start = offset as usize;
        let end = (start + LOG_CAP).min(len);
        text(
            bytes[start..end].to_vec(),
            vec![("X-New-Offset", end.to_string())],
        )
    } else {
        let start = len.saturating_sub(LOG_TAIL);
        text(
            bytes[start..].to_vec(),
            vec![
                ("X-New-Offset", len.to_string()),
                ("X-Log-Reset", "1".to_string()),
            ],
        )
    }
}

/// The byte offset of a log poll: absent means zero, malformed refuses in the 400
/// register rather than guessing. Extra query parameters are ignored.
fn log_offset(query: Option<&str>) -> Option<u64> {
    for pair in query.unwrap_or_default().split('&') {
        if let Some(("o", value)) = pair.split_once('=') {
            return value.parse::<u64>().ok();
        }
    }
    Some(0)
}

/// Whitelisted evidence, verbatim (R5). Three whole names plus `attempt-N.<suffix>` with
/// N all digits; every near-miss — a `.bak`, a non-numeric attempt, anything else — reads
/// as not-here and leaks nothing.
fn raw_file(home: &Path, id: &str, file: &str) -> Response {
    if !evidence_allowed(file) {
        return plain(Status::NotFound);
    }
    match world::read_bytes(&job::runs_dir(home).join(id).join(file)) {
        Ok(bytes) => text(bytes, Vec::new()),
        Err(_) => plain(Status::NotFound),
    }
}

fn evidence_allowed(file: &str) -> bool {
    const WHOLE: [&str; 3] = ["run.json", "supervisor.log", "resume.log"];
    const SUFFIXES: [&str; 3] = [".prompt.txt", ".stdout.json", ".stderr.log"];
    if WHOLE.contains(&file) {
        return true;
    }
    SUFFIXES.iter().any(|suffix| {
        file.strip_prefix("attempt-")
            .and_then(|rest| rest.strip_suffix(suffix))
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// A Reflect artifact under `stages/reflect/jobs/` or `stages/reflect/diffs/`, verbatim
/// (issue #109) — the proposal-queue projection lists these paths, so the raw route must
/// be able to serve them. The prefix rule is explicit and scoped: only those two
/// directories, exactly one filename deep. `parse_request`'s [`fs_safe`] gate has already
/// refused `.`, `..`, absolute components, and every byte outside `[A-Za-z0-9._-]` in each
/// segment, so the joined path cannot escape the run directory; anything that slips past
/// the route arm reads as not-here and leaks nothing.
fn raw_reflect(home: &Path, id: &str, kind: &str, name: &str) -> Response {
    let path = job::runs_dir(home)
        .join(id)
        .join("stages")
        .join("reflect")
        .join(kind)
        .join(name);
    match world::read_bytes(&path) {
        Ok(bytes) => text(bytes, Vec::new()),
        Err(_) => plain(Status::NotFound),
    }
}

/// A rendered HTML page: fresh, never cached.
fn html(body: String) -> Response {
    Response {
        status: Status::Ok,
        content_type: "text/html; charset=utf-8",
        cache: Cache::NoStore,
        body: body.into_bytes(),
        extra: Vec::new(),
    }
}

/// The 404 that carries the page chrome: it says nothing about what exists.
fn html_not_found() -> Response {
    Response {
        status: Status::NotFound,
        content_type: "text/html; charset=utf-8",
        cache: Cache::NoStore,
        body: page::not_found().into_bytes(),
        extra: Vec::new(),
    }
}

/// A byte-exact text answer with caller-named extra headers (`X-New-Offset` and friends).
fn text(body: Vec<u8>, extra: Vec<(&'static str, String)>) -> Response {
    Response {
        status: Status::Ok,
        content_type: "text/plain; charset=utf-8",
        cache: Cache::NoStore,
        body,
        extra,
    }
}

/// Frame a response onto the wire: exact `Content-Length`, close semantics, nosniff, a
/// real IMF-fixdate `Date`, then the caller's extra headers. Pure; fully testable.
pub fn frame(resp: &Response) -> Vec<u8> {
    let (code, phrase) = reason(resp.status);
    let cache_line = match resp.cache {
        Cache::NoStore => "no-store",
        Cache::Assets => "public, max-age=3600",
    };
    let mut out = Vec::with_capacity(resp.body.len() + 320);
    out.extend_from_slice(format!("HTTP/1.1 {code} {phrase}\r\n").as_bytes());
    out.extend_from_slice(b"Content-Type: ");
    out.extend_from_slice(resp.content_type.as_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(format!("Content-Length: {}\r\n", resp.body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n");
    out.extend_from_slice(b"X-Content-Type-Options: nosniff\r\n");
    out.extend_from_slice(b"Cache-Control: ");
    out.extend_from_slice(cache_line.as_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(format!("Date: {}\r\n", http_date(SystemTime::now())).as_bytes());
    for (name, value) in &resp.extra {
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&resp.body);
    out
}

/// Bind, announce, and serve until killed. Never returns `Ok` in normal operation.
pub fn serve(home: &Path, host: &str, port: u16) -> Result<(), String> {
    let listener = bind_with_retry(host, port)?;
    world::print_line(&format!("serving http://{host}:{port} — ctrl-c to stop"));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let home = home.to_path_buf();
        // One thread per connection; a dead client must never reach the accept loop.
        std::thread::spawn(move || handle(stream, &home));
    }
    Ok(())
}

/// Bind with a bounded `EADDRINUSE` retry (~5 s; `std` sets no `SO_REUSEADDR`, KTD13),
/// then give up in the could-not-answer register.
fn bind_with_retry(host: &str, port: u16) -> Result<TcpListener, String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match TcpListener::bind((host, port)) {
            Ok(listener) => return Ok(listener),
            Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "could not answer {host}:{port} — the address is still taken"
                    ));
                }
                world::sleep(Duration::from_millis(250));
            }
            Err(err) => return Err(format!("could not answer {host}:{port} — {err}")),
        }
    }
}

/// Read one bounded request, route it, write the framed answer, and drop the stream
/// (`Connection: close`). Every error path returns quietly; per-connection trouble is
/// that connection's problem only.
fn handle(mut stream: TcpStream, home: &Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() >= READ_CAP || find(&buf, b"\r\n\r\n").is_some() {
                    break;
                }
            }
            Err(_) => return,
        }
    }

    if find(&buf, b"\r\n\r\n").is_none() {
        // Read cap exhausted with no complete head: not a legal request, say 400.
        write_all(&stream, &frame(&plain(Status::BadRequest)));
        return;
    }

    let resp = match parse_request(&buf) {
        Ok(req) => route(&req, home),
        Err(status) => plain(status),
    };
    write_all(&stream, &frame(&resp));
}

/// Best-effort blocking write; failures die with the connection, not the server.
fn write_all(mut stream: &TcpStream, bytes: &[u8]) {
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
}

/// Minimal leak-nothing body for a bare status.
fn plain(status: Status) -> Response {
    let body: &[u8] = match status {
        Status::Ok => b"ok\n",
        Status::BadRequest => b"bad request\n",
        Status::NotFound => b"no such page\n",
        Status::MethodNotAllowed => b"get only\n",
        Status::NotImplemented => b"not implemented\n",
    };
    Response {
        status,
        content_type: "text/plain; charset=utf-8",
        cache: Cache::NoStore,
        body: body.to_vec(),
        extra: Vec::new(),
    }
}

/// An embedded asset: long-ish cache, exact bytes from the compiled constant.
fn asset(content_type: &'static str, source: &'static str) -> Response {
    Response {
        status: Status::Ok,
        content_type,
        cache: Cache::Assets,
        body: source.as_bytes().to_vec(),
        extra: Vec::new(),
    }
}

/// Numeric code and reason phrase for the status line.
fn reason(status: Status) -> (&'static str, &'static str) {
    match status {
        Status::Ok => ("200", "OK"),
        Status::BadRequest => ("400", "Bad Request"),
        Status::NotFound => ("404", "Not Found"),
        Status::MethodNotAllowed => ("405", "Method Not Allowed"),
        Status::NotImplemented => ("501", "Not Implemented"),
    }
}

/// First offset of `needle` in `haystack`, byte-wise.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Decode `%XX` escapes. Returns `None` on any malformed escape; the caller turns that
/// into 404 so broken encodings reveal nothing about routing. `+` is left alone — it is
/// form-encoding, not path-encoding.
fn percent_decode(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = (hex[0] as char).to_digit(16)?;
            let lo = (hex[1] as char).to_digit(16)?;
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The charset a segment must satisfy before it ever touches the filesystem:
/// `[A-Za-z0-9._-]+`, never `.`, never `..`. This also rules out `/`, `\`, NUL, spaces,
/// and every other surprise in one predicate.
fn fs_safe(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// RFC 7231 IMF-fixdate (`Tue, 19 Jan 2038 03:14:07 GMT`) from the system clock —
/// days-from-epoch to civil date by Hinnant's algorithm, no dependencies.
fn http_date(t: SystemTime) -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let seconds_of_day = secs.rem_euclid(86_400);

    // Civil-from-days (Hinnant 2012): valid over the full range we can ever print.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };

    // 1970-01-01 was a Thursday (index 4 with Sunday = 0).
    let weekday = (days.rem_euclid(7) + 4) % 7;
    format!(
        "{}, {:02} {} {} {:02}:{:02}:{:02} GMT",
        WEEKDAYS[weekday as usize],
        day,
        MONTHS[(month - 1) as usize],
        year,
        seconds_of_day / 3_600,
        (seconds_of_day % 3_600) / 60,
        seconds_of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY_ONE: &str = include_str!("../tests/fixtures/record/day-one.json");

    fn get(target: &str) -> Vec<u8> {
        format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n").into_bytes()
    }

    /// A scratch `~/.grind`; the caller plants runs into it and removes it after.
    fn scratch(tag: &str) -> std::path::PathBuf {
        world::temp_dir(tag)
    }

    fn plant(home: &Path, dir_name: &str, record: &str) {
        let run_dir = job::runs_dir(home).join(dir_name);
        world::create_dir_all(&run_dir).unwrap();
        world::write_atomic(&run_dir.join("run.json"), record).unwrap();
    }

    /// The day-one record under another identity and timeline, so two runs can be told
    /// apart by id and by activity.
    fn record_like_day_one(run_id: &str, created_at: &str, newest_attempt_end: &str) -> String {
        let mut value: serde_json::Value = serde_json::from_str(DAY_ONE).unwrap();
        value["run_id"] = serde_json::json!(run_id);
        value["created_at"] = serde_json::json!(created_at);
        value["attempts"][3]["ended_at"] = serde_json::json!(newest_attempt_end);
        value.to_string()
    }

    fn named_extra<'a>(resp: &'a Response, name: &str) -> &'a str {
        resp.extra
            .iter()
            .find(|(header, _)| *header == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("no {name} header"))
    }

    #[test]
    fn the_valid_get_parses_into_method_segments_query() {
        let req = parse_request(&get("/runs/grind_2026-08-21T10.00.00Z")).unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.segments, vec!["runs", "grind_2026-08-21T10.00.00Z"]);
        assert_eq!(req.query, None);
    }

    #[test]
    fn the_query_lands_in_the_request_after_the_first_question_mark() {
        let req = parse_request(&get("/f/runs/abc/log?o=42")).unwrap();
        assert_eq!(req.segments, vec!["f", "runs", "abc", "log"]);
        assert_eq!(req.query.as_deref(), Some("o=42"));
    }

    #[test]
    fn the_http11_request_without_host_is_rejected() {
        let bytes = b"GET / HTTP/1.1\r\n\r\n";
        assert_eq!(parse_request(bytes), Err(Status::BadRequest));
    }

    #[test]
    fn the_duplicated_host_is_rejected() {
        let bytes = b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n";
        assert_eq!(parse_request(bytes), Err(Status::BadRequest));
    }

    #[test]
    fn the_http10_request_without_host_is_legal() {
        let req = parse_request(b"GET /style.css HTTP/1.0\r\n\r\n".as_slice()).unwrap();
        assert_eq!(req.segments, vec!["style.css"]);
    }

    #[test]
    fn one_leading_crlf_is_tolerated() {
        let mut bytes = b"\r\n".to_vec();
        bytes.extend_from_slice(&get("/"));
        let bare = parse_request(&get("/")).unwrap();
        let padded = parse_request(&bytes).unwrap();
        assert_eq!(padded.segments, bare.segments);
        assert_eq!(padded.method, "GET");
    }

    #[test]
    fn the_oversized_head_is_rejected() {
        let mut bytes = b"GET / HTTP/1.1\r\nHost: h\r\nX-Pad: ".to_vec();
        bytes.extend(std::iter::repeat(b'a').take(HEAD_LIMIT));
        bytes.extend_from_slice(b"\r\n\r\n");
        assert_eq!(parse_request(&bytes), Err(Status::BadRequest));
    }

    #[test]
    fn the_unterminated_head_is_rejected_even_when_short() {
        let bytes = b"GET / HTTP/1.1\r\nHost: h\r\n";
        assert_eq!(parse_request(bytes), Err(Status::BadRequest));
    }

    #[test]
    fn the_malformed_percent_escape_is_not_found() {
        assert_eq!(parse_request(&get("/runs/a%zz")), Err(Status::NotFound));
    }

    #[test]
    fn the_encoded_dot_dot_segment_never_becomes_a_dot_segment() {
        assert_eq!(parse_request(&get("/runs/%2e%2e")), Err(Status::NotFound));
        assert_eq!(parse_request(&get("/%2e%2e/etc")), Err(Status::NotFound));
    }

    #[test]
    fn the_literal_dot_segments_are_not_found() {
        assert_eq!(parse_request(&get("/..")), Err(Status::NotFound));
        assert_eq!(parse_request(&get("/./x")), Err(Status::NotFound));
    }

    #[test]
    fn the_encoded_separator_stays_inside_one_segment_and_dies_on_charset() {
        // %2F must not become a path separator after decoding.
        assert_eq!(parse_request(&get("/f%2Froster")), Err(Status::NotFound));
    }

    #[test]
    fn the_encoded_space_fails_the_run_id_charset() {
        assert_eq!(parse_request(&get("/runs/a%20b")), Err(Status::NotFound));
    }

    #[test]
    fn the_backslash_and_nul_are_outside_the_charset() {
        assert_eq!(parse_request(&get("/runs/a%5Cb")), Err(Status::NotFound));
        assert_eq!(parse_request(&get("/runs/a%00b")), Err(Status::NotFound));
    }

    #[test]
    fn post_routes_to_405_with_allow_get() {
        let bytes = b"POST /runs/abc HTTP/1.1\r\nHost: h\r\n\r\n";
        let req = parse_request(bytes).unwrap();
        let resp = route(&req, Path::new("/tmp"));
        assert_eq!(resp.status, Status::MethodNotAllowed);
        assert_eq!(resp.extra, vec![("Allow", "GET".to_string())]);
    }

    #[test]
    fn every_other_known_method_also_gets_405() {
        for method in ["PUT", "DELETE", "HEAD", "OPTIONS", "PATCH"] {
            let bytes = format!("{method} / HTTP/1.1\r\nHost: h\r\n\r\n").into_bytes();
            let resp = route(&parse_request(&bytes).unwrap(), Path::new("/tmp"));
            assert_eq!(resp.status, Status::MethodNotAllowed, "{method}");
        }
    }

    #[test]
    fn the_unknown_method_token_gets_501() {
        let bytes = b"FOO / HTTP/1.1\r\nHost: h\r\n\r\n";
        let resp = route(&parse_request(bytes.as_slice()).unwrap(), Path::new("/tmp"));
        assert_eq!(resp.status, Status::NotImplemented);
    }

    #[test]
    fn the_unknown_route_is_a_leak_nothing_404() {
        let resp = route(
            &parse_request(&get("/nope/deeper")).unwrap(),
            Path::new("/tmp"),
        );
        assert_eq!(resp.status, Status::NotFound);
        assert_eq!(resp.body, b"no such page\n");
    }

    #[test]
    fn the_roster_page_renders_the_planted_run() {
        let home = scratch("serve-roster");
        plant(&home, "20260806-122620-snapper-28", DAY_ONE);
        let resp = route(&parse_request(&get("/")).unwrap(), &home);
        assert_eq!(resp.status, Status::Ok);
        assert_eq!(resp.content_type, "text/html; charset=utf-8");
        assert_eq!(resp.cache, Cache::NoStore);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(
            body.contains("Slice 1b: the agent surface and the ScreenSource seam"),
            "{body}"
        );
        world::remove_tree(&home);
    }

    #[test]
    fn the_roster_fragment_carries_the_board_root() {
        let home = scratch("serve-fragment");
        let resp = route(&parse_request(&get("/f/roster")).unwrap(), &home);
        assert_eq!(resp.status, Status::Ok);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("data-g-root=\"board\""), "{body}");
        world::remove_tree(&home);
    }

    #[test]
    fn an_unknown_or_damaged_run_is_an_html_404_that_leaks_nothing() {
        let home = scratch("serve-404");
        // A neighbouring record that does not parse costs itself and nothing else.
        plant(&home, "20260807-000000-junk-01", "{not a record");
        for target in [
            "/runs/20260806-999999-nowhere-99",
            "/f/runs/20260806-999999-nowhere-99",
            "/runs/20260807-000000-junk-01",
            "/f/runs/20260807-000000-junk-01",
        ] {
            let resp = route(&parse_request(&get(target)).unwrap(), &home);
            assert_eq!(resp.status, Status::NotFound, "{target}");
            assert_eq!(resp.body, page::not_found().into_bytes(), "{target}");
        }
        world::remove_tree(&home);
    }

    #[test]
    fn the_sort_and_dir_parameters_reorder_the_board_server_side() {
        let home = scratch("serve-sort");
        // Directory order (the default) puts `zulu` first; sorting by id or activity
        // must be able to disagree with it.
        plant(
            &home,
            "m-alpha",
            &record_like_day_one(
                "zulu",
                "2026-08-10T00:00:00+00:00",
                "2026-08-11T00:00:00+00:00",
            ),
        );
        plant(
            &home,
            "m-zulu",
            &record_like_day_one(
                "alpha",
                "2026-08-02T00:00:00+00:00",
                "2026-08-03T00:00:00+00:00",
            ),
        );
        let order = |target: &str| {
            let resp = route(&parse_request(&get(target)).unwrap(), &home);
            let body = String::from_utf8(resp.body).unwrap();
            let zulu = body.find("zulu").expect("zulu on the board");
            let alpha = body.find("alpha").expect("alpha on the board");
            if zulu < alpha {
                "zulu-first"
            } else {
                "alpha-first"
            }
        };
        assert_eq!(order("/"), "zulu-first", "directory order is the default");
        assert_eq!(order("/?sort=id&dir=asc"), "alpha-first");
        assert_eq!(order("/?sort=id&dir=desc"), "zulu-first");
        assert_eq!(order("/?sort=activity&dir=asc"), "alpha-first");
        assert_eq!(order("/?sort=activity&dir=desc"), "zulu-first");
        assert_eq!(
            order("/?sort=nonsense"),
            "zulu-first",
            "unknown sort is ignored"
        );
        assert_eq!(
            order("/?sort=id&dir=sideways"),
            "alpha-first",
            "unknown dir falls back to ascending"
        );
        world::remove_tree(&home);
    }

    #[test]
    fn the_log_delta_serves_from_zero_without_a_query() {
        let (resp, len) = poll_log("serve-log-0", None, "0123456789".repeat(100));
        assert_eq!(resp.body.len(), len);
        assert_eq!(named_extra(&resp, "X-New-Offset"), len.to_string());
    }

    #[test]
    fn the_log_delta_serves_the_requested_window_mid_file() {
        // 404 lands mid-tile: the window opens on the '4' of the fifth tile.
        let (resp, _) = poll_log("serve-log-mid", Some("o=404"), "0123456789".repeat(100));
        assert_eq!(resp.body.len(), 596);
        assert!(resp.body.starts_with(b"456789"));
        assert_eq!(named_extra(&resp, "X-New-Offset"), "1000");
    }

    #[test]
    fn the_log_delta_at_the_end_is_an_empty_delta_at_the_right_offset() {
        let (resp, _) = poll_log("serve-log-end", Some("o=1000"), "0123456789".repeat(100));
        assert!(resp.body.is_empty());
        assert_eq!(named_extra(&resp, "X-New-Offset"), "1000");
    }

    #[test]
    fn an_offset_past_the_end_resets_to_the_last_64k() {
        let (resp, len) = poll_log("serve-log-reset", Some("o=2000"), "0123456789".repeat(100));
        assert_eq!(resp.body.len(), len.min(LOG_TAIL));
        assert_eq!(named_extra(&resp, "X-New-Offset"), len.to_string());
        assert_eq!(named_extra(&resp, "X-Log-Reset"), "1");
    }

    #[test]
    fn a_single_poll_is_capped_at_one_mebibyte() {
        let big = "a".repeat(LOG_CAP + 500);
        let (resp, _) = poll_log("serve-log-cap", None, big);
        assert_eq!(resp.body.len(), LOG_CAP);
        assert_eq!(named_extra(&resp, "X-New-Offset"), LOG_CAP.to_string());
    }

    #[test]
    fn a_missing_supervisor_log_is_an_empty_delta_from_zero() {
        let home = scratch("serve-log-missing");
        let resp = route(
            &parse_request(&get("/f/runs/20260806-122620-snapper-28/log")).unwrap(),
            &home,
        );
        assert_eq!(resp.status, Status::Ok);
        assert!(resp.body.is_empty());
        assert_eq!(named_extra(&resp, "X-New-Offset"), "0");
        world::remove_tree(&home);
    }

    #[test]
    fn a_malformed_offset_refuses_as_a_bad_request() {
        let home = scratch("serve-log-bad");
        for query in ["o=", "o=abc", "o=-1", "o=1x"] {
            let target = format!("/f/runs/abc/log?{query}");
            let resp = route(&parse_request(&get(&target)).unwrap(), &home);
            assert_eq!(resp.status, Status::BadRequest, "{query}");
        }
        world::remove_tree(&home);
    }

    /// Plant one run's supervisor.log with `content`, poll it at `query`, and return the
    /// response plus the content length.
    fn poll_log(tag: &str, query: Option<&str>, content: String) -> (Response, usize) {
        let home = scratch(tag);
        let run_dir = job::runs_dir(&home).join("20260806-122620-snapper-28");
        world::create_dir_all(&run_dir).unwrap();
        world::write_atomic(&run_dir.join("supervisor.log"), &content).unwrap();
        let target = match query {
            Some(q) => format!("/f/runs/20260806-122620-snapper-28/log?{q}"),
            None => "/f/runs/20260806-122620-snapper-28/log".to_string(),
        };
        let resp = route(&parse_request(&get(&target)).unwrap(), &home);
        world::remove_tree(&home);
        (resp, content.len())
    }

    #[test]
    fn every_whitelisted_evidence_name_serves_verbatim_bytes() {
        let home = scratch("serve-raw-ok");
        let run_dir = job::runs_dir(&home).join("abc");
        world::create_dir_all(&run_dir).unwrap();
        for name in [
            "run.json",
            "supervisor.log",
            "resume.log",
            "attempt-12.prompt.txt",
            "attempt-12.stdout.json",
            "attempt-12.stderr.log",
        ] {
            world::write_atomic(&run_dir.join(name), "evidence bytes\n").unwrap();
            let target = format!("/raw/runs/abc/{name}");
            let resp = route(&parse_request(&get(&target)).unwrap(), &home);
            assert_eq!(resp.status, Status::Ok, "{name}");
            assert_eq!(resp.content_type, "text/plain; charset=utf-8", "{name}");
            assert_eq!(resp.cache, Cache::NoStore, "{name}");
            assert_eq!(resp.body, b"evidence bytes\n", "{name}");
        }
        world::remove_tree(&home);
    }

    #[test]
    fn every_near_miss_on_the_whitelist_reads_as_not_here() {
        let home = scratch("serve-raw-no");
        let run_dir = job::runs_dir(&home).join("abc");
        world::create_dir_all(&run_dir).unwrap();
        for name in [
            "run.json.bak",
            "run.jsonx",
            "attempt-x.stdout",
            "attempt-.prompt.txt",
            "attempt-1x.stderr.log",
            "Attempt-1.prompt.txt",
            "attempt-1.prompt.txt.bak",
            "secrets.env",
        ] {
            let target = format!("/raw/runs/abc/{name}");
            let resp = route(&parse_request(&get(&target)).unwrap(), &home);
            assert_eq!(resp.status, Status::NotFound, "{name}");
        }
        // A planted-but-unlisted file still reads as not-here.
        world::write_atomic(&run_dir.join("run.json.bak"), "stale").unwrap();
        let resp = route(
            &parse_request(&get("/raw/runs/abc/run.json.bak")).unwrap(),
            &home,
        );
        assert_eq!(resp.status, Status::NotFound);
        world::remove_tree(&home);
    }

    #[test]
    fn traversal_never_reaches_the_raw_handler() {
        // The parser refuses dot segments before routing; nothing downstream has to
        // re-check them.
        assert_eq!(
            parse_request(&get("/raw/runs/abc/../../etc/passwd")),
            Err(Status::NotFound)
        );
        // A missing whitelisted file is also a plain not-here.
        let home = scratch("serve-raw-gone");
        let resp = route(
            &parse_request(&get("/raw/runs/abc/supervisor.log")).unwrap(),
            &home,
        );
        assert_eq!(resp.status, Status::NotFound);
        assert_eq!(resp.body, b"no such page\n");
        world::remove_tree(&home);
    }

    #[test]
    fn a_reflect_job_draft_serves_verbatim_from_the_proposal_queue() {
        let home = scratch("serve-raw-job");
        let jobs = job::runs_dir(&home)
            .join("abc")
            .join("stages")
            .join("reflect")
            .join("jobs");
        world::create_dir_all(&jobs).unwrap();
        world::write_atomic(&jobs.join("follow-up.md"), "# drafted issue body\n").unwrap();
        let resp = route(
            &parse_request(&get("/raw/runs/abc/stages/reflect/jobs/follow-up.md")).unwrap(),
            &home,
        );
        assert_eq!(resp.status, Status::Ok);
        assert_eq!(resp.content_type, "text/plain; charset=utf-8");
        assert_eq!(resp.cache, Cache::NoStore);
        assert_eq!(resp.body, b"# drafted issue body\n");
        world::remove_tree(&home);
    }

    #[test]
    fn a_reflect_skill_diff_serves_verbatim_too() {
        let home = scratch("serve-raw-diff");
        let diffs = job::runs_dir(&home)
            .join("abc")
            .join("stages")
            .join("reflect")
            .join("diffs");
        world::create_dir_all(&diffs).unwrap();
        world::write_atomic(
            &diffs.join("skill.patch"),
            "--- a/SKILL.md\n+++ b/SKILL.md\n",
        )
        .unwrap();
        let resp = route(
            &parse_request(&get("/raw/runs/abc/stages/reflect/diffs/skill.patch")).unwrap(),
            &home,
        );
        assert_eq!(resp.status, Status::Ok);
        assert_eq!(resp.body, b"--- a/SKILL.md\n+++ b/SKILL.md\n");
        world::remove_tree(&home);
    }

    #[test]
    fn traversal_through_the_reflect_prefix_never_reaches_the_filesystem() {
        // The parser refuses dot segments per-segment before routing; the prefix rule
        // adds nothing that could reopen that hole.
        for target in [
            "/raw/runs/abc/stages/reflect/jobs/../secrets.env",
            "/raw/runs/abc/stages/reflect/jobs/%2e%2e/secrets.env",
            "/raw/runs/abc/stages/reflect/jobs/a%2Fb",
        ] {
            assert_eq!(
                parse_request(&get(target)),
                Err(Status::NotFound),
                "{target}"
            );
        }
    }

    #[test]
    fn a_post_against_a_reflect_artifact_is_method_not_allowed() {
        let bytes =
            b"POST /raw/runs/abc/stages/reflect/jobs/follow-up.md HTTP/1.1\r\nHost: h\r\n\r\n";
        let resp = route(&parse_request(bytes).unwrap(), Path::new("/tmp"));
        assert_eq!(resp.status, Status::MethodNotAllowed);
        assert_eq!(resp.extra, vec![("Allow", "GET".to_string())]);
    }

    #[test]
    fn every_reflect_prefix_near_miss_reads_as_not_here() {
        let home = scratch("serve-raw-miss");
        let reflect = job::runs_dir(&home)
            .join("abc")
            .join("stages")
            .join("reflect");
        world::create_dir_all(&reflect.join("other")).unwrap();
        world::create_dir_all(&reflect.join("jobs").join("sub")).unwrap();
        world::write_atomic(&reflect.join("other").join("x.md"), "planted").unwrap();
        world::write_atomic(&reflect.join("jobs").join("sub").join("deep.md"), "planted").unwrap();
        // Other stages stay dark, as do other directories under stages/reflect, deeper
        // nesting inside an allowed directory, and an absent file in an allowed one.
        for target in [
            "/raw/runs/abc/stages/plan/jobs/x.md",
            "/raw/runs/abc/stages/skills/diffs/x.patch",
            "/raw/runs/abc/stages/reflect/other/x.md",
            "/raw/runs/abc/stages/reflect/jobs/sub/deep.md",
            "/raw/runs/abc/stages/reflect/jobs/gone.md",
        ] {
            let resp = route(&parse_request(&get(target)).unwrap(), &home);
            assert_eq!(resp.status, Status::NotFound, "{target}");
            assert_eq!(resp.body, b"no such page\n", "{target}");
        }
        world::remove_tree(&home);
    }

    #[test]
    fn the_style_sheet_serves_as_a_cached_asset() {
        let resp = route(
            &parse_request(&get("/style.css")).unwrap(),
            Path::new("/tmp"),
        );
        assert_eq!(resp.status, Status::Ok);
        assert_eq!(resp.content_type, "text/css; charset=utf-8");
        assert_eq!(resp.cache, Cache::Assets);
    }

    #[test]
    fn the_script_serves_as_a_cached_asset() {
        let resp = route(
            &parse_request(&get("/script.js")).unwrap(),
            Path::new("/tmp"),
        );
        assert_eq!(resp.content_type, "text/javascript; charset=utf-8");
        assert_eq!(resp.cache, Cache::Assets);
    }

    #[test]
    fn the_frame_carries_the_exact_header_set_and_exact_length() {
        let resp = Response {
            status: Status::Ok,
            content_type: "text/html; charset=utf-8",
            cache: Cache::NoStore,
            body: b"<html></html>".to_vec(),
            extra: vec![("X-New-Offset", "128".to_string())],
        };
        let wire = String::from_utf8(frame(&resp)).unwrap();
        let mut lines = wire.split("\r\n").peekable();
        assert_eq!(lines.next(), Some("HTTP/1.1 200 OK"));

        let mut headers: Vec<String> = Vec::new();
        loop {
            let line = lines.next().unwrap();
            if line.is_empty() {
                break;
            }
            headers.push(line.to_string());
        }
        assert_eq!(headers.len(), 7, "six fixed headers plus one extra");
        assert!(headers.contains(&"Content-Type: text/html; charset=utf-8".into()));
        assert!(headers.contains(&format!("Content-Length: {}", resp.body.len())));
        assert!(headers.contains(&"Connection: close".into()));
        assert!(headers.contains(&"X-Content-Type-Options: nosniff".into()));
        assert!(headers.contains(&"Cache-Control: no-store".into()));
        assert!(headers.iter().any(|h| h.starts_with("Date: ")));
        assert!(headers.contains(&"X-New-Offset: 128".into()));

        let bytes = frame(&resp);
        let head_len = find(&bytes, b"\r\n\r\n").unwrap() + 4;
        assert_eq!(bytes.len(), head_len + resp.body.len());
        assert_eq!(&bytes[head_len..], b"<html></html>");
    }

    #[test]
    fn the_asset_frame_caches_for_an_hour() {
        let resp = route(
            &parse_request(&get("/style.css")).unwrap(),
            Path::new("/tmp"),
        );
        let wire = String::from_utf8(frame(&resp)).unwrap();
        assert!(wire.contains("Cache-Control: public, max-age=3600\r\n"));
        assert!(wire.starts_with("HTTP/1.1 200 OK\r\n"));
    }

    #[test]
    fn the_date_header_is_imf_fixdate() {
        let date = http_date(SystemTime::UNIX_EPOCH);
        assert_eq!(date, "Thu, 01 Jan 1970 00:00:00 GMT");

        // 2038-01-19T03:14:07Z — the signed-32-bit cliff, still correct here.
        let t = UNIX_EPOCH + Duration::from_secs(2_147_483_647);
        assert_eq!(http_date(t), "Tue, 19 Jan 2038 03:14:07 GMT");

        // Leap years land on the right days: 2024-02-29.
        let t = UNIX_EPOCH + Duration::from_secs(1_709_164_800);
        assert_eq!(http_date(t), "Thu, 29 Feb 2024 00:00:00 GMT");

        let now = http_date(SystemTime::now());
        assert_eq!(now.len(), 29);
        assert!(now.ends_with(" GMT"));
    }

    #[test]
    fn the_empty_and_absent_methods_are_bad_requests() {
        assert_eq!(
            parse_request(b" / HTTP/1.1\r\nHost: h\r\n\r\n".as_slice()),
            Err(Status::BadRequest)
        );
        assert_eq!(
            parse_request(b"HTTP/1.1\r\nHost: h\r\n\r\n".as_slice()),
            Err(Status::BadRequest)
        );
    }

    #[test]
    fn the_wrong_version_is_bad_request() {
        assert_eq!(
            parse_request(b"GET / HTTP/0.9\r\nHost: h\r\n\r\n".as_slice()),
            Err(Status::BadRequest)
        );
        assert_eq!(
            parse_request(b"GET / HTTP/2\r\nHost: h\r\n\r\n".as_slice()),
            Err(Status::BadRequest)
        );
    }
}
