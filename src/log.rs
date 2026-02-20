use console::{Style, Term, style};
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

const SKIP_RESP_HEADERS: &[&str] = &["server", "date", "connection", "transfer-encoding"];
const SKIP_REQ_HEADERS: &[&str] = &["accept-encoding", "connection"];
const BODY_PREVIEW_LINES: usize = 30;

fn elapsed() -> String {
    let secs = START.get_or_init(Instant::now).elapsed().as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn status_style(status: u16) -> Style {
    match status {
        200..=299 => Style::new().green(),
        300..=399 => Style::new().cyan(),
        400..=499 => Style::new().yellow(),
        _ => Style::new().red(),
    }
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "",
    }
}

fn extract_content_type(headers: &str) -> &str {
    for line in headers.lines() {
        if let Some(colon) = line.find(':')
            && line[..colon].eq_ignore_ascii_case("content-type")
        {
            return line[colon + 1..].trim();
        }
    }
    ""
}

fn find_key_colon(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 1;
    while i < b.len() {
        if b[i] == b'\\' {
            i += 2;
        } else if b[i] == b'"' {
            i += 1;
            return if b.get(i) == Some(&b':') {
                Some(i)
            } else {
                None
            };
        } else {
            i += 1;
        }
    }
    None
}

fn colorize_value(v: &str) -> String {
    match v {
        "true" | "false" => style(v).magenta().to_string(),
        "null" => style(v).dim().to_string(),
        "{" | "}" | "[" | "]" => style(v).dim().to_string(),
        v if v.starts_with('"') && v.ends_with('"') => style(v).green().to_string(),
        v if v.parse::<f64>().is_ok() => style(v).yellow().to_string(),
        v => v.to_owned(),
    }
}

fn colorize_json_line(line: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let trimmed = line.trim_start();

    if matches!(trimmed, "{" | "}" | "}," | "[" | "]" | "],") {
        return format!("{indent}{}", style(trimmed).dim());
    }

    if trimmed.starts_with('"')
        && let Some(colon_pos) = find_key_colon(trimmed)
    {
        let key_part = &trimmed[..colon_pos];
        let after = trimmed[colon_pos + 1..].trim_start();
        let (value_part, comma) = after
            .strip_suffix(',')
            .map(|v| (v, ","))
            .unwrap_or((after, ""));

        let key_col = style(key_part).dim();
        let val_col = colorize_value(value_part);
        return format!("{indent}{key_col}: {val_col}{comma}");
    }

    let (value_part, comma) = trimmed
        .strip_suffix(',')
        .map(|v| (v, ","))
        .unwrap_or((trimmed, ""));
    format!("{indent}{}{comma}", colorize_value(value_part))
}

fn colorize_json(s: &str) -> String {
    s.lines()
        .map(colorize_json_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_body(raw_headers: &str, body: &str, id: u64) -> String {
    if body.trim().is_empty() {
        return String::new();
    }

    let ct = extract_content_type(raw_headers);
    let looks_like_json = ct.contains("application/json") || {
        let t = body.trim();
        t.starts_with('{') || t.starts_with('[')
    };

    let formatted = if looks_like_json {
        serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .map(|p| colorize_json(&p))
            .unwrap_or_else(|| body.to_owned())
    } else {
        body.to_owned()
    };

    let lines: Vec<&str> = formatted.lines().collect();
    if lines.len() > BODY_PREVIEW_LINES {
        let shown = lines[..BODY_PREVIEW_LINES].join("\n");
        let remaining = lines.len() - BODY_PREVIEW_LINES;
        format!(
            "{shown}\n     {}",
            style(format!(
                "… {remaining} more lines  (tnnl replay #{id} to see full)"
            ))
            .dim()
        )
    } else {
        formatted
    }
}

pub fn banner(url: &str, local_host: &str, local_port: u16, inspect: bool) {
    use console::measure_text_width;

    let version = env!("CARGO_PKG_VERSION");
    let ver = format!("v{version}");
    let err = Term::stderr();

    let url_row = format!("{}  {}", style("➜").green(), style(url).green().bold());
    let local_row = format!("{}", style(format!("↳  {local_host}:{local_port}")).dim());
    let inspect_row = format!("{}", style("inspect mode").cyan());

    let mut inner_w = 40usize;
    inner_w = inner_w.max(measure_text_width(&url_row) + 4);
    inner_w = inner_w.max(measure_text_width(&local_row) + 4);
    if inspect {
        inner_w = inner_w.max(measure_text_width(&inspect_row) + 4);
    }

    let border = "─".repeat(inner_w);
    let empty = format!("│{}│", " ".repeat(inner_w));

    let row = |s: &str| -> String {
        let vis = measure_text_width(s);
        let pad = inner_w.saturating_sub(vis + 4);
        format!("│  {s}{}  │", " ".repeat(pad))
    };

    let title_pad = inner_w.saturating_sub(4 + ver.len() + 4);
    let title_row = format!(
        "│  {}{}{}  │",
        style("tnnl").bold(),
        " ".repeat(title_pad),
        style(&ver).dim(),
    );

    let _ = err.write_line("");
    let _ = err.write_line(&format!("┌{border}┐"));
    let _ = err.write_line(&title_row);
    let _ = err.write_line(&empty);
    let _ = err.write_line(&row(&url_row));
    let _ = err.write_line(&row(&local_row));
    if inspect {
        let _ = err.write_line(&empty);
        let _ = err.write_line(&row(&inspect_row));
    }
    let _ = err.write_line(&empty);
    let _ = err.write_line(&format!("└{border}┘"));
    let _ = err.write_line("");
}

pub fn request(method: &str, path: &str, status: u16, duration_ms: u64, id: u64) {
    let ss = status_style(status);
    let path_col = if path.len() > 26 {
        format!("{}…", &path[..25])
    } else {
        format!("{path:<26}")
    };
    let err = Term::stderr();
    let _ = err.write_line(&format!(
        "{}  {}  {} {} {} {}",
        style(elapsed()).dim(),
        style(format!("#{id:<3}")).dim(),
        style(format!("{method:<6}")).bold(),
        path_col,
        ss.clone().bold().apply_to(status),
        style(format!("{duration_ms}ms")).dim(),
    ));
}

pub fn inspect_request(id: u64, raw_headers: &str, body: &str) {
    let err = Term::stderr();
    let mut lines = raw_headers.lines();
    let req_line = lines.next().unwrap_or("");
    let mut parts = req_line.splitn(3, ' ');
    let method = parts.next().unwrap_or("?");
    let path = parts.next().unwrap_or("/");

    let _ = err.write_line("");
    let _ = err.write_line(&format!(
        "  {}  {} {}  {}",
        style("→").cyan().bold(),
        style(method).bold(),
        style(path).bold(),
        style(format!("#{id}")).dim(),
    ));

    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(colon) = line.find(':') {
            let name = &line[..colon];
            if SKIP_REQ_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                continue;
            }
            let value = line[colon + 1..].trim();
            let _ = err.write_line(&format!(
                "     {} {}",
                style(format!("{name}:")).dim(),
                value
            ));
        }
    }

    let formatted = format_body(raw_headers, body, id);
    if !formatted.is_empty() {
        let _ = err.write_line("");
        for line in formatted.lines() {
            let _ = err.write_line(&format!("     {line}"));
        }
    }
}

pub fn inspect_response(status: u16, raw_headers: &str, body: &str, id: u64) {
    let ss = status_style(status);
    let err = Term::stderr();

    let _ = err.write_line("");
    let _ = err.write_line(&format!(
        "  {}  {} {}",
        ss.clone().bold().apply_to("←"),
        ss.clone().bold().apply_to(status),
        style(status_text(status)).dim(),
    ));

    for line in raw_headers.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("HTTP/") {
            continue;
        }
        if let Some(colon) = line.find(':') {
            let name = &line[..colon];
            if SKIP_RESP_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
                continue;
            }
            let value = line[colon + 1..].trim();
            let _ = err.write_line(&format!(
                "     {} {}",
                style(format!("{name}:")).dim(),
                value
            ));
        }
    }

    let formatted = format_body(raw_headers, body, id);
    if !formatted.is_empty() {
        let _ = err.write_line("");
        for line in formatted.lines() {
            let _ = err.write_line(&format!("     {line}"));
        }
    }

    let _ = err.write_line("");
}

pub fn info(msg: &str) {
    let err = Term::stderr();
    let _ = err.write_line(&format!("{}  {msg}", style(elapsed()).dim()));
}

pub fn error(msg: &str) {
    let err = Term::stderr();
    let _ = err.write_line(&format!(
        "{}  {}  {msg}",
        style(elapsed()).dim(),
        style("error").red(),
    ));
}

pub fn success(msg: &str) {
    let err = Term::stderr();
    let _ = err.write_line(&format!(
        "{}  {}",
        style(elapsed()).dim(),
        style(msg).green()
    ));
}
