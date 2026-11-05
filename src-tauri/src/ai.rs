// 앱 안 AI 패널 — 로컬 Ollama에 "지금 보고 있는 문서" 맥락을 붙여 묻는다. 맥락은 MCP 도구
// (mcp::call_tool)를 그대로 써서, 외부 AI가 vimong --mcp로 보는 것과 같은 텍스트/이미지를 넘긴다.
// 도구 호출을 지원하는 모델이면 같은 MCP 도구(읽기 전용만)를 쥐여 줘서, 지금 페이지 밖의 내용은
// 모델이 목차/검색으로 찾아 필요한 페이지만 읽게 한다 — 파일 크기와 상관없이 전체를 다룰 수 있다.
use serde_json::{json, Value};
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::ipc::Channel;

// 문서 전체를 넘길 때 자르는 길이와 그때 늘려 잡는 컨텍스트 창 — Ollama 기본(4096 토큰)으로는
// 몇 페이지만 들어가고 나머지는 Ollama가 조용히 버린다. 도구를 못 쓰는 모델만 이 방식(앞부분만 본다)
const DOC_CHARS: usize = 40_000;
const DOC_NUM_CTX: u32 = 16_384; // 도구 결과가 쌓이는 도구 모드도 같은 크기

// 로컬 모델에게 주는 MCP 도구 — 읽기 전용만. 하이라이트/페이지 이동/저장은 사용자가 시킨 적 없는 변경이 될 수 있다
const TOOLS: [&str; 4] = ["get_outline", "search_text", "get_page_text", "get_notes"];
const TOOL_CHARS: usize = 12_000; // 도구 결과 하나의 상한 — 여러 페이지를 한꺼번에 읽어 컨텍스트 창을 넘기지 않게
const MAX_TOOL_ROUNDS: usize = 5; // 이만큼 도구를 부르고도 답을 안 하면 마지막엔 도구 없이 답하게 한다

// "현재 파일 전체"에서 질문이 장/절/페이지 범위를 가리키면 그 범위를 앱이 직접 끝까지 읽는다(read_scope) —
// 작은 모델은 도구가 있어도 안 부르고 "도구로 확인할 수 있다"고만 답하곤 해서, 무엇을 읽을지를 모델 판단에 맡기지 않는다.
// 한 번에 넘기는 글자 수 — 한글은 글자당 토큰이 많아 DOC_NUM_CTX(16k 토큰) 안에 질문·답까지 들어가게 잡는다
const DIRECT_CHARS: usize = 12_000; // 범위가 이보다 짧으면 통째로, 길면 나눠 읽고 요점만 모은다
const CHUNK_CHARS: usize = 8_000; // 나눠 읽을 때 한 번에 읽히는 분량
const OUTLINE_CHARS: usize = 8_000; // 범위를 고를 때 보여주는 목차 분량 — 넘치면 상위 두 단계만

static STOP: AtomicBool = AtomicBool::new(false);

// 설정 > AI의 서버 — provider: "ollama"(기본) | "openai"(OpenAI 호환: LM Studio, llama.cpp, MLX-LM, Jan 등).
// 대화/도구/범위 읽기는 안에서 Ollama 형식으로 다루고, OpenAI 호환이면 주고받는 지점(stream_reply)에서만 바꾼다.
#[derive(Clone, Default, serde::Deserialize)]
pub struct Server {
    #[serde(default)]
    provider: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    key: String,
}

impl Server {
    fn openai(&self) -> bool {
        self.provider == "openai"
    }

    // 비어 있으면 Ollama는 OLLAMA_HOST, 그것도 없으면 각자 기본 포트(Ollama 11434, LM Studio 1234).
    // OpenAI 호환 주소는 .../v1까지 적어도 되게 뗀다(경로는 여기서 붙인다). ponytail: ureq를 TLS 없이 넣어서 http만 된다
    fn base_url(&self) -> String {
        let env = if self.openai() { None } else { std::env::var("OLLAMA_HOST").ok() };
        let h = Some(self.host.trim()).filter(|h| !h.is_empty()).map(str::to_string).or(env);
        let h = h.map(|h| h.trim().trim_end_matches('/').trim_end_matches("/v1").to_string());
        match h {
            Some(h) if h.starts_with("http") => h,
            Some(h) if !h.is_empty() => format!("http://{h}"),
            _ if self.openai() => "http://127.0.0.1:1234".into(),
            _ => "http://127.0.0.1:11434".into(),
        }
    }

    fn post(&self, path: &str) -> ureq::Request {
        self.auth(ureq::post(&format!("{}{path}", self.base_url())))
    }

    fn get(&self, path: &str) -> ureq::Request {
        self.auth(ureq::get(&format!("{}{path}", self.base_url())))
    }

    fn auth(&self, r: ureq::Request) -> ureq::Request {
        if self.key.trim().is_empty() { r } else { r.set("Authorization", &format!("Bearer {}", self.key.trim())) }
    }

    fn http_err(&self, e: ureq::Error) -> String {
        match e {
            // Ollama는 {"error": "..."}, OpenAI 호환은 {"error": {"message": "..."}}
            ureq::Error::Status(code, r) => r
                .into_json::<Value>()
                .ok()
                .and_then(|v| v["error"].as_str().or(v["error"]["message"].as_str()).map(String::from))
                .unwrap_or_else(|| format!("The AI server returned HTTP {code}")),
            e if self.openai() => format!("Cannot reach the AI server at {} — is its local server running? ({e})", self.base_url()),
            e => format!("Cannot reach Ollama at {} — is `ollama serve` running? ({e})", self.base_url()),
        }
    }
}

static SERVER: std::sync::Mutex<Server> = std::sync::Mutex::new(Server { provider: String::new(), host: String::new(), key: String::new() });

#[tauri::command]
pub fn ai_set_server(server: Server) {
    *SERVER.lock().unwrap() = server;
}

// 지금 적용된 서버 — 설정 창에서 아직 적용 전인 서버로 목록/받기를 해 볼 때는 그 서버를 직접 넘긴다
fn current() -> Server {
    SERVER.lock().unwrap().clone()
}

#[tauri::command]
pub async fn ai_models(server: Option<Server>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = server.unwrap_or_else(current);
        let list: Value = s.get(if s.openai() { "/v1/models" } else { "/api/tags" })
            .call()
            .map_err(|e| s.http_err(e))?
            .into_json()
            .map_err(|e| e.to_string())?;
        let (items, key) = if s.openai() { (&list["data"], "id") } else { (&list["models"], "name") };
        Ok(items.as_array().into_iter().flatten().filter_map(|m| m[key].as_str().map(String::from)).collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

// 모델 받기/업데이트(ollama pull) — 같은 이름이면 바뀐 레이어만 받는다. 진행은 {status, completed, total}로.
// OpenAI 호환 서버에는 이런 API가 없어서 각 서버 앱(LM Studio 등)에서 받는다
#[tauri::command]
pub async fn ai_pull(model: String, server: Option<Server>, on_event: Channel<Value>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = server.unwrap_or_else(current);
        let resp = s.post("/api/pull")
            .send_json(json!({ "model": model.trim(), "stream": true }))
            .map_err(|e| s.http_err(e))?;
        for line in std::io::BufReader::new(resp.into_reader()).lines() {
            let v: Value = serde_json::from_str(&line.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            if let Some(err) = v["error"].as_str() {
                return Err(err.to_string());
            }
            on_event.send(json!({ "status": v["status"], "completed": v["completed"], "total": v["total"] })).map_err(|e| e.to_string())?;
        }
        Ok(())
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
// on_event: {"notice": 코드}(맥락을 못 붙였을 때, 답보다 먼저 한 번), {"tool": 이름, "args": 인자}(모델이 도구를
// 부를 때마다), {"delta": 답 조각}
#[tauri::command]
// pages/title: 목차 우클릭 "AI에게 묻기"로 정한 범위 — 있으면 맥락 옵션과 상관없이 그 범위를 끝까지 읽는다
pub async fn ai_chat(
    model: String,
    mut messages: Vec<Value>,
    context: String,
    pages: Option<String>,
    title: Option<String>,
    on_event: Channel<Value>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        STOP.store(false, Ordering::Relaxed);
        let send = |v: Value| on_event.send(v).map_err(|e| e.to_string());
        let question = messages.last().and_then(|m| m["content"].as_str()).unwrap_or("").to_string();
        let scope = match pages {
            Some(spec) => read_pages(&model, &question, &title.unwrap_or_default(), &spec, &send)?,
            None if context == "doc" => read_scope(&model, &question, &send)?,
            None => None,
        };
        let (system, images, notice, mut use_tools) = build_context(&context, scope.is_none() && supports_tools(&model), scope);
        if let Some(code) = notice {
            send(json!({ "notice": code }))?;
        }
        if !images.is_empty() {
            if let Some(last) = messages.last_mut() {
                last["images"] = json!(images);
            }
            // 답을 마크다운으로 저장할 때(ai_save_markdown) 같은 페이지 그림을 함께 넣으려고 알려 둔다
            let page = super::mcp::read_view().ok().and_then(|v| v["page"].as_u64()).unwrap_or(1);
            send(json!({ "image_pages": [page] }))?;
        }
        messages.insert(0, json!({ "role": "system", "content": system }));
        let tools = if use_tools { tool_defs() } else { json!([]) };
        for round in 0..=MAX_TOOL_ROUNDS {
            let mut body = json!({ "model": model, "messages": messages, "stream": true });
            if use_tools || context == "doc" {
                body["options"] = json!({ "num_ctx": DOC_NUM_CTX });
            }
            if use_tools && round < MAX_TOOL_ROUNDS {
                body["tools"] = tools.clone();
            }
            let (content, calls) = match stream_reply(body.clone(), &send) {
                // OpenAI 호환은 모델이 도구를 지원하는지 미리 알 길이 없어 일단 붙여 본다 — 거절하면 도구 없이 다시
                Err(_) if use_tools && round == 0 && current().openai() => {
                    use_tools = false;
                    body.as_object_mut().map(|b| b.remove("tools"));
                    stream_reply(body, &send)?
                }
                r => r?,
            };
            if calls.is_empty() || STOP.load(Ordering::Relaxed) {
                break;
            }
            messages.push(json!({ "role": "assistant", "content": content, "tool_calls": calls }));
            for call in &calls {
                let name = call["function"]["name"].as_str().unwrap_or("");
                let args = normalize_args(&call["function"]["arguments"]);
                send(json!({ "tool": name, "args": args }))?;
                // tool_call_id는 OpenAI 호환용(Ollama는 이름으로 짝짓고 모르는 필드는 무시한다)
                messages.push(json!({ "role": "tool", "tool_name": name, "tool_call_id": call["id"], "content": run_tool(name, &args) }));
            }
            if !content.is_empty() {
                send(json!({ "delta": "\n\n" }))?; // 도구를 부르기 전에 한 말과 이어질 답을 문단으로 나눈다
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

// AI 답을 마크다운 파일로 — image_pages(답을 만들 때 모델에게 보낸 페이지 그림)는 같은 이름 옆에 JPEG로 저장하고
// 맨 위에 링크로 넣는다. 그림은 MCP get_page_image와 같은 렌더링이라 모델이 본 그대로다.
// ponytail: 그 사이 다른 문서로 옮겼으면 EPUB은 기본 레이아웃으로 열려 페이지가 어긋날 수 있다(mcp::open_doc 참고)
#[tauri::command]
pub async fn ai_save_markdown(path: String, markdown: String, doc_path: String, image_pages: Vec<u32>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || write_markdown(&path, &markdown, &doc_path, &image_pages))
        .await
        .map_err(|e| e.to_string())?
}

fn write_markdown(path: &str, markdown: &str, doc_path: &str, image_pages: &[u32]) -> Result<(), String> {
    use base64::Engine;
    let out = std::path::Path::new(path);
    let stem = out.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "ai".into());
    let dir = out.parent().unwrap_or(std::path::Path::new("."));
    let mut md = String::new();
    if !image_pages.is_empty() {
        let page_count = super::mcp::open_doc(doc_path)?.page_count().map_err(|e| e.to_string())? as usize;
        for p in image_pages {
            let content = super::mcp::page_images(doc_path, &p.to_string(), page_count, 1.5)?;
            let data = content.iter().find_map(|c| c["data"].as_str()).ok_or("could not render the page image")?;
            let name = format!("{stem}-p{p}.jpg");
            let bytes = base64::engine::general_purpose::STANDARD.decode(data).map_err(|e| e.to_string())?;
            std::fs::write(dir.join(&name), bytes).map_err(|e| e.to_string())?;
            md += &format!("![p. {p}](<{name}>)\n\n"); // <>로 감싸야 파일 이름에 빈칸이 있어도 링크가 안 끊긴다
        }
    }
    md += unwrap_markdown_fence(markdown);
    md += "\n";
    std::fs::write(out, md).map_err(|e| e.to_string())
}

// "마크다운으로 저장해줘"에 모델은 흔히 문서 전체를 ```markdown … ``` 으로 감싸 보여준다 — 그대로 저장하면 펜스까지
// 파일에 들어가니, markdown/md 펜스가 하나 있으면 그 안만 꺼낸다. 다른 언어 코드 블록은 문서의 일부라 그대로 둔다.
// 모델이 안쪽 코드 블록도 같은 ``` 로 감싸는 일이 잦아서(CommonMark로는 거기서 닫힌다) 마지막 닫는 펜스까지를 안으로 본다.
fn unwrap_markdown_fence(text: &str) -> &str {
    let lines: Vec<(usize, &str)> = text.split_inclusive('\n').scan(0, |pos, l| { let start = *pos; *pos += l.len(); Some((start, l)) }).collect();
    let fence = |l: &str| {
        let t = l.trim();
        let marker = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let n = t.chars().take_while(|c| *c == marker).count();
        (n >= 3).then(|| (marker, n, t[n..].trim().to_ascii_lowercase()))
    };
    let opens: Vec<usize> = lines.iter().enumerate().filter(|(_, (_, l))| fence(l).is_some_and(|(_, _, lang)| lang == "markdown" || lang == "md")).map(|(i, _)| i).collect();
    let [open] = opens[..] else { return text };
    let (marker, n, _) = fence(lines[open].1).unwrap();
    let body_start = lines.get(open + 1).map_or(text.len(), |l| l.0);
    let close = lines[open + 1..].iter().rposition(|(_, l)| fence(l).is_some_and(|(m, k, lang)| m == marker && k >= n && lang.is_empty()));
    match close {
        Some(c) => &text[body_start..lines[open + 1 + c].0],
        None => &text[body_start..], // 답이 중간에 끊겨 닫는 펜스가 없을 때
    }
    .trim_end()
}

// 한 번의 채팅 스트림 — 답 조각은 바로 패널로 흘려보내고, 모은 본문과 도구 호출(Ollama 형식)을 돌려준다.
// body는 Ollama /api/chat 형식이고, OpenAI 호환 서버면 여기서 바꿔 보낸다
fn stream_reply(body: Value, send: &dyn Fn(Value) -> Result<(), String>) -> Result<(String, Vec<Value>), String> {
    let s = current();
    if s.openai() {
        let resp = s.post("/v1/chat/completions").send_json(to_openai_body(&body)).map_err(|e| s.http_err(e))?;
        let lines = std::io::BufReader::new(resp.into_reader()).lines().map(|l| l.map_err(|e| e.to_string()));
        return read_openai_stream(lines, send);
    }
    let resp = s.post("/api/chat").send_json(body).map_err(|e| s.http_err(e))?;
    let (mut content, mut calls) = (String::new(), Vec::new());
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
            content += s;
            send(json!({ "delta": s }))?;
        }
        calls.extend(v["message"]["tool_calls"].as_array().into_iter().flatten().cloned());
    }
    Ok((content, calls))
}

// Ollama /api/chat 요청을 OpenAI /v1/chat/completions 요청으로 — num_ctx는 OpenAI API에 없어서 서버 앱 설정을 따른다
fn to_openai_body(body: &Value) -> Value {
    let messages: Vec<Value> = body["messages"].as_array().into_iter().flatten().map(to_openai_message).collect();
    let mut out = json!({ "model": body["model"], "messages": messages, "stream": true });
    if let Some(tools) = body.get("tools") {
        out["tools"] = tools.clone(); // tool_defs()가 이미 OpenAI 함수 형식
    }
    if let Some(schema) = body.get("format") {
        out["response_format"] = json!({ "type": "json_schema", "json_schema": { "name": "reply", "schema": schema } });
    }
    out
}

fn to_openai_message(m: &Value) -> Value {
    let content = m["content"].as_str().unwrap_or("");
    match m["role"].as_str() {
        Some("tool") => json!({ "role": "tool", "tool_call_id": m["tool_call_id"], "content": content }),
        Some("assistant") if m["tool_calls"].is_array() => json!({
            "role": "assistant",
            "content": content,
            "tool_calls": m["tool_calls"].as_array().into_iter().flatten().map(|c| json!({
                "id": c["id"], "type": "function",
                // OpenAI는 인자를 JSON 문자열로 주고받는다
                "function": { "name": c["function"]["name"], "arguments": c["function"]["arguments"].to_string() },
            })).collect::<Vec<_>>(),
        }),
        _ => match m["images"].as_array() {
            Some(images) if !images.is_empty() => {
                let mut parts = vec![json!({ "type": "text", "text": content })];
                parts.extend(images.iter().filter_map(Value::as_str).map(|b64| json!({ "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{b64}") } })));
                json!({ "role": m["role"], "content": parts })
            }
            _ => json!({ "role": m["role"], "content": content }),
        },
    }
}

// OpenAI 스트림(SSE: "data: {...}" 줄, 끝은 "data: [DONE]") — 도구 호출은 조각(index별 id/이름/인자 문자열)으로 와서
// 모았다가 끝에 Ollama 형식({id, function: {name, arguments: 객체}})으로 돌려준다
fn read_openai_stream(
    lines: impl Iterator<Item = Result<String, String>>,
    send: &dyn Fn(Value) -> Result<(), String>,
) -> Result<(String, Vec<Value>), String> {
    let mut content = String::new();
    let mut calls: Vec<(String, String, String)> = Vec::new(); // (id, name, arguments)
    for line in lines {
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        let line = line?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data == "[DONE]" {
            break;
        }
        let v: Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
        if let Some(err) = v["error"]["message"].as_str().or(v["error"].as_str()) {
            return Err(err.to_string());
        }
        let delta = &v["choices"][0]["delta"];
        if let Some(s) = delta["content"].as_str().filter(|s| !s.is_empty()) {
            content += s;
            send(json!({ "delta": s }))?;
        }
        for c in delta["tool_calls"].as_array().into_iter().flatten() {
            let i = c["index"].as_u64().unwrap_or(calls.len() as u64) as usize;
            if calls.len() <= i {
                calls.resize(i + 1, Default::default());
            }
            let call = &mut calls[i];
            if let Some(id) = c["id"].as_str() {
                call.0 = id.to_string();
            }
            call.1 += c["function"]["name"].as_str().unwrap_or("");
            call.2 += c["function"]["arguments"].as_str().unwrap_or("");
        }
    }
    let calls = calls
        .into_iter()
        .filter(|c| !c.1.is_empty())
        .map(|(id, name, args)| json!({ "id": id, "function": { "name": name, "arguments": serde_json::from_str::<Value>(&args).unwrap_or(json!({})) } }))
        .collect();
    Ok((content, calls))
}

// 화면에 흘려보내지 않는 한 번의 응답(범위 고르기, 나눠 읽기) — 스트림으로 받아야 Esc(STOP)가 바로 먹힌다
fn complete(model: &str, system: &str, user: &str, format: Option<Value>) -> Result<String, String> {
    let mut body = json!({
        "model": model, "stream": true, "options": { "num_ctx": DOC_NUM_CTX },
        "messages": [{ "role": "system", "content": system }, { "role": "user", "content": user }],
    });
    if let Some(f) = format {
        body["format"] = f;
    }
    Ok(stream_reply(body, &|_| Ok(()))?.0)
}

// 목차를 (깊이, 제목, 0부터 센 페이지)로 펼친다 — 페이지가 없는 항목(목적지 없는 책갈피)은 범위를 못 정하니 뺀다
fn flatten_outline(entries: &[super::TocEntry], depth: usize, out: &mut Vec<(usize, String, u32)>) {
    for e in entries {
        if let Some(p) = e.page {
            out.push((depth, e.title.trim().to_string(), p));
        }
        flatten_outline(&e.children, depth + 1, out);
    }
}

// 항목 i의 페이지 범위(1부터, 양끝 포함) — 다음에 나오는 같은 단계 이상 항목의 직전 페이지까지
fn entry_range(flat: &[(usize, String, u32)], i: usize, page_count: u32) -> (u32, u32) {
    let (depth, _, start) = flat[i];
    let end = flat[i + 1..].iter().find(|(d, _, p)| *d <= depth && *p > start).map_or(page_count, |(_, _, p)| *p);
    (start + 1, end.max(start + 1))
}

// 페이지별 Markdown을 max 글자 이하 묶음으로 — 한 페이지가 max보다 길면 그 페이지만으로 한 묶음
fn chunk_pages(pages: &[(u32, String)], max: usize) -> Vec<(u32, u32, String)> {
    let mut chunks: Vec<(u32, u32, String)> = Vec::new();
    for (n, text) in pages {
        match chunks.last_mut() {
            Some((_, end, buf)) if buf.chars().count() + text.chars().count() <= max => {
                *end = *n;
                buf.push_str(text);
            }
            _ => chunks.push((*n, *n, text.clone())),
        }
    }
    chunks
}

// to_markdown 결과를 <!-- page N --> 표시로 잘라 페이지별로 — 제목 판별(본문 글자 크기)이 범위 전체 기준이 되게 한 번에 변환한다
fn split_pages(md: &str) -> Vec<(u32, String)> {
    md.split("<!-- page ")
        .filter_map(|part| {
            let (num, rest) = part.split_once(" -->")?;
            Some((num.trim().parse().ok()?, format!("<!-- page {num} -->{rest}")))
        })
        .collect()
}

// 질문이 가리키는 범위를 모델에게 고르게 하고(번호/페이지만 답하는 짧은 JSON이라 작은 모델도 잘한다), 그 범위를
// read_pages로 앱이 직접 읽어 맥락 블록으로 돌려준다. 특정 사실을 묻는 질문(범위 없음)이면 None — 기존처럼 도구/앞부분으로.
// 진행 상황은 {"tool": "scope"|"read"|"merge"} 이벤트로 답 위에 보인다.
fn read_scope(model: &str, question: &str, send: &dyn Fn(Value) -> Result<(), String>) -> Result<Option<String>, String> {
    let Ok(view) = super::mcp::read_view() else { return Ok(None) };
    let path = view["path"].as_str().unwrap_or("").to_string();
    let page_count = view["page_count"].as_u64().unwrap_or(0) as u32;
    let current = view["page"].as_u64().unwrap_or(1);
    let is_md = std::path::Path::new(&path).extension().is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
    if is_md || page_count == 0 {
        return Ok(None); // 마크다운은 원문 전체를 그대로 보낸다(build_context)
    }
    // 못 여는 파일(암호 등)은 build_context가 이유를 알린다
    let Ok(entries) = super::mcp::outline_entries(&path) else { return Ok(None) };
    let mut flat = Vec::new();
    flatten_outline(&entries, 0, &mut flat);
    let line = |i: usize, (d, t, p): &(usize, String, u32)| format!("[{i}] {}{t} (p.{})\n", "  ".repeat(*d), p + 1);
    let mut outline: String = flat.iter().enumerate().map(|(i, e)| line(i, e)).collect();
    if outline.chars().count() > OUTLINE_CHARS {
        // ponytail: 목차가 길면 상위 두 단계만 — 더 깊은 절은 고를 수 없다. 필요해지면 장을 먼저 고르고 그 안을 다시 고르게
        outline = flat.iter().enumerate().filter(|(_, e)| e.0 <= 1).map(|(i, e)| line(i, e)).collect::<String>().chars().take(OUTLINE_CHARS).collect();
    }
    let system = format!(
        "Decide which part of a {page_count}-page file the user's request is about. The user is viewing page {current}. \
         Reply with JSON only: {{\"scope\": \"entry\", \"entry\": N}} when the request names or refers to an outline entry \
         (a chapter, section or appendix, by number or title, or \"this chapter\"); {{\"scope\": \"pages\", \"pages\": \"10-20\"}} \
         when it gives page numbers; {{\"scope\": \"all\"}} when it is about the whole file (such as summarizing the whole document); \
         {{\"scope\": \"none\"}} when it asks something specific that is best found by searching."
    );
    let user = if outline.is_empty() { format!("The file has no outline.\n\nRequest: {question}") } else { format!("Outline:\n{outline}\nRequest: {question}") };
    let schema = json!({
        "type": "object",
        "properties": { "scope": { "enum": ["entry", "pages", "all", "none"] }, "entry": { "type": "integer" }, "pages": { "type": "string" } },
        "required": ["scope"],
    });
    let pick: Value = serde_json::from_str(&complete(model, &system, &user, Some(schema))?).unwrap_or_default();
    let (label, start, end) = match pick["scope"].as_str() {
        Some("entry") => match pick["entry"].as_u64().map(|i| i as usize).filter(|&i| i < flat.len()) {
            Some(i) => {
                let (s, e) = entry_range(&flat, i, page_count);
                (flat[i].1.clone(), s, e)
            }
            None => return Ok(None),
        },
        Some("pages") => {
            let Ok(pages) = super::parse_page_spec(pick["pages"].as_str().unwrap_or(""), page_count as usize) else { return Ok(None) };
            let (Some(&s), Some(&e)) = (pages.iter().min(), pages.iter().max()) else { return Ok(None) };
            (String::new(), s as u32 + 1, e as u32 + 1)
        }
        Some("all") => (String::new(), 1, page_count),
        _ => return Ok(None),
    };
    send(json!({ "tool": "scope", "args": { "title": label, "p": format!("{start}–{end}") } }))?;
    read_pages(model, question, &label, &format!("{start}-{end}"), send)
}

// 정해진 범위(spec: "96-110" 또는 "3-10,15")를 끝까지 읽어 맥락 블록으로 — 짧으면 통째로, 길면 나눠 읽고 요점만 모은다
fn read_pages(model: &str, question: &str, label: &str, spec: &str, send: &dyn Fn(Value) -> Result<(), String>) -> Result<Option<String>, String> {
    let Ok(view) = super::mcp::read_view() else { return Ok(None) };
    let path = view["path"].as_str().unwrap_or("").to_string();
    let page_count = view["page_count"].as_u64().unwrap_or(0) as usize;
    // 못 여는 파일(암호 등)은 build_context가 이유를 알린다
    let Ok(md) = super::mcp::to_markdown(&path, spec, page_count) else { return Ok(None) };
    let scope_label = if label.is_empty() { format!("pages {spec}") } else { format!("{label} (pages {spec})") };
    let pages = split_pages(&md);
    if pages.iter().all(|(_, t)| t.split_once("-->").is_none_or(|(_, body)| body.trim().is_empty())) {
        return Ok(None); // 스캔 등 텍스트가 없는 범위 — build_context가 이유("no_text")를 알린다
    }
    if md.chars().count() <= DIRECT_CHARS {
        return Ok(Some(format!("<document scope=\"{scope_label}\">\n{md}\n</document>")));
    }
    // 길면 나눠 읽고 요점만 모은다 — 모아도 길면(책 전체 등) 요점끼리 다시 묶어 줄인다
    let chunks = chunk_pages(&pages, CHUNK_CHARS);
    let read_system = "You are reading part of a document to help answer a user's request. Write concise notes, in the language \
                       of the request, with everything in these pages that matters for the request — key points, definitions, \
                       examples and conclusions — citing page numbers like (p. 12). Notes only, no preamble.";
    let mut notes = Vec::new();
    for (i, (s, e, text)) in chunks.iter().enumerate() {
        if STOP.load(Ordering::Relaxed) {
            return Ok(None);
        }
        send(json!({ "tool": "read", "args": { "p": format!("{s}–{e}"), "i": i + 1, "n": chunks.len() } }))?;
        let note = complete(model, read_system, &format!("Request: {question}\n\n<pages from=\"{s}\" to=\"{e}\">\n{text}\n</pages>"), None)?;
        notes.push((*s, *e, format!("Notes on pages {s}–{e}:\n{note}\n\n")));
    }
    while notes.iter().map(|n| n.2.chars().count()).sum::<usize>() > DIRECT_CHARS && notes.len() > 1 {
        let groups = chunk_pages(&notes.iter().map(|n| (n.0, n.2.clone())).collect::<Vec<_>>(), CHUNK_CHARS);
        let ends: Vec<u32> = notes.iter().map(|n| n.1).collect();
        let mut merged = Vec::new();
        for (i, (s, e, text)) in groups.iter().enumerate() {
            if STOP.load(Ordering::Relaxed) {
                return Ok(None);
            }
            send(json!({ "tool": "merge", "args": { "i": i + 1, "n": groups.len() } }))?;
            let note = complete(model, read_system, &format!("Request: {question}\n\nCombine these notes into shorter notes:\n{text}"), None)?;
            // groups의 끝은 마지막 묶음의 "시작" 페이지라, 그 묶음의 실제 끝 페이지로 바꿔 적는다
            let end = notes.iter().position(|n| n.0 == *e).map_or(*e, |k| ends[k]);
            merged.push((*s, end, format!("Notes on pages {s}–{end}:\n{note}\n\n")));
        }
        if merged.len() >= notes.len() {
            break; // 더 줄지 않으면(묶음 하나가 이미 상한 근처) 그대로 쓴다
        }
        notes = merged;
    }
    let notes: String = notes.into_iter().map(|n| n.2).collect();
    Ok(Some(format!(
        "<notes scope=\"{scope_label}\">\nThese notes were taken by reading every page of this part in full. Answer from them, citing page numbers.\n\n{notes}</notes>"
    )))
}

// Ollama가 모델 정보에 밝히는 기능 목록으로 판단 — 못 알아내면(옛 Ollama 등) 도구 없이.
// OpenAI 호환은 이런 API가 없어 일단 쓴다고 보고, 거절하면 ai_chat이 도구 없이 다시 보낸다
fn supports_tools(model: &str) -> bool {
    let s = current();
    if s.openai() {
        return true;
    }
    s.post("/api/show")
        .send_json(json!({ "model": model }))
        .ok()
        .and_then(|r| r.into_json::<Value>().ok())
        .is_some_and(|v| v["capabilities"].as_array().is_some_and(|c| c.iter().any(|x| x == "tools")))
}

// MCP tools/list의 정의를 Ollama(OpenAI 함수 호출) 형식으로 옮긴다 — 설명을 한 곳에서만 관리하려고
fn tool_defs() -> Value {
    let list = super::mcp::handle("tools/list", &json!({})).unwrap_or_default();
    let defs: Vec<Value> = list["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| TOOLS.contains(&t["name"].as_str().unwrap_or("")))
        .map(|t| json!({ "type": "function", "function": { "name": t["name"], "description": t["description"], "parameters": t["inputSchema"] } }))
        .collect();
    json!(defs)
}

// 작은 모델은 pages를 3처럼 숫자로 주곤 하는데 MCP 도구는 "3" 같은 문자열만 읽는다
fn normalize_args(args: &Value) -> Value {
    let mut args = if args.is_object() { args.clone() } else { json!({}) };
    if let Some(n) = args["pages"].as_u64() {
        args["pages"] = json!(n.to_string());
    }
    args
}

// 실패도 모델에게 그대로 알려서 다른 방법을 찾게 한다(없는 페이지, 잘못된 인자 등)
fn run_tool(name: &str, args: &Value) -> String {
    if !TOOLS.contains(&name) {
        return format!("Error: unknown tool {name}");
    }
    match super::mcp::call_tool(name, args) {
        Ok(content) => {
            let text: String = content.iter().filter_map(|c| c["text"].as_str()).collect::<Vec<_>>().join("\n");
            if text.chars().count() <= TOOL_CHARS {
                return text;
            }
            let cut: String = text.chars().take(TOOL_CHARS).collect();
            format!("{cut}\n\n[Truncated — request fewer pages to read the rest.]")
        }
        Err(e) => format!("Error: {e}"),
    }
}

// 세 번째 값은 맥락을 못 붙였을 때 패널에 띄울 이유 — 코드만 넘기고 문구는 프론트(i18n 'ai.notice_*')가 고른다.
// "password": 암호 문서(MCP의 open_doc은 앱에서 푼 암호를 모른다), "unsupported": 못 여는 형식,
// "no_text": 열었지만 텍스트가 없음(스캔/이미지). 네 번째 값은 도구를 쓸지 — 모델이 지원하고 MuPDF로 열리는 파일일 때만
// scope: read_scope가 읽어 온 범위 블록 — 있으면 지금 페이지 대신 그걸 붙인다
fn build_context(context: &str, can_use_tools: bool, scope: Option<String>) -> (String, Vec<String>, Option<&'static str>, bool) {
    let mut system = String::from(
        "You are an assistant built into Vimong, a document viewer. Answer in the language the user writes in. \
         Base your answers on the document content below when it is relevant, and say so when the answer is not in it. \
         Use Markdown.",
    );
    let mut images = Vec::new();
    // 패널은 문서가 열려 있을 때만 뜨니 기록이 없다는 건 hwp처럼 set_current_view가 경로를 비워 보낸 형식이다
    let Ok(view) = super::mcp::read_view() else { return (system, images, Some("unsupported"), false) };
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
    // 마크다운은 MCP 도구가 못 연다. 도구를 쓰면 "파일 전체"도 지금 페이지만 붙이고 나머지는 모델이 찾아 읽는다
    let want_tools = can_use_tools && !is_md;
    if let Some(block) = scope {
        if let Some(name) = file.file_name() {
            system += &format!("\n\nThe user is viewing \"{}\", page {page}.", name.to_string_lossy());
        }
        system += &format!("\n\n{block}");
        if let Some(sel) = view["selection"].as_str().filter(|s| !s.trim().is_empty()) {
            system += &format!("\n\nThe user selected this passage; \"this\" usually refers to it:\n<selection>\n{sel}\n</selection>");
        }
        return (system, images, None, false);
    }
    let (label, text) = if is_md || (context == "doc" && !want_tools) {
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
    let use_tools = want_tools && text.is_ok();
    if use_tools {
        let page_count = view["page_count"].as_u64().unwrap_or(0);
        system += &format!(
            "\n\nThe file has {page_count} pages and only the page the user is viewing is shown above. When the question needs \
             other parts of the file, use the tools: get_outline for its structure, search_text to find the pages where a topic \
             appears, then get_page_text for those pages (a few at a time). Call the tools yourself before answering — never \
             ask the user for permission or tell them which tool to use. Cite page numbers in your answer."
        );
    }
    if let Some(sel) = view["selection"].as_str().filter(|s| !s.trim().is_empty()) {
        system += &format!("\n\nThe user selected this passage; \"this\" usually refers to it:\n<selection>\n{sel}\n</selection>");
    }
    if context == "image" && text.is_ok() && !is_md {
        let content = super::mcp::call_tool("get_page_image", &json!({})).unwrap_or_default();
        images.extend(content.iter().filter_map(|c| c["data"].as_str().map(String::from)));
    }
    (system, images, notice, use_tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_defs_expose_only_read_only_mcp_tools_in_ollama_format() {
        let defs = tool_defs();
        let names: Vec<&str> = defs.as_array().unwrap().iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert_eq!(names.len(), TOOLS.len());
        assert!(TOOLS.iter().all(|t| names.contains(t)));
        let search = defs.as_array().unwrap().iter().find(|d| d["function"]["name"] == "search_text").unwrap();
        assert_eq!(search["type"], "function");
        assert_eq!(search["function"]["parameters"]["required"][0], "query");
        assert_eq!(run_tool("add_highlight", &json!({})), "Error: unknown tool add_highlight");
    }

    #[test]
    fn outline_entry_spans_until_the_next_entry_at_the_same_level() {
        let flat: Vec<(usize, String, u32)> = vec![
            (0, "15장".into(), 80),
            (0, "16장 한글조판의 실제".into(), 95),
            (1, "16.1 글꼴".into(), 97),
            (1, "16.2 줄 간격".into(), 102),
            (0, "17장".into(), 110),
            (0, "부록".into(), 130),
        ];
        assert_eq!(entry_range(&flat, 1, 150), (96, 110)); // 하위 절은 건너뛰고 17장 직전까지
        assert_eq!(entry_range(&flat, 2, 150), (98, 102));
        assert_eq!(entry_range(&flat, 3, 150), (103, 110)); // 마지막 절은 상위 단계 다음 장 직전까지
        assert_eq!(entry_range(&flat, 5, 150), (131, 150)); // 마지막 항목은 문서 끝까지
    }

    #[test]
    fn pages_split_by_marker_and_group_under_the_limit() {
        let md = "<!-- page 3 -->\n\naaaa\n\n<!-- page 4 -->\n\nbbbb\n\n<!-- page 5 -->\n\ncccccccccccccccccccccccccccccc\n\n";
        let pages = split_pages(md);
        assert_eq!(pages.iter().map(|p| p.0).collect::<Vec<_>>(), vec![3, 4, 5]);
        assert!(pages[1].1.starts_with("<!-- page 4 -->") && pages[1].1.contains("bbbb"));
        let chunks = chunk_pages(&pages, 50);
        assert_eq!(chunks.iter().map(|c| (c.0, c.1)).collect::<Vec<_>>(), vec![(3, 4), (5, 5)]);
        assert!(chunks[0].2.contains("aaaa") && chunks[0].2.contains("bbbb"));
    }

    #[test]
    fn real_pdf_markdown_splits_into_its_pages() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/synctex-demo.pdf");
        let count = mupdf::Document::open(path).unwrap().page_count().unwrap() as usize;
        let pages = split_pages(&crate::mcp::to_markdown(path, &format!("1-{count}"), count).unwrap());
        assert_eq!(pages.iter().map(|p| p.0 as usize).collect::<Vec<_>>(), (1..=count).collect::<Vec<_>>());
    }

    #[test]
    fn saved_markdown_links_page_images_written_next_to_it() {
        let dir = std::env::temp_dir().join(format!("vimong-ai-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("my notes.md");
        let doc = concat!(env!("CARGO_MANIFEST_DIR"), "/../testdata/synctex-demo.pdf");
        write_markdown(out.to_str().unwrap(), "## 요약\n\n본문", doc, &[1]).unwrap();
        let md = std::fs::read_to_string(&out).unwrap();
        assert!(md.starts_with("![p. 1](<my notes-p1.jpg>)\n\n## 요약"), "{md}");
        let jpg = std::fs::read(dir.join("my notes-p1.jpg")).unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]); // JPEG
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn markdown_fence_around_the_document_is_dropped_but_inner_code_kept() {
        // 앞뒤 설명 + 문서 전체를 감싼 펜스 — 안쪽 코드 블록도 같은 ``` 로 감싸져 있다
        let reply = "요약입니다.\n\n```markdown\n# 제6장\n\n```tex\n\\def\\a{b}\n```\n\n끝\n```\n\n필요하면 말씀하세요.";
        assert_eq!(unwrap_markdown_fence(reply), "# 제6장\n\n```tex\n\\def\\a{b}\n```\n\n끝");
        assert_eq!(unwrap_markdown_fence("```md\n# A\n"), "# A"); // 닫는 펜스 없이 끊긴 답
        assert_eq!(unwrap_markdown_fence("~~~~markdown\n# A\n~~~~\n"), "# A");
        // markdown 펜스가 없거나 둘 이상이면 손대지 않는다
        let plain = "# A\n\n```python\nprint(1)\n```";
        assert_eq!(unwrap_markdown_fence(plain), plain);
        let two = "```markdown\na\n```\n```markdown\nb\n```";
        assert_eq!(unwrap_markdown_fence(two), two);
    }

    #[test]
    fn host_setting_overrides_env_and_gets_a_scheme() {
        let s = |provider: &str, host: &str| Server { provider: provider.into(), host: host.into(), key: String::new() }.base_url();
        assert_eq!(s("ollama", "192.168.0.5:11434/"), "http://192.168.0.5:11434");
        assert_eq!(s("ollama", "http://gpu-box:11434"), "http://gpu-box:11434");
        assert_eq!(s("openai", ""), "http://127.0.0.1:1234"); // LM Studio 기본
        assert_eq!(s("openai", "http://localhost:8080/v1/"), "http://localhost:8080"); // .../v1까지 적어도
    }

    #[test]
    fn openai_requests_carry_images_tool_calls_and_json_schema() {
        let body = json!({
            "model": "m", "stream": true, "options": { "num_ctx": 16384 }, "format": { "type": "object" },
            "messages": [
                { "role": "user", "content": "q", "images": ["QUJD"] },
                { "role": "assistant", "content": "", "tool_calls": [{ "id": "c1", "function": { "name": "search_text", "arguments": { "query": "x" } } }] },
                { "role": "tool", "tool_name": "search_text", "tool_call_id": "c1", "content": "hits" },
            ],
        });
        let out = to_openai_body(&body);
        assert!(out.get("options").is_none());
        assert_eq!(out["response_format"]["json_schema"]["schema"]["type"], "object");
        assert_eq!(out["messages"][0]["content"][1]["image_url"]["url"], "data:image/jpeg;base64,QUJD");
        assert_eq!(out["messages"][1]["tool_calls"][0]["function"]["arguments"], "{\"query\":\"x\"}");
        assert_eq!(out["messages"][2], json!({ "role": "tool", "tool_call_id": "c1", "content": "hits" }));
    }

    #[test]
    fn openai_stream_collects_text_and_split_tool_calls() {
        let lines = [
            r#"data: {"choices":[{"delta":{"content":"찾아"}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{"content":"볼게요"}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"search_text","arguments":""}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"query\":"}}]}}]}"#,
            r#"data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"vcenter\"}"}}]}}]}"#,
            "data: [DONE]",
        ];
        let sent = std::cell::RefCell::new(String::new());
        let send = |v: Value| { sent.borrow_mut().push_str(v["delta"].as_str().unwrap_or("")); Ok(()) };
        let (content, calls) = read_openai_stream(lines.iter().map(|l| Ok(l.to_string())), &send).unwrap();
        assert_eq!(content, "찾아볼게요");
        assert_eq!(*sent.borrow(), "찾아볼게요");
        assert_eq!(calls, vec![json!({ "id": "call_1", "function": { "name": "search_text", "arguments": { "query": "vcenter" } } })]);
    }

    #[test]
    fn numeric_pages_become_page_specs() {
        assert_eq!(normalize_args(&json!({ "pages": 3 }))["pages"], "3");
        assert_eq!(normalize_args(&json!({ "pages": "1-3" }))["pages"], "1-3");
        assert_eq!(normalize_args(&Value::Null), json!({}));
    }
}
