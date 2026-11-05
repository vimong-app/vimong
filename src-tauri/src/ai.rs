// 앱 안 AI 패널 — 로컬 Ollama에 "지금 보고 있는 문서" 맥락을 붙여 묻는다. 맥락은 MCP 도구
// (mcp::call_tool)를 그대로 써서, 외부 AI가 vimong --mcp로 보는 것과 같은 텍스트/이미지를 넘긴다.
use serde_json::{json, Value};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::ipc::Channel;

// 문서 전체를 넘길 때 자르는 길이와 그때 늘려 잡는 컨텍스트 창 — Ollama 기본(4096 토큰)으로는
// 몇 페이지만 들어가고 나머지는 Ollama가 조용히 버린다.
// ponytail: 고정 상한, 긴 문서는 앞부분만 본다 — 필요해지면 검색(search_text)으로 관련 페이지만 골라 넣기
const DOC_CHARS: usize = 40_000;
const DOC_NUM_CTX: u32 = 16_384;

static STOP: AtomicBool = AtomicBool::new(false);

// ponytail: 주소는 OLLAMA_HOST(없으면 로컬 기본 포트)만 본다 — 원격/https 서버가 필요해지면 설정으로 뺀다
fn base_url() -> String {
    match std::env::var("OLLAMA_HOST") {
        Ok(h) if h.starts_with("http") => h,
        Ok(h) if !h.is_empty() => format!("http://{h}"),
        _ => "http://127.0.0.1:11434".into(),
    }
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => r
            .into_json::<Value>()
            .ok()
            .and_then(|v| v["error"].as_str().map(String::from))
            .unwrap_or_else(|| format!("Ollama returned HTTP {code}")),
        e => format!("Cannot reach Ollama at {} — is `ollama serve` running? ({e})", base_url()),
    }
}

#[tauri::command]
pub async fn ai_models() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let tags: Value = ureq::get(&format!("{}/api/tags", base_url()))
            .call()
            .map_err(http_err)?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok(tags["models"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str().map(String::from)).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn ai_stop() {
    STOP.store(true, Ordering::Relaxed);
}

// context: "page" | "doc"(지금 파일 전체) | "image" — 선택한 텍스트가 있으면 어느 경우든 같이 넘긴다.
// 맥락은 대화 기록에 넣지 않고 매 질문마다 그 순간의 화면 기준으로 system 메시지에 새로 붙인다
// (사용자가 대화 중에 페이지를 넘겨도 "이 페이지"가 지금 페이지를 가리키게).
#[tauri::command]
// on_event: {"notice": 코드}(맥락을 못 붙였을 때, 답보다 먼저 한 번) 또는 {"delta": 답 조각}
pub async fn ai_chat(model: String, mut messages: Vec<Value>, context: String, on_event: Channel<Value>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        STOP.store(false, Ordering::Relaxed);
        let (system, images, notice) = build_context(&context);
        if let Some(code) = notice {
            on_event.send(json!({ "notice": code })).map_err(|e| e.to_string())?;
        }
        if !images.is_empty() {
            if let Some(last) = messages.last_mut() {
                last["images"] = json!(images);
            }
        }
        messages.insert(0, json!({ "role": "system", "content": system }));
        let mut body = json!({ "model": model, "messages": messages, "stream": true });
        if context == "doc" {
            body["options"] = json!({ "num_ctx": DOC_NUM_CTX });
        }
        let resp = ureq::post(&format!("{}/api/chat", base_url())).send_json(body).map_err(http_err)?;
        // 스트림은 줄마다 JSON 하나(NDJSON)
        for line in std::io::BufReader::new(resp.into_reader()).lines() {
            if STOP.load(Ordering::Relaxed) {
                break; // 응답 리더를 버리면 연결이 끊겨 Ollama도 생성을 멈춘다
            }
            let v: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if let Some(err) = v["error"].as_str() {
                return Err(err.to_string());
            }
            if let Some(s) = v["message"]["content"].as_str().filter(|s| !s.is_empty()) {
                on_event.send(json!({ "delta": s })).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

// 세 번째 값은 맥락을 못 붙였을 때 패널에 띄울 이유 — 코드만 넘기고 문구는 프론트(i18n 'ai.notice_*')가 고른다.
// "password": 암호 문서(MCP의 open_doc은 앱에서 푼 암호를 모른다), "unsupported": 못 여는 형식,
// "no_text": 열었지만 텍스트가 없음(스캔/이미지)
fn build_context(context: &str) -> (String, Vec<String>, Option<&'static str>) {
    let mut system = String::from(
        "You are an assistant built into Vimong, a document viewer. Answer in the language the user writes in. \
         Base your answers on the document content below when it is relevant, and say so when the answer is not in it. \
         Use Markdown.",
    );
    let mut images = Vec::new();
    // 패널은 문서가 열려 있을 때만 뜨니 기록이 없다는 건 hwp처럼 set_current_view가 경로를 비워 보낸 형식이다
    let Ok(view) = super::mcp::read_view() else { return (system, images, Some("unsupported")) };
    let path = view["path"].as_str().unwrap_or("");
    let page = view["page"].as_u64().unwrap_or(1);
    let file = std::path::Path::new(path);
    // 마크다운은 MuPDF로 안 열고 앱이 원문을 직접 그린다 — 페이지 구분이 없으니 범위와 상관없이 원문 전체
    let is_md = file.extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    let text_of = |args: Value| -> Result<String, String> {
        if is_md {
            return std::fs::read_to_string(path).map_err(|e| e.to_string());
        }
        let content = super::mcp::call_tool("get_page_text", &args)?;
        Ok(content.first().and_then(|v| v["text"].as_str()).unwrap_or("").to_string())
    };
    let (label, text) = if is_md || context == "doc" {
        ("the whole file", text_of(json!({ "pages": "all" })).map(|t| t.chars().take(DOC_CHARS).collect::<String>()))
    } else {
        ("the page the user is viewing", text_of(json!({})))
    };
    if let Some(name) = file.file_name() {
        system += &format!("\n\nThe user is viewing \"{}\", page {page}.", name.to_string_lossy());
    }
    let mut notice = None;
    match &text {
        Err(e) => notice = Some(if e == super::mcp::PASSWORD_ERR { "password" } else { "unsupported" }),
        Ok(t) if t.trim().is_empty() => {
            if context != "image" {
                notice = Some("no_text"); // 이미지 모드면 페이지 그림이 대신 간다
            }
        }
        Ok(t) => system += &format!("\n\n<document scope=\"{label}\">\n{t}\n</document>"),
    }
    if let Some(sel) = view["selection"].as_str().filter(|s| !s.trim().is_empty()) {
        system += &format!("\n\nThe user selected this passage; \"this\" usually refers to it:\n<selection>\n{sel}\n</selection>");
    }
    if context == "image" && text.is_ok() && !is_md {
        let content = super::mcp::call_tool("get_page_image", &json!({})).unwrap_or_default();
        images.extend(content.iter().filter_map(|c| c["data"].as_str().map(String::from)));
    }
    (system, images, notice)
}
