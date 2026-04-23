//! Thin HTTP provisioning frontend.
//!
//! This module intentionally owns only transport and HTML rendering. All
//! durable provisioning state and validation still go through bunzo-provisiond.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::str;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use bunzo_proto::async_io::{read_frame_async, write_frame_async};
use bunzo_proto::{
    Envelope, ProvisionClientMessage, ProvisionServerFrame, ProvisionServerMessage,
    ProvisioningSetupInput, ProvisioningStatus, PROTOCOL_VERSION,
};
use listenfd::ListenFd;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream, UnixStream};
use tokio::time::timeout;

use crate::config::RECOMMENDED_OPENAI_MODEL;
use crate::provisioning::SOCKET_PATH as PROVISIOND_SOCKET;

pub const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";
pub const DEFAULT_GUEST_PORT: u16 = 8080;

const MAX_HEADER_BYTES: usize = 32 * 1024;
const MAX_BODY_BYTES: usize = 16 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
enum ProvisionClientError {
    Unreachable(String),
    Protocol(String),
    Remote { code: String, text: String },
}

impl fmt::Display for ProvisionClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(reason) => write!(f, "bunzo-provisiond unreachable: {reason}"),
            Self::Protocol(reason) => write!(f, "protocol error: {reason}"),
            Self::Remote { code, text } => write!(f, "[{code}] {text}"),
        }
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

struct FlashMessage {
    kind: FlashKind,
    text: String,
}

enum FlashKind {
    Success,
    Error,
}

pub async fn run_server() -> Result<()> {
    let listener = acquire_listener().await?;
    eprintln!(
        "bunzo-setup-httpd: accepting connections on {}",
        DEFAULT_BIND_ADDR
    );

    loop {
        let (stream, _addr) = listener.accept().await?;
        tokio::spawn(async move {
            if let Err(err) = handle_connection(stream).await {
                eprintln!("bunzo-setup-httpd: connection ended: {err:#}");
            }
        });
    }
}

async fn acquire_listener() -> Result<TcpListener> {
    let mut listenfd = ListenFd::from_env();
    if let Some(std_listener) = listenfd.take_tcp_listener(0)? {
        std_listener.set_nonblocking(true)?;
        eprintln!("bunzo-setup-httpd: using socket-activated listener from systemd");
        return TcpListener::from_std(std_listener).context("wrapping inherited listener");
    }

    let listener = TcpListener::bind(DEFAULT_BIND_ADDR)
        .await
        .with_context(|| format!("binding {DEFAULT_BIND_ADDR}"))?;
    eprintln!("bunzo-setup-httpd: bound {DEFAULT_BIND_ADDR} directly");
    Ok(listener)
}

async fn handle_connection(mut stream: TcpStream) -> Result<()> {
    let request = read_request(&mut stream).await?;
    let response = route_request(request).await;
    write_response(&mut stream, response).await
}

async fn route_request(request: HttpRequest) -> HttpResponse {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => match request_provisioning_status().await {
            Ok(status) => html_response(render_page(Some(&status), None, None)),
            Err(err) => html_response(render_page(
                None,
                Some(&format!("{err}")),
                Some(&FlashMessage {
                    kind: FlashKind::Error,
                    text: format!("status check failed: {err}"),
                }),
            )),
        },
        ("GET", "/status") => match request_provisioning_status().await {
            Ok(status) => json_response("200 OK", &status),
            Err(err) => json_response(
                "503 Service Unavailable",
                &serde_json::json!({
                    "error": err.to_string(),
                }),
            ),
        },
        ("POST", "/setup") => match parse_setup_form(&request) {
            Ok(setup) => match request_apply_setup(&setup).await {
                Ok(status) => html_response(render_page(
                    Some(&status),
                    None,
                    Some(&FlashMessage {
                        kind: FlashKind::Success,
                        text: format!(
                            "validated OpenAI access for {}, applied the hostname, and rendered {} for {}",
                            status.device_name.as_deref().unwrap_or("this device"),
                            status
                                .rendered_config_path
                                .as_deref()
                                .unwrap_or("/etc/bunzo/bunzod.toml"),
                            status.model.as_deref().unwrap_or(RECOMMENDED_OPENAI_MODEL),
                        ),
                    }),
                )),
                Err(err) => {
                    let status = request_provisioning_status().await.ok();
                    html_response(render_page(
                        status.as_ref(),
                        None,
                        Some(&FlashMessage {
                            kind: FlashKind::Error,
                            text: format!("setup failed: {err}"),
                        }),
                    ))
                }
            },
            Err(err) => html_response(render_page(
                None,
                None,
                Some(&FlashMessage {
                    kind: FlashKind::Error,
                    text: format!("invalid request: {err}"),
                }),
            )),
        },
        ("GET", "/favicon.ico") => HttpResponse {
            status: "204 No Content",
            content_type: "text/plain; charset=utf-8",
            body: Vec::new(),
        },
        _ => html_with_status(
            "404 Not Found",
            "<!doctype html><meta charset=\"utf-8\"><title>not found</title><p>not found</p>"
                .to_string(),
        ),
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut buf = Vec::new();
    let header_end = loop {
        if let Some(offset) = find_header_end(&buf) {
            break offset;
        }
        if buf.len() >= MAX_HEADER_BYTES {
            bail!("request headers exceed {MAX_HEADER_BYTES} bytes");
        }
        let mut chunk = [0u8; 1024];
        let read = timeout(REQUEST_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("timed out reading request headers")??;
        if read == 0 {
            bail!("client closed before sending a complete request");
        }
        buf.extend_from_slice(&chunk[..read]);
    };

    let header_bytes = &buf[..header_end];
    let header_text =
        str::from_utf8(header_bytes).context("request headers are not valid utf-8")?;

    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| anyhow!("missing request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing request method"))?
        .to_string();
    let target = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing request target"))?;
    let version = request_parts
        .next()
        .ok_or_else(|| anyhow!("missing request version"))?;
    if !version.starts_with("HTTP/1.") {
        bail!("unsupported HTTP version '{version}'");
    }

    let (path, _query) = match target.split_once('?') {
        Some((path, query)) => (path.to_string(), Some(query.to_string())),
        None => (target.to_string(), None),
    };

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| anyhow!("malformed header line '{line}'"))?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>())
        .transpose()
        .context("parsing content-length")?
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        bail!("request body exceeds {MAX_BODY_BYTES} bytes");
    }

    let body_start = header_end + 4;
    let mut body = buf[body_start..].to_vec();
    while body.len() < content_length {
        let mut chunk = vec![0u8; content_length - body.len()];
        let read = timeout(REQUEST_TIMEOUT, stream.read(&mut chunk))
            .await
            .context("timed out reading request body")??;
        if read == 0 {
            bail!("client closed before sending the full request body");
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(content_length);

    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

async fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    timeout(REQUEST_TIMEOUT, stream.write_all(header.as_bytes()))
        .await
        .context("timed out writing response headers")??;
    if !response.body.is_empty() {
        timeout(REQUEST_TIMEOUT, stream.write_all(&response.body))
            .await
            .context("timed out writing response body")??;
    }
    timeout(REQUEST_TIMEOUT, stream.shutdown())
        .await
        .context("timed out shutting down response stream")??;
    Ok(())
}

async fn request_provisioning_status() -> Result<ProvisioningStatus, ProvisionClientError> {
    let id = next_request_id("status");
    let message = ProvisionClientMessage::GetProvisioningStatus { id: id.clone() };
    let mut stream = connect_provisiond().await?;
    write_provision_request(&mut stream, &message).await?;
    let (read_half, _write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    loop {
        let frame: ProvisionServerFrame = read_provision_frame(&mut reader).await?;
        match frame.msg {
            ProvisionServerMessage::ProvisioningStatus {
                id: status_id,
                status,
            } if status_id == id => return Ok(status),
            ProvisionServerMessage::ProvisioningStatus { .. } => {}
            ProvisionServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(ProvisionClientError::Remote { code, text });
            }
            ProvisionServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

async fn request_apply_setup(
    setup: &ProvisioningSetupInput,
) -> Result<ProvisioningStatus, ProvisionClientError> {
    let id = next_request_id("apply");
    let message = ProvisionClientMessage::ApplySetup {
        id: id.clone(),
        setup: setup.clone(),
    };
    let mut stream = connect_provisiond().await?;
    write_provision_request(&mut stream, &message).await?;
    let (read_half, _write_half) = stream.split();
    let mut reader = BufReader::new(read_half);

    loop {
        let frame: ProvisionServerFrame = read_provision_frame(&mut reader).await?;
        match frame.msg {
            ProvisionServerMessage::ProvisioningResult {
                id: result_id,
                status,
            } if result_id == id => return Ok(status),
            ProvisionServerMessage::ProvisioningResult { .. } => {}
            ProvisionServerMessage::Error {
                id: err_id,
                code,
                text,
            } if err_id == id || err_id.is_empty() => {
                return Err(ProvisionClientError::Remote { code, text });
            }
            ProvisionServerMessage::Error { .. } => {}
            _ => {}
        }
    }
}

async fn connect_provisiond() -> Result<UnixStream, ProvisionClientError> {
    timeout(REQUEST_TIMEOUT, UnixStream::connect(PROVISIOND_SOCKET))
        .await
        .map_err(|_| {
            ProvisionClientError::Unreachable(format!(
                "timed out connecting to {PROVISIOND_SOCKET}"
            ))
        })?
        .map_err(|err| ProvisionClientError::Unreachable(err.to_string()))
}

async fn write_provision_request(
    stream: &mut UnixStream,
    message: &ProvisionClientMessage,
) -> Result<(), ProvisionClientError> {
    let envelope = Envelope::new(message.clone());
    timeout(REQUEST_TIMEOUT, write_frame_async(stream, &envelope))
        .await
        .map_err(|_| ProvisionClientError::Protocol("timed out writing request".into()))?
        .map_err(|err| ProvisionClientError::Protocol(err.to_string()))
}

async fn read_provision_frame<R>(
    reader: &mut R,
) -> Result<ProvisionServerFrame, ProvisionClientError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let frame: ProvisionServerFrame = timeout(REQUEST_TIMEOUT, read_frame_async(reader))
        .await
        .map_err(|_| ProvisionClientError::Protocol("timed out waiting for reply".into()))?
        .map_err(|err: io::Error| {
            if err.kind() == io::ErrorKind::UnexpectedEof {
                ProvisionClientError::Protocol(
                    "bunzo-provisiond closed the connection before replying".into(),
                )
            } else {
                ProvisionClientError::Protocol(err.to_string())
            }
        })?;

    if frame.v != PROTOCOL_VERSION {
        return Err(ProvisionClientError::Protocol(format!(
            "client speaks v{}, bunzo-provisiond speaks v{}",
            PROTOCOL_VERSION, frame.v
        )));
    }

    Ok(frame)
}

fn parse_setup_form(request: &HttpRequest) -> Result<ProvisioningSetupInput> {
    let content_type = request
        .headers
        .get("content-type")
        .map(String::as_str)
        .unwrap_or("");
    if !content_type.starts_with("application/x-www-form-urlencoded") {
        bail!("expected application/x-www-form-urlencoded");
    }

    let raw_body = str::from_utf8(&request.body).context("request body is not valid utf-8")?;
    let form = parse_form_fields(raw_body)?;
    Ok(ProvisioningSetupInput {
        device_name: non_empty(form.get("device_name").cloned()),
        connectivity_kind: non_empty(form.get("connectivity_kind").cloned()),
        provider_kind: non_empty(form.get("provider_kind").cloned()),
        api_key: form.get("api_key").cloned().unwrap_or_default(),
    })
}

fn parse_form_fields(raw: &str) -> Result<HashMap<String, String>> {
    let mut fields = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (name, value) = match pair.split_once('=') {
            Some((name, value)) => (name, value),
            None => (pair, ""),
        };
        let name = decode_form_component(name)?;
        let value = decode_form_component(value)?;
        fields.insert(name, value);
    }
    Ok(fields)
}

fn decode_form_component(raw: &str) -> Result<String> {
    let bytes = raw.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                decoded.push(b' ');
                i += 1;
            }
            b'%' => {
                if i + 2 >= bytes.len() {
                    bail!("incomplete percent-encoding in form body");
                }
                let hi = decode_hex(bytes[i + 1])?;
                let lo = decode_hex(bytes[i + 2])?;
                decoded.push((hi << 4) | lo);
                i += 3;
            }
            byte => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(decoded).context("decoded form body is not valid utf-8")
}

fn decode_hex(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => bail!("invalid hex digit in form body"),
    }
}

fn render_page(
    status: Option<&ProvisioningStatus>,
    page_error: Option<&str>,
    flash: Option<&FlashMessage>,
) -> String {
    let page_title = match status {
        Some(status) if status.ready => "bunzo is ready",
        Some(status) => match status.phase.as_str() {
            "failed_recoverable" => "bunzo setup needs attention",
            "validating" => "bunzo is validating setup",
            _ => "bunzo setup",
        },
        None => "bunzo setup",
    };
    let phase = status
        .map(|status| status.phase.as_str())
        .unwrap_or("unknown");
    let phase_class = match phase {
        "ready" => "phase-ready",
        "failed_recoverable" => "phase-failed",
        "validating" => "phase-validating",
        _ => "phase-pending",
    };
    let device_name = status
        .and_then(|status| status.device_name.as_deref())
        .unwrap_or("");
    let connectivity = status
        .and_then(|status| status.connectivity_kind.as_deref())
        .unwrap_or("existing_network");
    let provider = status
        .and_then(|status| status.provider_kind.as_deref())
        .unwrap_or("openai");
    let model = status
        .and_then(|status| status.model.as_deref())
        .unwrap_or(RECOMMENDED_OPENAI_MODEL);
    let detail = status
        .and_then(|status| status.detail.as_deref())
        .unwrap_or("setup has not completed yet");
    let flash_html = flash
        .map(|flash| {
            let class_name = match flash.kind {
                FlashKind::Success => "flash-success",
                FlashKind::Error => "flash-error",
            };
            format!(
                "<div class=\"flash {class_name}\">{}</div>",
                escape_html(&flash.text)
            )
        })
        .unwrap_or_default();
    let page_error_html = page_error
        .map(|err| {
            format!(
                "<div class=\"flash flash-error\">{}</div>",
                escape_html(err)
            )
        })
        .unwrap_or_default();
    let status_ready = status.is_some_and(|status| status.ready);
    let ready_text = if status_ready { "ready" } else { "not ready" };
    let rendered_config = status
        .and_then(|status| status.rendered_config_path.as_deref())
        .unwrap_or("/etc/bunzo/bunzod.toml");

    format!(
        concat!(
            "<!doctype html>",
            "<html lang=\"en\"><head>",
            "<meta charset=\"utf-8\">",
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">",
            "<title>{page_title}</title>",
            "<style>",
            ":root {{ --ink: #f3efe6; --paper: #101724; --paper-soft: #182234; --line: rgba(243,239,230,0.14); --accent: #f59e0b; --good: #3ecf8e; --bad: #ff8b8b; --muted: #9fb0c7; font-family: \"Avenir Next\", \"Segoe UI\", sans-serif; }}",
            "* {{ box-sizing: border-box; }}",
            "body {{ margin: 0; min-height: 100vh; background: radial-gradient(circle at top, #223550 0%, #101724 45%, #08101a 100%); color: var(--ink); }}",
            ".shell {{ width: min(780px, calc(100vw - 24px)); margin: 0 auto; padding: 24px 0 40px; }}",
            ".hero {{ padding: 18px 20px 10px; }}",
            ".eyebrow {{ color: var(--accent); font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.16em; margin-bottom: 10px; }}",
            "h1 {{ margin: 0; font-size: clamp(2rem, 7vw, 3.4rem); line-height: 0.95; }}",
            ".summary {{ margin-top: 12px; max-width: 52ch; color: var(--muted); line-height: 1.45; }}",
            ".panel {{ background: linear-gradient(180deg, rgba(24,34,52,0.94), rgba(12,18,28,0.98)); border: 1px solid var(--line); border-radius: 22px; padding: 18px; box-shadow: 0 28px 80px rgba(0,0,0,0.28); }}",
            ".stack {{ display: grid; gap: 16px; }}",
            ".status-grid {{ display: grid; gap: 12px; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); margin-top: 14px; }}",
            ".metric {{ padding: 12px; border-radius: 16px; background: rgba(255,255,255,0.03); border: 1px solid rgba(255,255,255,0.05); }}",
            ".metric label {{ display: block; color: var(--muted); font-size: 0.78rem; text-transform: uppercase; letter-spacing: 0.08em; margin-bottom: 6px; }}",
            ".metric strong {{ font-size: 1rem; word-break: break-word; }}",
            ".phase {{ display: inline-flex; align-items: center; gap: 8px; padding: 8px 12px; border-radius: 999px; font-size: 0.88rem; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase; }}",
            ".phase-ready {{ background: rgba(62,207,142,0.14); color: var(--good); }}",
            ".phase-failed {{ background: rgba(255,139,139,0.14); color: var(--bad); }}",
            ".phase-validating {{ background: rgba(245,158,11,0.14); color: var(--accent); }}",
            ".phase-pending {{ background: rgba(159,176,199,0.14); color: var(--muted); }}",
            ".flash {{ padding: 14px 16px; border-radius: 16px; font-weight: 600; }}",
            ".flash-success {{ background: rgba(62,207,142,0.13); color: var(--good); border: 1px solid rgba(62,207,142,0.28); }}",
            ".flash-error {{ background: rgba(255,139,139,0.13); color: #ffd2d2; border: 1px solid rgba(255,139,139,0.28); }}",
            "form {{ display: grid; gap: 12px; }}",
            ".field {{ display: grid; gap: 6px; }}",
            ".field label {{ color: var(--muted); font-size: 0.86rem; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase; }}",
            "input, select, button {{ width: 100%; border-radius: 14px; border: 1px solid rgba(255,255,255,0.08); padding: 14px 15px; font: inherit; }}",
            "input, select {{ background: rgba(5,10,18,0.7); color: var(--ink); }}",
            "button {{ background: linear-gradient(135deg, #f59e0b, #ffcb66); color: #1f1300; font-weight: 800; border: 0; cursor: pointer; }}",
            ".footnote {{ color: var(--muted); font-size: 0.92rem; line-height: 1.45; }}",
            "a {{ color: #ffd08a; }}",
            "@media (max-width: 640px) {{ .shell {{ width: min(100vw - 16px, 780px); padding-top: 16px; }} .panel {{ padding: 16px; border-radius: 18px; }} h1 {{ letter-spacing: -0.03em; }} }}",
            "</style></head><body>",
            "<main class=\"shell stack\">",
            "<section class=\"hero\">",
            "<div class=\"eyebrow\">bunzo provisioning</div>",
            "<h1>{page_title}</h1>",
            "<p class=\"summary\">This screen is a thin frontend over <code>bunzo-provisiond</code>. Setup writes canonical state under <code>/var/lib/bunzo/</code>, applies the device name as the system hostname, and re-renders runtime config from there.</p>",
            "</section>",
            "<section class=\"panel stack\">",
            "{flash_html}{page_error_html}",
            "<div class=\"phase {phase_class}\">phase: {phase}</div>",
            "<div class=\"status-grid\">",
            "<div class=\"metric\"><label>Readiness</label><strong>{ready_text}</strong></div>",
            "<div class=\"metric\"><label>Device</label><strong>{device_name}</strong></div>",
            "<div class=\"metric\"><label>Connectivity</label><strong>{connectivity}</strong></div>",
            "<div class=\"metric\"><label>Backend</label><strong>{provider}</strong></div>",
            "<div class=\"metric\"><label>Model</label><strong>{model}</strong></div>",
            "<div class=\"metric\"><label>Rendered Config</label><strong>{rendered_config}</strong></div>",
            "</div>",
            "<div class=\"metric\"><label>Detail</label><strong>{detail}</strong></div>",
            "</section>",
            "<section class=\"panel stack\">",
            "<form method=\"post\" action=\"/setup\">",
            "<div class=\"field\"><label for=\"device_name\">Device name</label><input id=\"device_name\" name=\"device_name\" type=\"text\" value=\"{device_name_value}\" placeholder=\"bunzo\" autocomplete=\"off\"></div>",
            "<div class=\"field\"><label for=\"connectivity_kind\">Connectivity mode</label><select id=\"connectivity_kind\" name=\"connectivity_kind\"><option value=\"existing_network\">existing_network</option></select></div>",
            "<div class=\"field\"><label for=\"provider_kind\">Provider</label><select id=\"provider_kind\" name=\"provider_kind\"><option value=\"openai\">openai ({recommended_model})</option></select></div>",
            "<div class=\"field\"><label for=\"api_key\">OpenAI API key</label><input id=\"api_key\" name=\"api_key\" type=\"password\" placeholder=\"sk-...\" autocomplete=\"off\"></div>",
            "<button type=\"submit\">Validate and Provision</button>",
            "</form>",
            "<p class=\"footnote\">The chosen device name becomes the live and persistent system hostname. This slice intentionally keeps networking narrow: the frontend only supports <code>existing_network</code>, and the backend remains pinned to the GPT-5.4 family with <code>{recommended_model}</code> as the current setup default.</p>",
            "<p class=\"footnote\">Need machine-readable status for smoke tests? Use <a href=\"/status\">/status</a>.</p>",
            "</section>",
            "</main></body></html>"
        ),
        page_title = escape_html(page_title),
        flash_html = flash_html,
        page_error_html = page_error_html,
        phase_class = phase_class,
        phase = escape_html(phase),
        ready_text = ready_text,
        device_name = escape_html(if device_name.is_empty() {
            "pending"
        } else {
            device_name
        }),
        connectivity = escape_html(connectivity),
        provider = escape_html(provider),
        model = escape_html(model),
        rendered_config = escape_html(rendered_config),
        detail = escape_html(detail),
        device_name_value = escape_html(device_name),
        recommended_model = escape_html(RECOMMENDED_OPENAI_MODEL),
    )
}

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn html_response(body: String) -> HttpResponse {
    html_with_status("200 OK", body)
}

fn html_with_status(status: &'static str, body: String) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "text/html; charset=utf-8",
        body: body.into_bytes(),
    }
}

fn json_response<T: serde::Serialize>(status: &'static str, body: &T) -> HttpResponse {
    let body = serde_json::to_vec_pretty(body)
        .unwrap_or_else(|_| b"{\"error\":\"serialization failed\"}".to_vec());
    HttpResponse {
        status,
        content_type: "application/json; charset=utf-8",
        body,
    }
}

fn next_request_id(prefix: &str) -> String {
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("http-{prefix}-{seq}")
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|window| window == b"\r\n\r\n")
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_fields_decode_percent_encoding() {
        let fields =
            parse_form_fields("device_name=bunzo+qemu&provider_kind=openai&api_key=sk-test%2Babc")
                .unwrap();
        assert_eq!(
            fields.get("device_name").map(String::as_str),
            Some("bunzo qemu")
        );
        assert_eq!(
            fields.get("api_key").map(String::as_str),
            Some("sk-test+abc")
        );
    }

    #[test]
    fn html_escape_handles_special_characters() {
        assert_eq!(escape_html("<bunzo>&\"'"), "&lt;bunzo&gt;&amp;&quot;&#39;");
    }
}
