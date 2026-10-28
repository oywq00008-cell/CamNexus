//! 局域网探测：端口扫描 + ONVIF/SADP 组播 + RTSP 路径握手
//!
//! 设计目标：在只知道摄像头 IP 的情况下，自动找出可用的 RTSP 地址，
//! 并尽量减少带密码的尝试次数（部分品牌密码错误多次会锁定账号）。

use crate::i18n::{self, tr};
use base64::{Engine, engine::general_purpose::STANDARD};
use md5::{Digest, Md5};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc::Sender};
use std::time::{Duration, Instant};

pub const SCAN_PORTS: &[u16] = &[554, 80, 8000, 8080, 8899, 37777, 5000];

#[derive(Clone, Debug)]
pub struct UrlTest {
    pub url: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub ip: String,
    pub ports: Vec<u16>,
    pub hint: String,
    pub urls: Vec<UrlTest>,
}

#[derive(Clone, Debug)]
pub enum ScanMsg {
    Progress(usize, usize),
    Device(Device),
    Done(Vec<Device>),
}

pub fn local_ip() -> String {
    UdpSocket::bind("0.0.0.0:0")
        .ok()
        .and_then(|s| {
            s.connect("223.5.5.5:80").ok()?;
            s.local_addr().ok()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|| "127.0.0.1".into())
}

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    let mut out = String::with_capacity(32);
    for b in d.iter() {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn port_open(ip: &str, port: u16, timeout: Duration) -> bool {
    match format!("{ip}:{port}").parse::<SocketAddr>() {
        Ok(addr) => TcpStream::connect_timeout(&addr, timeout).is_ok(),
        Err(_) => false,
    }
}

// ---------------------------------------------------------------- ONVIF / SADP
fn udp_probe(port: u16, payload: &[u8], wait: Duration) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let sock = match UdpSocket::bind("0.0.0.0:0") {
        Ok(s) => s,
        Err(_) => return out,
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = sock.send_to(payload, ("239.255.255.250", port));
    let deadline = Instant::now() + wait;
    let mut seen = std::collections::HashSet::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        if let Ok((n, addr)) = sock.recv_from(&mut buf) {
            let ip = addr.ip().to_string();
            if seen.insert(ip.clone()) {
                out.push((ip, String::from_utf8_lossy(&buf[..n]).to_string()));
            }
        }
    }
    out
}

const WS_PROBE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope" xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl">
<e:Header><w:MessageID>uuid:8b6f0e2a-1c1e-4a2b-9f3e-camnexus0001</w:MessageID><w:To e:mustUnderstand="true">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To><w:Action e:mustUnderstand="true">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action></e:Header>
<e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body></e:Envelope>"#;

const SADP_PROBE: &str = r#"<?xml version="1.0" encoding="utf-8"?><Probe><Uuid>8B6F0E2A-1C1E-4A2B-9F3E-CAMNEXUS0001</Uuid><Types>inquiry</Types></Probe>"#;

/// 返回 ip → 型号/名称 的提示信息
fn multicast_identify() -> HashMap<String, (String, String)> {
    let mut map: HashMap<String, (String, String)> = HashMap::new();

    for (ip, body) in udp_probe(3702, WS_PROBE.as_bytes(), Duration::from_millis(1800)) {
        let mut name = String::new();
        let mut hw = String::new();
        for (key, tag) in [("name", "name"), ("hardware", "hardware")] {
            let pat = format!("onvif://www.onvif.org/{key}/");
            if let Some(p) = body.find(&pat) {
                let rest = &body[p + pat.len()..];
                let end = rest
                    .find(|c: char| c == '<' || c == '"' || c.is_whitespace())
                    .unwrap_or(rest.len());
                let v = rest[..end].replace("%20", " ");
                if tag == "name" {
                    name = v;
                } else {
                    hw = v;
                }
            }
        }
        let label = if !hw.is_empty() { hw } else { name };
        map.insert(ip, ("ONVIF".into(), label));
    }

    for (ip, body) in udp_probe(37020, SADP_PROBE.as_bytes(), Duration::from_millis(1500)) {
        let get = |tag: &str| -> String {
            let open = format!("<{tag}>");
            let close = format!("</{tag}>");
            match (body.find(&open), body.find(&close)) {
                (Some(a), Some(b)) if b > a + open.len() => body[a + open.len()..b].trim().to_string(),
                _ => String::new(),
            }
        };
        let model = get("DeviceDescription");
        let sn = get("DeviceSN");
        let label = if !model.is_empty() {
            format!("{model}{}", if sn.is_empty() { String::new() } else { format!(" / {sn}") })
        } else {
            tr("海康 SADP").into()
        };
        map.insert(ip, ("Hikvision-SADP".into(), label));
    }

    map
}

// ---------------------------------------------------------------- HTTP 指纹
fn http_banner(ip: &str) -> String {
    let addr = match format!("{ip}:80").parse::<SocketAddr>() {
        Ok(a) => a,
        Err(_) => return String::new(),
    };
    let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(700)) else {
        return String::new();
    };
    let _ = s.set_read_timeout(Some(Duration::from_millis(900)));
    let req = format!(
        "GET / HTTP/1.0\r\nHost: {ip}\r\nUser-Agent: CamNexus-probe\r\nConnection: close\r\n\r\n"
    );
    if s.write_all(req.as_bytes()).is_err() {
        return String::new();
    }
    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    while let Ok(n) = s.read(&mut buf) {
        if n == 0 {
            break;
        }
        acc.extend_from_slice(&buf[..n]);
        if acc.len() > 6000 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&acc).to_lowercase();
    for (key, label) in [
        ("hikvision", tr("海康威视")),
        ("dvrdvs", tr("海康威视")),
        ("360", "360"),
        ("dahua", tr("大华")),
        ("onvif", "ONVIF"),
        ("ipc", "IPC"),
        ("web service", tr("Web 服务")),
    ] {
        if text.contains(key) {
            return label.into();
        }
    }
    String::new()
}

// ---------------------------------------------------------------- RTSP
struct RtspResp {
    code: u16,
    headers: HashMap<String, String>,
    body: String,
}

fn connect_rtsp(ip: &str, port: u16) -> std::io::Result<TcpStream> {
    let addr: SocketAddr = format!("{ip}:{port}")
        .parse()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, tr("地址无效")))?;
    let s = TcpStream::connect_timeout(&addr, Duration::from_millis(2500))?;
    s.set_read_timeout(Some(Duration::from_millis(3000)))?;
    s.set_write_timeout(Some(Duration::from_millis(2000)))?;
    Ok(s)
}

/// 在同一条 TCP 连接上收发一次请求。
/// 注意：部分设备把 digest nonce 与连接绑定，认证必须在同一连接完成。
fn rtsp_exchange(
    s: &mut TcpStream,
    ip: &str,
    port: u16,
    method: &str,
    path: &str,
    auth: Option<&str>,
    cseq: u32,
) -> std::io::Result<RtspResp> {
    let uri = format!("rtsp://{ip}:{port}{path}");
    let mut req = format!("{method} {uri} RTSP/1.0\r\nCSeq: {cseq}\r\nUser-Agent: CamNexus\r\n");
    if method == "DESCRIBE" {
        req.push_str("Accept: application/sdp\r\n");
    }
    if let Some(a) = auth {
        req.push_str(&format!("Authorization: {a}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;

    let mut acc: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let mut body_start: Option<usize> = None;
    let mut want: usize = 0;
    let start = Instant::now();
    loop {
        if let Some(bs) = body_start
            && acc.len() >= bs + want {
                break;
            }
        if start.elapsed() > Duration::from_secs(4) {
            break;
        }
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                acc.extend_from_slice(&buf[..n]);
                if body_start.is_none()
                    && let Some(p) = find_sub(&acc, b"\r\n\r\n") {
                        body_start = Some(p + 4);
                        let head = String::from_utf8_lossy(&acc[..p]).to_string();
                        want = parse_content_length(&head);
                        if want == 0 && head.contains("200") && method == "DESCRIBE" {
                            // SDP 可能没有 Content-Length，继续读一小段
                            want = 0;
                        }
                    }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(_) => break,
        }
    }

    let text = String::from_utf8_lossy(&acc).to_string();
    let (head, body) = match find_sub(&acc, b"\r\n\r\n") {
        Some(p) => (
            String::from_utf8_lossy(&acc[..p]).to_string(),
            String::from_utf8_lossy(&acc[p + 4..]).to_string(),
        ),
        None => (text.clone(), String::new()),
    };

    let mut headers = HashMap::new();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or("");
    let code = status
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    Ok(RtspResp { code, headers, body })
}

fn find_sub(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_content_length(head: &str) -> usize {
    for line in head.split("\r\n") {
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length") {
                return v.trim().parse().unwrap_or(0);
            }
    }
    0
}

fn digest_auth(challenge: &str, method: &str, uri: &str, user: &str, pass: &str) -> Option<String> {
    let lower = challenge.to_lowercase();
    if !lower.starts_with("digest") {
        return None;
    }
    let mut f: HashMap<String, String> = HashMap::new();
    let mut rest = &challenge[6..];
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().trim_matches(',').trim().to_lowercase();
        let after = &rest[eq + 1..];
        let (val, next) = if let Some(quoted) = after.strip_prefix('"') {
            let end = quoted.find('"')?;
            (quoted[..end].to_string(), &quoted[end + 1..])
        } else {
            let end = after.find(',').unwrap_or(after.len());
            (after[..end].trim().to_string(), &after[end..])
        };
        f.insert(key, val);
        rest = next;
    }

    let realm = f.get("realm").cloned().unwrap_or_default();
    let nonce = f.get("nonce").cloned().unwrap_or_default();
    let qop = f.get("qop").cloned().unwrap_or_default();
    let opaque = f.get("opaque").cloned();

    let ha1 = md5_hex(&format!("{user}:{realm}:{pass}"));
    let ha2 = md5_hex(&format!("{method}:{uri}"));
    let cnonce = "0a4f113b";
    let nc = "00000001";

    let response = if qop.contains("auth") {
        md5_hex(&format!("{ha1}:{nonce}:{nc}:{cnonce}:auth:{ha2}"))
    } else {
        md5_hex(&format!("{ha1}:{nonce}:{ha2}"))
    };

    let mut h = format!(
        "Digest username=\"{user}\", realm=\"{realm}\", nonce=\"{nonce}\", uri=\"{uri}\", response=\"{response}\""
    );
    if qop.contains("auth") {
        h.push_str(&format!(", qop=auth, nc={nc}, cnonce=\"{cnonce}\""));
    }
    if let Some(o) = opaque {
        h.push_str(&format!(", opaque=\"{o}\""));
    }
    Some(h)
}

fn codec_summary(sdp: &str) -> String {
    let up = sdp.to_uppercase();
    let mut parts: Vec<String> = Vec::new();
    for c in ["H265", "H264", "MP4V-ES", "MJPEG", "JPEG"] {
        if up.contains(c) {
            parts.push(c.to_string());
        }
    }
    if let Some(p) = sdp.find("a=framesize:") {
        let rest = &sdp[p + 12..];
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        parts.push(rest[..end].trim().to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join(" ")
    }
}

fn candidates(hint: &str) -> Vec<&'static str> {
    let h = hint.to_lowercase();
    if h.contains(tr("海康")) || h.contains("hikvision") || h.contains("sadp") {
        return vec!["/Streaming/Channels/102", "/Streaming/Channels/101"];
    }
    if h.contains("360") {
        return vec![
            "/live/ch00_0",
            "/cam/realmonitor?channel=0&subtype=1",
            "/ch0_0.h264",
            "/live/ch0",
        ];
    }
    // 未知品牌：覆盖主流写法，但控制在 6 次以内
    vec![
        "/Streaming/Channels/102",
        "/live/ch00_0",
        "/cam/realmonitor?channel=0&subtype=1",
        "/stream1",
        "/h264/ch1/sub/av_stream",
        "/ch0_0.h264",
    ]
}

/// 在单条 TCP 连接上完成 DESCRIBE →（按需认证）DESCRIBE，返回 (是否可用, 说明)
///
/// 注意两点而不是想当然：
/// 1. 不少设备对 OPTIONS 一律回 200，不能用它判断是否需要认证，必须直接发 DESCRIBE；
/// 2. digest 的 nonce 可能与 TCP 连接绑定，认证必须复用触发挑战的那条连接。
fn probe_path(ip: &str, port: u16, path: &str, user: &str, pass: &str) -> (bool, String) {
    let mut s = match connect_rtsp(ip, port) {
        Ok(s) => s,
        Err(e) => return (false, i18n::err(tr("连接失败"), e)),
    };

    let first = match rtsp_exchange(&mut s, ip, port, "DESCRIBE", path, None, 1) {
        Ok(r) => r,
        Err(e) => return (false, format!("{e}")),
    };

    if first.code == 200 {
        let codec = codec_summary(&first.body);
        return (
            true,
            if codec.is_empty() {
                tr("200 · 无需认证").into()
            } else {
                format!("200 · {codec}")
            },
        );
    }
    if first.code != 401 {
        return (false, i18n::describe_code(first.code));
    }

    let challenge = first
        .headers
        .get("www-authenticate")
        .cloned()
        .unwrap_or_default();
    let uri = format!("rtsp://{ip}:{port}{path}");
    let auth = if challenge.to_lowercase().starts_with("basic") {
        format!("Basic {}", STANDARD.encode(format!("{user}:{pass}")))
    } else {
        match digest_auth(&challenge, "DESCRIBE", &uri, user, pass) {
            Some(a) => a,
            None => return (false, tr("无法解析认证方式").into()),
        }
    };

    let mut result = rtsp_exchange(&mut s, ip, port, "DESCRIBE", path, Some(&auth), 2);
    // 若服务端在 401 后关掉了连接，退化为新连接 + 预置认证头
    if result.is_err()
        && let Ok(mut s2) = connect_rtsp(ip, port) {
            result = rtsp_exchange(&mut s2, ip, port, "DESCRIBE", path, Some(&auth), 1);
        }

    match result {
        Ok(d) if d.code == 200 => {
            let codec = codec_summary(&d.body);
            (
                true,
                if codec.is_empty() {
                    tr("200 · 认证通过").into()
                } else {
                    format!("200 · {codec}")
                },
            )
        }
        Ok(d) => (false, i18n::describe_code(d.code)),
        Err(e) => (false, format!("{e}")),
    }
}

fn probe_rtsp(ip: &str, port: u16, user: &str, pass: &str, hint: &str) -> Vec<UrlTest> {
    let mut results = Vec::new();
    for path in candidates(hint) {
        let display = if user.is_empty() {
            format!("rtsp://{ip}:{port}{path}")
        } else {
            format!("rtsp://{user}:***@{ip}:{port}{path}")
        };
        let (ok, detail) = probe_path(ip, port, path, user, pass);
        results.push(UrlTest {
            url: display,
            ok,
            detail,
        });
        if ok {
            break;
        }
    }
    results
}

// ---------------------------------------------------------------- 对外入口
/// 解析 rtsp://user:pass@host:port/path
pub fn parse_rtsp_url(url: &str) -> Option<(String, u16, String, String, String)> {
    let rest = url.trim().strip_prefix("rtsp://")?;
    let (cred, hostpart) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (hostport, path) = match hostpart.find('/') {
        Some(i) => (&hostpart[..i], &hostpart[i..]),
        None => (hostpart, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(554)),
        None => (hostport.to_string(), 554),
    };
    let (user, pass) = match cred {
        Some(c) => match c.split_once(':') {
            Some((u, p)) => (pct_decode(u), pct_decode(p)),
            None => (pct_decode(c), String::new()),
        },
        None => (String::new(), String::new()),
    };
    Some((host, port, path.to_string(), user, pass))
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("");
            if let Ok(v) = u8::from_str_radix(hex, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 对单个地址做握手测试（不枚举候选路径）
pub fn probe_url(url: &str) -> UrlTest {
    let display = mask_creds(url);
    let Some((host, port, path, user, pass)) = parse_rtsp_url(url) else {
        return UrlTest {
            url: display,
            ok: false,
            detail: tr("地址格式无法解析").into(),
        };
    };

    let (ok, detail) = probe_path(&host, port, &path, &user, &pass);
    UrlTest {
        url: display,
        ok,
        detail: if ok {
            detail
        } else if detail.contains("401") {
            i18n::wrong_credentials(&detail)
        } else {
            detail
        },
    }
}

fn mask_creds(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.rfind('@') {
            let cred = &rest[..at];
            let user = cred.split(':').next().unwrap_or("");
            return format!("{scheme}{user}:***@{}", &rest[at + 1..]);
        }
    }
    url.to_string()
}

pub fn scan_async(prefix: String, user: String, pass: String, tx: Sender<ScanMsg>) {
    std::thread::spawn(move || {
        let total = 254usize;
        let next = Arc::new(AtomicUsize::new(1));
        let found: Arc<Mutex<Vec<Device>>> = Arc::new(Mutex::new(Vec::new()));
        let workers = 96;

        let mc = multicast_identify();

        std::thread::scope(|scope| {
            for _ in 0..workers {
                let next = next.clone();
                let found = found.clone();
                let tx = tx.clone();
                let prefix = prefix.clone();
                let mc = mc.clone();
                let user = user.clone();
                let pass = pass.clone();
                scope.spawn(move || {
                    loop {
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        if i > 254 {
                            break;
                        }
                        let ip = format!("{prefix}.{i}");
                        let ports: Vec<u16> = SCAN_PORTS
                            .iter()
                            .copied()
                            .filter(|p| port_open(&ip, *p, Duration::from_millis(320)))
                            .collect();
                        let _ = tx.send(ScanMsg::Progress(i.min(254), total));
                        if ports.is_empty() {
                            continue;
                        }

                        let mut hint = mc.get(&ip).map(|(k, v)| format!("{k} {v}")).unwrap_or_default();
                        if hint.is_empty() && ports.contains(&80) {
                            hint = http_banner(&ip);
                        }

                        let mut urls = Vec::new();
                        if ports.contains(&554) {
                            let port = 554;
                            urls = probe_rtsp(&ip, port, &user, &pass, &hint);
                        }

                        let dev = Device {
                            ip,
                            ports,
                            hint,
                            urls,
                        };
                        if let Ok(mut g) = found.lock() {
                            g.push(dev.clone());
                        }
                        let _ = tx.send(ScanMsg::Device(dev));
                    }
                });
            }
        });

        let mut list = found.lock().map(|g| g.clone()).unwrap_or_default();
        list.sort_by_key(|d| {
            d.ip.split('.')
                .next_back()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
        });
        let _ = tx.send(ScanMsg::Done(list));
    });
}
