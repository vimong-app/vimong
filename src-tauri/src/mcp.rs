// `vimong --mcp` — Claude 같은 AI 클라이언트가 stdio로 붙는 MCP 서버.
// 실행 중인 앱이 "지금 보고 있는 문서/페이지"를 current_view.json에 기록하고(set_current_view),
// 이 서버는 그 파일만 읽는다 — 앱과 직접 IPC를 하지 않으니 앱이 꺼져 있으면 "열린 문서 없음"이 된다.
// MCP는 줄 단위 JSON-RPC라 SDK 의존성 없이 serde_json만으로 처리한다.
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;

// Tauri의 app_config_dir()와 같은 위치 — MCP 모드는 Tauri 앱 없이 돌기 때문에 직접 계산한다.
pub(crate) fn current_view_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(target_os = "windows")]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    base.map(|b| b.join("com.demo.vimong").join("current_view.json"))
}

pub fn run() {
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
        // id가 없으면 notification(notifications/initialized 등) — 응답하지 않는다
        let Some(id) = msg.get("id").cloned() else { continue };
        let resp = match handle(msg["method"].as_str().unwrap_or(""), &msg["params"]) {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(e) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": e } }),
        };
        let _ = writeln!(out, "{resp}");
        let _ = out.flush();
    }
}

fn handle(method: &str, params: &Value) -> Result<Value, String> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "vimong", "version": env!("CARGO_PKG_VERSION") },
            // 사용자는 대화 밖에서 언제든 문서/페이지를 바꾸는데 서버가 그걸 먼저 알릴 방법이 없다 —
            // 이전 결과를 재사용하지 말고 매번 다시 확인하라고 시스템 프롬프트 수준에서 못박는다.
            "instructions": "The user switches documents, pages and text selection in Vimong at any time, outside this conversation. Whenever the user refers to the document or page they are viewing (\"this\", \"current\", \"here\", or no explicit file), call get_current_view first — never reuse a path, page or selection from earlier in the conversation. When the user refers to text they selected or highlighted on screen (\"this sentence\", \"the selected part\"), call get_selection. To look at figures, charts, equations or scanned pages, use get_page_image.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": [
            {
                "name": "get_current_view",
                "description": "Return the document and page the user is currently viewing in the Vimong PDF viewer: {path, page (1-based), page_count}. The view changes outside the conversation, so call this again on every request about the current document instead of reusing an earlier result.",
                "inputSchema": { "type": "object", "properties": {} },
            },
            {
                "name": "save_pages",
                "description": "Save pages of the document currently open in Vimong as a new PDF. Defaults to the page the user is currently viewing. Saves the file as it is on disk (unsaved annotations in Vimong are not included).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "output_path": { "type": "string", "description": "Output PDF path. Relative paths are resolved against the open document's folder." },
                        "pages": pages_schema(),
                    },
                    "required": ["output_path"],
                },
            },
            {
                "name": "save_markdown",
                "description": "Extract the text of the document currently open in Vimong and save it as a Markdown file. Headings are detected from font size; each page starts with a <!-- page N --> comment. Defaults to the page the user is currently viewing.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "output_path": { "type": "string", "description": "Output .md path. Relative paths are resolved against the open document's folder." },
                        "pages": pages_schema(),
                    },
                    "required": ["output_path"],
                },
            },
            {
                "name": "get_page_text",
                "description": "Return the text of the document currently open in Vimong as Markdown (headings detected from font size, <!-- page N --> before each page). Text only: figures, images and equations are not included. Defaults to the page the user is currently viewing.",
                "inputSchema": { "type": "object", "properties": { "pages": pages_schema() } },
            },
            {
                "name": "get_notes",
                "description": "List the highlights (with the highlighted text) and sticky notes the user made in the PDF currently open in Vimong. Reads the file on disk, so annotations not yet saved with :w are not included. Defaults to all pages.",
                "inputSchema": { "type": "object", "properties": { "pages": { "type": "string", "description": "1-based page spec like \"3\" or \"1-3,7\". Defaults to all pages." } } },
            },
            {
                "name": "get_selection",
                "description": "Return the text the user currently has selected in Vimong (empty if nothing is selected).",
                "inputSchema": { "type": "object", "properties": {} },
            },
            {
                "name": "get_page_image",
                "description": "Render pages of the document currently open in Vimong as JPEG images, so you can see figures, charts, equations, layout or scanned pages that get_page_text cannot capture. At most 5 pages per call. Defaults to the page the user is currently viewing.",
                "inputSchema": { "type": "object", "properties": {
                    "pages": pages_schema(),
                    "scale": { "type": "number", "description": "Render scale (1.0 = 72 dpi). Default 1.5; raise to 2-3 for small text." },
                } },
            },
            {
                "name": "goto_page",
                "description": "Scroll Vimong to the given page so the user sees it on screen.",
                "inputSchema": { "type": "object", "properties": { "page": { "type": "integer", "description": "1-based page number." } }, "required": ["page"] },
            },
            {
                "name": "add_highlight",
                "description": "Highlight a passage in the PDF currently open in Vimong, optionally with a comment shown as a sticky note next to it. Give the first few words of the passage as start and its last few words as end (omit end to highlight just start), copied from get_page_text; matching ignores case, spaces, punctuation and hyphenation. The highlight is added unsaved, like one the user made: they save it with :w or undo it with u. Returns the highlighted text so you can check the span.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "page": { "type": "integer", "description": "1-based page number. Defaults to the page the user is currently viewing." },
                        "start": { "type": "string", "description": "First words of the passage, unique on the page." },
                        "end": { "type": "string", "description": "Last words of the passage. Omit to highlight start alone." },
                        "comment": { "type": "string", "description": "Optional comment, added as a sticky note at the end of the highlight." },
                        "color": { "type": "string", "description": "Optional #RRGGBB color. Defaults to the user's highlight color." },
                    },
                    "required": ["start"],
                },
            },
            {
                "name": "search_text",
                "description": "Find a word or phrase in the document currently open in Vimong without reading the whole text. Returns each match as its page number and the surrounding text, with the match in **bold**. Matching ignores case, spaces, punctuation and hyphenation and also finds it inside longer words. Plain text only, no regex. For a topic rather than an exact term, search several synonyms or key terms in turn; read a hit's full page with get_page_text. Defaults to all pages.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Word or phrase to find." },
                        "pages": { "type": "string", "description": "1-based page spec like \"3\" or \"1-3,7\". Defaults to all pages." },
                    },
                    "required": ["query"],
                },
            },
            {
                "name": "get_outline",
                "description": "Return the table of contents (outline/bookmarks) of the document currently open in Vimong, with 1-based page numbers. Use it to map chapter or section names to page ranges.",
                "inputSchema": { "type": "object", "properties": {} },
            },
        ]})),
        // 도구 실행 실패는 JSON-RPC 에러가 아니라 isError 결과로 돌려줘야 AI가 메시지를 읽는다
        "tools/call" => Ok(match call_tool(params["name"].as_str().unwrap_or(""), &params["arguments"]) {
            Ok(content) => json!({ "content": content }),
            Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
        }),
        _ => Err(format!("method not found: {method}")),
    }
}

// ponytail: 앱이 비정상 종료하면 current_view.json이 남아 오래된 상태를 돌려준다 — 문제되면 pid를 같이 기록해 살아있는지 확인
fn read_view() -> Result<Value, String> {
    let no_view = || "No document is open in Vimong (or Vimong is not running).".to_string();
    let text = current_view_path().and_then(|p| std::fs::read_to_string(p).ok()).ok_or_else(no_view)?;
    serde_json::from_str(&text).map_err(|_| no_view())
}

// 도구 결과가 너무 크면 대화 컨텍스트를 다 잡아먹으니 자른다 — 더 필요하면 pages를 나눠 다시 부르게 안내
const MAX_TEXT_CHARS: usize = 100_000;

const MAX_IMAGE_PAGES: usize = 5; // 페이지 이미지는 한 장에 수백 KB라 한 번에 너무 많이 보내지 않는다

// 결과는 MCP content 배열 — 대부분 텍스트 하나, get_page_image만 이미지 여러 개
fn call_tool(name: &str, args: &Value) -> Result<Vec<Value>, String> {
    const TOOLS: [&str; 11] = [
        "get_current_view", "save_pages", "save_markdown", "get_page_text", "get_notes", "get_outline",
        "get_selection", "get_page_image", "goto_page", "add_highlight", "search_text",
    ];
    if !TOOLS.contains(&name) {
        return Err(format!("unknown tool: {name}"));
    }
    let text = |s: String| vec![json!({ "type": "text", "text": s })];
    let view = read_view()?;
    match name {
        "get_current_view" => return Ok(text(view.to_string())),
        "get_selection" => {
            let sel = view["selection"].as_str().unwrap_or("");
            return Ok(text(if sel.is_empty() { "No text is selected in Vimong.".to_string() } else { sel.to_string() }));
        }
        "goto_page" => {
            let page = args["page"].as_u64().ok_or("page is required")?;
            let page_count = view["page_count"].as_u64().unwrap_or(0);
            if page == 0 || page > page_count {
                return Err(format!("page must be between 1 and {page_count}"));
            }
            signal_app(&["--goto-page", &page.to_string()])?;
            return Ok(text(format!("Moved Vimong to page {page}")));
        }
        "add_highlight" => return add_highlight(args, &view).map(text),
        _ => {}
    }
    let pages = page_spec(args, &view, name == "get_notes" || name == "search_text");
    if name == "get_page_image" {
        let path = view["path"].as_str().ok_or("invalid view state")?;
        let page_count = view["page_count"].as_u64().unwrap_or(0) as usize;
        let scale = args["scale"].as_f64().unwrap_or(1.5).clamp(0.5, 4.0) as f32;
        return page_images(path, &pages, page_count, scale);
    }
    call_text_tool(name, args, &view, &pages).map(text)
}

// 이미 떠 있는 앱으로 single-instance 인자 전달(forward-search와 같은 경로) — 자식은
// 인자만 넘기고 바로 끝난다. 출력이 MCP stdout에 섞이면 안 되니 버린다.
// ponytail: current_view.json이 비정상 종료로 남아 있으면 앱이 새로 뜬다(전달한 동작은 무시됨)
fn signal_app(args: &[&str]) -> Result<(), String> {
    let status = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("failed to signal Vimong ({status})"));
    }
    Ok(())
}

const NOTE_ICON_SIZE: f32 = 18.0; // lib.rs add_note의 ICON_SIZE와 같게
const MIN_CANDIDATE_CHARS: usize = 4; // 못 찾았을 때 후보로 보여줄 최소 일치 길이(정규화 글자 수)

// 위치는 여기서(디스크 파일로) 찾고 앱엔 좌표만 넘긴다 — 앱으로 보내는 신호는 응답이 없어서,
// 못 찾았을 때 AI에게 이유와 후보를 돌려주려면 매칭을 이쪽에서 끝내야 한다. 하이라이트는
// 글자 위치를 바꾸지 않으니 디스크 파일과 앱이 들고 있는 미저장 문서의 글자 좌표는 같다.
fn add_highlight(args: &Value, view: &Value) -> Result<String, String> {
    let path = view["path"].as_str().ok_or("invalid view state")?;
    let page_count = view["page_count"].as_u64().unwrap_or(0);
    let page = args["page"].as_u64().or(view["page"].as_u64()).unwrap_or(0);
    if page == 0 || page > page_count {
        return Err(format!("page must be between 1 and {page_count}"));
    }
    let start = args["start"].as_str().ok_or("start is required")?;
    let color = args["color"].as_str();
    if color.is_some_and(|c| c.len() != 7 || !c.starts_with('#') || !c[1..].chars().all(|h| h.is_ascii_hexdigit())) {
        return Err("color must be #RRGGBB".to_string());
    }
    let doc = open_doc(path)?;
    if !doc.is_pdf() {
        return Err("Highlights are only supported for PDF files".to_string());
    }
    let load = |p: u64| doc.load_page(p as i32 - 1).map_err(|e| e.to_string());
    let pg = load(page)?;
    let chars = super::extract_chars(&pg)?;
    let hay = normalize(chars.iter().map(|(c, _)| *c));
    let needle = |s: &str| normalize(s.chars()).into_iter().map(|(c, _)| c).collect::<Vec<_>>();

    let s = needle(start);
    if s.is_empty() {
        return Err("start must contain letters or digits".to_string());
    }
    let a = match find_all(&hay, &s)[..] {
        [a] => a,
        [] => {
            let mut msg = format!("Could not find \"{start}\" on page {page}.");
            let elsewhere: Vec<String> = (1..=page_count)
                .filter(|&p| p != page)
                .filter(|&p| {
                    load(p).and_then(|pg| super::extract_chars(&pg)).is_ok_and(|cs| !find_all(&normalize(cs.iter().map(|(c, _)| *c)), &s).is_empty())
                })
                .map(|p| p.to_string())
                .collect();
            if !elsewhere.is_empty() {
                msg += &format!(" It appears on page {}.", elsewhere.join(", "));
            } else if let Some(at) = (MIN_CANDIDATE_CHARS..s.len()).rev().find_map(|k| find_all(&hay, &s[..k]).first().copied()) {
                // ponytail: 앞부분이 가장 길게 맞는 곳만 후보로 — 앞쪽 단어가 틀린 경우엔 후보가 없다
                let snippet: String = chars[hay[at].1..].iter().map(|(c, _)| *c).take(start.chars().count() + 40).collect();
                msg += &format!(" Closest text there: \"{}\"", snippet.trim());
            }
            return Err(msg + " Copy start/end verbatim from get_page_text.");
        }
        ref hits => return Err(format!("\"{start}\" appears {} times on page {page} — use a longer start.", hits.len())),
    };
    let mut b = a + s.len();
    if let Some(end) = args["end"].as_str().filter(|e| !e.is_empty()) {
        let e = needle(end);
        let rel = find_all(&hay[a..], &e).first().copied().filter(|_| !e.is_empty());
        b = a + rel.ok_or_else(|| format!("Found start on page {page} but not \"{end}\" after it."))? + e.len();
    }
    let span = &chars[span_range(&chars, hay[a].1, hay[b - 1].1)];
    let rects = line_rects(span);
    let last = rects.last().ok_or("matched text has no position")?;
    let width = pg.bounds().map_err(|e| e.to_string())?.x1;

    let payload = json!({
        "path": path,
        "page": page - 1,
        "rects": rects,
        "color": color,
        "comment": args["comment"].as_str().filter(|c| !c.trim().is_empty()),
        "note": [(last[2] + 2.0).min(width - NOTE_ICON_SIZE), last[1]],
    });
    signal_app(&["--mcp-highlight", &payload.to_string()])?;
    let text: String = span.iter().map(|(c, _)| *c).collect();
    Ok(format!("Highlighted on page {page}: \"{}\" (unsaved — the user saves with :w or undoes with u)", text.trim()))
}

// 글자·숫자만 소문자로 남기고(합자는 풀어서) 원래 몇 번째 글자였는지 함께 기록한다 — 하이픈,
// 따옴표, 대시, 공백, Markdown 기호처럼 옮겨 적으며 달라지기 쉬운 것은 전부 비교에서 빠진다.
fn normalize(text: impl Iterator<Item = char>) -> Vec<(char, usize)> {
    let mut out = Vec::new();
    for (i, c) in text.enumerate() {
        let lig = match c {
            'ﬀ' => "ff",
            'ﬁ' => "fi",
            'ﬂ' => "fl",
            'ﬃ' => "ffi",
            'ﬄ' => "ffl",
            _ => "",
        };
        let expanded: Vec<char> = if lig.is_empty() { vec![c] } else { lig.chars().collect() };
        out.extend(expanded.into_iter().flat_map(char::to_lowercase).filter(|c| c.is_alphanumeric()).map(|c| (c, i)));
    }
    out
}

// 비교에서 빠진 앞뒤 문장부호(따옴표, 마침표, 괄호)도 단어에 붙어 있으면 같이 칠한다
fn span_range(chars: &[(char, mupdf::Quad)], mut i0: usize, mut i1: usize) -> std::ops::RangeInclusive<usize> {
    let punct = |i: usize| chars.get(i).is_some_and(|(c, _)| !c.is_alphanumeric() && !c.is_whitespace());
    while i0 > 0 && punct(i0 - 1) {
        i0 -= 1;
    }
    while punct(i1 + 1) {
        i1 += 1;
    }
    i0..=i1
}

fn find_all(hay: &[(char, usize)], pat: &[char]) -> Vec<usize> {
    if pat.is_empty() {
        return vec![];
    }
    hay.windows(pat.len()).enumerate().filter(|(_, w)| w.iter().map(|(c, _)| c).eq(pat)).map(|(i, _)| i).collect()
}

// 글자 사각형을 줄 단위로 합친다 — 세로 범위가 겹치고 오른쪽으로 이어지면 같은 줄
fn line_rects(span: &[(char, mupdf::Quad)]) -> Vec<[f32; 4]> {
    let mut rects: Vec<[f32; 4]> = Vec::new();
    for (c, q) in span {
        if c.is_whitespace() {
            continue;
        }
        let r = super::quad_to_rect(q);
        match rects.last_mut() {
            Some(l) if r[1] < l[3] && r[3] > l[1] && r[0] >= l[0] => {
                *l = [l[0].min(r[0]), l[1].min(r[1]), l[2].max(r[2]), l[3].max(r[3])];
            }
            _ => rects.push(r),
        }
    }
    rects
}

// pages 인자 해석 — 생략=현재 페이지(get_notes만 전체), "all"=전체, 나머지는 "1-3,7" 형식 그대로
fn page_spec(args: &Value, view: &Value, default_all: bool) -> String {
    let all = format!("1-{}", view["page_count"].as_u64().unwrap_or(0));
    match args["pages"].as_str() {
        None if default_all => all,
        None => view["page"].to_string(),
        Some("all") => all,
        Some(s) => s.to_string(),
    }
}

fn page_images(path: &str, spec: &str, page_count: usize, scale: f32) -> Result<Vec<Value>, String> {
    use base64::Engine;
    let pages = super::parse_page_spec(spec, page_count)?;
    if pages.len() > MAX_IMAGE_PAGES {
        return Err(format!("At most {MAX_IMAGE_PAGES} pages per call — split the request."));
    }
    let doc = open_doc(path)?;
    let matrix = mupdf::Matrix::new_scale(scale, scale);
    let mut content = Vec::new();
    for i in pages {
        let pixmap = doc
            .load_page(i as i32)
            .and_then(|p| p.to_pixmap(&matrix, &mupdf::Colorspace::device_rgb(), false, true))
            .map_err(|e| e.to_string())?;
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
            .encode(pixmap.samples(), pixmap.width(), pixmap.height(), image::ExtendedColorType::Rgb8)
            .map_err(|e| e.to_string())?;
        content.push(json!({ "type": "text", "text": format!("Page {}", i + 1) }));
        content.push(json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(&jpeg),
            "mimeType": "image/jpeg",
        }));
    }
    Ok(content)
}

fn call_text_tool(name: &str, args: &Value, view: &Value, pages: &str) -> Result<String, String> {
    let pages = pages.to_string();
    let path = view["path"].as_str().ok_or("invalid view state")?;
    let page_count = view["page_count"].as_u64().unwrap_or(0) as usize;
    match name {
        "get_page_text" => {
            let md = to_markdown(path, &pages, page_count)?;
            if md.chars().count() <= MAX_TEXT_CHARS {
                return Ok(md);
            }
            let cut: String = md.chars().take(MAX_TEXT_CHARS).collect();
            Ok(format!("{cut}\n\n[Truncated at {MAX_TEXT_CHARS} characters — request fewer pages to read the rest.]"))
        }
        "get_notes" => notes(path, &pages, page_count),
        "get_outline" => outline(path),
        "search_text" => search_text(path, args["query"].as_str().ok_or("query is required")?, &pages, page_count),
        _ => {
            let out = args["output_path"].as_str().ok_or("output_path is required")?;
            let out = super::resolve_out_path(path, out.to_string());
            if name == "save_pages" {
                super::write_pdf_pages(path, &out, Some(&pages), page_count, None)?;
            } else {
                std::fs::write(&out, to_markdown(path, &pages, page_count)?).map_err(|e| e.to_string())?;
            }
            Ok(format!("Saved page(s) {pages} of {path} to {out}"))
        }
    }
}

fn open_doc(path: &str) -> Result<mupdf::Document, String> {
    let doc = mupdf::Document::open(path).map_err(|e| e.to_string())?;
    if doc.needs_password().map_err(|e| e.to_string())? {
        return Err("Password-protected documents are not supported".to_string());
    }
    Ok(doc)
}

const MAX_SEARCH_HITS: usize = 50; // 흔한 단어는 매치가 수백 개라 앞쪽만 보여주고 총 개수를 알린다
const SEARCH_CONTEXT_CHARS: usize = 60; // 매치 앞뒤로 붙일 문맥 길이(원문 글자 수)

// 매칭은 add_highlight와 같은 normalize/find_all — AI가 옮겨 적은 검색어의 대소문자·하이픈·따옴표
// 차이를 흡수한다. 앱의 / 검색(search_all)은 앱 프로세스 상태에 묶여 있어 여기선 못 쓴다.
fn search_text(path: &str, query: &str, spec: &str, page_count: usize) -> Result<String, String> {
    let pat: Vec<char> = normalize(query.chars()).into_iter().map(|(c, _)| c).collect();
    if pat.is_empty() {
        return Err("query must contain letters or digits".to_string());
    }
    let doc = open_doc(path)?;
    let mut hits = Vec::new();
    let mut total = 0;
    for i in super::parse_page_spec(spec, page_count)? {
        let pg = doc.load_page(i as i32).map_err(|e| e.to_string())?;
        let text: Vec<char> = super::extract_chars(&pg)?.into_iter().map(|(c, _)| c).collect();
        let snippets = page_hits(&text, &pat);
        total += snippets.len();
        hits.extend(snippets.into_iter().map(|s| format!("p.{}: {s}", i + 1)));
    }
    if total == 0 {
        return Ok(format!("No matches for \"{query}\"."));
    }
    let shown = hits.len().min(MAX_SEARCH_HITS);
    let mut out = format!("{total} match(es) for \"{query}\"");
    if total > shown {
        out += &format!(", showing the first {shown} — narrow pages or use a longer query for the rest");
    }
    Ok(out + ":\n" + &hits[..shown].join("\n"))
}

// 한 페이지의 매치마다 "…앞 문맥 **매치** 뒤 문맥…" 한 줄. 줄바꿈·연속 공백은 공백 하나로.
fn page_hits(text: &[char], pat: &[char]) -> Vec<String> {
    let hay = normalize(text.iter().copied());
    find_all(&hay, pat)
        .into_iter()
        .map(|at| {
            let (a, b) = (hay[at].1, hay[at + pat.len() - 1].1 + 1);
            let (pre, post) = (a.saturating_sub(SEARCH_CONTEXT_CHARS), (b + SEARCH_CONTEXT_CHARS).min(text.len()));
            let s = |r: &[char]| r.iter().collect::<String>();
            let snippet = format!("{}**{}**{}", s(&text[pre..a]), s(&text[a..b]), s(&text[b..post]));
            let ell = |cut: bool| if cut { "…" } else { "" };
            format!("{}{}{}", ell(pre > 0), snippet.split_whitespace().collect::<Vec<_>>().join(" "), ell(post < text.len()))
        })
        .collect()
}

// 하이라이트 주석엔 글자 자체가 없고 영역(quad)만 있어서, 페이지 텍스트에서 그 영역 안에 중심이
// 들어오는 글자를 모아 "하이라이트한 문장"을 복원한다. 메모(Text 주석)는 notes_on_page 재사용.
fn notes(path: &str, spec: &str, page_count: usize) -> Result<String, String> {
    use mupdf::pdf::{PdfAnnotationType, PdfDocument, PdfPage};
    let pdf: PdfDocument = open_doc(path)?.try_into().map_err(|_| "Annotations are only supported for PDF files".to_string())?;
    let mut out = String::new();
    for i in super::parse_page_spec(spec, page_count)? {
        let page: PdfPage = pdf.load_page(i as i32).and_then(|p| p.try_into()).map_err(|e| e.to_string())?;
        let mut items = Vec::new();
        let mut text_page = None;
        for a in page.annotations() {
            if !matches!(a.r#type(), Ok(PdfAnnotationType::Highlight)) {
                continue;
            }
            let quads: Vec<[f32; 4]> = a.quad_points().unwrap_or_default().iter().map(super::quad_to_rect).collect();
            if text_page.is_none() {
                text_page = Some(page.to_text_page(mupdf::TextPageFlags::DEHYPHENATE).map_err(|e| e.to_string())?);
            }
            let mut lines = Vec::new();
            for block in text_page.as_ref().unwrap().blocks() {
                for line in block.lines() {
                    let s: String = line
                        .chars()
                        .filter(|c| {
                            let [x0, y0, x1, y1] = super::quad_to_rect(&c.quad());
                            let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                            quads.iter().any(|q| q[0] <= cx && cx <= q[2] && q[1] <= cy && cy <= q[3])
                        })
                        .filter_map(|c| c.char())
                        .collect();
                    if !s.trim().is_empty() {
                        lines.push(s.trim().to_string());
                    }
                }
            }
            let comment = a.contents().ok().flatten().filter(|c| !c.is_empty()).map(|c| format!(" — comment: {c}")).unwrap_or_default();
            items.push(format!("- highlight: \"{}\"{comment}", lines.join(" ")));
        }
        for n in super::notes_on_page(&page) {
            items.push(format!("- note: {}", n.contents));
        }
        if !items.is_empty() {
            out += &format!("## Page {}\n{}\n\n", i + 1, items.join("\n"));
        }
    }
    Ok(if out.is_empty() { "No highlights or notes found.".to_string() } else { out })
}

fn outline(path: &str) -> Result<String, String> {
    fn walk(entries: &[super::TocEntry], depth: usize, out: &mut String) {
        for e in entries {
            let page = e.page.map(|p| format!(" (p. {})", p + 1)).unwrap_or_default();
            *out += &format!("{}- {}{page}\n", "  ".repeat(depth), e.title);
            walk(&e.children, depth + 1, out);
        }
    }
    let toc: Vec<super::TocEntry> = open_doc(path)?.outlines().map_err(|e| e.to_string())?.into_iter().map(super::convert_outline).collect();
    let mut out = String::new();
    walk(&toc, 0, &mut out);
    Ok(if out.is_empty() { "This document has no outline.".to_string() } else { out })
}

fn pages_schema() -> Value {
    json!({ "type": "string", "description": "1-based page spec like \"3\" or \"1-3,7\", or \"all\". Defaults to the page the user is currently viewing." })
}

// MuPDF에 마크다운 출력이 없어서 텍스트 블록(문단)을 직접 순회한다 — 본문보다 글자가 큰 짧은 블록을
// 제목으로 보고, 큰 크기부터 #, ##, ### 순서로 수준을 매긴다.
// ponytail: 글자 크기만 본다 — 본문 크기의 굵은 제목, 목록, 표는 평문 문단으로 나온다. 필요하면
// TextCharFlags(BOLD)와 줄 머리 기호(•, 1.)로 확장
fn to_markdown(path: &str, spec: &str, page_count: usize) -> Result<String, String> {
    let doc = open_doc(path)?;
    let mut pages = Vec::new();
    for i in super::parse_page_spec(spec, page_count)? {
        let page = doc.load_page(i as i32).map_err(|e| e.to_string())?;
        let text = page.to_text_page(mupdf::TextPageFlags::DEHYPHENATE).map_err(|e| e.to_string())?;
        let mut blocks = Vec::new();
        for block in text.blocks() {
            let (mut sizes, mut lines) = (Vec::new(), Vec::new());
            for line in block.lines() {
                let s: String = line.chars().filter_map(|c| { sizes.push(c.size()); c.char() }).collect();
                lines.push(s.trim().to_string());
            }
            let text = lines.join(" ").trim().to_string();
            if !text.is_empty() {
                blocks.push(Block { size: sizes.iter().sum::<f32>() / sizes.len() as f32, chars: sizes.len(), text });
            }
        }
        pages.push((i + 1, blocks));
    }
    Ok(blocks_to_markdown(&pages))
}

struct Block {
    size: f32,   // 블록 글자 크기 평균(pt)
    chars: usize,
    text: String,
}

const HEADING_MIN_RATIO: f32 = 1.15; // 본문보다 이만큼 커야 제목
const HEADING_MAX_CHARS: usize = 120; // 이보다 긴 블록은 글자가 커도 문단(슬라이드 본문 등)

fn blocks_to_markdown(pages: &[(usize, Vec<Block>)]) -> String {
    let round = |s: f32| (s * 2.0).round() / 2.0; // 0.5pt 단위로 묶어 미세한 차이를 무시
    // 본문 크기 = 글자 수 기준으로 가장 많이 쓰인 크기
    let mut weight: Vec<(f32, usize)> = Vec::new();
    for b in pages.iter().flat_map(|(_, bs)| bs) {
        match weight.iter_mut().find(|(s, _)| *s == round(b.size)) {
            Some(w) => w.1 += b.chars,
            None => weight.push((round(b.size), b.chars)),
        }
    }
    let body = weight.iter().max_by_key(|w| w.1).map_or(0.0, |w| w.0);
    let is_heading = |b: &Block| b.size >= body * HEADING_MIN_RATIO && b.text.chars().count() <= HEADING_MAX_CHARS;
    let mut levels: Vec<f32> = pages.iter().flat_map(|(_, bs)| bs).filter(|b| is_heading(b)).map(|b| round(b.size)).collect();
    levels.sort_by(|a, b| b.total_cmp(a));
    levels.dedup();

    let mut md = String::new();
    for (num, blocks) in pages {
        md += &format!("<!-- page {num} -->\n\n");
        for b in blocks {
            if is_heading(b) {
                let level = levels.iter().position(|&s| s == round(b.size)).unwrap_or(0).min(2) + 1;
                md += &format!("{} {}\n\n", "#".repeat(level), b.text);
            } else {
                md += &format!("{}\n\n", b.text);
            }
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_tools_and_reports_unknown_method() {
        let tools = handle("tools/list", &Value::Null).unwrap();
        assert_eq!(tools["tools"].as_array().unwrap().len(), 11);
        assert!(handle("nope", &Value::Null).is_err());
        let r = handle("tools/call", &json!({ "name": "nope", "arguments": {} })).unwrap();
        assert_eq!(r["isError"], true);
    }

    #[test]
    fn normalized_match_ignores_hyphens_quotes_and_markdown() {
        let page = "the \u{201c}atten-\ntion\u{201d} mechanism is e\u{fb03}cient";
        let hay = normalize(page.chars());
        let pat = |s: &str| normalize(s.chars()).into_iter().map(|(c, _)| c).collect::<Vec<_>>();
        let hits = find_all(&hay, &pat("**\"Attention\" Mechanism"));
        assert_eq!(hits.len(), 1);
        assert_eq!(hay[hits[0]].1, page.chars().position(|c| c == 'a').unwrap()); // 원래 글자 위치로 되짚기
        assert_eq!(find_all(&hay, &pat("efficient")).len(), 1);
        assert!(find_all(&hay, &pat("attention model")).is_empty());
    }

    #[test]
    fn search_hits_show_match_in_context() {
        let text: Vec<char> = "We apply Drop-\nout here. Later, dropout\n  again.".chars().collect();
        let pat: Vec<char> = "dropout".chars().collect();
        assert_eq!(page_hits(&text, &pat), vec!["We apply **Drop- out** here. Later, dropout again.", "We apply Drop- out here. Later, **dropout** again."]);
        assert!(page_hits(&text, &"missing".chars().collect::<Vec<_>>()).is_empty());
        let long: Vec<char> = format!("{}dropout{}", "a".repeat(100), "b".repeat(100)).chars().collect();
        let hit = &page_hits(&long, &pat)[0];
        assert!(hit.starts_with('…') && hit.ends_with('…') && hit.contains("a**dropout**b"));
        let pdf = concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/synctex-demo.pdf");
        let out = search_text(pdf, "line 8", "1", 1).unwrap();
        assert!(out.starts_with("1 match(es)") && out.contains("p.1: ") && out.contains("**line 8**"), "{out}");
        assert!(search_text(pdf, "nowhere-to-be-found", "1", 1).unwrap().starts_with("No matches"));
    }

    #[test]
    fn passage_across_lines_becomes_one_rect_per_line() {
        let doc = open_doc(concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/synctex-demo.pdf")).unwrap();
        let chars = crate::extract_chars(&doc.load_page(0).unwrap()).unwrap();
        let hay = normalize(chars.iter().map(|(c, _)| *c));
        let pat = |s: &str| normalize(s.chars()).into_iter().map(|(c, _)| c).collect::<Vec<_>>();
        let (s, e) = (pat("This is line 8"), pat("this exact line."));
        let a = find_all(&hay, &s)[0];
        let b = a + find_all(&hay[a..], &e)[0] + e.len();
        let span = &chars[span_range(&chars, hay[a].1, hay[b - 1].1)];
        let text: String = span.iter().map(|(c, _)| *c).collect();
        assert_eq!(text.split_whitespace().collect::<Vec<_>>().join(" "), "This is line 8 of the source. Ctrl(or Cmd)+click on this sentence in the rendered PDF and it should jump back to this exact line.");
        assert_eq!(line_rects(span).len(), 2);
    }

    #[test]
    fn larger_short_blocks_become_headings() {
        let b = |size, text: &str| Block { size, chars: text.len(), text: text.to_string() };
        let pages = vec![(1, vec![
            b(20.0, "Title"),
            b(14.0, "Section"),
            b(10.0, &format!("Body text{}", " filler".repeat(60))), // 글자 수가 가장 많은 크기 = 본문
            b(14.2, "Another section"),
            b(14.0, &"x".repeat(200)), // 글자는 크지만 너무 길어서 문단
        ])];
        let md = blocks_to_markdown(&pages);
        assert!(md.contains("<!-- page 1 -->"));
        assert!(md.contains("\n# Title\n"));
        assert!(md.contains("\n## Section\n"));
        assert!(md.contains("\n## Another section\n"));
        assert!(md.contains("\nBody text"));
        assert!(!md.contains("# xxx"));
    }
}
