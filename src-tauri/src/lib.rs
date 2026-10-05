use base64::{engine::general_purpose, Engine};
use mupdf::color::AnnotationColor;
use mupdf::{
    Colorspace, DestinationKind, Document, ImageFormat, Matrix, MetadataName, OptionalContentRef, Pixmap, Point,
    Quad, Rect,
};
use mupdf::pdf::{
    Encryption, FieldFlags, InsertPdfOptions, PageImageInfo, PageSelection, PdfAnnotationType, PdfDocument,
    PdfObject, PdfPage, PdfWriteOptions, WidgetType,
};
use regex::RegexBuilder;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
#[cfg(target_os = "macos")]
mod ocr_macos;
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    Emitter, Manager, State,
};

// ── i18n ──────────────────────────────────────────────────────
struct Labels {
    file: &'static str,
    new_tab: &'static str,
    next_tab: &'static str,
    prev_tab: &'static str,
    close_tab: &'static str,
    doc_info: &'static str,
    print: &'static str,
    password: &'static str,
    save_as: &'static str,
    export: &'static str,
    tab: &'static str,
    edit: &'static str,
    copy: &'static str,
    tools: &'static str,
    pointer: &'static str,
    text_select: &'static str,
    highlight_tool: &'static str,
    view: &'static str,
    actual_size: &'static str,
    zoom_to_fit: &'static str,
    zoom_to_fit_height: &'static str,
    zoom_in: &'static str,
    zoom_out: &'static str,
    zoom_to_selection: &'static str,
    show_sidebar: &'static str,
    hide_sidebar: &'static str,
    show_toolbar: &'static str,
    hide_toolbar: &'static str,
    fullscreen: &'static str,
    view_single: &'static str,
    view_single_continuous: &'static str,
    view_two: &'static str,
    view_two_continuous: &'static str,
    view_horizontal_continuous: &'static str,
    help: &'static str,
    shortcuts: &'static str,
    commands: &'static str,
    open_source: &'static str,
    settings: &'static str,
    quit: &'static str,
    about: &'static str,
    reopen_closed_tab: &'static str,
}

fn detect_lang() -> &'static str {
    let locale = sys_locale::get_locale().unwrap_or_default().to_lowercase();
    if locale.starts_with("ko") { return "ko"; }
    if locale.starts_with("ja") { return "ja"; }
    if locale.starts_with("zh-hans") || locale.starts_with("zh-cn") || locale.starts_with("zh-sg") {
        return "zh-hans";
    }
    if locale.starts_with("zh") { return "zh-hant"; }
    if locale.starts_with("es") { return "es"; }
    "en"
}

// 사용자가 설정에서 언어를 직접 고르면 이 파일에 저장해서 다음 실행 때도 유지되게 한다.
// 세션 중 언어를 바꾸면 이 파일 저장과 별개로 relabel_menu로 메뉴도 즉시 다시 라벨링한다.
fn lang_override_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("lang.txt"))
}

fn resolve_lang(app: &tauri::AppHandle) -> String {
    lang_override_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "auto")
        .unwrap_or_else(|| detect_lang().to_string())
}

#[tauri::command]
fn set_app_language(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    let path = lang_override_path(&app).ok_or("no config dir")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if lang == "auto" {
        let _ = std::fs::remove_file(&path);
    } else {
        std::fs::write(&path, &lang).map_err(|e| e.to_string())?;
    }
    let resolved = if lang == "auto" { detect_lang().to_string() } else { lang };
    if let Some(state) = app.try_state::<MenuLang>() {
        *state.0.lock().unwrap() = resolved.clone();
    }
    relabel_menu(&app, &labels_for(&resolved));
    Ok(())
}

// window.print()는 Tauri 문서상 "모든 플랫폼에서 동작"이라고 되어 있지만, 실제로는 webview
// 구현체(wry)에 따라 조용히 아무 것도 안 하는 경우가 있었다 — WebviewWindow::print()는 같은
// 문서에서 "현재 macOS만 지원"이라고 명시된 네이티브 경로라 더 신뢰할 수 있다. JS 쪽에서 이
// 커맨드를 먼저 시도하고, 실패하면(Windows/Linux 등 미지원 플랫폼) window.print()로 폴백한다.
#[tauri::command]
fn print_webview(window: tauri::WebviewWindow) -> Result<(), String> {
    window.print().map_err(|e| e.to_string())
}

// 프론트에서 set_fullscreen()과 setFocus()를 각각 별도 invoke로 호출했더니(둘 다 에러 없이
// "성공"은 했다) webview가 first responder를 못 되찾는 문제가 있었다 — 매 invoke가 IPC
// 왕복을 거치는 JS 비동기 콜백이라, macOS가 "진짜 사용자 제스처의 연장"으로 안 쳐서
// 포커스 이양을 막았을 가능성이 있다. 이 커맨드는 두 네이티브 호출을 하나의 Rust 함수
// 안에서 IPC 왕복 없이 곧바로 이어붙여서, 그 경계 자체를 없앤다.
//
// 그런데도 window.set_focus()(macOS의 makeKeyAndOrderFront:)만으로는 webview가 first
// responder를 못 되찾았다 — tao가 예전의 확실한 makeFirstResponder_ 호출 대신 macOS가
// 알아서 정해주길 기대하는 initialResponder 방식으로 바꾼 뒤로 생긴 알려진 버그
// (https://github.com/tauri-apps/tao/issues/208, 미해결). 그래서 [NSWindow
// makeFirstResponder:webview]를 직접 호출한다.
#[cfg(target_os = "macos")]
fn reclaim_webview_focus(window: &tauri::WebviewWindow) {
    use objc2_app_kit::NSWindow;
    use objc2_web_kit::WKWebView;
    let _ = window.with_webview(|webview| {
        let ns_window_ptr = webview.ns_window() as *const NSWindow;
        let wk_webview_ptr = webview.inner() as *const WKWebView;
        if ns_window_ptr.is_null() || wk_webview_ptr.is_null() {
            return;
        }
        // SAFETY: 두 포인터 다 wry가 관리하는 살아있는 NSWindow/WKWebView를 가리키며,
        // with_webview()의 클로저는 메인 스레드에서 실행된다고 문서화돼 있다(AppKit 호출
        // 요구사항 충족).
        unsafe {
            let ns_window: &NSWindow = &*ns_window_ptr;
            let wk_webview: &WKWebView = &*wk_webview_ptr;
            ns_window.makeFirstResponder(Some(wk_webview));
        }
    });
}
#[cfg(not(target_os = "macos"))]
fn reclaim_webview_focus(_window: &tauri::WebviewWindow) {}

// macOS Space 전환 애니메이션(0.3~0.5초)이 실제로 끝나기 전에 makeFirstResponder를 불러도
// 애니메이션이 마무리되면서 다시 놓치는 것 같았다 — 즉시 한 번, 그리고 애니메이션이 끝날
// 만한 시점에 백그라운드 스레드에서 몇 번 더 재시도한다. 간격을 150/400/800ms로 뒀더니
// 사용자가 전환 직후 바로 키를 누르면(특히 첫 150ms 안) 아직 재시도가 안 와서 삑- 소리만
// 나고 한두 번 더 눌러야 되는 문제가 있었다 — 초반을 훨씬 촘촘하게 당겨서 그 틈을 줄인다.
// with_webview()가 내부적으로 메인 스레드로 안전하게 넘겨주므로 여기서 직접 AppKit을
// 건드리지는 않는다.
fn reclaim_webview_focus_retrying(window: tauri::WebviewWindow) {
    reclaim_webview_focus(&window);
    std::thread::spawn(move || {
        for delay_ms in [30, 60, 100, 150, 250, 400, 600] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            reclaim_webview_focus(&window);
        }
    });
}

// 전체화면 종료 애니메이션이 끝나면서 창이 다시 화면 꽉 찬 크기로 스냅되는 경우가 있어서,
// reclaim_webview_focus_retrying과 같은 패턴(즉시 + 몇 차례 재시도)으로 원래 크기/위치를
// 되돌린다.
fn restore_window_frame_retrying(
    window: tauri::WebviewWindow,
    pos: tauri::PhysicalPosition<i32>,
    size: tauri::PhysicalSize<u32>,
) {
    let _ = window.set_size(size);
    let _ = window.set_position(pos);
    std::thread::spawn(move || {
        for delay_ms in [100, 300, 600] {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            let _ = window.set_size(size);
            let _ = window.set_position(pos);
        }
    });
}

#[tauri::command]
fn set_fullscreen_and_focus(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    fullscreen: bool,
) -> Result<(), String> {
    // macOS가 전체화면 종료 후 원래 창 크기로 알아서 복원해줄 거라 기대했는데, 화면을 꽉
    // 채운(최대화 비슷한) 크기로 남는 경우가 있었다 — 진입 전에 직접 저장해뒀다가 나갈 때
    // 되돌린다.
    if fullscreen {
        if let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) {
            if let Some(state) = app.try_state::<SavedWindowFrame>() {
                *state.0.lock().unwrap() = Some((pos, size));
            }
        }
    }
    window.set_fullscreen(fullscreen).map_err(|e| e.to_string())?;
    let _ = window.set_focus();
    if !fullscreen {
        if let Some(state) = app.try_state::<SavedWindowFrame>() {
            if let Some((pos, size)) = state.0.lock().unwrap().take() {
                restore_window_frame_retrying(window.clone(), pos, size);
            }
        }
    }
    reclaim_webview_focus_retrying(window);
    Ok(())
}

fn labels_for(lang: &str) -> Labels {
    match lang {
        "ko" => Labels {
            file: "파일", new_tab: "새 탭", next_tab: "다음 탭", prev_tab: "이전 탭", close_tab: "탭 닫기", doc_info: "문서 정보", print: "인쇄", password: "암호", save_as: "다른 이름으로 저장...", export: "내보내기...",
            tab: "탭",
            edit: "편집", copy: "복사",
            tools: "도구", pointer: "포인터", text_select: "텍스트 선택", highlight_tool: "하이라이트",
            view: "보기", actual_size: "실제 크기", zoom_to_fit: "폭 맞춤", zoom_to_fit_height: "높이 맞춤",
            zoom_in: "확대", zoom_out: "축소", zoom_to_selection: "선택 영역 확대",
            show_sidebar: "사이드바 보이기", hide_sidebar: "사이드바 숨기기",
            show_toolbar: "툴바 보이기", hide_toolbar: "툴바 숨기기",
            fullscreen: "전체 화면",
            view_single: "한 페이지", view_single_continuous: "한 페이지 연속",
            view_two: "두 페이지", view_two_continuous: "두 페이지 연속",
            view_horizontal_continuous: "가로 연속",
            help: "도움말", shortcuts: "단축키", commands: "명령어", open_source: "오픈소스", settings: "설정", quit: "Vimong 종료", about: "Vimong 정보", reopen_closed_tab: "닫은 탭 다시 열기",
        },
        "ja" => Labels {
            file: "ファイル", new_tab: "新しいタブ", next_tab: "次のタブ", prev_tab: "前のタブ", close_tab: "タブを閉じる", doc_info: "書類情報", print: "印刷", password: "パスワード", save_as: "名前を付けて保存...", export: "エクスポート...",
            tab: "タブ",
            edit: "編集", copy: "コピー",
            tools: "ツール", pointer: "ポインタ", text_select: "テキスト選択", highlight_tool: "ハイライト",
            view: "表示", actual_size: "実際のサイズ", zoom_to_fit: "幅に合わせる", zoom_to_fit_height: "高さに合わせる",
            zoom_in: "拡大", zoom_out: "縮小", zoom_to_selection: "選択範囲を拡大",
            show_sidebar: "サイドバーを表示", hide_sidebar: "サイドバーを隠す",
            show_toolbar: "ツールバーを表示", hide_toolbar: "ツールバーを隠す",
            fullscreen: "フルスクリーン",
            view_single: "単一ページ", view_single_continuous: "単一ページ（連続）",
            view_two: "見開き2ページ", view_two_continuous: "見開き2ページ（連続）",
            view_horizontal_continuous: "横スクロール（連続）",
            help: "ヘルプ", shortcuts: "キーボードショートカット", commands: "コマンド", open_source: "オープンソース", settings: "設定", quit: "Vimongを終了", about: "Vimongについて", reopen_closed_tab: "閉じたタブを再度開く",
        },
        "zh-hans" => Labels {
            file: "文件", new_tab: "新建标签页", next_tab: "下一个标签页", prev_tab: "上一个标签页", close_tab: "关闭标签页", doc_info: "文档信息", print: "打印", password: "密码", save_as: "另存为...", export: "导出...",
            tab: "标签页",
            edit: "编辑", copy: "复制",
            tools: "工具", pointer: "指针", text_select: "选择文本", highlight_tool: "荧光笔",
            view: "显示", actual_size: "实际大小", zoom_to_fit: "适合宽度", zoom_to_fit_height: "适合高度",
            zoom_in: "放大", zoom_out: "缩小", zoom_to_selection: "缩放所选内容",
            show_sidebar: "显示侧边栏", hide_sidebar: "隐藏侧边栏",
            show_toolbar: "显示工具栏", hide_toolbar: "隐藏工具栏",
            fullscreen: "全屏",
            view_single: "单页", view_single_continuous: "单页连续",
            view_two: "双页", view_two_continuous: "双页连续",
            view_horizontal_continuous: "横向连续",
            help: "帮助", shortcuts: "键盘快捷键", commands: "命令", open_source: "开源许可", settings: "设置", quit: "退出 Vimong", about: "关于 Vimong", reopen_closed_tab: "重新打开关闭的标签页",
        },
        "zh-hant" => Labels {
            file: "檔案", new_tab: "新增標籤頁", next_tab: "下一個標籤頁", prev_tab: "上一個標籤頁", close_tab: "關閉標籤頁", doc_info: "文件資訊", print: "列印", password: "密碼", save_as: "另存新檔...", export: "匯出...",
            tab: "標籤頁",
            edit: "編輯", copy: "複製",
            tools: "工具", pointer: "指標", text_select: "選取文字", highlight_tool: "螢光筆",
            view: "顯示", actual_size: "實際大小", zoom_to_fit: "符合寬度", zoom_to_fit_height: "符合高度",
            zoom_in: "放大", zoom_out: "縮小", zoom_to_selection: "縮放所選項目",
            show_sidebar: "顯示側邊欄", hide_sidebar: "隱藏側邊欄",
            show_toolbar: "顯示工具列", hide_toolbar: "隱藏工具列",
            fullscreen: "全螢幕",
            view_single: "單頁", view_single_continuous: "單頁連續",
            view_two: "雙頁", view_two_continuous: "雙頁連續",
            view_horizontal_continuous: "橫向連續",
            help: "輔助說明", shortcuts: "鍵盤快捷鍵", commands: "命令", open_source: "開放原始碼", settings: "設定", quit: "結束 Vimong", about: "關於 Vimong", reopen_closed_tab: "重新開啟已關閉的標籤頁",
        },
        "es" => Labels {
            file: "Archivo", new_tab: "Nueva pestaña", next_tab: "Pestaña siguiente", prev_tab: "Pestaña anterior", close_tab: "Cerrar pestaña", doc_info: "Información del documento", print: "Imprimir", password: "Contraseña", save_as: "Guardar como...", export: "Exportar...",
            tab: "Pestaña",
            edit: "Editar", copy: "Copiar",
            tools: "Herramientas", pointer: "Puntero", text_select: "Seleccionar texto", highlight_tool: "Resaltador",
            view: "Ver", actual_size: "Tamaño real", zoom_to_fit: "Ajustar al ancho", zoom_to_fit_height: "Ajustar a la altura",
            zoom_in: "Acercar", zoom_out: "Alejar", zoom_to_selection: "Ampliar selección",
            show_sidebar: "Mostrar barra lateral", hide_sidebar: "Ocultar barra lateral",
            show_toolbar: "Mostrar barra de herramientas", hide_toolbar: "Ocultar barra de herramientas",
            fullscreen: "Pantalla completa",
            view_single: "Una página", view_single_continuous: "Una página continua",
            view_two: "Dos páginas", view_two_continuous: "Dos páginas continuas",
            view_horizontal_continuous: "Continuo horizontal",
            help: "Ayuda", shortcuts: "Atajos de teclado", commands: "Comandos", open_source: "Código abierto", settings: "Ajustes", quit: "Salir de Vimong", about: "Acerca de Vimong", reopen_closed_tab: "Reabrir pestaña cerrada",
        },
        _ => Labels {
            file: "File", new_tab: "New Tab", next_tab: "Next Tab", prev_tab: "Previous Tab", close_tab: "Close Tab", doc_info: "Document Info", print: "Print", password: "Password", save_as: "Save As...", export: "Export...",
            tab: "Tab",
            edit: "Edit", copy: "Copy",
            tools: "Tools", pointer: "Pointer", text_select: "Select Text", highlight_tool: "Highlight",
            view: "View", actual_size: "Actual Size", zoom_to_fit: "Zoom To Fit Width", zoom_to_fit_height: "Zoom To Fit Height",
            zoom_in: "Zoom In", zoom_out: "Zoom Out", zoom_to_selection: "Zoom To Selection",
            show_sidebar: "Show Sidebar", hide_sidebar: "Hide Sidebar",
            show_toolbar: "Show Toolbar", hide_toolbar: "Hide Toolbar",
            fullscreen: "Full Screen",
            view_single: "Single Page", view_single_continuous: "Single Page Continuous",
            view_two: "Two Pages", view_two_continuous: "Two Pages Continuous",
            view_horizontal_continuous: "Horizontal Continuous",
            help: "Help", shortcuts: "Keyboard Shortcuts", commands: "Commands", open_source: "Open Source", settings: "Settings", quit: "Quit Vimong", about: "About Vimong", reopen_closed_tab: "Reopen Closed Tab",
        },
    }
}

// 암호 걸린 문서를 열었을 때 인증 전까지 담아두는 곳 — 인증 성공 전에 tab.doc/path를
// 먼저 덮어써버리면, 사용자가 암호 입력을 취소했을 때 그 탭에 원래 열려 있던(정상적으로
// 보고 있던) 문서가 사라지고 잠긴 문서로 바뀐 채 남아버린다.
struct PendingOpen {
    doc: Document,
    path: String,
    file_size_kb: f64,
    file_name: String,
}

// ── PDF 문서 상태 (탭 하나) ────────────────────────────────────
struct PdfDoc {
    doc: Option<Document>,
    path: Option<String>,
    // open_pdf 시점에 캐싱 — get_file_info가 렌더 락과 경쟁하지 않도록
    cached_page_count: u32,
    cached_file_size_kb: f64,
    cached_file_name: String,
    pending: Option<PendingOpen>,
    dirty: bool, // 하이라이트 등 아직 디스크에 저장 안 한 변경사항이 있는지 — :w로 저장하면 꺼짐
    // add_highlight/add_shape가 성공할 때마다 (페이지 번호, 주석 타입)을 순서대로 쌓아둔다 —
    // u(undo)를 누르면 마지막 항목을 꺼내서 그 페이지의 그 타입 중 마지막 주석을 지운다(LIFO).
    // 하이라이트와 도형이 섞여도 종류별로 정확히 짚어 지우려고 타입까지 같이 기록한다.
    annotation_undo_stack: Vec<(u32, PdfAnnotationType)>,
}

impl PdfDoc {
    fn empty() -> Self {
        PdfDoc {
            doc: None, path: None,
            cached_page_count: 0, cached_file_size_kb: 0.0, cached_file_name: String::new(),
            pending: None, dirty: false, annotation_undo_stack: Vec::new(),
        }
    }
}

unsafe impl Send for PdfDoc {}

// ── 멀티탭 앱 상태 ─────────────────────────────────────────────
struct AppState {
    tabs: Vec<PdfDoc>,
    current: usize,
}

impl AppState {
    fn new() -> Self {
        AppState { tabs: vec![PdfDoc::empty()], current: 0 }
    }
    fn tab(&self) -> &PdfDoc { &self.tabs[self.current] }
    fn tab_mut(&mut self) -> &mut PdfDoc { &mut self.tabs[self.current] }
}

// ── 탭 관리 커맨드 ─────────────────────────────────────────────
#[tauri::command]
fn new_tab(app: tauri::AppHandle, state: State<Mutex<AppState>>) -> usize {
    let mut s = state.lock().unwrap();
    s.tabs.push(PdfDoc::empty());
    s.current = s.tabs.len() - 1;
    let (idx, count) = (s.current, s.tabs.len());
    drop(s);
    update_tab_menu_enabled(&app, count);
    idx
}

// 하이라이트 등 :w로 아직 저장 안 한 변경사항이 있는 탭인지 — 탭을 닫기 전에 프론트엔드가
// 확인창을 띄울지 판단하는 용도.
#[tauri::command]
fn is_tab_dirty(idx: usize, state: State<Mutex<AppState>>) -> bool {
    state.lock().unwrap().tabs.get(idx).is_some_and(|t| t.dirty)
}

#[tauri::command]
fn close_tab(app: tauri::AppHandle, idx: usize, state: State<Mutex<AppState>>) -> Result<usize, String> {
    let mut s = state.lock().unwrap();
    if idx >= s.tabs.len() { return Err("Invalid tab index".to_string()); }
    if s.tabs.len() == 1 {
        // 마지막 탭: 문서만 닫고 탭은 유지
        s.tabs[0] = PdfDoc::empty();
        s.current = 0;
        drop(s);
        update_tab_menu_enabled(&app, 1);
        return Ok(0);
    }
    s.tabs.remove(idx);
    // idx보다 앞 탭이 없어지면 current 인덱스가 한 칸씩 밀리므로 같이 보정해야
    // 활성 탭이 아닌 다른 탭을 닫았을 때 엉뚱한 탭으로 전환되지 않는다
    if idx < s.current { s.current -= 1; }
    else if s.current >= s.tabs.len() { s.current = s.tabs.len() - 1; }
    let (cur, count) = (s.current, s.tabs.len());
    drop(s);
    update_tab_menu_enabled(&app, count);
    Ok(cur)
}

#[tauri::command]
fn switch_tab(idx: usize, state: State<Mutex<AppState>>) -> Result<(), String> {
    let mut s = state.lock().unwrap();
    if idx >= s.tabs.len() { return Err("Invalid tab index".to_string()); }
    s.current = idx;
    Ok(())
}

// 탭바 드래그 재정렬 — from 위치의 탭을 to 위치로 옮기고, 그 사이에 끼는 다른 탭들처럼
// current 인덱스도 같이 밀어준다. 반환값은 재조정된 current(활성 탭이 이번에 옮겨진 그
// 탭이면 to를, 아니면 밀린 결과를 그대로 돌려줘서 프론트가 별도 계산 없이 반영하게 한다.
#[tauri::command]
fn reorder_tab(from: usize, to: usize, state: State<Mutex<AppState>>) -> Result<usize, String> {
    let mut s = state.lock().unwrap();
    if from >= s.tabs.len() || to >= s.tabs.len() { return Err("Invalid tab index".to_string()); }
    if from == to { return Ok(s.current); }
    let doc = s.tabs.remove(from);
    s.tabs.insert(to, doc);
    s.current = if s.current == from {
        to
    } else if from < to && s.current > from && s.current <= to {
        s.current - 1
    } else if from > to && s.current >= to && s.current < from {
        s.current + 1
    } else {
        s.current
    };
    Ok(s.current)
}

// 내보내기 모달에서 파일명을 직접 입력받을 때 덮어쓰기 확인용 — 네이티브 저장 다이얼로그를
// 안 거치면 OS가 자동으로 해주던 "이미 있는 파일" 경고가 없어지므로 직접 확인해야 한다.
#[tauri::command]
fn path_exists(path: String) -> bool {
    std::path::Path::new(&path).exists()
}

// :e/:w/:export 명령의 경로 인자에서 Tab 자동완성으로 쓴다 — dir(빈 문자열이면 기준 폴더
// 자체)을 resolve_out_path와 같은 규칙(~/절대/상대)으로 풀어서 그 안의 항목 이름을 나열한다.
// 문서가 안 열려 있으면(:e) 앱 작업 디렉터리를 기준으로 삼는다.
#[tauri::command]
fn list_dir_entries(dir: String, doc_path: Option<String>) -> Vec<String> {
    let doc_dir = || {
        doc_path.as_deref()
            .and_then(|dp| std::path::Path::new(dp).parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    };
    let base = if dir.is_empty() {
        doc_dir()
    } else {
        let expanded = expand_tilde(&dir);
        let p = std::path::Path::new(&expanded);
        if p.is_absolute() { p.to_path_buf() } else { doc_dir().join(&expanded) }
    };
    let mut entries: Vec<String> = std::fs::read_dir(&base)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir { format!("{name}/") } else { name }
        })
        .collect();
    entries.sort();
    entries
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle, confirmed: State<QuitConfirmed>) {
    // app.exit()은 강제 종료가 아니라 RunEvent::ExitRequested를 다시 발생시킨다 —
    // 플래그 없이 부르면 우리가 등록한 핸들러가 탭 2개 이상이라고 또 막아버려서
    // "종료" 버튼을 눌러도 아무 일도 안 일어나는 것처럼 보인다.
    confirmed.0.store(true, Ordering::Relaxed);
    app.exit(0);
}

// ── PDF 커맨드 ─────────────────────────────────────────────────
#[derive(serde::Serialize)]
struct OpenResult {
    page_count: u32,
    needs_password: bool,
    // EPUB/HTML처럼 고정 페이지가 아니라 재배치(layout)로 페이지를 만드는 문서인지 —
    // 참이면 프론트에서 set_reflow_font_size로 실제 페이지를 만든다(그 전엔 mupdf 내부
    // 기본값으로 대충 나뉜 페이지 수라 의미가 없다).
    is_reflowable: bool,
}

// EPUB/HTML 재배치 페이지 크기(pt) — 화면 표시 크기는 기존 scale(맞춤/줌)이 알아서 맞춰주므로
// 여기 절대값 자체는 중요하지 않고, 폭 대비 폰트 크기 비율(줄바꿈 밀도)과 페이지 비율만 좌우한다.
const REFLOW_WIDTH: f32 = 450.0;
const REFLOW_HEIGHT: f32 = 650.0;

// hwp 등 MuPDF가 못 여는 포맷은 프론트에서 별도 라이브러리로 렌더링한다 —
// 여기서는 그 라이브러리에 넘길 원본 바이트만 그대로 돌려준다.
#[tauri::command]
fn read_file_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    std::fs::read(&path).map(tauri::ipc::Response::new).map_err(|e| e.to_string())
}

// 마지막으로 읽던 페이지 복원(프론트엔드의 vimong-last-page)이 파일 경로만 보고 판단하면,
// 같은 경로에 완전히 다른 문서가 덮어써져도 예전 페이지로 잘못 점프한다 — mtime을 같이
// 저장해뒀다가 달라지면 무시하게 하는 용도.
#[tauri::command]
fn get_file_mtime(path: String) -> Result<u64, String> {
    std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map_err(|e| e.to_string())?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn open_pdf(app: tauri::AppHandle, path: String) -> Result<OpenResult, String> {
    // Document::open + is_reflowable + page_count는 여러 언어/스크립트가 섞인 EPUB에서
    // 시스템 폰트 폴백 매칭(CoreText XPC 왕복)이 문자열마다 반복되어 수십 초~분 단위로
    // 걸릴 수 있다. 이 커맨드가 동기 fn이면 Tauri가 WebKit IPC를 처리하는 메인 스레드에서
    // 곧바로 실행하므로, 그동안 이벤트 루프 전체(창 그리기 포함)가 멎어 "앱이 뻗은" 것처럼
    // 보인다 — spawn_blocking으로 별도 스레드에 태워야 메인 스레드가 살아있다.
    // (State는 'static이 아니라 spawn_blocking 클로저에 못 들고 들어가서 AppHandle로 받는다)
    tauri::async_runtime::spawn_blocking(move || {
        // :e ~/foo.pdf도 열 수 있도록 여기서 한 번만 풀고, 이후 tab.path에도 이 값을 저장한다 —
        // 그래야 나중에 :w/:export가 resolve_out_path로 "문서와 같은 폴더"를 계산할 때도 맞는다.
        let path = expand_tilde(&path);
        let resolved = std::fs::canonicalize(&path)
            .unwrap_or_else(|_| std::path::PathBuf::from(&path));
        let path_str = resolved.to_str().ok_or("Invalid path")?;
        let doc = Document::open(path_str).map_err(|e| e.to_string())?;
        let needs_password = doc.needs_password().map_err(|e| e.to_string())?;
        let file_size_kb = std::fs::metadata(&path).map(|m| m.len() as f64 / 1024.0).unwrap_or(0.0);
        let file_name = std::path::Path::new(&path)
            .file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
        let state = app.state::<Mutex<AppState>>();
        if needs_password {
            // 인증 전까지는 pending에만 담아둔다 — 여기서 바로 tab.doc를 덮어쓰면 사용자가
            // 암호 입력을 취소했을 때 이 탭에 원래 열려 있던 문서가 잠긴 문서로 바뀐 채
            // 사라져버린다. (암호 걸린 재배치 문서는 실사용 사례가 없다시피 해서 이 경로에선
            // is_reflowable을 그냥 false로 둔다 — 인증 후 반영 안 됨)
            let mut s = state.lock().unwrap();
            let tab = s.tab_mut();
            tab.pending = Some(PendingOpen { doc, path, file_size_kb, file_name });
            return Ok(OpenResult { page_count: 0, needs_password: true, is_reflowable: false });
        }
        let is_reflowable = doc.is_reflowable().unwrap_or(false);
        let count = doc.page_count().map_err(|e| e.to_string())? as u32;
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        tab.cached_page_count = count;
        tab.cached_file_size_kb = file_size_kb;
        tab.cached_file_name = file_name;
        tab.doc = Some(doc);
        tab.path = Some(path);
        tab.pending = None;
        tab.dirty = false;
        tab.annotation_undo_stack.clear();
        Ok(OpenResult { page_count: count, needs_password: false, is_reflowable })
    }).await.map_err(|e| e.to_string())?
}

// EPUB/HTML 등 재배치 문서의 폰트 크기(em, pt)를 바꿔서 다시 페이지를 나눈다 — zi/zo가
// 고정 페이지 문서에서는 캔버스 확대/축소를, 재배치 문서에서는 이 relayout을 부르게 해서
// (mdMode의 mdScale과 같은 맥락) 확대할수록 글자가 커지고 그만큼 다시 줄바꿈되게 한다.
// 페이지 수가 바뀌므로 새 페이지 수를 돌려주고, 호출부(프론트)가 캐시를 전부 무효화한다.
#[tauri::command]
async fn set_reflow_font_size(app: tauri::AppHandle, em: f32) -> Result<u32, String> {
    // open_pdf와 같은 이유(main.rs 메인 스레드가 멎어 앱이 뻗은 것처럼 보임)로
    // spawn_blocking을 쓴다 — layout()+page_count()가 여러 언어/스크립트가 섞인 문서에서는
    // 수십 초 넘게 걸릴 수 있다. State는 'static이 아니라 spawn_blocking 클로저에 못
    // 들고 들어가서 AppHandle로 받는다.
    // Mutex는 짧게만 잡는다: tab.doc를 잠깐 꺼내(take) 락을 풀고 계산한 뒤 다시 넣는다 —
    // 그 사이 같은 탭에 대한 다른 문서 명령이 오면 "열린 문서가 없다"는 에러 하나로 실패하는
    // 정도가, 그동안 다른 탭 작업까지 전부 막는 것보다 낫다.
    // 나중에 되돌려놓을 탭을 인덱스로 기억해둔다 — s.tab_mut()(= s.current 기준)을 그대로
    // 다시 쓰면, 계산하는 동안 사용자가 다른 탭으로 옮겨갔을 때 엉뚱한(지금 보고 있는) 탭에
    // 이 문서를 꽂아버리게 된다.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let (idx, mut doc) = {
            let mut s = state.lock().unwrap();
            let idx = s.current;
            (idx, s.tab_mut().doc.take().ok_or("열린 문서가 없습니다")?)
        };
        let result = doc.layout(REFLOW_WIDTH, REFLOW_HEIGHT, em)
            .map_err(|e| e.to_string())
            .and_then(|_| doc.page_count().map_err(|e| e.to_string()));
        let mut s = state.lock().unwrap();
        let count = result.map(|c| c as u32);
        if let Some(tab) = s.tabs.get_mut(idx) {
            tab.doc = Some(doc); // 성공하든 실패하든 꺼냈던 문서는 반드시 되돌려놓는다
            if let Ok(c) = count { tab.cached_page_count = c; }
        }
        count
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn unlock_pdf(password: String, state: State<Mutex<AppState>>) -> Result<u32, String> {
    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    let pending = tab.pending.as_mut().ok_or("No pending password-protected document")?;
    let ok = pending.doc.authenticate(&password).map_err(|e| e.to_string())?;
    if !ok { return Err("Incorrect password".to_string()); }
    let count = pending.doc.page_count().map_err(|e| e.to_string())? as u32;
    let pending = tab.pending.take().unwrap();
    tab.cached_page_count = count;
    tab.cached_file_size_kb = pending.file_size_kb;
    tab.cached_file_name = pending.file_name;
    tab.doc = Some(pending.doc);
    tab.path = Some(pending.path);
    tab.dirty = false;
    tab.annotation_undo_stack.clear();
    Ok(count)
}

#[tauri::command]
fn set_pdf_password(password: String, state: State<Mutex<AppState>>) -> Result<(), String> {
    apply_pdf_password(&state, Some(password))
}

#[tauri::command]
fn remove_pdf_password(state: State<Mutex<AppState>>) -> Result<(), String> {
    apply_pdf_password(&state, None)
}

// 현재 보고 있는(이미 인증된) 문서를 그대로 재사용해서 새 파일로 저장한 뒤 원본에 덮어쓴다 —
// 다시 열어서 인증할 필요가 없고, 비밀번호 제거도 "지금 보고 있다 = 이미 풀렸다"를 그대로
// 이용한다. 임시 파일에 먼저 쓰고 rename으로 교체해서 저장 도중 실패해도 원본이 안 깨진다.
fn apply_pdf_password(
    state: &State<Mutex<AppState>>,
    new_password: Option<String>,
) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };
    if !doc.is_pdf() {
        state.lock().unwrap().tab_mut().doc = Some(doc);
        return Err("PDF 문서만 암호를 설정/해제할 수 있습니다".to_string());
    }

    let result: Result<(), String> = (|| {
        let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut opts = PdfWriteOptions::default();
        match &new_password {
            Some(pw) => {
                opts.set_encryption(Encryption::Aes256)
                    .set_owner_password(pw)
                    .set_user_password(pw);
            }
            None => {
                opts.set_encryption(Encryption::None);
            }
        }
        let tmp_path = format!("{path}.vimong-tmp");
        pdf.save_with_options(&tmp_path, opts).map_err(|e| e.to_string())?;
        drop(pdf); // 원본 path에 대한 읽기 핸들을 닫아야 교체(rename) 가능 — 특히 Windows
        std::fs::rename(&tmp_path, &path).map_err(|e| e.to_string())
    })();

    // 성공하든 실패하든 파일을 다시 열어서 tab.doc를 채워둔다 — 실패했다고 탭을 doc 없는
    // 상태로 방치하지 않는다.
    let mut reopened = Document::open(&path).map_err(|e| e.to_string())?;
    if result.is_ok() {
        if let Some(pw) = &new_password {
            reopened.authenticate(pw).ok();
        }
    }
    let count = reopened.page_count().unwrap_or(0) as u32;
    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    tab.doc = Some(reopened);
    tab.cached_page_count = count;
    result
}

fn rect_to_quad([x0, y0, x1, y1]: [f32; 4]) -> Quad {
    Quad {
        ul: Point { x: x0, y: y0 },
        ur: Point { x: x1, y: y0 },
        ll: Point { x: x0, y: y1 },
        lr: Point { x: x1, y: y1 },
    }
}

fn rects_overlap(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

// 서로 겹치는 사각형들을 바운딩 박스 하나로 합친다. 겹치는 쌍이 없어질 때까지 반복한다 —
// add_highlight가 새로 칠할 자리와 겹치는 기존 하이라이트를 하나로 병합해 다시 그릴 때 쓴다.
fn merge_overlapping_rects(mut rects: Vec<[f32; 4]>) -> Vec<[f32; 4]> {
    loop {
        let mut merged_any = false;
        'outer: for i in 0..rects.len() {
            for j in (i + 1)..rects.len() {
                if rects_overlap(rects[i], rects[j]) {
                    let b = rects.remove(j);
                    let a = &mut rects[i];
                    a[0] = a[0].min(b[0]);
                    a[1] = a[1].min(b[1]);
                    a[2] = a[2].max(b[2]);
                    a[3] = a[3].max(b[3]);
                    merged_any = true;
                    break 'outer;
                }
            }
        }
        if !merged_any {
            return rects;
        }
    }
}

// PDF Square 주석엔 모서리 반지름 개념이 없어서(RD는 구름 테두리용 여백이지 반지름이
// 아니다), rounded 사각형은 Polygon 주석의 꼭짓점을 둥근 모서리를 짧은 직선 여러 개로
// 근사한 경로로 채워 흉내 낸다. Polygon도 Square/Circle과 똑같이 interior_color/color/
// border_width/opacity를 그대로 지원해서(annot.update()가 알아서 채워 그려준다), 별도
// appearance stream을 직접 만들 필요가 없다.
//
// Square/Circle은 MuPDF가 appearance를 만들 때 테두리 중심선을 Rect보다 half-width만큼
// 안쪽으로 자동으로 들여서, 스트로크 바깥쪽 경계가 정확히 Rect(=사용자가 드래그한 크기)에
// 맞도록 보정해준다(pdf-appearance.c의 "No part of what we draw should extend outside of
// Rect" 규칙). 반면 Polygon은 그런 보정이 없어 주어진 꼭짓점을 그대로 스트로크 중심선으로
// 쓰기 때문에, 보정 없이 그리면 테두리가 half-width만큼 바깥으로 삐져나와 드래그한 크기보다
// 도형이 커 보인다. 그래서 여기서도 직접 half-width만큼 안쪽으로 들여서 계산한다.
fn rounded_rect_vertices(x0: f32, y0: f32, x1: f32, y1: f32, stroke_width: f32) -> Vec<Point> {
    let (x0, x1) = (x0.min(x1), x0.max(x1));
    let (y0, y1) = (y0.min(y1), y0.max(y1));
    let inset = (stroke_width / 2.0).max(0.0).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
    let (x0, x1) = (x0 + inset, x1 - inset);
    let (y0, y1) = (y0 + inset, y1 - inset);
    let (w, h) = (x1 - x0, y1 - y0);
    let r = (0.15 * w.min(h)).clamp(0.0, 16.0).min(w / 2.0).min(h / 2.0);
    const SEGS: i32 = 8;
    // (중심 x, 중심 y, 시작각, 끝각) — 사각형 둘레를 따라 시계 방향으로 네 모서리
    let corners = [
        (x0 + r, y0 + r, 180.0_f32, 270.0_f32),
        (x1 - r, y0 + r, 270.0, 360.0),
        (x1 - r, y1 - r, 0.0, 90.0),
        (x0 + r, y1 - r, 90.0, 180.0),
    ];
    let mut pts = Vec::with_capacity(corners.len() * (SEGS as usize + 1));
    for (cx, cy, a0, a1) in corners {
        for i in 0..=SEGS {
            let t = (a0 + (a1 - a0) * (i as f32 / SEGS as f32)).to_radians();
            pts.push(Point { x: cx + r * t.cos(), y: cy + r * t.sin() });
        }
    }
    pts
}

// 선택한 글자들의 [x0,y0,x1,y1] 사각형(들)에 지정한 색(RGB, 0.0~1.0)의 형광펜 하이라이트
// 주석을 추가하고 원본 파일에 그대로 저장한다. apply_pdf_password와 같은 패턴 — 임시 파일에 먼저 쓰고 rename으로
// 교체해서 저장 도중 실패해도 원본이 안 깨지고, 성공/실패 무관하게 파일을 다시 열어 tab.doc를
// 채운다(annotation.update() 이후 appearance stream까지 반영된 상태를 그대로 보여주기 위함).
#[tauri::command]
fn add_highlight(
    state: State<Mutex<AppState>>,
    page_num: u32,
    rects: Vec<[f32; 4]>,
    color: [f32; 3],
    opacity: f32,
) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };
    if !doc.is_pdf() {
        state.lock().unwrap().tab_mut().doc = Some(doc);
        return Err("PDF 문서에만 하이라이트를 추가할 수 있습니다".to_string());
    }

    // 디스크엔 아직 안 쓴다 — :w로 명시적으로 저장하기 전까진 화면에만 보이는 상태로 둔다.
    // PdfDocument는 Document를 감싸고 있을 뿐 같은 fz_document를 참조하므로, Deref로 얻은
    // &Document를 clone()하면(참조 카운트만 올림) annotation.update()로 이미 반영된 내용을
    // 그대로 들고 있는 Document 핸들을 파일을 거치지 않고 바로 얻을 수 있다.
    let result: Result<Document, String> = (|| {
        let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let mut page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;

        // 같은/겹치는 자리를 두 번 하이라이트하면 주석 두 개가 겹쳐 보여(색이 진해지거나
        // 두 개로 보임) 지저분해진다 — 새로 칠할 사각형과 겹치는 기존 하이라이트는 지우고
        // 그 사각형들을 새 사각형과 합쳐 하나의 주석으로 다시 만든다.
        let mut merged_rects = rects.clone();
        let mut overlapping_xrefs = Vec::new();
        for old in page.annotations() {
            if !matches!(old.r#type(), Ok(PdfAnnotationType::Highlight)) {
                continue;
            }
            let Ok(quads) = old.quad_points() else { continue };
            let old_rects: Vec<[f32; 4]> = quads.iter().map(|q| [q.ul.x, q.ul.y, q.lr.x, q.lr.y]).collect();
            if old_rects.iter().any(|o| merged_rects.iter().any(|n| rects_overlap(*o, *n))) {
                if let Ok(xref) = old.xref() {
                    overlapping_xrefs.push(xref);
                    merged_rects.extend(old_rects);
                }
            }
        }
        for xref in overlapping_xrefs {
            if let Some(old) = find_annot_by_xref(&page, xref) {
                page.delete_annotation(old).map_err(|e| e.to_string())?;
            }
        }
        let merged_rects = merge_overlapping_rects(merged_rects);

        let mut annot = page.create_annotation(PdfAnnotationType::Highlight).map_err(|e| e.to_string())?;
        let quads: Vec<Quad> = merged_rects.iter().map(|r| rect_to_quad(*r)).collect();
        annot.set_quad_points(quads).map_err(|e| e.to_string())?;
        annot.set_color(AnnotationColor::Rgb { red: color[0], green: color[1], blue: color[2] }).map_err(|e| e.to_string())?;
        annot.set_opacity(opacity).map_err(|e| e.to_string())?;
        annot.update().map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            tab.annotation_undo_stack.push((page_num, PdfAnnotationType::Highlight));
            Ok(())
        }
        Err(e) => {
            // 중간에 실패하면 메모리 상태가 어중간할 수 있으니, 마지막으로 저장된 디스크
            // 상태에서 다시 열어 탭을 복구한다 — 디스크에 없던 하이라이트는(이번 것 포함해서
            // 이전에 저장 안 하고 쌓아둔 것까지) 전부 사라지므로, undo 스택도 같이 비워야
            // undo_annotation이 이미 없는 항목을 지우려 들지 않는다.
            tab.dirty = false;
            tab.annotation_undo_stack.clear();
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// 선/사각형/rounded 사각형/타원(ellipse) 도형 주석 추가 — add_highlight와 완전히 같은 패턴
// (디스크엔 안 쓰고 Document::clone()으로 메모리에만 반영, 실패하면 디스크 상태로 복구).
// kind: "line" | "rect" | "rounded_rect" | "ellipse". rounded_rect는 PDF Square에 모서리
// 반지름 개념이 없어 Polygon 근사(rounded_rect_vertices)로 그린다. PDF 주석 스펙상 도형
// 하나에 불투명도(/CA)는 하나뿐이라 선/내부 불투명도를 따로 둘 수 없어 opacity 하나를 공유한다.
#[tauri::command]
fn add_shape(
    state: State<Mutex<AppState>>,
    page_num: u32,
    kind: String,
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    stroke_color: [f32; 3],
    fill_color: [f32; 3],
    stroke_width: f32,
    opacity: f32,
) -> Result<(), String> {
    let annot_type = match kind.as_str() {
        "line" => PdfAnnotationType::Line,
        "rect" => PdfAnnotationType::Square,
        "rounded_rect" => PdfAnnotationType::Polygon,
        "ellipse" => PdfAnnotationType::Circle,
        other => return Err(format!("알 수 없는 도형 종류: {other}")),
    };

    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };
    if !doc.is_pdf() {
        state.lock().unwrap().tab_mut().doc = Some(doc);
        return Err("PDF 문서에만 도형을 추가할 수 있습니다".to_string());
    }

    let result: Result<Document, String> = (|| {
        let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let mut page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut annot = page.create_annotation(annot_type).map_err(|e| e.to_string())?;
        match annot_type {
            PdfAnnotationType::Line => {
                annot.set_line(Point { x: x0, y: y0 }, Point { x: x1, y: y1 }).map_err(|e| e.to_string())?;
            }
            PdfAnnotationType::Polygon => {
                annot.set_vertices(rounded_rect_vertices(x0, y0, x1, y1, stroke_width)).map_err(|e| e.to_string())?;
                annot.set_interior_color(AnnotationColor::Rgb { red: fill_color[0], green: fill_color[1], blue: fill_color[2] }).map_err(|e| e.to_string())?;
            }
            _ => {
                // Square/Circle는 스트로크 중심선이 여기서 넘기는 Rect 그대로라, Rect를
                // 드래그한 크기 그대로 넘기면 테두리가 선 두께 절반만큼 바깥으로 삐져나와
                // 드래그한 크기보다 도형이 커 보인다(실제 렌더 픽셀로 직접 확인함). 그래서
                // rounded_rect_vertices와 똑같이 절반만큼 안쪽으로 들인 Rect를 넘긴다 —
                // 그래야 테두리 바깥쪽 경계가 드래그한 크기에 정확히 맞는다.
                let (rx0, rx1) = (x0.min(x1), x0.max(x1));
                let (ry0, ry1) = (y0.min(y1), y0.max(y1));
                let inset = (stroke_width / 2.0).max(0.0).min((rx1 - rx0) / 2.0).min((ry1 - ry0) / 2.0);
                let rect = Rect::new(rx0 + inset, ry0 + inset, rx1 - inset, ry1 - inset);
                annot.set_rect(rect).map_err(|e| e.to_string())?;
                annot.set_interior_color(AnnotationColor::Rgb { red: fill_color[0], green: fill_color[1], blue: fill_color[2] }).map_err(|e| e.to_string())?;
            }
        }
        annot.set_color(AnnotationColor::Rgb { red: stroke_color[0], green: stroke_color[1], blue: stroke_color[2] }).map_err(|e| e.to_string())?;
        annot.set_border_width(stroke_width).map_err(|e| e.to_string())?;
        annot.set_opacity(opacity).map_err(|e| e.to_string())?;
        annot.update().map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            tab.annotation_undo_stack.push((page_num, annot_type));
            Ok(())
        }
        Err(e) => {
            tab.dirty = false;
            tab.annotation_undo_stack.clear();
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// MuPDF가 Text 주석에 기본으로 그려주는 아이콘(annot.set_icon_name("Comment") 등)은
// 검은 테두리 정사각형 틀에 검은 말풍선을 박아넣는 고정 디자인이라("1 w\n0.5 0.5 15 15 re\nb")
// 색을 바꿔도 테두리는 항상 검은색이고 투박해 보인다. 그래서 기본 아이콘 appearance를 우리가
// 만든 얇은 색칠된 원 하나로 직접 덮어써서 더 가볍고 깔끔하게 보이게 한다. annot.update()가
// 먼저 기본 appearance를 만들고 나면 그 뒤에 /AP /N을 우리 Form XObject로 바꿔치기한다.
fn set_flat_note_icon(pdf: &mut PdfDocument, annot: &mut mupdf::pdf::PdfAnnotation, rect: [f32; 4], color: [f32; 3]) -> Result<(), mupdf::Error> {
    let [x0, y0, x1, y1] = rect;
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let radius = ((x1 - x0).min(y1 - y0) / 2.0 - 1.0).max(1.0);
    const K: f32 = 0.5523; // 원을 4개의 3차 베지어로 근사할 때 쓰는 상수 — MuPDF의 draw_circle과 동일
    let (top, bottom, left, right) = (cy + radius, cy - radius, cx - radius, cx + radius);
    let (kx, ky) = (K * radius, K * radius);
    let mut content = format!(
        "{r:.3} {g:.3} {b:.3} rg\n\
         {cx:.2} {top:.2} m\n\
         {cx1:.2} {top:.2} {right:.2} {cy1:.2} {right:.2} {cy:.2} c\n\
         {right:.2} {cy2:.2} {cx1:.2} {bottom:.2} {cx:.2} {bottom:.2} c\n\
         {cx2:.2} {bottom:.2} {left:.2} {cy2:.2} {left:.2} {cy:.2} c\n\
         {left:.2} {cy1:.2} {cx2:.2} {top:.2} {cx:.2} {top:.2} c\n\
         h\nf\n",
        r = color[0], g = color[1], b = color[2],
        cx = cx, cy = cy, top = top, bottom = bottom, left = left, right = right,
        cx1 = cx + kx, cx2 = cx - kx, cy1 = cy + ky, cy2 = cy - ky,
    );
    // 색 원만으로는 "메모"라고 알아보기 어려워서(그냥 점처럼 보임), 툴바 메모 아이콘과 같은
    // 말풍선+꼬리 모양을 흰색으로 원 안에 겹쳐 그린다. 직선만으로 이루어진 다각형이라(곡선
    // 없음) 이 작은 크기에서도 안정적으로 또렷하게 보인다. 좌표는 24x24 기준 툴바 아이콘
    // 경로(M4 4h16v12H10l-4 4v-4H4z)를 중심(12,10) 기준 상대좌표로 바꿔 반지름에 비례해
    // 그대로 축소한 것 — 두 아이콘이 같은 모양을 쓰게 맞췄다.
    let k = radius * 0.078125; // (목표 말풍선 반너비 radius*0.625) / (원본 아이콘 반너비 8)
    let pt = |dx: f32, dy: f32| (cx + k * dx, cy + k * dy);
    let pts = [
        pt(-8.0, -6.0),
        pt(8.0, -6.0),
        pt(8.0, 6.0),
        pt(-2.0, 6.0),
        pt(-6.0, 10.0),
        pt(-6.0, 6.0),
        pt(-8.0, 6.0),
    ];
    content.push_str("1 1 1 rg\n");
    for (i, (px, py)) in pts.iter().enumerate() {
        content.push_str(&format!("{px:.2} {py:.2} {}\n", if i == 0 { "m" } else { "l" }));
    }
    content.push_str("h\nf\n");
    let buf: mupdf::Buffer = content.as_str().try_into()?;

    let mut form = pdf.new_dict()?;
    form.dict_put("Type", pdf.new_name("XObject")?)?;
    form.dict_put("Subtype", pdf.new_name("Form")?)?;
    let mut bbox = pdf.new_array()?;
    for v in [x0, y0, x1, y1] {
        bbox.array_push(pdf.new_real(v)?)?;
    }
    form.dict_put("BBox", bbox)?;
    form.dict_put("Resources", pdf.new_dict()?)?;
    let stream_ref = pdf.add_stream(&buf, Some(&form), false)?;

    let mut ap = pdf.new_dict()?;
    ap.dict_put("N", stream_ref)?;
    let mut obj = annot.object();
    obj.dict_put("AP", ap)?;
    Ok(())
}

// 메모(PDF의 "Text" 주석 — 클릭하면 팝업으로 내용이 뜨는 스티키 노트 아이콘) 추가.
// add_highlight/add_shape와 완전히 같은 패턴(디스크엔 안 쓰고 Document::clone()으로 메모리에만
// 반영, 실패하면 디스크 상태로 복구). 팝업 내용을 담는 별도 Popup 주석은 만들지 않는다 —
// Text 타입 자체가 markup 주석이라 뷰어가 /Contents를 자동으로 팝업에 띄워준다.
#[tauri::command]
fn add_note(
    state: State<Mutex<AppState>>,
    page_num: u32,
    x: f32,
    y: f32,
    text: String,
    color: [f32; 3],
) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };
    if !doc.is_pdf() {
        state.lock().unwrap().tab_mut().doc = Some(doc);
        return Err("PDF 문서에만 메모를 추가할 수 있습니다".to_string());
    }

    let result: Result<Document, String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let mut page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut annot = page.create_annotation(PdfAnnotationType::Text).map_err(|e| e.to_string())?;
        const ICON_SIZE: f32 = 18.0;
        let rect = [x, y, x + ICON_SIZE, y + ICON_SIZE];
        annot.set_rect(Rect::new(rect[0], rect[1], rect[2], rect[3])).map_err(|e| e.to_string())?;
        annot.set_icon_name("Comment").map_err(|e| e.to_string())?;
        annot.set_contents(&text).map_err(|e| e.to_string())?;
        annot.set_color(AnnotationColor::Rgb { red: color[0], green: color[1], blue: color[2] }).map_err(|e| e.to_string())?;
        annot.update().map_err(|e| e.to_string())?;
        set_flat_note_icon(&mut pdf, &mut annot, rect, color).map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            tab.annotation_undo_stack.push((page_num, PdfAnnotationType::Text));
            Ok(())
        }
        Err(e) => {
            tab.dirty = false;
            tab.annotation_undo_stack.clear();
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

#[derive(serde::Serialize)]
struct NoteInfo {
    xref: i32,
    rect: [f32; 4],
    contents: String,
}

// 현재 페이지의 메모(Text 주석) 목록을 위치/내용과 함께 돌려준다 — 프론트엔드가 클릭 좌표와
// rect를 비교해서 "기존 메모를 클릭했는지"를 판단하는 데 쓴다. xref(PDF 오브젝트 참조 번호)를
// 식별자로 쓴다 — mupdf-rs 주석 API엔 별도 id가 없지만 xref는 문서를 다시 쓰기 전까진
// 안정적으로 같은 주석을 가리킨다. Document::clone()은 참조 카운트만 올릴 뿐 내부 MuPDF
// 컨텍스트는 공유되므로, load_page 등 실제 MuPDF 호출이 끝날 때까지 락(s)을 계속 들고
// 있어야 한다 — 락을 먼저 풀면 render_page 등 다른 스레드의 동시 MuPDF 접근과 경합해
// 네이티브 크래시(annotation appearance 재생성 중 SIGSEGV)로 이어진다.
#[tauri::command]
fn notes_on_page(page: &PdfPage) -> Vec<NoteInfo> {
    page.annotations()
        .filter(|a| matches!(a.r#type(), Ok(PdfAnnotationType::Text)))
        .filter_map(|a| {
            let xref = a.xref().ok()?;
            let r = a.rect().ok()?;
            let contents = a.contents().ok().flatten().unwrap_or_default().to_string();
            Some(NoteInfo { xref, rect: [r.x0, r.y0, r.x1, r.y1], contents })
        })
        .collect()
}

#[tauri::command]
fn get_page_notes(state: State<Mutex<AppState>>, page_num: u32) -> Result<Vec<NoteInfo>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Ok(vec![]);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
    let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    Ok(notes_on_page(&page))
}

#[derive(serde::Serialize)]
struct HighlightInfo {
    xref: i32,
    rect: [f32; 4],
}

// 현재 페이지의 하이라이트 주석 목록 — get_page_notes와 같은 패턴. 프론트엔드가 우클릭 메뉴에서
// "그 자리에 하이라이트가 있는지" 판단해 삭제 메뉴 항목을 넣을지 정하는 데 쓴다.
#[tauri::command]
fn get_page_highlights(state: State<Mutex<AppState>>, page_num: u32) -> Result<Vec<HighlightInfo>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Ok(vec![]);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
    let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    Ok(page
        .annotations()
        .filter(|a| matches!(a.r#type(), Ok(PdfAnnotationType::Highlight)))
        .filter_map(|a| {
            let xref = a.xref().ok()?;
            let r = a.rect().ok()?;
            Some(HighlightInfo { xref, rect: [r.x0, r.y0, r.x1, r.y1] })
        })
        .collect())
}

#[derive(serde::Serialize)]
struct DocNoteInfo {
    page: u32,
    xref: i32,
    rect: [f32; 4],
    contents: String,
}

// 사이드바 "메모" 탭 — 문서 전체 페이지를 돌면서 메모를 한 번에 모아 돌려준다(search_all과
// 같은 목적: 페이지마다 따로 부르지 않고 한 번에 긁어와서 목록으로 보여주기 위함).
#[tauri::command]
fn get_all_notes(state: State<Mutex<AppState>>) -> Result<Vec<DocNoteInfo>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Ok(vec![]);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let count = pdf.page_count().map_err(|e| e.to_string())?;
    let mut notes = Vec::new();
    for p in 0..count {
        let page: mupdf::Page = pdf.load_page(p).map_err(|e| e.to_string())?;
        let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        notes.extend(notes_on_page(&page).into_iter().map(|n| DocNoteInfo {
            page: p as u32,
            xref: n.xref,
            rect: n.rect,
            contents: n.contents,
        }));
    }
    Ok(notes)
}

// extract_image()은 스트림을 원본 그대로(read_raw_stream) 돌려준다 — DCTDecode(JPEG)나
// JPXDecode(JPEG2000)는 그 바이트 자체가 이미 완성된 이미지 파일이라 그대로 저장하면 되지만,
// FlateDecode나 필터 없음(순수 픽셀 나열)은 압축을 풀어도 "raw 픽셀"일 뿐 파일 형식이
// 아니라서 Pixmap을 직접 만들어 PNG로 인코딩해야 한다. 여기선 흔한 경우(8비트,
// Gray/RGB/CMYK)만 지원하고 그 외(인덱스 컬러, 1/16비트 등)는 그 이미지 하나만 건너뛴다.
//
// color_space 필드는 이름("DeviceRGB")이 아니라 "7 0 R" 같은 간접 참조 문자열로 나오는
// 경우가 있어(실제로 겪음 — 이름 매칭이 항상 통하지 않는다) 이름은 안 쓰고, 디코딩된 바이트
// 길이를 픽셀 수로 나눠 채널 수를 역산한다. 8bpc라 나눗셈이 정확히 떨어져야 정상이다.
fn resolve_or_clone(obj: &PdfObject) -> Option<PdfObject> {
    match obj.resolve().ok().flatten() {
        Some(r) => Some(r),
        None => obj.try_clone().ok(),
    }
}

// [/Indexed base hival lookup] 배열이면 (hival+1)*3 바이트짜리 RGB 팔레트로 변환해 돌려준다.
// base가 Gray/RGB/CMYK(또는 그 ICCBased 버전)가 아니거나 파싱에 실패하면 None.
fn indexed_rgb_palette(cs: &PdfObject) -> Option<Vec<u8>> {
    let cs = resolve_or_clone(cs)?;
    if !cs.is_array().ok()? || cs.len().ok()? < 4 {
        return None;
    }
    let family = cs.get_array(0).ok().flatten()?;
    if family.as_name().ok()?.as_slice() != b"Indexed" {
        return None;
    }
    let base = resolve_or_clone(&cs.get_array(1).ok().flatten()?)?;
    let base_n = base_colorspace_channels(&base)?;
    let hival = cs.get_array(2).ok().flatten()?.as_int().ok()?.max(0) as usize;
    // lookup은 resolve하지 않고 그대로 쓴다 — read_stream()/is_stream()은 (스트림인 경우)
    // indirect 참조 자체에 바로 걸어야 스트림 버퍼를 찾는다. resolve()로 한 번 역참조하고 나면
    // (dict/array와 달리) 스트림 연결 정보가 사라져 is_stream()이 false로 뒤집힌다 — 실제로
    // 겪은 mupdf-rs 동작.
    let lookup = cs.get_array(3).ok().flatten()?;
    let raw_lut = if lookup.is_stream().ok()? { lookup.read_stream().ok()? } else { lookup.as_bytes().ok()? };
    if raw_lut.len() < (hival + 1) * base_n {
        return None;
    }
    let mut rgb = Vec::with_capacity((hival + 1) * 3);
    for i in 0..=hival {
        let off = i * base_n;
        match base_n {
            1 => rgb.extend_from_slice(&[raw_lut[off], raw_lut[off], raw_lut[off]]),
            3 => rgb.extend_from_slice(&raw_lut[off..off + 3]),
            4 => {
                let (c, m, y, k) = (raw_lut[off], raw_lut[off + 1], raw_lut[off + 2], raw_lut[off + 3]);
                let conv = |ch: u8| (255 - ch as u16).saturating_sub(k as u16).max(0).min(255) as u8;
                rgb.extend_from_slice(&[conv(c), conv(m), conv(y)]);
            }
            _ => return None,
        }
    }
    Some(rgb)
}

// DeviceGray/RGB/CMYK(이름 또는 ICCBased 스트림의 /N)의 채널 수. Indexed 팔레트의 base
// 색공간을 해석하는 데만 쓴다 — Separation/DeviceN 등 그 외는 지원하지 않는다.
fn base_colorspace_channels(base: &PdfObject) -> Option<usize> {
    if let Ok(name) = base.as_name() {
        return match name.as_slice() {
            b"DeviceGray" | b"CalGray" => Some(1),
            b"DeviceRGB" | b"CalRGB" => Some(3),
            b"DeviceCMYK" => Some(4),
            _ => None,
        };
    }
    if base.is_array().ok()? && base.len().ok()? >= 2 {
        let family = base.get_array(0).ok().flatten()?;
        if family.as_name().ok()?.as_slice() == b"ICCBased" {
            let stream = resolve_or_clone(&base.get_array(1).ok().flatten()?)?;
            return match stream.get_dict("N").ok().flatten()?.as_int().ok()? {
                1 => Some(1),
                3 => Some(3),
                4 => Some(4),
                _ => None,
            };
        }
    }
    None
}

// 1비트/픽셀로 팩킹된 로우 데이터(각 행은 바이트 경계에서 시작 — PDF 이미지 데이터 표준
// 규칙)를 팔레트를 통해 RGB로 풀어낸다. 스캔한 문서(팩스로 압축된 흑백 페이지)가 흔히
// 이 구조(1bpc + 2색 Indexed)를 쓴다.
fn unpack_1bpc_indexed_to_rgb(packed: &[u8], width: u32, height: u32, palette: &[u8]) -> Option<Vec<u8>> {
    let (width, height) = (width as usize, height as usize);
    let bytes_per_row = width.div_ceil(8);
    if packed.len() < bytes_per_row * height {
        return None;
    }
    let max_index = palette.len() / 3;
    if max_index == 0 {
        return None;
    }
    let mut rgb = Vec::with_capacity(width * height * 3);
    for y in 0..height {
        let row = &packed[y * bytes_per_row..(y + 1) * bytes_per_row];
        for x in 0..width {
            let bit = (row[x / 8] >> (7 - (x % 8))) & 1;
            let idx = (bit as usize).min(max_index - 1) * 3;
            rgb.extend_from_slice(&palette[idx..idx + 3]);
        }
    }
    Some(rgb)
}

fn decode_simple_image_to_png(pdf: &PdfDocument, info: &PageImageInfo) -> Result<Vec<u8>, String> {
    let pixel_count = info.width as usize * info.height as usize;
    if pixel_count == 0 {
        return Err("이미지 크기가 0입니다".to_string());
    }
    let obj = pdf.new_indirect(info.xref, 0).map_err(|e| e.to_string())?;
    // extract_image()이 아니라 여기서 새로 읽는 이유: encoded는 raw(미해제) 바이트라 픽셀로 못
    // 쓴다 — read_stream()은 Filter(CCITTFaxDecode 포함)를 전부 풀어서 실제 픽셀 바이트를 준다.
    let raw = obj.read_stream().map_err(|e| e.to_string())?;

    let (channels, cs, decoded): (usize, Colorspace, Vec<u8>) = match info.bits_per_component {
        Some(8) => {
            let channels = raw.len() / pixel_count;
            let cs = match channels {
                1 => Colorspace::device_gray(),
                3 => Colorspace::device_rgb(),
                4 => Colorspace::device_cmyk(),
                _ => return Err(format!("지원하지 않는 채널 수: {channels}")),
            };
            (channels, cs, raw)
        }
        // 스캔한 문서(CCITT 팩스로 압축된 흑백 페이지)가 흔히 이 구조 — 1비트 인덱스 컬러.
        // 실제로 겪은 문제: 이걸 지원 안 해서 스캔 문서 페이지의 이미지가 통째로 빠졌었다.
        Some(1) => {
            let resolved = resolve_or_clone(&obj).ok_or("이미지 오브젝트를 읽을 수 없습니다")?;
            let cs_obj = resolved.get_dict("ColorSpace").map_err(|e| e.to_string())?.ok_or("색공간 정보가 없습니다")?;
            let palette = indexed_rgb_palette(&cs_obj).ok_or("지원하지 않는 1비트 색공간입니다(Indexed RGB/Gray/CMYK만 지원)")?;
            let rgb = unpack_1bpc_indexed_to_rgb(&raw, info.width, info.height, &palette)
                .ok_or("픽셀 데이터 크기가 예상과 다릅니다")?;
            (3, Colorspace::device_rgb(), rgb)
        }
        other => return Err(format!("지원하지 않는 비트 심도: {other:?}")),
    };
    let expected_len = pixel_count * channels;
    if decoded.len() < expected_len {
        return Err("픽셀 데이터 크기가 예상과 다릅니다".to_string());
    }

    // /SMask(알파 채널)를 무시하고 불투명하게 저장하면, 투명 영역 바깥의 정의 안 된 색상
    // 값(로고/아이콘 PNG를 PDF에 심을 때 흔히 검정)이 그대로 드러나 완전히 다른 그림처럼
    // 보인다 — 실제로 로고가 박힌 PDF에서 이 문제로 재현됨. SMask 크기가 원본과 같을 때만
    // 알파로 합성한다.
    let alpha = read_smask_alpha(pdf, &obj, info.width, info.height);
    let mut pixmap =
        Pixmap::new_with_w_h(&cs, info.width as i32, info.height as i32, alpha.is_some()).map_err(|e| e.to_string())?;
    let samples = pixmap.samples_mut();
    match alpha {
        None => samples[..expected_len].copy_from_slice(&decoded[..expected_len]),
        Some(alpha) => {
            // mupdf 픽스맵은 알파가 있으면 프리멀티플라이드로 저장하고, PNG로 쓸 때 자체적으로
            // 언프리멀티플라이한다 — 그래서 여기서 색상 채널에 알파를 미리 곱해 넣어야 한다.
            for px in 0..pixel_count {
                let a = alpha[px] as u16;
                for c in 0..channels {
                    let v = decoded[px * channels + c] as u16;
                    samples[px * (channels + 1) + c] = (v * a / 255) as u8;
                }
                samples[px * (channels + 1) + channels] = alpha[px];
            }
        }
    }
    let mut buf = Vec::new();
    pixmap.write_to(&mut buf, ImageFormat::PNG).map_err(|e| e.to_string())?;
    Ok(buf)
}

// 이미지 오브젝트에 /SMask가 있고 크기·비트심도가 원본과 같으면 그 그레이스케일 스트림을
// 알파 채널로 읽어온다. 없거나 안 맞으면 None — 이 경우 이전처럼 불투명하게 저장한다.
fn read_smask_alpha(pdf: &PdfDocument, obj: &PdfObject, width: u32, height: u32) -> Option<Vec<u8>> {
    let resolved = resolve_or_clone(obj)?;
    let smask = resolved.get_dict("SMask").ok().flatten()?;
    let smask_xref = smask.as_indirect().ok()?;
    let smask_obj = pdf.new_indirect(smask_xref, 0).ok()?;
    let smask_resolved = resolve_or_clone(&smask_obj)?;
    let sw = smask_resolved.get_dict("Width").ok().flatten()?.as_int().ok()? as u32;
    let sh = smask_resolved.get_dict("Height").ok().flatten()?.as_int().ok()? as u32;
    let bpc = smask_resolved.get_dict("BitsPerComponent").ok().flatten().and_then(|v| v.as_int().ok());
    if sw != width || sh != height || bpc != Some(8) {
        return None;
    }
    let data = smask_obj.read_stream().ok()?;
    let expected = width as usize * height as usize;
    if data.len() < expected {
        return None;
    }
    Some(data[..expected].to_vec())
}

// mupdf-rs가 돌려주는 Filter 이름은 PDF 이름 오브젝트 표기 그대로("/DCTDecode")라
// 슬래시가 붙어 있다 — 이 값과 매칭할 땐 항상 슬래시 포함 문자열을 써야 한다.
fn raw_passthrough_ext(filter: Option<&str>) -> Option<&'static str> {
    match filter {
        Some("/DCTDecode") => Some("jpg"),
        Some("/JPXDecode") => Some("jp2"),
        _ => None,
    }
}

// PdfPage::images()(mupdf-rs)는 페이지 자신의 /Resources/XObject 한 단계만 보고, Form
// XObject 안에 중첩된 이미지는 찾지 못한다 — 그런데 여러 PDF 생성기가 재사용 가능한 그래픽을
// Form으로 감싸서 리소스에 넣는다(실제로 겪은 파일에서 이미지 4개 중 3개가 Form 안에 있어서
// 하나만 찾아지는 문제가 있었다). 그래서 페이지 Resources부터 시작해 Form을 재귀적으로 따라
// 들어가며 모든 이미지를 직접 모은다.
fn page_resources(page: &PdfPage) -> Option<PdfObject> {
    let page_obj = page.object();
    match page_obj.get_dict("Resources").ok().flatten() {
        Some(r) if r.is_dict().unwrap_or(false) => Some(r),
        _ => page_obj
            .get_dict_inheritable("Resources")
            .ok()
            .flatten()
            .filter(|r| r.is_dict().unwrap_or(false)),
    }
}

fn image_info_from_resolved(resolved: &PdfObject, xref: i32) -> Option<PageImageInfo> {
    let width = resolved.get_dict("Width").ok().flatten()?.as_int().ok()?.max(0) as u32;
    let height = resolved.get_dict("Height").ok().flatten()?.as_int().ok()?.max(0) as u32;
    let bits_per_component = resolved.get_dict("BitsPerComponent").ok().flatten().and_then(|v| v.as_int().ok());
    let filter = resolved.get_dict("Filter").ok().flatten().map(|v| v.to_string());
    Some(PageImageInfo { name: String::new(), xref, width, height, bits_per_component, color_space: None, filter })
}

fn collect_images_recursive(
    resources: &PdfObject,
    seen: &mut std::collections::HashSet<i32>,
    depth: u32,
    out: &mut Vec<PageImageInfo>,
) {
    if depth > 8 {
        return; // 비정상적으로 깊은/순환 Form 중첩에 대한 방어
    }
    let Some(xobjects) = resources.get_dict("XObject").ok().flatten() else { return };
    let Ok(len) = xobjects.dict_len() else { return };
    for idx in 0..len as i32 {
        let Some(val) = xobjects.get_dict_val(idx).ok().flatten() else { continue };
        let xref = val.as_indirect().unwrap_or(0);
        // 이미 처리한 오브젝트는 건너뛴다 — 여러 Form이 같은 이미지를 공유하거나(중복 추출
        // 방지) Form이 순환 참조하는 경우(무한 재귀 방지) 둘 다 막아준다.
        if xref != 0 && !seen.insert(xref) {
            continue;
        }
        let Some(resolved) = val.resolve().ok().flatten() else { continue };
        let Some(subtype) = resolved.get_dict("Subtype").ok().flatten() else { continue };
        let Ok(subtype_name) = subtype.as_name() else { continue };
        match subtype_name.as_slice() {
            b"Image" => {
                if let Some(info) = image_info_from_resolved(&resolved, xref) {
                    out.push(info);
                }
            }
            b"Form" => {
                if let Some(sub_res) = resolved.get_dict("Resources").ok().flatten() {
                    collect_images_recursive(&sub_res, seen, depth + 1, out);
                }
            }
            _ => {}
        }
    }
}

fn page_images(page: &PdfPage) -> Vec<PageImageInfo> {
    let Some(resources) = page_resources(page) else { return Vec::new() };
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    collect_images_recursive(&resources, &mut seen, 0, &mut out);
    out
}

// 이미지가 없는 페이지에서 :images를 실행했을 때 폴더 선택창부터 띄우고 나서야 "이미지
// 없음"을 알리면 사용자가 괜히 폴더를 고르게 된다 — 프론트엔드가 폴더 선택창을 띄우기 전에
// 먼저 이걸로 개수를 확인한다.
#[tauri::command]
fn page_image_count(state: State<Mutex<AppState>>, page_num: u32) -> Result<u32, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Ok(0);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
    let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    Ok(page_images(&page).len() as u32)
}

// 현재 페이지에 박혀 있는 이미지를 사용자가 고른 폴더에 파일로 저장한다. 저장한 개수를
// 돌려준다(0이면 프론트에서 "이미지 없음"으로 안내). 실패한 이미지 하나 때문에 나머지까지
// 못 뽑으면 안 되니, 개별 이미지 실패는 건너뛰고 계속 진행한다.
#[tauri::command]
fn export_page_images(state: State<Mutex<AppState>>, page_num: u32, dir: String) -> Result<u32, String> {
    let s = state.lock().unwrap();
    let tab = s.tab();
    // 파일명만("page15_image1.png")으로 저장하면 다른 문서의 같은 페이지 번호를 같은 폴더에
    // 뽑을 때 서로 덮어쓴다 — 문서 파일명(확장자 제외)을 앞에 붙여 구분한다.
    let doc_stem = tab.cached_file_name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&tab.cached_file_name).to_string();
    let doc = tab.doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Err("PDF 문서에서만 이미지를 추출할 수 있습니다".to_string());
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
    let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let infos = page_images(&page);

    let mut saved = 0u32;
    for (i, info) in infos.iter().enumerate() {
        let extracted = match pdf.extract_image(info.xref) {
            Ok(img) => img,
            Err(_) => continue,
        };
        let (ext, bytes): (&str, Vec<u8>) = match raw_passthrough_ext(extracted.filter.as_deref()) {
            Some(ext) => (ext, extracted.encoded),
            None => match decode_simple_image_to_png(&pdf, info) {
                Ok(png) => ("png", png),
                Err(_) => continue, // 지원 안 하는 색공간/비트심도 — 이 이미지만 건너뜀
            },
        };
        let base = format!("{doc_stem}_page{}_image{}", page_num + 1, i + 1);
        let out_path = unique_path(&dir, &base, ext);
        if std::fs::write(&out_path, &bytes).is_ok() {
            saved += 1;
        }
    }
    Ok(saved)
}

// 같은 이름의 파일이 이미 있으면 덮어쓰지 않고 "(1)", "(2)"... 를 붙여 새 이름을 찾는다.
fn unique_path(dir: &str, base: &str, ext: &str) -> String {
    let plain = format!("{dir}/{base}.{ext}");
    if !std::path::Path::new(&plain).exists() {
        return plain;
    }
    let mut n = 1u32;
    loop {
        let candidate = format!("{dir}/{base}({n}).{ext}");
        if !std::path::Path::new(&candidate).exists() {
            return candidate;
        }
        n += 1;
    }
}

#[cfg(test)]
mod page_image_export_tests {
    use super::{decode_simple_image_to_png, raw_passthrough_ext, PdfDocument, Pixmap};
    use mupdf::pdf::{InsertImageOptions, PageImageSource};
    use mupdf::{Colorspace, Pixel, Rect};

    #[test]
    fn raw_passthrough_ext_matches_slash_prefixed_filter_names() {
        assert_eq!(raw_passthrough_ext(Some("/DCTDecode")), Some("jpg"));
        assert_eq!(raw_passthrough_ext(Some("/JPXDecode")), Some("jp2"));
        assert_eq!(raw_passthrough_ext(Some("DCTDecode")), None);
        assert_eq!(raw_passthrough_ext(Some("/FlateDecode")), None);
        assert_eq!(raw_passthrough_ext(None), None);
    }

    fn solid_pixmap(w: i32, h: i32, r: u8, g: u8, b: u8) -> Pixmap {
        let mut pixmap = Pixmap::new_with_w_h(&Colorspace::device_rgb(), w, h, false).unwrap();
        for chunk in pixmap.samples_mut().chunks_exact_mut(3) {
            chunk.copy_from_slice(&[r, g, b]);
        }
        pixmap
    }

    #[test]
    fn decodes_uncompressed_rgb_image_back_to_matching_pixels() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let src = solid_pixmap(4, 3, 10, 200, 30);

        let placement = page
            .insert_image(&mut doc, Rect::new(0.0, 0.0, 40.0, 30.0), PageImageSource::Pixmap(&src), InsertImageOptions::default())
            .unwrap();
        let infos = page.images().unwrap();
        let info = infos.iter().find(|i| i.xref == placement.xref).unwrap();

        // 이 테스트가 실제로 확인하려는 경로(FlateDecode/무필터 raw 픽셀)를 타는지 먼저 확인 —
        // 필터가 DCTDecode/JPXDecode였다면 애초에 decode_simple_image_to_png를 안 거친다.
        let extracted = doc.extract_image(placement.xref).unwrap();
        assert_ne!(extracted.filter.as_deref(), Some("/DCTDecode"));
        assert_ne!(extracted.filter.as_deref(), Some("/JPXDecode"));

        let png_bytes = decode_simple_image_to_png(&doc, info).unwrap();
        let decoded = mupdf::Image::from_bytes(&png_bytes).unwrap().to_pixmap().unwrap();
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 3);
        assert_eq!(decoded.pixel(0, 0).unwrap(), Pixel::rgb(10, 200, 30));
        assert_eq!(decoded.pixel(3, 2).unwrap(), Pixel::rgb(10, 200, 30));
    }

    #[test]
    fn composites_smask_alpha_instead_of_ignoring_it() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        // 1x2 RGBA: 완전 불투명 빨강, 절반 투명 초록(프리멀티플라이드로 저장) — insert_image가
        // 알파 있는 Pixmap을 넣으면 mupdf가 알아서 베이스 이미지 + 별도 /SMask 오브젝트로
        // 나눠 저장한다(실제 PDF의 로고/아이콘 이미지가 이렇게 저장되는 것과 같은 구조).
        let mut src = Pixmap::new_with_w_h(&Colorspace::device_rgb(), 1, 2, true).unwrap();
        src.samples_mut()[0..4].copy_from_slice(&[255, 0, 0, 255]);
        src.samples_mut()[4..8].copy_from_slice(&[0, 128, 0, 128]);

        let placement = page
            .insert_image(&mut doc, Rect::new(0.0, 0.0, 10.0, 20.0), PageImageSource::Pixmap(&src), InsertImageOptions::default())
            .unwrap();
        let infos = page.images().unwrap();
        let info = infos.iter().find(|i| i.xref == placement.xref).unwrap();

        let png_bytes = decode_simple_image_to_png(&doc, info).unwrap();
        let decoded = mupdf::Image::from_bytes(&png_bytes).unwrap().to_pixmap().unwrap();
        assert!(decoded.alpha(), "SMask가 있으면 결과 PNG도 알파 채널을 가져야 한다");

        let opaque = decoded.pixel(0, 0).unwrap();
        assert_eq!(opaque.components()[3], 255, "완전 불투명 픽셀의 알파는 그대로 255여야 한다");
        assert!(opaque.components()[0] > 200 && opaque.components()[1] < 50, "빨강이 유지돼야 한다(다른 채널로 안 섞임)");

        let translucent = decoded.pixel(0, 1).unwrap();
        assert!(
            (100..160).contains(&translucent.components()[3]),
            "SMask의 절반 투명 값이 반영돼야 한다 (got {})",
            translucent.components()[3]
        );
        assert!(translucent.components()[1] > 50, "초록 색상이 검게 뭉개지면 안 된다(알파 무시 시 나던 버그)");
    }

    #[test]
    fn rejects_unsupported_bit_depth() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let src = solid_pixmap(2, 2, 0, 0, 0);
        let placement = page
            .insert_image(&mut doc, Rect::new(0.0, 0.0, 20.0, 20.0), PageImageSource::Pixmap(&src), InsertImageOptions::default())
            .unwrap();
        let mut infos = page.images().unwrap();
        let info = infos.iter_mut().find(|i| i.xref == placement.xref).unwrap();
        info.bits_per_component = Some(16); // 실제로는 8비트지만 지원 안 하는 값으로 흉내
        assert!(decode_simple_image_to_png(&doc, info).is_err());
    }

    #[test]
    fn finds_images_nested_inside_form_xobjects() {
        use super::{page_images, PdfObject};

        let mut doc = PdfDocument::new();
        let page = doc.new_page((200.0, 200.0)).unwrap();
        // 실제로 겪은 문제 — PDF 생성기가 이미지를 페이지 리소스에 직접 두지 않고 재사용 가능한
        // Form XObject로 감싸서 넣는 경우가 있다. 그러면 이미지는 페이지 자신의
        // /Resources/XObject가 아니라 Form의 /Resources/XObject 안에 있다. 여기서 이 구조를
        // 그대로 손으로 만들어서 page_images()가 한 단계 더 들어가 찾아내는지 확인한다.
        let src = solid_pixmap(2, 2, 10, 20, 30);
        let mut scratch_page = doc.new_page((10.0, 10.0)).unwrap();
        let placement = scratch_page
            .insert_image(&mut doc, Rect::new(0.0, 0.0, 2.0, 2.0), PageImageSource::Pixmap(&src), InsertImageOptions::default())
            .unwrap();

        let image_obj = doc.new_indirect(placement.xref, 0).unwrap();
        let mut form_xobjects = doc.new_dict().unwrap();
        form_xobjects.dict_put("Im0", image_obj).unwrap();
        let mut form_resources = doc.new_dict().unwrap();
        form_resources.dict_put("XObject", form_xobjects).unwrap();

        let mut form = doc.new_dict().unwrap();
        form.dict_put("Type", doc.new_name("XObject").unwrap()).unwrap();
        form.dict_put("Subtype", doc.new_name("Form").unwrap()).unwrap();
        form.dict_put("Resources", form_resources).unwrap();
        let mut bbox = doc.new_array().unwrap();
        for v in [0.0, 0.0, 10.0, 10.0] {
            bbox.array_push(doc.new_real(v).unwrap()).unwrap();
        }
        form.dict_put("BBox", bbox).unwrap();
        let empty_buf: mupdf::Buffer = "".try_into().unwrap();
        let form_ref = doc.add_stream(&empty_buf, Some(&form), false).unwrap();

        let mut page_resources = page.resources().unwrap();
        let mut page_xobjects: PdfObject =
            page_resources.get_dict("XObject").unwrap().unwrap_or_else(|| doc.new_dict().unwrap());
        page_xobjects.dict_put("Fm0", form_ref).unwrap();
        page_resources.dict_put("XObject", page_xobjects).unwrap();

        let found = page_images(&page);
        assert_eq!(found.len(), 1, "Form 안에 중첩된 이미지도 찾아야 한다");
        assert_eq!(found[0].xref, placement.xref);
        assert_eq!(found[0].width, 2);
        assert_eq!(found[0].height, 2);
    }

    #[test]
    fn unpacks_1bpc_indexed_rows_byte_aligned_per_row() {
        // 3px 폭 * 2행 — 폭이 8의 배수가 아니라 각 행이 바이트 경계에서 시작해야 한다는 PDF
        // 규칙(패딩 비트 존재)을 실제로 검증한다. 팔레트: 0=흰색, 1=검정.
        let palette = [255u8, 255, 255, 0, 0, 0];
        // 1행: 1 0 1 (+ 패딩 5비트) = 0b10100000 = 0xA0. 2행: 0 1 0 (+패딩) = 0b01000000 = 0x40.
        let packed = [0xA0u8, 0x40];
        let rgb = super::unpack_1bpc_indexed_to_rgb(&packed, 3, 2, &palette).unwrap();
        assert_eq!(
            rgb,
            vec![
                0, 0, 0, 255, 255, 255, 0, 0, 0, // row 0: 검 흰 검
                255, 255, 255, 0, 0, 0, 255, 255, 255, // row 1: 흰 검 흰
            ]
        );
    }

    #[test]
    fn parses_indexed_rgb_palette_from_stream_lookup() {
        // 실제 스캔 문서(CCITT 팩스 압축 흑백 페이지)에서 흔한 형태 —
        // [/Indexed /DeviceRGB 1 <lookup>], lookup은 흰/검 2개 항목. 팔레트 바이트에 0x00이
        // 섞여 있어 PDF 문자열(new_string)로는 못 넣으므로(널 바이트 문제) 스트림으로 심는다.
        let mut doc = PdfDocument::new();
        let lut_bytes: [u8; 6] = [0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x00];
        let buf: mupdf::Buffer = lut_bytes.as_slice().try_into().unwrap();
        let lookup_ref = doc.add_stream(&buf, None, false).unwrap();

        let mut cs = doc.new_array().unwrap();
        cs.array_push(doc.new_name("Indexed").unwrap()).unwrap();
        cs.array_push(doc.new_name("DeviceRGB").unwrap()).unwrap();
        cs.array_push(doc.new_int(1).unwrap()).unwrap();
        cs.array_push(lookup_ref).unwrap();

        let palette = super::indexed_rgb_palette(&cs).unwrap();
        assert_eq!(palette, vec![255, 255, 255, 0, 0, 0]);
    }
}

fn find_annot_by_xref(page: &PdfPage, xref: i32) -> Option<mupdf::pdf::PdfAnnotation> {
    page.annotations().find(|a| a.xref().ok() == Some(xref))
}

// 기존 메모 내용/색상 수정. add_note와 달리 새 주석을 만드는 게 아니라 xref로 찾은 주석을
// 그대로 고쳐 쓰므로 annotation_undo_stack은 건드리지 않는다(u/⌘Z는 "추가 취소"만 다룬다).
#[tauri::command]
fn update_note(
    state: State<Mutex<AppState>>,
    page_num: u32,
    xref: i32,
    text: String,
    color: [f32; 3],
) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<Document, String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut annot = find_annot_by_xref(&page, xref).ok_or("수정할 메모를 찾지 못했습니다")?;
        annot.set_contents(&text).map_err(|e| e.to_string())?;
        annot.set_color(AnnotationColor::Rgb { red: color[0], green: color[1], blue: color[2] }).map_err(|e| e.to_string())?;
        annot.update().map_err(|e| e.to_string())?;
        let r = annot.rect().map_err(|e| e.to_string())?;
        set_flat_note_icon(&mut pdf, &mut annot, [r.x0, r.y0, r.x1, r.y1], color).map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(())
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// 주석 삭제(메모/하이라이트/도형 공통). xref로 특정 주석 하나만 정확히 지운다(같은 페이지의
// 다른 메모/하이라이트/도형은 안 건드림) — 타입을 안 가리므로 어떤 주석이든 xref만 알면 지울 수 있다.
#[tauri::command]
fn delete_annotation(state: State<Mutex<AppState>>, page_num: u32, xref: i32) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<Document, String> = (|| {
        let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let mut page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let annot = find_annot_by_xref(&page, xref).ok_or("삭제할 주석을 찾지 못했습니다")?;
        page.delete_annotation(annot).map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(())
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// 하이라이트/도형 실행 취소(u, ⌘Z) — annotation_undo_stack에서 가장 최근 (페이지, 타입)을
// 꺼내, 그 페이지에서 그 타입의 마지막 주석(annotations()는 /Annots 배열 순서로 순회하고,
// create_annotation은 항상 끝에 추가하므로 "그 타입 중 마지막"이 곧 가장 최근에 추가한 것)을
// 지운다. 타입까지 짚어서 지우기 때문에 하이라이트와 도형이 같은 페이지에 섞여 있어도 엉뚱한
// 주석을 지우지 않는다. 되돌릴 게 없으면 Ok(None).
#[tauri::command]
fn undo_annotation(state: State<Mutex<AppState>>) -> Result<Option<u32>, String> {
    let (doc, path, page_num, kind) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let Some((page_num, kind)) = tab.annotation_undo_stack.pop() else { return Ok(None) };
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path, page_num, kind)
    };

    let result: Result<Document, String> = (|| {
        let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let mut page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let last = page
            .annotations()
            .filter(|a| matches!(a.r#type(), Ok(t) if t == kind))
            .last()
            .ok_or("되돌릴 주석을 찾지 못했습니다")?;
        page.delete_annotation(last).map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = !tab.annotation_undo_stack.is_empty();
            Ok(Some(page_num))
        }
        Err(e) => {
            // add_highlight/add_shape의 실패 복구와 같은 이유로, 실패하면 디스크 상태로
            // 되돌리고 남은 undo 스택도 더는 신뢰할 수 없으니 비운다.
            tab.dirty = false;
            tab.annotation_undo_stack.clear();
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

fn find_widget_by_xref(page: &PdfPage, xref: i32) -> Option<mupdf::pdf::PdfWidget> {
    page.widgets().find(|w| w.xref().ok() == Some(xref))
}

// 리셋 버튼 액션이 정말 /S /ResetForm인지 확인한다. PDF 버튼의 /A 액션은 ResetForm 외에도
// SubmitForm(필드 값을 원격 서버로 전송)·Launch(외부 프로그램 실행)·JavaScript·URI 등일 수
// 있는데, 그런 액션은 절대 자동 실행하면 안 된다(외부로 데이터가 나가거나 임의 실행이
// 일어남) — 그래서 /S 이름이 정확히 "ResetForm"일 때만 상호작용 가능한 버튼으로 취급하고
// 그 외는 전부 "other"(비활성)로 둔다. /Fields 대상 목록은 안 읽는다(아래 reset_form_fields
// 참고 — 이 리셋은 필드 지정 없이 항상 문서 전체를 초기화하는 것으로 단순화했다).
fn button_action_is_reset_form(widget: &mupdf::pdf::PdfWidget) -> bool {
    let obj = widget.annotation().object();
    let Ok(Some(a)) = obj.get_dict("A") else { return false };
    let Ok(Some(s)) = a.get_dict("S") else { return false };
    matches!(s.as_name(), Ok(name) if name == b"ResetForm")
}

fn widget_kind(widget: &mupdf::pdf::PdfWidget) -> &'static str {
    match widget.r#type() {
        Ok(WidgetType::Text) => "text",
        Ok(WidgetType::Checkbox) => "checkbox",
        Ok(WidgetType::RadioButton) => "radio",
        Ok(WidgetType::Combobox) => "combobox",
        Ok(WidgetType::Listbox) => "listbox",
        Ok(WidgetType::Button) if button_action_is_reset_form(widget) => "reset_button",
        _ => "other",
    }
}

// 체크박스/라디오의 "켜짐" 상태 이름을 구한다. PDF는 필드마다 켜짐 값 이름이 제각각이라
// ("Yes"가 흔하지만 스펙상 강제는 아니고, 라디오는 보통 "0"/"1"처럼 그룹 내 위치로 매김)
// 고정 문자열로 토글할 수 없다 — mupdf C에는 이걸 구해주는 pdf_button_field_on_state가
// 있지만 Rust 바인딩엔 노출이 안 돼 있어, 같은 로직(외관 딕셔너리 /AP의 /N 또는 /D에서
// "Off"가 아닌 키 찾기)을 직접 구현한다.
fn widget_on_value(widget: &mupdf::pdf::PdfWidget) -> Option<String> {
    let obj = widget.annotation().object();
    let ap = obj.get_dict("AP").ok()??;
    for key in ["N", "D"] {
        let Ok(Some(states)) = ap.get_dict(key) else { continue };
        let Ok(iter) = states.dict_iter() else { continue };
        for (k, _) in iter.flatten() {
            if let Ok(name) = k.as_name() {
                let name = String::from_utf8_lossy(&name).into_owned();
                if name != "Off" {
                    return Some(name);
                }
            }
        }
    }
    None
}

// 콤보박스/리스트박스의 선택지(/Opt) — [내보낼 값, 화면 표시용 문자열] 쌍인 경우와 단순
// 문자열인 경우(표시=값) 둘 다 있을 수 있다(스펙). set_value에 넘길 값은 value, 사용자에게
// 보여줄 라벨은 label. Opt는 부모 필드에서 상속될 수 있어 get_dict_inheritable로 찾는다.
#[derive(serde::Serialize, Clone)]
struct ChoiceOption {
    value: String,
    label: String,
}

fn choice_options(widget: &mupdf::pdf::PdfWidget) -> Vec<ChoiceOption> {
    let obj = widget.annotation().object();
    let Ok(Some(opt)) = obj.get_dict_inheritable("Opt") else { return Vec::new() };
    let Ok(iter) = opt.array_iter() else { return Vec::new() };
    iter.flatten()
        .filter_map(|entry| {
            if matches!(entry.is_array(), Ok(true)) {
                let value = entry.get_array(0).ok().flatten().and_then(|o| o.as_string().ok())?;
                let label = entry.get_array(1).ok().flatten().and_then(|o| o.as_string().ok()).unwrap_or_else(|| value.clone());
                Some(ChoiceOption { value, label })
            } else {
                entry.as_string().ok().map(|s| ChoiceOption { value: s.clone(), label: s })
            }
        })
        .collect()
}

// 텍스트 필드의 최대 길이(/MaxLen, 부모 필드에서 상속될 수 있음). COMB 플래그와 같이 쓰이면
// "네모 칸 하나에 글자 하나씩"(comb) 서식이 되는데, mupdf Rust 바인딩엔 pdf_text_widget_max_len이
// 안 나와 있어 choice_options와 같은 방식으로 raw 딕셔너리를 직접 읽는다.
fn field_max_len(widget: &mupdf::pdf::PdfWidget) -> Option<i32> {
    let obj = widget.annotation().object();
    obj.get_dict_inheritable("MaxLen").ok().flatten()?.as_int().ok()
}

#[derive(serde::Serialize)]
struct FormFieldInfo {
    xref: i32,
    kind: String,
    name: Option<String>,
    value: Option<String>,
    rect: [f32; 4],
    readonly: bool,
    multiline: bool,
    comb: bool,
    max_len: Option<i32>,
    options: Vec<ChoiceOption>,
}

// 현재 페이지의 폼 필드(AcroForm 위젯) 목록 — get_page_highlights와 같은 패턴. 텍스트/체크박스/
// 라디오/콤보박스/리스트박스/리셋 버튼까지 상호작용 가능하게 다루고(서명 등 나머지는
// kind: "other"로만 알려주고 프론트에서 읽기 전용 취급), 클릭 판정에 쓴다. PDF 내장
// JavaScript는 절대 실행하지 않는다 — mupdf의 JS 엔진(mujs)은 opt-in이라 pdf_enable_js를
// 호출하기 전까진 꺼져 있고, 이 파일 어디서도 그걸 호출하지 않는다.
#[tauri::command]
fn get_page_form_fields(state: State<Mutex<AppState>>, page_num: u32) -> Result<Vec<FormFieldInfo>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Ok(vec![]);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
    let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    Ok(page
        .widgets()
        .filter_map(|w| {
            let xref = w.xref().ok()?;
            let r = w.annotation().rect().ok()?;
            let kind = widget_kind(&w);
            let options = if kind == "combobox" || kind == "listbox" { choice_options(&w) } else { Vec::new() };
            let flags = w.field_flags().unwrap_or(FieldFlags::empty());
            Some(FormFieldInfo {
                xref,
                kind: kind.to_string(),
                name: w.name().ok().flatten(),
                value: w.value().ok().flatten(),
                rect: [r.x0, r.y0, r.x1, r.y1],
                readonly: w.is_readonly().unwrap_or(false),
                multiline: flags.contains(FieldFlags::MULTILINE),
                comb: flags.contains(FieldFlags::COMB),
                max_len: field_max_len(&w),
                options,
            })
        })
        .collect())
}

// 텍스트 필드 값 설정 — update_note와 완전히 같은 패턴(새 주석이 아니라 기존 위젯을 xref로
// 찾아 고쳐 쓰므로 annotation_undo_stack은 안 건드림. 값 하나 덮어쓰는 거라 잘못 입력했으면
// 다시 입력하면 되므로 되돌리기는 없다). ignore_trigger_events=true로 필드에 계산/검증/포맷
// 스크립트(JS)가 달려 있어도 절대 실행하지 않는다.
#[tauri::command]
fn set_form_text_value(state: State<Mutex<AppState>>, page_num: u32, xref: i32, value: String) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<Document, String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut widget = find_widget_by_xref(&page, xref).ok_or("필드를 찾지 못했습니다")?;
        widget.set_value(&mut pdf, &value, true).map_err(|e| e.to_string())?;
        widget.update().map_err(|e| e.to_string())?;
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(())
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// 체크박스 토글 — 새 값을 프론트에 돌려줘서 다시 조회하지 않고 바로 반영할 수 있게 한다.
#[tauri::command]
fn toggle_form_checkbox(state: State<Mutex<AppState>>, page_num: u32, xref: i32) -> Result<String, String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<(Document, String), String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut widget = find_widget_by_xref(&page, xref).ok_or("필드를 찾지 못했습니다")?;
        let on_value = widget_on_value(&widget).unwrap_or_else(|| "Yes".to_string());
        let current = widget.value().map_err(|e| e.to_string())?;
        let next = if current.as_deref() == Some(on_value.as_str()) { "Off".to_string() } else { on_value };
        widget.set_value(&mut pdf, &next, true).map_err(|e| e.to_string())?;
        widget.update().map_err(|e| e.to_string())?;
        Ok(((*pdf).clone(), next))
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok((updated, next)) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(next)
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// 라디오 버튼 선택 — 체크박스와 달리 토글이 아니라 "이걸 켠다"만 한다. NO_TOGGLE_TO_OFF
// 플래그가 흔히 붙어 있어(이미 켜진 걸 다시 눌러도 안 꺼짐) 토글 로직 자체가 필요 없다.
//
// set_value는 그룹이 공유하는 필드의 /V는 제대로 바꿔주지만(그래서 값 자체는 항상 맞음),
// 실제로 확인해보니(같은 그룹 안에서 다른 쪽으로 다시 선택하는 경우) 형제 위젯 자신의
// 외관 선택자(/AS)는 안 건드려줘서 이전에 켰던 쪽이 시각적으로 계속 켜진 채로 남는다 —
// 즉 Male을 먼저 켜고 Female을 누르면 값은 Female로 바뀌는데 그림은 둘 다 켜진 것처럼
// 보인다. mupdf가 알아서 정리해줄 거라 기대했지만 그러지 않아서, 같은 필드 이름을 가진
// 나머지 위젯들의 /AS를 직접 "Off"로 되돌린다 — /AP/N 안에 이미 있는 정적 상태라
// update()로 다시 그릴 필요는 없다.
#[tauri::command]
fn select_form_radio(state: State<Mutex<AppState>>, page_num: u32, xref: i32) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<Document, String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let mut widget = find_widget_by_xref(&page, xref).ok_or("필드를 찾지 못했습니다")?;
        let field_name = widget.name().ok().flatten();
        let on_value = widget_on_value(&widget).ok_or("라디오 버튼의 선택 값을 찾지 못했습니다")?;
        widget.set_value(&mut pdf, &on_value, true).map_err(|e| e.to_string())?;
        widget.update().map_err(|e| e.to_string())?;
        if let Some(name) = field_name {
            for sibling in page.widgets() {
                if sibling.xref().ok() == Some(xref) {
                    continue;
                }
                if sibling.name().ok().flatten().as_deref() != Some(name.as_str()) {
                    continue;
                }
                let mut obj = sibling.annotation().object();
                obj.dict_put("AS", PdfObject::new_name("Off").map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            }
        }
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(())
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// "초기화" 버튼 — /Fields 대상 지정은 안 읽고(위 button_action_is_reset_form 주석 참고)
// 항상 문서의 모든 페이지, 모든 필드를 기본값으로 되돌린다. xref로 클릭된 버튼이 실제로
// ResetForm 액션인지 한 번 더 확인한다(방어적으로 — 프론트가 이미 kind로 걸러서 보내지만,
// 다른 종류의 버튼에 대해 이 커맨드가 잘못 호출되는 경우를 대비).
#[tauri::command]
fn reset_form_fields(state: State<Mutex<AppState>>, page_num: u32, xref: i32) -> Result<(), String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<Document, String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let page: mupdf::Page = pdf.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let page: PdfPage = page.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let button = find_widget_by_xref(&page, xref).ok_or("버튼을 찾지 못했습니다")?;
        if !button_action_is_reset_form(&button) {
            return Err("지원하지 않는 버튼입니다".to_string());
        }
        let page_count = pdf.page_count().map_err(|e| e.to_string())?;
        for pn in 0..page_count {
            let p: mupdf::Page = pdf.load_page(pn).map_err(|e| e.to_string())?;
            let p: PdfPage = p.try_into().map_err(|e: mupdf::Error| e.to_string())?;
            for mut w in p.widgets().collect::<Vec<_>>() {
                w.reset(&mut pdf).map_err(|e| e.to_string())?;
                w.update().map_err(|e| e.to_string())?;
            }
        }
        Ok((*pdf).clone())
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok(updated) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(())
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// Producer 필드에 "MuPDF"가 이미 없으면 뒤에 이어 붙인다("; MuPDF") — 원본 도구 정보는
// 보존하면서 이 파일을 실제로 (다시) 쓴 게 MuPDF라는 것도 남긴다(copy_pdf_info와 같은 목적,
// 다만 이쪽은 새 문서로 복사하는 게 아니라 같은 문서를 그대로 고쳐서 저장하는 경로라 직접
// Info 딕셔너리를 손봐야 한다). 이미 붙어 있으면(:w를 여러 번 눌러도) 중복으로 안 붙인다.
fn append_mupdf_to_producer(pdf: &mut PdfDocument) -> Result<(), mupdf::Error> {
    let current = pdf.metadata(MetadataName::Producer).unwrap_or_default();
    if current.contains("MuPDF") {
        return Ok(());
    }
    let new_value = if current.is_empty() { "MuPDF".to_string() } else { format!("{current}; MuPDF") };
    let info = pdf.trailer()?.get_dict("Info")?;
    let mut info = match info {
        Some(info) => info,
        None => {
            let d = pdf.new_dict()?;
            let info_ref = pdf.add_object(&d)?;
            pdf.trailer()?.dict_put("Info", info_ref)?;
            d
        }
    };
    info.dict_put("Producer", PdfObject::new_string(&new_value)?)?;
    Ok(())
}

// r/R로 돌린 화면 회전은 지금까지 뷰 전용이었다(문서 자체는 안 바뀜) — 저장할 때는 그 값을
// 각 페이지의 기존 회전에 "더해서" 실제로 반영한다. r/R은 "지금 보고 있는 페이지"만 돌리므로
// (문서 전체가 아니라) 페이지마다 회전값이 다를 수 있어 (페이지 번호, 회전) 목록을 받는다.
// 절대값으로 덮어쓰지 않고 기존 회전에 델타로 더해야, 페이지가 원래 갖고 있던 회전(예: 스캔
// 원본이 이미 90도였던 경우)과 합쳐져 렌더링에서 보던 모습과 저장 결과가 일치한다.
fn apply_view_rotation(pdf: &mut PdfDocument, page_rotations: &[(u32, i32)]) -> Result<(), mupdf::Error> {
    for &(page_num, rotation) in page_rotations {
        if rotation == 0 {
            continue;
        }
        let page: mupdf::Page = pdf.load_page(page_num as i32)?;
        let mut page: PdfPage = page.try_into()?;
        let current = page.rotation()?;
        page.set_rotation((current + rotation).rem_euclid(360))?;
    }
    Ok(())
}

// 하이라이트 등 메모리에만 있던 변경을 원본 파일에 실제로 저장한다(vim의 :w와 같은 자리).
// apply_pdf_password와 같은 안전한 저장 패턴(임시 파일 + rename)을 쓴다.
#[tauri::command]
// idx를 안 주면 현재 탭에 저장한다(:w). 저장 안 하고 닫으려는 "다른"(비활성) 탭을 확인창에서
// "저장" 선택했을 때는 idx로 그 탭을 직접 지정한다 — 그 탭으로 전환하지 않고도 저장하기 위함.
// page_rotations: r/R로 돌린 (페이지 번호, 화면 회전(도, 시계방향)) 목록 — 빈 배열이면 아무
// 것도 안 건드린다. 저장 대상이 지금 보고 있는 탭이 아니면(백그라운드 탭 저장) 프론트엔드가
// 항상 빈 배열을 넘긴다 — 그 탭의 화면 회전이 아니므로 적용할 대상이 없다.
fn save_document(state: State<Mutex<AppState>>, idx: Option<usize>, page_rotations: Vec<(u32, i32)>) -> Result<(), String> {
    let (doc, path, target) = {
        let mut s = state.lock().unwrap();
        let target = idx.unwrap_or(s.current);
        if target >= s.tabs.len() {
            return Err("Invalid tab index".to_string());
        }
        let path = s.tabs[target].path.clone().ok_or("No document open")?;
        let doc = s.tabs[target].doc.take().ok_or("No document open")?;
        (doc, path, target)
    };
    if !doc.is_pdf() {
        state.lock().unwrap().tabs[target].doc = Some(doc);
        return Err("PDF 문서만 저장할 수 있습니다".to_string());
    }

    let result: Result<(), String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        append_mupdf_to_producer(&mut pdf).map_err(|e| e.to_string())?;
        apply_view_rotation(&mut pdf, &page_rotations).map_err(|e| e.to_string())?;
        let tmp_path = format!("{path}.vimong-tmp");
        // garbage_level(4): 참조가 끊긴 오브젝트 제거 + 중복 오브젝트 병합(가장 철저한 단계).
        // :w를 여러 번 누를 때마다(하이라이트/도형/메모 추가·삭제 반복) 옛 버전의 오브젝트가
        // 안 쓰이고 남아 파일이 계속 불어나는 걸 막는다. compress로 스트림도 압축한다.
        let mut opts = PdfWriteOptions::default();
        opts.set_garbage_level(4).set_compress(true);
        pdf.save_with_options(&tmp_path, opts).map_err(|e| e.to_string())?;
        drop(pdf);
        std::fs::rename(&tmp_path, &path).map_err(|e| e.to_string())
    })();

    let reopened = Document::open(&path).map_err(|e| e.to_string())?;
    let count = reopened.page_count().unwrap_or(0) as u32;
    let mut s = state.lock().unwrap();
    let tab = &mut s.tabs[target];
    tab.doc = Some(reopened);
    tab.cached_page_count = count;
    // 저장 성공/실패 상관없이 위에서 디스크의 파일을 다시 열어 tab.doc를 교체했으니, 그 안엔
    // 저장 전에 메모리에만 있던 하이라이트가 더 이상 없다 — undo 스택도 같이 비워야 한다.
    tab.annotation_undo_stack.clear();
    if result.is_ok() {
        tab.dirty = false;
    }
    result
}

// ":w 1-3,7-8 out.pdf" 형태의 페이지 스펙(1-indexed, 콤마로 구간 나열)을
// select_pages가 받는 0-indexed 페이지 번호 목록으로 변환한다.
fn parse_page_spec(spec: &str, page_count: usize) -> Result<Vec<usize>, String> {
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let (a, b) = part.split_once('-').unwrap_or((part, part));
        let start: usize = a.parse().map_err(|_| format!("잘못된 페이지 범위: {part}"))?;
        let end: usize = b.parse().map_err(|_| format!("잘못된 페이지 범위: {part}"))?;
        if start == 0 || end < start || end > page_count {
            return Err(format!("페이지 범위가 문서 범위(1-{page_count})를 벗어났습니다: {part}"));
        }
        pages.extend((start - 1)..end);
    }
    Ok(pages)
}

// "~"나 "~/..."는 셸이 대신 풀어주지 않는 명령 인자(:export, :w)로 들어오므로 직접 확장한다.
// 확장 못 하면(HOME/USERPROFILE 없음) 원본 그대로 둬서 이후 상대경로 처리로 넘어가게 한다.
fn expand_tilde(path: &str) -> String {
    let home = || std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE"));
    if path == "~" {
        return home().unwrap_or_else(|_| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = home() {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
}

// 상대경로는 앱 작업 디렉터리가 아니라 지금 열려 있는 PDF의 위치를 기준으로 푼다
fn resolve_out_path(doc_path: &str, out_path: String) -> String {
    let out_path = expand_tilde(&out_path);
    if std::path::Path::new(&out_path).is_absolute() {
        out_path
    } else {
        std::path::Path::new(doc_path)
            .parent()
            .map(|dir| dir.join(&out_path).to_string_lossy().into_owned())
            .unwrap_or(out_path)
    }
}

#[cfg(test)]
mod resolve_out_path_tests {
    use super::resolve_out_path;

    #[test]
    fn filename_only_goes_next_to_doc() {
        assert_eq!(resolve_out_path("/docs/a.pdf", "out.png".into()), "/docs/out.png");
    }

    #[test]
    fn relative_path_resolves_against_doc_dir() {
        assert_eq!(resolve_out_path("/docs/a.pdf", "sub/out.png".into()), "/docs/sub/out.png");
        assert_eq!(resolve_out_path("/docs/a.pdf", "../out.png".into()), "/docs/../out.png");
    }

    #[test]
    fn absolute_path_used_as_is() {
        assert_eq!(resolve_out_path("/docs/a.pdf", "/tmp/out.png".into()), "/tmp/out.png");
    }

    #[test]
    fn tilde_expands_to_home() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        assert_eq!(resolve_out_path("/docs/a.pdf", "~/out.png".into()), format!("{home}/out.png"));
    }
}

// insert_pdf로 페이지만 그래프트하면 문서 정보(Info 딕셔너리 — 제목/저자/생성 프로그램 등)는
// 안 따라온다. dst는 PdfDocument::new()로 만든 빈 문서라 Info 자체가 없으므로, 새로 만들어
// src의 값을 그대로 옮겨 심는다(LuaTeX 등으로 만든 원본의 메타데이터가 export 후 사라지던
// 문제). Vimong 문서 정보 패널엔 Producer 한 줄만 있고 Creator를 따로 보여줄 자리가 없어서,
// Producer를 MuPDF로 통째로 덮어쓰면 원본 도구 정보(예: LuaTeX)가 화면에서 사라진 것처럼
// 보인다 — 그래서 원본 값을 지우지 않고 뒤에 "; MuPDF"를 이어 붙인다. 여러 도구를 거친
// PDF의 Producer가 세미콜론으로 이어지는 건 흔한 관례이고, Artifex가 AGPL 사용자에게
// 권고하는 "retain the producer line"도 이렇게 지킨다.
fn copy_pdf_info(src: &PdfDocument, dst: &mut PdfDocument) -> Result<(), mupdf::Error> {
    use MetadataName::*;
    let fields = [
        ("Title", Title), ("Author", Author), ("Subject", Subject), ("Keywords", Keywords),
        ("Creator", Creator),
        ("CreationDate", CreationDate), ("ModDate", ModDate),
    ];
    let mut info = dst.new_dict()?;
    for (key, name) in fields {
        if let Ok(val) = src.metadata(name) {
            if !val.is_empty() {
                info.dict_put(key, PdfObject::new_string(&val)?)?;
            }
        }
    }
    let producer = src.metadata(Producer).unwrap_or_default();
    let producer = if producer.is_empty() { "MuPDF".to_string() } else { format!("{producer}; MuPDF") };
    info.dict_put("Producer", PdfObject::new_string(&producer)?)?;
    let info_ref = dst.add_object(&info)?;
    dst.trailer()?.dict_put("Info", info_ref)?;
    Ok(())
}

#[cfg(test)]
mod copy_pdf_info_tests {
    use super::{copy_pdf_info, PdfDocument, PdfObject};
    use mupdf::MetadataName;

    #[test]
    fn transfers_creator_and_producer() {
        let mut src = PdfDocument::new();
        let mut info = src.new_dict().unwrap();
        info.dict_put("Creator", PdfObject::new_string("LuaTeX").unwrap()).unwrap();
        info.dict_put("Producer", PdfObject::new_string("LuaTeX-1.17.0").unwrap()).unwrap();
        let info_ref = src.add_object(&info).unwrap();
        src.trailer().unwrap().dict_put("Info", info_ref).unwrap();

        let mut dst = PdfDocument::new();
        copy_pdf_info(&src, &mut dst).unwrap();

        assert_eq!(dst.metadata(MetadataName::Creator).unwrap(), "LuaTeX");
        assert_eq!(dst.metadata(MetadataName::Producer).unwrap(), "LuaTeX-1.17.0; MuPDF");
    }

    #[test]
    fn appends_mupdf_even_when_source_has_no_producer() {
        let src = PdfDocument::new();
        let mut dst = PdfDocument::new();
        copy_pdf_info(&src, &mut dst).unwrap();
        assert_eq!(dst.metadata(MetadataName::Producer).unwrap(), "MuPDF");
    }
}

#[cfg(test)]
mod highlight_annotation_tests {
    use super::{merge_overlapping_rects, rect_to_quad, rects_overlap, AnnotationColor, PdfAnnotationType, PdfDocument};

    #[test]
    fn merge_overlapping_rects_combines_only_overlapping_pairs() {
        // 겹치는 두 개(같은 줄을 두 번 칠한 경우)는 하나로, 안 겹치는 건(다른 줄) 그대로 둔다.
        let rects = vec![[10.0, 10.0, 60.0, 20.0], [40.0, 10.0, 100.0, 20.0], [10.0, 30.0, 60.0, 40.0]];
        let mut merged = merge_overlapping_rects(rects);
        merged.sort_by(|a, b| a[1].partial_cmp(&b[1]).unwrap());
        assert_eq!(merged, vec![[10.0, 10.0, 100.0, 20.0], [10.0, 30.0, 60.0, 40.0]]);
    }

    #[test]
    fn rects_overlap_detects_touching_but_not_disjoint() {
        assert!(rects_overlap([0.0, 0.0, 10.0, 10.0], [5.0, 5.0, 15.0, 15.0]));
        assert!(!rects_overlap([0.0, 0.0, 10.0, 10.0], [10.0, 10.0, 20.0, 20.0])); // 모서리만 맞닿음
        assert!(!rects_overlap([0.0, 0.0, 10.0, 10.0], [20.0, 20.0, 30.0, 30.0]));
    }

    #[test]
    fn creates_highlight_with_matching_quad_and_color() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Highlight).unwrap();

        let rect = [10.0, 20.0, 110.0, 40.0];
        annot.set_quad_points(vec![rect_to_quad(rect)]).unwrap();
        annot.set_color(AnnotationColor::Rgb { red: 1.0, green: 0.9, blue: 0.0 }).unwrap();
        annot.set_opacity(0.5).unwrap();
        annot.update().unwrap();

        assert_eq!(annot.r#type().unwrap(), PdfAnnotationType::Highlight);
        let quads = annot.quad_points().unwrap();
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].ul.x, rect[0]);
        assert_eq!(quads[0].ul.y, rect[1]);
        assert_eq!(quads[0].lr.x, rect[2]);
        assert_eq!(quads[0].lr.y, rect[3]);
        match annot.color().unwrap() {
            Some(AnnotationColor::Rgb { red, green, blue }) => {
                assert_eq!((red, green, blue), (1.0, 0.9, 0.0));
            }
            other => panic!("expected Rgb color, got {other:?}"),
        }
        assert_eq!(annot.opacity().unwrap(), 0.5);
    }

    // get_page_highlights가 돌려주는 rect()가 실제로 해당 하이라이트를 덮는지 확인한다 —
    // 우클릭 지점이 이 rect 안에 들어와야 컨텍스트 메뉴에서 "하이라이트 삭제"를 찾을 수 있다.
    #[test]
    fn rect_after_update_covers_the_quad_used_for_hit_testing() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Highlight).unwrap();

        let quad_rect = [10.0, 20.0, 110.0, 40.0];
        annot.set_quad_points(vec![rect_to_quad(quad_rect)]).unwrap();
        annot.update().unwrap();

        let r = annot.rect().unwrap();
        let (mx, my) = ((quad_rect[0] + quad_rect[2]) / 2.0, (quad_rect[1] + quad_rect[3]) / 2.0);
        assert!(r.x0 <= mx && mx <= r.x1 && r.y0 <= my && my <= r.y1, "rect {r:?} does not cover quad midpoint ({mx}, {my})");
    }

    #[test]
    fn undo_deletes_only_the_most_recently_added_highlight() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();

        let mut first = page.create_annotation(PdfAnnotationType::Highlight).unwrap();
        first.set_quad_points(vec![rect_to_quad([10.0, 10.0, 50.0, 20.0])]).unwrap();
        first.update().unwrap();

        let mut second = page.create_annotation(PdfAnnotationType::Highlight).unwrap();
        second.set_quad_points(vec![rect_to_quad([10.0, 30.0, 50.0, 40.0])]).unwrap();
        second.update().unwrap();

        // undo_annotation과 같은 로직: /Annots 순서상 마지막 Highlight를 찾아 지운다.
        let last = page
            .annotations()
            .filter(|a| matches!(a.r#type(), Ok(PdfAnnotationType::Highlight)))
            .last()
            .unwrap();
        page.delete_annotation(last).unwrap();

        let remaining: Vec<_> = page
            .annotations()
            .filter(|a| matches!(a.r#type(), Ok(PdfAnnotationType::Highlight)))
            .collect();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].quad_points().unwrap()[0].ul.y, 10.0);
    }
}

#[cfg(test)]
mod shape_annotation_tests {
    use super::{rect_to_quad, rounded_rect_vertices, AnnotationColor, PdfAnnotationType, PdfDocument, Point, Rect};

    #[test]
    fn rounded_rect_vertices_stays_within_bounds_and_closes_the_loop() {
        let pts = rounded_rect_vertices(10.0, 10.0, 110.0, 60.0, 0.0);
        assert!(pts.len() > 8);
        for p in &pts {
            assert!((10.0..=110.0).contains(&p.x), "x out of bounds: {p:?}");
            assert!((10.0..=60.0).contains(&p.y), "y out of bounds: {p:?}");
        }
        // 시작점(top-left 모서리 시작)과 끝점(bottom-left 모서리 끝)은 둘 다 왼쪽 변 위에
        // 있어야 한다 — appearance 생성 쪽이 마지막 점에서 첫 점으로 "h"(closepath)를 그어
        // 도형을 닫으므로, x가 같은 왼쪽 변이어야 사각형이 비뚤어지지 않는다.
        let (first, last) = (pts[0], *pts.last().unwrap());
        assert!((first.x - 10.0).abs() < 0.001);
        assert!((last.x - 10.0).abs() < 0.001);
    }

    #[test]
    fn rounded_rect_vertices_handles_degenerate_zero_size() {
        // 드래그 폭/높이가 0에 가까울 때 반지름/inset 계산이 나눗셈으로 터지지 않는지
        let pts = rounded_rect_vertices(10.0, 10.0, 10.0, 10.0, 4.0);
        assert!(pts.iter().all(|p| p.x == 10.0 && p.y == 10.0));
    }

    #[test]
    fn rounded_rect_vertices_insets_by_half_stroke_width() {
        // Square/Circle은 MuPDF가 테두리를 Rect 안쪽으로 자동으로 들여 그려서 스트로크
        // 바깥쪽 경계가 정확히 드래그한 크기에 맞는다. Polygon은 그 보정이 없어서 직접
        // half-width만큼 들여야 완성된 도형이 드래그한 크기보다 커지지 않는다.
        let pts = rounded_rect_vertices(10.0, 10.0, 110.0, 60.0, 10.0); // stroke_width=10 → inset 5
        for p in &pts {
            assert!((15.0..=105.0).contains(&p.x), "x not inset: {p:?}");
            assert!((15.0..=55.0).contains(&p.y), "y not inset: {p:?}");
        }
    }

    #[test]
    fn creates_rect_shape_with_stroke_fill_width_and_opacity() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Square).unwrap();
        annot.set_rect(Rect::new(10.0, 10.0, 90.0, 50.0)).unwrap();
        annot.set_interior_color(AnnotationColor::Rgb { red: 0.2, green: 0.4, blue: 0.6 }).unwrap();
        annot.set_color(AnnotationColor::Rgb { red: 1.0, green: 0.0, blue: 0.0 }).unwrap();
        annot.set_border_width(3.0).unwrap();
        annot.set_opacity(0.7).unwrap();
        annot.update().unwrap();

        assert_eq!(annot.r#type().unwrap(), PdfAnnotationType::Square);
        assert_eq!(annot.border_width().unwrap(), 3.0);
        assert_eq!(annot.opacity().unwrap(), 0.7);
        match annot.interior_color().unwrap() {
            Some(AnnotationColor::Rgb { red, green, blue }) => assert_eq!((red, green, blue), (0.2, 0.4, 0.6)),
            other => panic!("expected Rgb interior color, got {other:?}"),
        }
    }

    // add_shape가 실제로 호출하는 순서 그대로(create -> set_rect -> interior_color -> color ->
    // border_width -> opacity -> update) Square를 만들었을 때 /Rect가 입력 그대로 유지되는지
    // 확인한다 — "드래그한 크기보다 도형이 미세하게 커진다"는 리포트를 조사하며, MuPDF가
    // set_rect() 호출 시 RD(rect-diff)만큼 Rect를 확장하는 경로가 있어 의심했으나(자세한 건
    // pdf-annot.c의 pdf_set_annot_rect), 실제로는 새로 만든 Square의 기본 RD가 0이라
    // 확장이 일어나지 않고, update() 이후에도 /Rect는 입력과 정확히 같게 유지됨을 확인했다.
    // annot.bounds()만 (히트테스트용 여유 마진 포함이라) 입력보다 크게 나오는데, 실제로
    // 그려지는 픽셀과는 무관한 값이라 렌더링 크기 문제의 원인이 아니다.
    #[test]
    fn square_rect_stays_exact_after_update() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Square).unwrap();
        let input = Rect::new(10.0, 10.0, 90.0, 50.0);
        annot.set_rect(input).unwrap();
        annot.set_interior_color(AnnotationColor::Rgb { red: 1.0, green: 0.2, blue: 0.2 }).unwrap();
        annot.set_color(AnnotationColor::Rgb { red: 1.0, green: 0.2, blue: 0.2 }).unwrap();
        annot.set_border_width(2.0).unwrap();
        annot.set_opacity(1.0).unwrap();
        annot.update().unwrap();
        assert_eq!(annot.rect().unwrap(), input);
        annot.update().unwrap();
        assert_eq!(annot.rect().unwrap(), input, "반복 update()에도 Rect가 안 변해야 한다");
    }

    // Square/Circle는 annot.set_rect()에 넘긴 Rect를 스트로크 중심선으로 그대로 쓴다(자동으로
    // Rect 안쪽으로 들여 그려주는 게 아니었다 — 실제로 렌더링해서 픽셀 단위로 확인해서 알아낸
    // 사실. "드래그한 크기보다 도형이 미세하게 커진다"는 리포트의 진짜 원인이었다). 그래서
    // add_shape가 하듯 선 두께 절반만큼 안쪽으로 들인 Rect를 넘겨야, 스트로크 바깥쪽 경계가
    // 드래그한 크기에 정확히 맞는다. 이 테스트는 실제로 페이지를 래스터화해서 의도한 픽셀
    // 경계 밖으로 색이 번지지 않는지 픽셀 단위로 검증한다(메타데이터인 annot.rect()만 봐서는
    // 이 버그가 안 잡힌다).
    #[test]
    fn square_and_circle_stroke_stays_within_intended_pixel_bounds() {
        use crate::{Colorspace, Matrix};
        for kind in [PdfAnnotationType::Square, PdfAnnotationType::Circle] {
            let mut doc = PdfDocument::new();
            let mut page = doc.new_page((200.0, 200.0)).unwrap();
            let mut annot = page.create_annotation(kind).unwrap();
            let (x0, y0, x1, y1) = (50.0_f32, 50.0_f32, 150.0_f32, 100.0_f32);
            let border_width = 2.0_f32;
            // add_shape의 Square/Circle 분기와 동일한 보정
            let inset = (border_width / 2.0).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
            annot.set_rect(Rect::new(x0 + inset, y0 + inset, x1 - inset, y1 - inset)).unwrap();
            annot.set_interior_color(AnnotationColor::Rgb { red: 1.0, green: 0.0, blue: 0.0 }).unwrap();
            annot.set_color(AnnotationColor::Rgb { red: 1.0, green: 0.0, blue: 0.0 }).unwrap();
            annot.set_border_width(border_width).unwrap();
            annot.set_opacity(1.0).unwrap();
            annot.update().unwrap();
            drop(annot);

            let scale = 2.0_f32;
            let pixmap = page.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, true).unwrap();
            let w = pixmap.width();
            let samples = pixmap.samples();
            let is_white = |x: u32, y: u32| {
                let idx = ((y * w + x) * 3) as usize;
                samples[idx] > 250 && samples[idx + 1] > 250 && samples[idx + 2] > 250
            };
            // 의도한 왼쪽 경계(x0*scale) 바로 바깥은 흰색이어야 하고, 경계 위는 색이 있어야 한다
            let left_px = (x0 * scale) as u32;
            let mid_y = ((y0 + y1) / 2.0 * scale) as u32;
            assert!(is_white(left_px - 2, mid_y), "{kind:?}: 의도한 경계보다 2px 바깥까지 색이 번짐(커짐)");
            assert!(!is_white(left_px, mid_y), "{kind:?}: 의도한 경계 위에 색이 없음(반대로 작아짐)");
        }
    }

    // MuPDF는 타원 곡선을 절대좌표 베지어로 직접 써서 그리기 때문에(원을 비균등 스케일해서
    // 그리는 방식이 아니다), 실제 렌더링된 픽셀로 확인해보면 아주 길쭉한 타원이라도 곡률이
    // 급한 극점과 완만한 옆면에서 테두리 두께가 똑같이 균일하다. "드래그를 끝내면 테두리가
    // 두꺼워 보인다"는 리포트를 조사하며 백엔드가 원인인지 먼저 확인하려고 픽셀 단위로 직접
    // 측정했다 — 원인은 여기가 아니라 미리보기(CSS border-radius로 그린 타원은 가로/세로
    // 반지름에서 각각 선 두께를 그대로 빼는 방식이라 길쭉할수록 불균일해진다) 쪽이었다.
    #[test]
    fn elongated_ellipse_stroke_thickness_stays_uniform() {
        use crate::{Colorspace, Matrix};
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((400.0, 400.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Circle).unwrap();
        let (x0, y0, x1, y1) = (150.0_f32, 20.0_f32, 250.0_f32, 380.0_f32); // 1:3.6 비율의 긴 타원
        let border_width = 4.0_f32;
        let inset = (border_width / 2.0).min((x1 - x0) / 2.0).min((y1 - y0) / 2.0);
        annot.set_rect(Rect::new(x0 + inset, y0 + inset, x1 - inset, y1 - inset)).unwrap();
        annot.set_interior_color(AnnotationColor::Rgb { red: 1.0, green: 1.0, blue: 0.0 }).unwrap();
        annot.set_color(AnnotationColor::Rgb { red: 0.6, green: 0.4, blue: 0.0 }).unwrap();
        annot.set_border_width(border_width).unwrap();
        annot.set_opacity(1.0).unwrap();
        annot.update().unwrap();
        drop(annot);

        let scale = 2.0_f32;
        let pixmap = page.to_pixmap(&Matrix::new_scale(scale, scale), &Colorspace::device_rgb(), false, true).unwrap();
        let w = pixmap.width();
        let samples = pixmap.samples();
        let px = |x: u32, y: u32| {
            let idx = ((y * w + x) * 3) as usize;
            (samples[idx], samples[idx + 1], samples[idx + 2])
        };
        let is_border = |c: (u8, u8, u8)| c.0 < 200 && c.2 < 100; // 갈색(0.6,0.4,0.0)만 해당, 노란 내부/흰 배경 제외
        let cx = ((x0 + x1) / 2.0 * scale) as u32;
        let cy = ((y0 + y1) / 2.0 * scale) as u32;
        let left_edge_px = (x0 * scale) as u32;
        let top_edge_px = (y0 * scale) as u32;
        let side_thickness = (0..40).filter(|&dx| is_border(px(left_edge_px - 20 + dx, cy))).count();
        let pole_thickness = (0..40).filter(|&dy| is_border(px(cx, top_edge_px - 20 + dy))).count();
        let expected = (border_width * scale) as usize;
        assert_eq!(side_thickness, expected, "옆면(완만한 곡률) 테두리 두께가 기대와 다름");
        assert_eq!(pole_thickness, expected, "극점(급한 곡률) 테두리 두께가 기대와 다름");
    }

    #[test]
    fn creates_line_shape_with_given_endpoints() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Line).unwrap();
        annot.set_line(Point { x: 5.0, y: 5.0 }, Point { x: 95.0, y: 45.0 }).unwrap();
        annot.update().unwrap();

        let (a, b) = annot.line().unwrap();
        assert_eq!((a.x, a.y), (5.0, 5.0));
        assert_eq!((b.x, b.y), (95.0, 45.0));
    }

    #[test]
    fn creates_rounded_rect_as_filled_polygon() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Polygon).unwrap();
        annot.set_vertices(rounded_rect_vertices(10.0, 10.0, 90.0, 50.0, 2.0)).unwrap();
        annot.set_interior_color(AnnotationColor::Rgb { red: 0.0, green: 1.0, blue: 0.0 }).unwrap();
        annot.update().unwrap();

        assert_eq!(annot.r#type().unwrap(), PdfAnnotationType::Polygon);
        assert!(annot.vertices().unwrap().len() > 8);
    }

    // undo_annotation처럼 (페이지, 타입)으로 짚어 지우면, 같은 페이지에 하이라이트와 도형이
    // 섞여 있어도 엉뚱한 타입을 지우지 않는다는 걸 확인한다.
    #[test]
    fn undo_by_type_ignores_other_annotation_kinds_on_same_page() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();

        let mut hl = page.create_annotation(PdfAnnotationType::Highlight).unwrap();
        hl.set_quad_points(vec![rect_to_quad([0.0, 0.0, 20.0, 10.0])]).unwrap();
        hl.update().unwrap();

        let mut rect = page.create_annotation(PdfAnnotationType::Square).unwrap();
        rect.set_rect(Rect::new(30.0, 30.0, 60.0, 60.0)).unwrap();
        rect.update().unwrap();

        // 마지막으로 추가한 건 Square지만, undo 대상 타입을 Highlight로 지정하면 Square는
        // 안 건드리고 Highlight만 지워야 한다(스택에 (page, Highlight)가 먼저 쌓였던 상황 재현).
        let target_type = PdfAnnotationType::Highlight;
        let last = page
            .annotations()
            .filter(|a| matches!(a.r#type(), Ok(t) if t == target_type))
            .last()
            .unwrap();
        page.delete_annotation(last).unwrap();

        assert_eq!(page.annotations().count(), 1);
        assert_eq!(page.annotations().next().unwrap().r#type().unwrap(), PdfAnnotationType::Square);
    }
}

#[cfg(test)]
mod note_annotation_tests {
    use super::{find_annot_by_xref, set_flat_note_icon, AnnotationColor, PdfAnnotationType, PdfDocument, Rect};

    #[test]
    fn creates_note_with_contents_icon_and_color() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Text).unwrap();
        annot.set_rect(Rect::new(20.0, 20.0, 38.0, 38.0)).unwrap();
        annot.set_icon_name("Comment").unwrap();
        annot.set_contents("회의 전에 확인").unwrap();
        annot.set_color(AnnotationColor::Rgb { red: 1.0, green: 0.8, blue: 0.0 }).unwrap();
        annot.update().unwrap();

        assert_eq!(annot.r#type().unwrap(), PdfAnnotationType::Text);
        assert_eq!(annot.contents().unwrap(), Some("회의 전에 확인"));
        assert_eq!(annot.icon_name().unwrap(), Some("Comment"));
        match annot.color().unwrap() {
            Some(AnnotationColor::Rgb { red, green, blue }) => assert_eq!((red, green, blue), (1.0, 0.8, 0.0)),
            other => panic!("expected Rgb color, got {other:?}"),
        }
    }

    // update_note/delete_annotation이 xref로 정확히 그 주석 하나만 찾아 고치는지 확인한다 — 같은
    // 페이지에 메모가 여러 개 있어도 엉뚱한 걸 건드리면 안 된다.
    #[test]
    fn find_by_xref_locates_the_correct_note_among_several() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((200.0, 200.0)).unwrap();

        let mut first = page.create_annotation(PdfAnnotationType::Text).unwrap();
        first.set_rect(Rect::new(10.0, 10.0, 28.0, 28.0)).unwrap();
        first.set_contents("첫 번째").unwrap();
        first.update().unwrap();
        let first_xref = first.xref().unwrap();

        let mut second = page.create_annotation(PdfAnnotationType::Text).unwrap();
        second.set_rect(Rect::new(50.0, 50.0, 68.0, 68.0)).unwrap();
        second.set_contents("두 번째").unwrap();
        second.update().unwrap();
        let second_xref = second.xref().unwrap();

        assert_ne!(first_xref, second_xref);

        let mut found = find_annot_by_xref(&page, second_xref).unwrap();
        assert_eq!(found.contents().unwrap(), Some("두 번째"));
        found.set_contents("두 번째 수정됨").unwrap();
        found.update().unwrap();

        // 첫 번째는 안 건드려졌어야 한다
        let untouched = find_annot_by_xref(&page, first_xref).unwrap();
        assert_eq!(untouched.contents().unwrap(), Some("첫 번째"));

        page.delete_annotation(untouched).unwrap();
        assert!(find_annot_by_xref(&page, first_xref).is_none());
        assert_eq!(
            find_annot_by_xref(&page, second_xref).unwrap().contents().unwrap(),
            Some("두 번째 수정됨")
        );
    }

    // 기본 아이콘(검은 테두리 사각형)을 우리가 만든 색칠된 원으로 덮어썼는지 실제 렌더링한
    // 픽셀로 확인한다 — 중심은 지정한 색이어야 하고, 모서리(기존엔 검은 테두리가 있던 자리)는
    // 배경색(흰색)이어야 한다.
    #[test]
    fn flat_note_icon_replaces_default_icon_with_a_colored_circle() {
        let mut doc = PdfDocument::new();
        let mut page = doc.new_page((100.0, 100.0)).unwrap();
        let mut annot = page.create_annotation(PdfAnnotationType::Text).unwrap();
        let rect = [10.0_f32, 10.0, 28.0, 28.0];
        annot.set_rect(Rect::new(rect[0], rect[1], rect[2], rect[3])).unwrap();
        annot.set_icon_name("Comment").unwrap();
        annot.set_contents("test").unwrap();
        let color = [1.0_f32, 0.8, 0.0];
        annot.set_color(AnnotationColor::Rgb { red: color[0], green: color[1], blue: color[2] }).unwrap();
        annot.update().unwrap();
        set_flat_note_icon(&mut doc, &mut annot, rect, color).unwrap();
        drop(annot);

        let pixmap = page.to_pixmap(&crate::Matrix::IDENTITY, &crate::Colorspace::device_rgb(), false, true).unwrap();
        // 원 안쪽이지만 말풍선 글리프 밖(왼쪽 가장자리 쪽)은 지정한 색이어야 한다
        let circle_only = pixmap.pixel(12, 19).unwrap();
        assert_eq!(circle_only.components(), &[255, 204, 0], "원 부분이 지정한 색이 아님");
        // 중심(말풍선 안)은 흰색 글리프여야 "메모"라고 알아볼 수 있다
        let bubble = pixmap.pixel(19, 19).unwrap();
        assert_eq!(bubble.components(), &[255, 255, 255], "말풍선 글리프가 안 그려짐");
        let corner = pixmap.pixel(10, 10).unwrap();
        assert_eq!(corner.components(), &[255, 255, 255], "모서리에 옛 검은 테두리 흔적이 남아있음");
    }
}

#[cfg(test)]
mod unpremultiply_rgba_tests {
    use super::unpremultiply_rgba;

    #[test]
    fn recovers_straight_color_from_premultiplied_low_alpha() {
        // 마젠타(255,0,255)를 alpha=24/255(~10%)로 premultiply하면 (24,0,24,24) —
        // 이걸 그대로 straight alpha로 오인해 흰 배경과 합성하면 회색으로 보이던 버그.
        let mut buf = [24u8, 0, 24, 24];
        unpremultiply_rgba(&mut buf);
        assert_eq!(buf, [255, 0, 255, 24]);
    }

    #[test]
    fn leaves_fully_opaque_and_fully_transparent_untouched() {
        let mut buf = [10u8, 20, 30, 255, 40, 50, 60, 0];
        unpremultiply_rgba(&mut buf);
        assert_eq!(buf, [10, 20, 30, 255, 40, 50, 60, 0]);
    }
}

#[cfg(test)]
mod append_mupdf_to_producer_tests {
    use super::{append_mupdf_to_producer, PdfDocument, PdfObject};
    use mupdf::MetadataName;

    #[test]
    fn appends_when_producer_exists_without_mupdf() {
        let mut doc = PdfDocument::new();
        let mut info = doc.new_dict().unwrap();
        info.dict_put("Producer", PdfObject::new_string("LuaTeX-1.17.0").unwrap()).unwrap();
        let info_ref = doc.add_object(&info).unwrap();
        doc.trailer().unwrap().dict_put("Info", info_ref).unwrap();

        append_mupdf_to_producer(&mut doc).unwrap();
        assert_eq!(doc.metadata(MetadataName::Producer).unwrap(), "LuaTeX-1.17.0; MuPDF");
    }

    #[test]
    fn does_not_double_append_when_already_present() {
        let mut doc = PdfDocument::new();
        let mut info = doc.new_dict().unwrap();
        info.dict_put("Producer", PdfObject::new_string("LuaTeX-1.17.0; MuPDF").unwrap()).unwrap();
        let info_ref = doc.add_object(&info).unwrap();
        doc.trailer().unwrap().dict_put("Info", info_ref).unwrap();

        append_mupdf_to_producer(&mut doc).unwrap();
        assert_eq!(doc.metadata(MetadataName::Producer).unwrap(), "LuaTeX-1.17.0; MuPDF");
    }

    #[test]
    fn creates_info_dict_when_missing() {
        let mut doc = PdfDocument::new();
        append_mupdf_to_producer(&mut doc).unwrap();
        assert_eq!(doc.metadata(MetadataName::Producer).unwrap(), "MuPDF");
    }
}

#[cfg(test)]
mod apply_view_rotation_tests {
    use super::{apply_view_rotation, PdfDocument};

    #[test]
    fn empty_list_is_a_no_op() {
        let mut doc = PdfDocument::new();
        doc.new_page((200.0, 100.0)).unwrap();
        apply_view_rotation(&mut doc, &[]).unwrap();
        let page: super::PdfPage = doc.load_page(0).unwrap().try_into().unwrap();
        assert_eq!(page.rotation().unwrap(), 0);
    }

    #[test]
    fn only_affects_the_listed_page_not_others() {
        let mut doc = PdfDocument::new();
        doc.new_page((200.0, 100.0)).unwrap();
        doc.new_page((200.0, 100.0)).unwrap();
        // 두 번째 페이지는 이미 90도로 저장돼 있던 경우를 흉내낸다(예: 스캔 원본)
        {
            let mut p1: super::PdfPage = doc.load_page(1).unwrap().try_into().unwrap();
            p1.set_rotation(90).unwrap();
        }

        // r/R은 "지금 보고 있는 페이지"만 돌리므로 목록엔 페이지 1(두 번째)만 들어있다
        apply_view_rotation(&mut doc, &[(1, 90)]).unwrap();

        let p0: super::PdfPage = doc.load_page(0).unwrap().try_into().unwrap();
        let p1: super::PdfPage = doc.load_page(1).unwrap().try_into().unwrap();
        assert_eq!(p0.rotation().unwrap(), 0, "목록에 없는 페이지는 안 건드려야 함");
        assert_eq!(p1.rotation().unwrap(), 180, "원래 90도였던 페이지는 화면 회전만큼 더해져야 함");
    }

    #[test]
    fn wraps_around_360_degrees() {
        let mut doc = PdfDocument::new();
        doc.new_page((200.0, 100.0)).unwrap();
        {
            let mut p0: super::PdfPage = doc.load_page(0).unwrap().try_into().unwrap();
            p0.set_rotation(270).unwrap();
        }
        apply_view_rotation(&mut doc, &[(0, 180)]).unwrap();
        let p0: super::PdfPage = doc.load_page(0).unwrap().try_into().unwrap();
        assert_eq!(p0.rotation().unwrap(), 90, "270 + 180 = 450, 360으로 나눈 나머지는 90이어야 함");
    }

    #[test]
    fn applies_different_rotations_to_different_pages_independently() {
        let mut doc = PdfDocument::new();
        doc.new_page((200.0, 100.0)).unwrap();
        doc.new_page((200.0, 100.0)).unwrap();
        apply_view_rotation(&mut doc, &[(0, 90), (1, 180)]).unwrap();
        let p0: super::PdfPage = doc.load_page(0).unwrap().try_into().unwrap();
        let p1: super::PdfPage = doc.load_page(1).unwrap().try_into().unwrap();
        assert_eq!(p0.rotation().unwrap(), 90);
        assert_eq!(p1.rotation().unwrap(), 180);
    }
}

// path에 열려 있는 문서를 다시 읽어와 (지정했다면 page_spec만큼만 골라) out_path에 저장한다.
// 이미 열려 있는 tab.doc는 건드리지 않는다 — 페이지를 골라내는 과정이 문서를 제자리에서
// 변형하므로, 그걸 그대로 썼다간 저장 후 화면에 보이는 문서도 같이 페이지가 잘려나간다.
// save_pdf(:w)와 export_document(pdf 포맷)가 공유한다.
fn write_pdf_pages(
    path: &str,
    out_path: &str,
    page_spec: Option<&str>,
    page_count: usize,
    password: Option<&str>,
) -> Result<(), String> {
    if page_spec.is_none() && password.is_none() {
        // 페이지 지정도 암호 설정도 없으면 그냥 원본 그대로 복사 — 재인코딩할 이유가 없다
        return std::fs::copy(path, out_path).map(|_| ()).map_err(|e| e.to_string());
    }

    let doc = Document::open(path).map_err(|e| e.to_string())?;
    if doc.needs_password().map_err(|e| e.to_string())? {
        return Err("암호로 보호된 문서는 저장할 수 없습니다".to_string());
    }
    let src_pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;

    let mut opts = PdfWriteOptions::default();
    opts.set_garbage_level(4).set_compress(true);
    if let Some(pw) = password {
        opts.set_encryption(Encryption::Aes256).set_owner_password(pw).set_user_password(pw);
    }

    let Some(spec) = page_spec else {
        // 페이지 선택 없이 암호만 적용하는 경우 — 페이지를 그대로 두고 다시 쓴다
        return src_pdf.save_with_options(out_path, opts).map_err(|e| e.to_string());
    };
    let pages = parse_page_spec(spec, page_count)?;

    // select_pages는 페이지 트리 링크만 끊을 뿐이라, 스캔본 PDF처럼 Resources가 상위 Pages
    // 노드에서 상속되는 문서는 다른 페이지의 이미지가 여전히 "도달 가능"한 상태로 남아
    // 가비지 컬렉션으로도 안 지워진다(실측: 48페이지 스캔본 1페이지만 골라도 55MB 그대로).
    // 새 문서에 페이지를 그래프트(insert_pdf)해서 옮기면 그 페이지가 실제로 쓰는 리소스만
    // 딸려온다 — 같은 문서로 실측 시 872KB.
    let mut dst = PdfDocument::new();
    let insert_opts = InsertPdfOptions { source_pages: PageSelection::Pages(pages), ..Default::default() };
    dst.insert_pdf(&src_pdf, insert_opts).map_err(|e| e.to_string())?;
    copy_pdf_info(&src_pdf, &mut dst).map_err(|e| e.to_string())?;
    dst.save_with_options(out_path, opts).map_err(|e| e.to_string())
}

#[tauri::command]
fn save_pdf(
    out_path: String,
    page_spec: Option<String>,
    password: Option<String>,
    state: State<Mutex<AppState>>,
) -> Result<(), String> {
    let (path, page_count) = {
        let s = state.lock().unwrap();
        let tab = s.tab();
        (tab.path.clone().ok_or_else(|| "열린 문서가 없습니다".to_string())?, tab.cached_page_count as usize)
    };
    let out_path = resolve_out_path(&path, out_path);
    write_pdf_pages(&path, &out_path, page_spec.as_deref(), page_count, password.as_deref())
}

// out_path의 파일명에 "-{page}" 접미사를 붙인다 — 이미지 포맷은 파일 하나에 여러 페이지를
// 못 담으므로, 페이지 스펙이 여러 페이지를 가리키면 페이지마다 별도 파일로 내보낸다.
fn numbered_export_path(out_path: &str, page_1indexed: usize) -> String {
    let path = std::path::Path::new(out_path);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("export");
    let numbered = format!("{stem}-{page_1indexed}.{ext}");
    match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(numbered).to_string_lossy().into_owned(),
        _ => numbered,
    }
}

// mupdf 크레이트의 Pixmap::save_as는 PNG/PNM/PAM/PSD/PS만 지원하고 JPEG는 없다(MuPDF의 C API
// 자체에는 fz_save_pixmap_as_jpeg가 있지만 이 크레이트가 감싸지 않았다) — 이미 갖고 있는 RGB
// 원시 픽셀을 image 크레이트의 JPEG 인코더로 직접 인코딩한다.
fn save_jpeg(pixmap: &Pixmap, file_path: &str) -> Result<(), String> {
    let file = std::fs::File::create(file_path).map_err(|e| e.to_string())?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(file, 90)
        .encode(pixmap.samples(), pixmap.width(), pixmap.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())
}

// 여러 페이지 Pixmap을 위아래로 이어붙여 하나의 Pixmap으로 합친다("페이지마다 별도 파일" 대신
// "한 파일로 합치기" 내보내기 옵션용). 캔버스 폭은 가장 넓은 페이지 기준이고, 그보다 좁은
// 페이지는 왼쪽 정렬 + 나머지는 흰색으로 채운다.
// ponytail: 페이지를 전부 메모리에 올려둔 뒤 합친다 — 수백 페이지짜리 문서를 통째로 합치면
// 메모리를 많이 쓴다. 필요해지면 스트리밍(파일에 순서대로 이어쓰기)으로 바꾼다.
fn merge_pixmaps_vertically(pixmaps: &[Pixmap]) -> Result<Pixmap, String> {
    let width = pixmaps.iter().map(|p| p.width()).max().unwrap_or(0);
    let height: u32 = pixmaps.iter().map(|p| p.height()).sum();
    let mut merged = Pixmap::new_with_w_h(&Colorspace::device_rgb(), width as i32, height as i32, false)
        .map_err(|e| e.to_string())?;
    merged.samples_mut().fill(255);
    let dst_stride = merged.stride() as usize;
    let mut y_offset = 0usize;
    for p in pixmaps {
        let src_stride = p.stride() as usize;
        let row_bytes = p.width() as usize * 3;
        for row in 0..p.height() as usize {
            let src_row = &p.samples()[row * src_stride..row * src_stride + row_bytes];
            let dst_start = (y_offset + row) * dst_stride;
            merged.samples_mut()[dst_start..dst_start + row_bytes].copy_from_slice(src_row);
        }
        y_offset += p.height() as usize;
    }
    Ok(merged)
}

// 파일 메뉴 "내보내기" / ":export" 명령 — 현재 문서를 PNG/JPG/PDF로 내보낸다. page_spec이
// 없으면 PDF는 전체 문서, 이미지는 전체 페이지를 각각 별도 파일로 내보낸다(현재 페이지만
// 내보내는 기본값은 프론트에서 page_spec을 채워 넣어 구현한다). merge가 true면 이미지 포맷의
// 여러 페이지를 위아래로 이어붙인 파일 하나로 내보낸다(PDF는 원래도 페이지 여러 개가 파일
// 하나에 들어가므로 영향 없음).
#[tauri::command]
async fn export_document(
    app: tauri::AppHandle,
    out_path: String,
    format: String,
    page_spec: Option<String>,
    merge: bool,
    password: Option<String>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let (path, page_count) = {
            let s = state.lock().unwrap();
            let tab = s.tab();
            (tab.path.clone().ok_or_else(|| "열린 문서가 없습니다".to_string())?, tab.cached_page_count as usize)
        };
        let out_path = resolve_out_path(&path, out_path);

        if format == "pdf" {
            let pages: Vec<usize> = match &page_spec {
                Some(spec) => parse_page_spec(spec, page_count)?,
                None => (0..page_count).collect(),
            };
            // merge가 꺼져 있고 페이지가 여러 장이면 PNG/JPG처럼 페이지마다 별도 PDF 파일로
            // 쪼갠다. merge가 켜져 있거나(또는 페이지 한 장뿐이면) 기존처럼 한 파일에 합친다.
            if !merge && pages.len() > 1 {
                let doc = Document::open(&path).map_err(|e| e.to_string())?;
                if doc.needs_password().map_err(|e| e.to_string())? {
                    return Err("암호로 보호된 문서는 저장할 수 없습니다".to_string());
                }
                let src_pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
                for page_idx in &pages {
                    let mut opts = PdfWriteOptions::default();
                    opts.set_garbage_level(4).set_compress(true);
                    if let Some(pw) = &password {
                        opts.set_encryption(Encryption::Aes256).set_owner_password(pw).set_user_password(pw);
                    }
                    let mut dst = PdfDocument::new();
                    let insert_opts = InsertPdfOptions { source_pages: PageSelection::Pages(vec![*page_idx]), ..Default::default() };
                    dst.insert_pdf(&src_pdf, insert_opts).map_err(|e| e.to_string())?;
                    copy_pdf_info(&src_pdf, &mut dst).map_err(|e| e.to_string())?;
                    dst.save_with_options(&numbered_export_path(&out_path, page_idx + 1), opts).map_err(|e| e.to_string())?;
                }
                return Ok(());
            }
            return write_pdf_pages(&path, &out_path, page_spec.as_deref(), page_count, password.as_deref());
        }

        let pages: Vec<usize> = match &page_spec {
            Some(spec) => parse_page_spec(spec, page_count)?,
            None => (0..page_count).collect(),
        };
        let doc = Document::open(&path).map_err(|e| e.to_string())?;
        let matrix = Matrix::new_scale(2.0, 2.0);

        if merge && pages.len() > 1 {
            let mut pixmaps = Vec::with_capacity(pages.len());
            for page_idx in &pages {
                let page = doc.load_page(*page_idx as i32).map_err(|e| e.to_string())?;
                pixmaps.push(page.to_pixmap(&matrix, &Colorspace::device_rgb(), false, true).map_err(|e| e.to_string())?);
            }
            let merged = merge_pixmaps_vertically(&pixmaps)?;
            return match format.as_str() {
                "png" => merged.save_as(&out_path, ImageFormat::PNG).map_err(|e| e.to_string()),
                "jpg" => save_jpeg(&merged, &out_path),
                other => Err(format!("지원하지 않는 내보내기 형식: {other}")),
            };
        }

        let multi = pages.len() > 1;
        for page_idx in pages {
            let page = doc.load_page(page_idx as i32).map_err(|e| e.to_string())?;
            let pixmap = page.to_pixmap(&matrix, &Colorspace::device_rgb(), false, true).map_err(|e| e.to_string())?;
            let file_path = if multi { numbered_export_path(&out_path, page_idx + 1) } else { out_path.clone() };
            match format.as_str() {
                "png" => pixmap.save_as(&file_path, ImageFormat::PNG).map_err(|e| e.to_string())?,
                "jpg" => save_jpeg(&pixmap, &file_path)?,
                other => return Err(format!("지원하지 않는 내보내기 형식: {other}")),
            }
        }
        Ok(())
    }).await.map_err(|e| e.to_string())?
}

// ── SyncTeX (LaTeX 정방향/역방향 검색) ──────────────────────────
// https://www.sumatrapdfreader.org/docs/LaTeX-integration 과 같은 기능. .synctex.gz
// 파일 포맷을 직접 파싱하는 대신, TeX 배포판(TeX Live/MiKTeX/MacTeX)에 이미 같이 설치되는
// synctex CLI를 그대로 호출한다 — synctex(5) 매뉴얼 자체가 "이 파일 포맷을 직접 파싱할
// 필요가 없다, 공식 synctex 명령행 도구를 쓰라"고 명시한다.

// macOS/Linux에서 Finder(더블클릭)로 띄운 GUI 앱은 로그인 셸의 PATH를 못 물려받는다 —
// `~/.local/texlive/.../bin`처럼 TeX 배포판 설치 스크립트가 .zshrc/.zprofile 등에만
// 추가해둔 경로가 여기 해당한다. 터미널에서 `tauri dev`로 실행할 땐 되는데 패키징된 .app을
// 더블클릭하면 synctex/에디터 CLI를 못 찾는 전형적인 증상이 이것 — 로그인 셸에게
// `command -v`로 한 번만 물어봐서 절대경로를 알아내고 이름당 캐시해둔다.
fn resolve_program(name: &str) -> String {
    static CACHE: std::sync::OnceLock<Mutex<std::collections::HashMap<String, String>>> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    if let Some(p) = cache.lock().unwrap().get(name) { return p.clone(); }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let quoted = format!("'{}'", name.replace('\'', "'\\''"));
    let resolved = Command::new(&shell)
        .args(["-ilc", &format!("command -v {quoted}")])
        .output()
        .ok()
        // 인터랙티브 셸이라 rc 파일의 배너/알림 문구가 stdout에 섞여 나올 수 있어, "/"로
        // 시작하는 마지막 줄(command -v의 실제 출력)만 취한다.
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().rev().find(|l| l.starts_with('/')).map(str::to_string))
        .unwrap_or_else(|| name.to_string()); // 못 찾아도 원래 이름 그대로 — Command가 원래 하던 대로 실패해 에러 메시지를 낸다
    cache.lock().unwrap().insert(name.to_string(), resolved.clone());
    resolved
}

fn run_synctex(args: &[&str]) -> Result<String, String> {
    let output = Command::new(resolve_program("synctex")).args(args).output().map_err(|e| {
        format!("synctex 실행 실패({e}) — TeX 배포판(TeX Live/MiKTeX/MacTeX)이 설치되어 있고 PATH에 잡히는지 확인하세요")
    })?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

// "SyncTeX result begin" ~ "SyncTeX result end" 첫 블록만 파싱한다 — synctex 문서에 "여러
// 결과 중 첫 번째가 보통 가장 정확하다"고 되어 있다.
fn parse_synctex_block(stdout: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let mut in_block = false;
    for line in stdout.lines() {
        if line.starts_with("SyncTeX result begin") { in_block = true; continue; }
        if line.starts_with("SyncTeX result end") { break; }
        if !in_block { continue; }
        if let Some((k, v)) = line.split_once(':') {
            map.insert(k.to_string(), v.trim().to_string());
        }
    }
    map
}

#[derive(serde::Serialize)]
struct SynctexForwardHit {
    page: u32,      // 0-based
    rect: [f32; 4], // [x0,y0,x1,y1] — 강조 표시용(페이지 point 좌표, 기존 검색 하이라이트와 동일 규약)
    y: f32,         // 스크롤 대상 y(페이지 상단 기준)
}

// 소스(.tex) 파일의 line:column → PDF의 page/좌표. 에디터(vimtex 등)가 커맨드라인으로
// Vimong을 호출할 때 쓴다.
#[tauri::command]
async fn synctex_forward(tex_file: String, line: u32, column: i32, pdf_path: String) -> Result<SynctexForwardHit, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = run_synctex(&["view", "-i", &format!("{line}:{column}:{tex_file}"), "-o", &pdf_path])?;
        let m = parse_synctex_block(&out);
        let get = |k: &str| m.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let page1: u32 = m.get("Page").and_then(|v| v.parse().ok())
            .ok_or("이 위치에 대응하는 PDF 내용을 찾지 못했습니다 (synctex 파일이 없거나 오래됐을 수 있습니다)")?;
        let (h, v, w, hh) = (get("h"), get("v"), get("W"), get("H"));
        Ok(SynctexForwardHit { page: page1.saturating_sub(1), rect: [h, v - hh, h + w, v], y: v - hh })
    }).await.map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
struct SynctexInverseHit {
    file: String,
    line: u32,
    column: i32,
}

// PDF의 page(1-based)/x,y(point, 좌상단 기준) → 소스(.tex) 파일의 line. PDF에서
// Ctrl(⌘)+클릭했을 때 쓴다.
#[tauri::command]
async fn synctex_inverse(pdf_path: String, page: u32, x: f32, y: f32) -> Result<SynctexInverseHit, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let out = run_synctex(&["edit", "-o", &format!("{page}:{x}:{y}:{pdf_path}")])?;
        let m = parse_synctex_block(&out);
        let file = m.get("Input").cloned()
            .ok_or("이 위치에 대응하는 소스 파일을 찾지 못했습니다 (synctex 파일이 없거나 오래됐을 수 있습니다)")?;
        let line = m.get("Line").and_then(|v| v.parse().ok()).unwrap_or(1);
        let column = m.get("Column").and_then(|v| v.parse().ok()).unwrap_or(-1);
        Ok(SynctexInverseHit { file, line, column })
    }).await.map_err(|e| e.to_string())?
}

// "code -g \"%f:%l\"" 같은 사용자 설정 명령을 %f(파일)/%l(줄)/%c(컬럼)로 치환해 실행한다.
// 경로에 공백이 섞여도 안전하게(쉘을 거치지 않고 argv 배열로 바로 spawn) 큰따옴표만 지원하는
// 최소 토크나이저 — SumatraPDF의 InverseSearchCmdLine과 같은 관습(%f/%l/%c 플레이스홀더).
fn tokenize_cmd(template: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_cur = false;
    for c in template.chars() {
        match c {
            '"' => { in_quotes = !in_quotes; has_cur = true; }
            c if c.is_whitespace() && !in_quotes => {
                if has_cur { tokens.push(std::mem::take(&mut cur)); has_cur = false; }
            }
            c => { cur.push(c); has_cur = true; }
        }
    }
    if has_cur { tokens.push(cur); }
    tokens
}

#[tauri::command]
async fn open_in_editor(cmd_template: String, file: String, line: u32, column: i32) -> Result<(), String> {
    // file은 synctex(.synctex.gz) 파싱 결과라 PDF와 함께 배포되는 신뢰할 수 없는 데이터다.
    // 템플릿을 먼저 채운 뒤 통째로 재토큰화하면 file에 섞인 공백/따옴표가 새 argv 항목을
    // 만들어낼 수 있어(인자 주입) — 템플릿을 먼저 토큰화하고, 그 결과 각 토큰 안에서만
    // %f/%l/%c를 치환한다. 이러면 file 값이 무엇이든 원래 자리 하나의 인자로만 들어간다.
    if file.starts_with('-') {
        return Err("잘못된 소스 파일 경로입니다".to_string());
    }
    // resolve_program이 처음 호출되는 프로그램명이면 로그인 셸을 띄워서 PATH를 물어보므로
    // (수백 ms 걸릴 수 있음) 다른 무거운 커맨드들과 같은 이유로 메인 스레드를 막지 않는다.
    tauri::async_runtime::spawn_blocking(move || {
        let col = if column < 0 { "0".to_string() } else { column.to_string() };
        let argv: Vec<String> = tokenize_cmd(&cmd_template)
            .into_iter()
            .map(|tok| tok.replace("%f", &file).replace("%l", &line.to_string()).replace("%c", &col))
            .collect();
        let (program, args) = argv.split_first().ok_or("에디터 명령이 비어 있습니다 — 설정에서 지정하세요")?;
        Command::new(resolve_program(program)).args(args).spawn().map_err(|e| e.to_string())?;
        Ok(())
    }).await.map_err(|e| e.to_string())?
}

// `vimong --forward-search <tex파일> <줄번호> <pdf파일>` — SumatraPDF의 -forward-search와
// 같은 관습. 에디터가 컴파일 후 이 인자로 Vimong을 실행하면(단일 인스턴스 재사용) 해당
// 위치로 점프한다.
#[derive(Clone, serde::Serialize)]
struct ForwardSearchArgs {
    tex_file: String,
    line: u32,
    pdf_path: String,
}

fn parse_forward_search_args(args: &[String]) -> Option<ForwardSearchArgs> {
    let idx = args.iter().position(|a| a == "--forward-search")?;
    Some(ForwardSearchArgs {
        tex_file: args.get(idx + 1)?.clone(),
        line: args.get(idx + 2)?.parse().ok()?,
        pdf_path: args.get(idx + 3)?.clone(),
    })
}

// 앱을 처음 띄운 프로세스 자체가 --forward-search로 실행된 경우를 위한 것 — 프론트엔드가
// 이벤트 리스너를 걸기 전에 emit해버리면 유실되므로, 시작 시점엔 여기 담아뒀다가
// onMount에서 직접 가져가게 한다. 이미 떠 있는 인스턴스로 들어오는 두 번째 실행은
// tauri_plugin_single_instance 콜백에서 바로 이벤트로 보낸다(그땐 리스너가 이미 붙어 있음).
struct PendingForwardSearch(Mutex<Option<ForwardSearchArgs>>);

#[tauri::command]
fn take_pending_forward_search(state: State<PendingForwardSearch>) -> Option<ForwardSearchArgs> {
    state.0.lock().unwrap().take()
}

// Finder에서 PDF를 더블클릭했을 때 macOS가 경로를 넘기는 경로는 두 가지다:
// 1) 콜드 스타트(앱이 안 떠 있었음) — 잘 알려진 AppKit 레이스 때문에 RunEvent::Opened
//    (application:openURLs:)가 앱이 완전히 뜨기 전에 와서 유실되는 경우가 흔해, 이때는
//    macOS가 그 경로를 argv로도 같이 넘겨준다. 그래서 --forward-search와 똑같이 여기서
//    std::env::args()를 동기적으로 먼저 확인해둔다.
// 2) 웜 스타트(이미 떠 있는 인스턴스) — 새 프로세스가 안 뜨니 argv가 없고, RunEvent::Opened로만 온다.
struct PendingFileOpen(Mutex<Option<String>>);

#[tauri::command]
fn take_pending_file_open(state: State<PendingFileOpen>) -> Option<String> {
    state.0.lock().unwrap().take()
}

fn parse_pending_file_open_arg(args: &[String]) -> Option<String> {
    args.get(1).filter(|a| !a.starts_with('-')).cloned()
}

#[cfg(test)]
mod parse_page_spec_tests {
    use super::parse_page_spec;

    #[test]
    fn single_page() {
        assert_eq!(parse_page_spec("1", 10).unwrap(), vec![0]);
    }

    #[test]
    fn range() {
        assert_eq!(parse_page_spec("1-3", 10).unwrap(), vec![0, 1, 2]);
    }

    #[test]
    fn comma_separated_pages_and_ranges() {
        assert_eq!(
            parse_page_spec("1-3,7-8,20-22", 22).unwrap(),
            vec![0, 1, 2, 6, 7, 19, 20, 21]
        );
    }

    #[test]
    fn out_of_bounds_rejected() {
        assert!(parse_page_spec("5", 3).is_err());
        assert!(parse_page_spec("1-5", 3).is_err());
        assert!(parse_page_spec("0", 3).is_err());
    }
}

#[cfg(test)]
mod synctex_tests {
    use super::{parse_synctex_block, tokenize_cmd};

    #[test]
    fn parse_view_result_block() {
        // 실제 `synctex view` stdout 형태(배너/트레일링 텍스트 포함)를 그대로 반영
        let stdout = "This is SyncTeX command line utility, version 1.5\n\
SyncTeX result begin\nOutput:main.pdf\nPage:1\nx:171.128296\ny:134.764618\n\
h:133.768356\nv:134.764618\nW:343.711060\nH:6.918498\nbefore:\noffset:-1\nmiddle:\nafter:\n\
SyncTeX result end\n";
        let m = parse_synctex_block(stdout);
        assert_eq!(m.get("Page").unwrap(), "1");
        assert_eq!(m.get("h").unwrap(), "133.768356");
        assert_eq!(m.get("W").unwrap(), "343.711060");
    }

    #[test]
    fn parse_edit_result_block_with_windows_path() {
        // Input:은 경로라 콜론을 여러 개 포함할 수 있다(Windows 드라이브 문자) — 첫 콜론만
        // 키/값 구분자로 써야 한다
        let stdout = "SyncTeX result begin\nOutput:main.pdf\nInput:C:\\Users\\me\\main.tex\n\
Line:6\nColumn:-1\nSyncTeX result end\n";
        let m = parse_synctex_block(stdout);
        assert_eq!(m.get("Input").unwrap(), "C:\\Users\\me\\main.tex");
        assert_eq!(m.get("Line").unwrap(), "6");
    }

    #[test]
    fn tokenize_handles_quoted_paths_with_spaces() {
        assert_eq!(
            tokenize_cmd(r#"code -g "/path with spaces/main.tex:3""#),
            vec!["code", "-g", "/path with spaces/main.tex:3"]
        );
    }

    #[test]
    fn placeholder_substitution_does_not_inject_extra_args() {
        // synctex 파싱 결과(file)는 신뢰할 수 없는 데이터 — 공백/따옴표가 섞여 있어도
        // 템플릿을 먼저 토큰화한 뒤 각 토큰 안에서만 치환하면 argv 개수가 늘어나지 않는다.
        let file = r#"foo" bar --exec evil"#;
        let argv: Vec<String> = tokenize_cmd(r#"code -g "%f:%l""#)
            .into_iter()
            .map(|tok| tok.replace("%f", file).replace("%l", "3"))
            .collect();
        assert_eq!(argv, vec!["code".to_string(), "-g".to_string(), format!("{file}:3")]);
    }

    #[test]
    fn tokenize_plain_whitespace() {
        assert_eq!(tokenize_cmd("vim +3 main.tex"), vec!["vim", "+3", "main.tex"]);
    }
}

// get_page_chars의 OCR 폴백이 같은 페이지를 또 래스터화하지 않도록, 가장 최근에 render_page가
// 그린 원본 RGBA를 하나만 들고 있는다(문서당 최대 한 페이지 분량 — 다음 렌더 때 덮어씀).
// 구조체 자체는 플랫폼 무관하게 두고 .manage()도 무조건 등록해서 macOS 전용 cfg를
// invoke_handler 체인 중간에 끼워 넣지 않아도 되게 한다 — non-macOS에선 그냥 항상 None.
struct LastRender {
    page: u32,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

// MuPDF의 alpha 채널 pixmap은 premultiplied RGBA다("over" 합성을 premultiplied로 계산하기
// 때문). 브라우저 Canvas의 putImageData는 straight(비premultiplied) alpha를 기대하므로,
// 완전 불투명(a=255)/완전 투명(a=0)만 있던 지금까지는 둘이 같아서 문제가 없었지만, 하이라이트에
// 불투명도를 도입해 중간 alpha가 생기면서 색이 어긋난다(예: 마젠타 10% 불투명도가 회색으로
// 보이던 버그 — premultiplied된 저채도 RGB를 straight로 착각해 흰 배경과 합성한 결과).
fn unpremultiply_rgba(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[tauri::command]
async fn render_page(app: tauri::AppHandle, page_num: u32, scale: f32, rotation: f32) -> Result<tauri::ipc::Response, String> {
    // 페이지 래스터화도 무거운 동기 작업이라 메인 스레드에서 돌리면 그동안 웹뷰가 화면을 못 그림
    let buf = tauri::async_runtime::spawn_blocking(move || {
        // PNG 인코딩이 "페이지 열기 지연"의 실제 원인이었다 — 실측: 래스터화 자체는
        // 20~75ms인데 PNG 압축이 135~213ms로 그보다 훨씬 크다. alpha=true로 RGBA를 얻어서
        // 압축 없이 원시 바이트 그대로 보내면 이 비용이 통째로 사라진다(raw 접근은
        // 마이크로초 단위) — 프론트엔드가 canvas에 putImageData로 직접 그린다.
        // 앞 4바이트=width, 다음 4바이트=height(u32 LE)를 헤더로 붙여서 픽셀 배열 크기를
        // 프론트엔드가 정확히 알 수 있게 한다.
        //
        // to_pixmap이 돌려주는 Pixmap은 문서/페이지와 독립된 소유 버퍼라, AppState 락은
        // 래스터화가 끝나는 이 블록까지만 붙들고 풀어준다 — 페이지가 큰 문서(스캔본 등)는
        // 아래 unpremultiply_rgba/바이트 복사가 픽셀 수에 비례해 수십~수백ms 걸릴 수 있는데,
        // 락을 계속 쥐고 있으면 그동안 다른 render_page/render_thumbnail 요청이 다 멈춰서
        // 기다린다. 락 밖에서 후처리하면 그 요청들의 래스터화 단계와 겹쳐 돌 수 있다.
        let mut pixmap = {
            let state = app.state::<Mutex<AppState>>();
            let s = state.lock().unwrap();
            let doc = s.tab().doc.as_ref().ok_or("No document open")?;
            let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
            let mut matrix = Matrix::new_scale(scale, scale);
            matrix.rotate(rotation);
            page.to_pixmap(&matrix, &Colorspace::device_rgb(), true, true)
                .map_err(|e| e.to_string())?
        };
        let w = pixmap.width();
        let h = pixmap.height();
        unpremultiply_rgba(pixmap.samples_mut());
        let samples = pixmap.samples();
        #[cfg(target_os = "macos")]
        {
            let cache = app.state::<Mutex<Option<LastRender>>>();
            *cache.lock().unwrap() = Some(LastRender { page: page_num, w, h, rgba: samples.to_vec() });
        }
        let mut out = Vec::with_capacity(8 + samples.len());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(samples);
        Ok::<Vec<u8>, String>(out)
    }).await.map_err(|e| e.to_string())??;
    Ok(tauri::ipc::Response::new(buf))
}

#[tauri::command]
async fn render_thumbnail(app: tauri::AppHandle, page_num: u32, scale: f32) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let s = state.lock().unwrap();
        let doc = s.tab().doc.as_ref().ok_or("No document open")?;
        let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let matrix = Matrix::new_scale(scale, scale);
        let pixmap = page
            .to_pixmap(&matrix, &Colorspace::device_rgb(), false, true)
            .map_err(|e| e.to_string())?;
        let mut buf: Vec<u8> = Vec::new();
        pixmap.write_to(&mut buf, ImageFormat::PNG).map_err(|e| e.to_string())?;
        Ok(general_purpose::STANDARD.encode(buf))
    }).await.map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
struct FileInfo {
    path: String,
    file_name: String,
    page_count: u32,
    file_size_kb: f64,
    is_pdf: bool,
    pdf_version: String,
    producer: String,
    creation_date: String,
    mod_date: String,
    title: String,
    author: String,
    subject: String,
}

// 일부 PDF 생성 도구(CloudConvert 등)가 Info 딕셔너리 문자열을 표준(UTF-16BE, FE FF BOM)이
// 아닌 순서(FF FE, LE)로 쓰고 고정폭으로 NUL 패딩해서 남긴다 — MuPDF가 이걸 읽을 때 각 NUL을
// 유효하지 않은 오버롱 UTF-8(C0 80, "Modified UTF-8"에서 NUL을 C 문자열 종료 없이 담는 관례)로
// 내보내서, 그대로 두면 문서 정보 화면에 "CloudConvert" 뒤로 깨진 문자가 줄줄이 붙어 보인다.
// U+FFFD(그 잘못된 바이트를 러스트가 손실 변환하며 넣은 대체 문자)와 실제 NUL을 지우면
// 원래 의도한 텍스트만 남는다.
fn sanitize_metadata_string(s: String) -> String {
    s.chars().filter(|&c| c != '\u{FFFD}' && c != '\0').collect::<String>().trim().to_string()
}

#[cfg(test)]
mod sanitize_metadata_string_tests {
    use super::sanitize_metadata_string;

    #[test]
    fn strips_replacement_chars_and_nuls_from_padding() {
        let corrupted = format!("CloudConvert{}", "\u{FFFD}".repeat(16));
        assert_eq!(sanitize_metadata_string(corrupted), "CloudConvert");
        assert_eq!(sanitize_metadata_string("Title\0\0\0".to_string()), "Title");
    }

    #[test]
    fn leaves_clean_string_untouched() {
        assert_eq!(sanitize_metadata_string("LuaTeX-1.17.0".to_string()), "LuaTeX-1.17.0");
    }
}

#[tauri::command]
fn get_file_info(state: State<Mutex<AppState>>) -> Result<FileInfo, String> {
    let s = state.lock().unwrap();
    let tab = s.tab();
    let doc = tab.doc.as_ref().ok_or("No document open")?;
    let is_pdf = doc.is_pdf();
    let meta = |name: MetadataName| if is_pdf {
        sanitize_metadata_string(doc.metadata(name).unwrap_or_default())
    } else {
        String::new()
    };
    Ok(FileInfo {
        path: tab.path.clone().unwrap_or_default(),
        file_name: tab.cached_file_name.clone(),
        page_count: tab.cached_page_count,
        file_size_kb: tab.cached_file_size_kb,
        is_pdf,
        pdf_version: meta(MetadataName::Format),
        producer: meta(MetadataName::Producer),
        creation_date: meta(MetadataName::CreationDate),
        mod_date: meta(MetadataName::ModDate),
        title: meta(MetadataName::Title),
        author: meta(MetadataName::Author),
        subject: meta(MetadataName::Subject),
    })
}

#[tauri::command]
async fn get_page_size(app: tauri::AppHandle, page_num: u32) -> Result<[f32; 2], String> {
    // render_page가 같은 Mutex를 붙든 채 고배율 래스터화를 도는 동안 이 커맨드가 동기로
    // state.lock()을 기다리면 메인 스레드가 그 시간만큼 막혀 웹뷰 컴포지팅이 멎는다
    // (트랙패드로 페이지 경계를 넘을 때마다 호출되는 경로라 고배율 줌에서 화면이 통째로
    // 까맣게 굳는 원인이었다) — render_page와 동일하게 spawn_blocking으로 옮긴다.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let s = state.lock().unwrap();
        let doc = s.tab().doc.as_ref().ok_or("No document open")?;
        let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let bounds = page.bounds().map_err(|e| e.to_string())?;
        Ok([bounds.x1 - bounds.x0, bounds.y1 - bounds.y0])
    }).await.map_err(|e| e.to_string())?
}

// 연속/두 페이지 보기 모드는 스크롤 컨테이너에 모든 페이지 자리를 미리 예약해야 해서
// 문서를 열 때(혹은 그 보기 모드로 전환할 때) 페이지마다 get_page_size를 왕복하는 대신
// 한 번의 Mutex 잠금으로 전체 크기를 몰아서 가져온다.
#[tauri::command]
async fn get_all_page_sizes(app: tauri::AppHandle) -> Result<Vec<[f32; 2]>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let s = state.lock().unwrap();
        let doc = s.tab().doc.as_ref().ok_or("No document open")?;
        let count = s.tab().cached_page_count;
        (0..count)
            .map(|i| {
                let page = doc.load_page(i as i32).map_err(|e| e.to_string())?;
                let b = page.bounds().map_err(|e| e.to_string())?;
                Ok([b.x1 - b.x0, b.y1 - b.y0])
            })
            .collect()
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
fn search_page(
    page_num: u32,
    query: String,
    state: State<Mutex<AppState>>,
) -> Result<Vec<[f32; 4]>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.as_ref().ok_or("No document open")?;
    let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
    let hits = page.search(&query, 64).map_err(|e| e.to_string())?;
    Ok(hits.into_iter().map(|q| {
        let x0 = q.ul.x.min(q.ll.x);
        let y0 = q.ul.y.min(q.ur.y);
        let x1 = q.ur.x.max(q.lr.x);
        let y1 = q.ll.y.max(q.lr.y);
        [x0, y0, x1, y1]
    }).collect())
}

#[derive(serde::Serialize, PartialEq, Eq, PartialOrd, Ord)]
struct FontInfo {
    name: String,
    bold: bool,
    italic: bool,
    monospaced: bool,
}

#[tauri::command]
async fn get_fonts(app: tauri::AppHandle) -> Result<Vec<FontInfo>, String> {
    // 폰트 스캔은 무거운 동기 작업이라 메인 스레드에서 돌리면 그동안 웹뷰가 화면을 못 그림
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let s = state.lock().unwrap();
        let doc = s.tab().doc.as_ref().ok_or("No document open")?;
        let count = doc.page_count().map_err(|e| e.to_string())?;
        let mut seen: std::collections::BTreeMap<String, FontInfo> = std::collections::BTreeMap::new();
        for p in 0..count {
            let page = doc.load_page(p).map_err(|e| e.to_string())?;
            let text = page.to_text_page(mupdf::TextPageFlags::empty()).map_err(|e| e.to_string())?;
            for block in text.blocks() {
                for line in block.lines() {
                    for ch in line.chars() {
                        if let Some(font) = ch.font() {
                            let raw = safe_font_name(&font);
                            let name = raw.find('+').map(|i| raw[i+1..].to_string()).unwrap_or(raw);
                            seen.entry(name.clone()).or_insert_with(|| FontInfo {
                                name, bold: font.is_bold(), italic: font.is_italic(),
                                monospaced: font.is_monospaced(),
                            });
                        }
                    }
                }
            }
        }
        Ok(seen.into_values().collect())
    }).await.map_err(|e| e.to_string())?
}

// 마크다운 코드 폰트 설정 드롭다운용 — 문서 안 폰트(get_fonts)와 달리 시스템에 실제로
// 설치된 폰트를 훑는다. 코드 표시용이라 고정폭(모노스페이스) 폰트만 골라낸다 — 폰트 하나당
// 대표 글리프를 실제로 로드해서 is_monospace()를 봐야 해서(패밀리 이름만으론 알 수 없음)
// all_families보다 훨씬 느리다(폰트 많은 실사용 환경에서 수 초). 코어 수만큼 스레드로
// 나눠 돌려서 줄인다 — SystemSource를 스레드끼리 공유하지 않고 스레드마다 새로 만드는 건,
// 이게 CoreText/DirectWrite/fontconfig를 감싼 얇은 핸들이라 만드는 비용이 거의 없고, 그
// 내부 FFI 핸들이 스레드 간 공유(Sync)해도 안전한지 보장이 없어서 아예 공유를 안 하는
// 쪽이 안전하기 때문. 디스크/레지스트리 스캔이라 메인 스레드를 막지 않게 spawn_blocking.
#[tauri::command]
async fn list_system_fonts() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let families = font_kit::source::SystemSource::new()
            .all_families()
            .map_err(|e| e.to_string())?;
        let n_threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
        let chunk_size = families.len().div_ceil(n_threads).max(1);
        let mut monospace: Vec<String> = std::thread::scope(|scope| {
            families
                .chunks(chunk_size)
                .map(|chunk| {
                    scope.spawn(move || {
                        let source = font_kit::source::SystemSource::new();
                        chunk
                            .iter()
                            .filter(|name| {
                                source
                                    .select_family_by_name(name)
                                    .ok()
                                    .and_then(|fam| fam.fonts().first().cloned())
                                    .and_then(|handle| handle.load().ok())
                                    .is_some_and(|font| font.is_monospace())
                            })
                            .cloned()
                            .collect::<Vec<String>>()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        });
        monospace.sort();
        monospace.dedup();
        Ok(monospace)
    }).await.map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
struct FontStat {
    name: String,
    count: u32,
    percent: f32,
}

struct CancelFlag(AtomicBool);
struct QuitConfirmed(AtomicBool);

// 전체화면 진입 전 창 위치/크기 — macOS가 전체화면 종료 후 이 크기로 복원해줄 거라
// 기대했는데, 원래 크기가 아니라 화면을 꽉 채운(최대화 비슷한) 크기로 남는 경우가
// 있어서 직접 저장해뒀다가 되돌린다.
struct SavedWindowFrame(Mutex<Option<(tauri::PhysicalPosition<i32>, tauri::PhysicalSize<u32>)>>);

// 탭이 1개뿐일 때 Tab 메뉴의 다음/이전 탭 항목을 비활성화하기 위해 들고 있는다.
struct TabMenuItems {
    next: MenuItem<tauri::Wry>,
    prev: MenuItem<tauri::Wry>,
    reopen: MenuItem<tauri::Wry>,
}

// 사이드바 표시 상태(프론트엔드 전용 state)에 맞춰 메뉴 항목 텍스트를
// "사이드바 보이기" ↔ "사이드바 숨기기"로 갱신하기 위해 들고 있는다.
struct SidebarMenuItem(MenuItem<tauri::Wry>);

// 사이드바가 지금 보이는지 — 언어를 바꿔 메뉴를 다시 라벨링할 때 toggle_sb에
// hide_sidebar/show_sidebar 중 어느 쪽을 넣을지 알아야 해서 별도로 들고 있는다.
struct SidebarVisibleState(AtomicBool);

// 툴바 표시 상태(프론트엔드 전용 state)에 맞춰 메뉴 항목 텍스트를
// "툴바 보이기" ↔ "툴바 숨기기"로 갱신하기 위해 들고 있는다. SidebarMenuItem/
// SidebarVisibleState와 같은 패턴.
struct ToolbarMenuItem(MenuItem<tauri::Wry>);
struct ToolbarVisibleState(AtomicBool);

// 지금 메뉴에 적용된 언어. 세션 중 :set lang=으로 바뀌면 여기도 같이 갱신하고
// 메뉴 전체를 즉시 다시 라벨링한다(파일 저장은 다음 실행용으로 별도).
struct MenuLang(Mutex<String>);

// 언어가 바뀔 때 다시 라벨링해야 하는, 메뉴 트리 전반에 흩어진 항목들.
// 플랫폼 공통으로 만들어지는 것만 담고, macOS 전용(new_tab/doc_info/password/quit/
// file_menu/tab_menu)은 PlatformMenuItems에 따로 담는다 — cfg 블록 안에서만 만들어져서.
struct AppMenuItems {
    close: MenuItem<tauri::Wry>,
    actual_size: MenuItem<tauri::Wry>,
    zoom_fit: MenuItem<tauri::Wry>,
    zoom_fit_h: MenuItem<tauri::Wry>,
    zoom_in: MenuItem<tauri::Wry>,
    zoom_out: MenuItem<tauri::Wry>,
    zoom_sel: MenuItem<tauri::Wry>,
    view_menu: Submenu<tauri::Wry>,
    fullscreen: MenuItem<tauri::Wry>,
    copy_sel: MenuItem<tauri::Wry>,
    edit_menu: Submenu<tauri::Wry>,
    tools_menu: Submenu<tauri::Wry>,
    shortcuts: MenuItem<tauri::Wry>,
    commands: MenuItem<tauri::Wry>,
    open_source: MenuItem<tauri::Wry>,
    settings: MenuItem<tauri::Wry>,
    help_menu: Submenu<tauri::Wry>,
}

struct PlatformMenuItems {
    new_tab: MenuItem<tauri::Wry>,
    doc_info: MenuItem<tauri::Wry>,
    print: MenuItem<tauri::Wry>,
    password: MenuItem<tauri::Wry>,
    save_as: MenuItem<tauri::Wry>,
    export: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
    file_menu: Submenu<tauri::Wry>,
    tab_menu: Submenu<tauri::Wry>,
}

fn relabel_menu(app: &tauri::AppHandle, l: &Labels) {
    if let Some(m) = app.try_state::<AppMenuItems>() {
        let _ = m.close.set_text(l.close_tab);
        let _ = m.actual_size.set_text(l.actual_size);
        let _ = m.zoom_fit.set_text(l.zoom_to_fit);
        let _ = m.zoom_fit_h.set_text(l.zoom_to_fit_height);
        let _ = m.zoom_in.set_text(l.zoom_in);
        let _ = m.zoom_out.set_text(l.zoom_out);
        let _ = m.zoom_sel.set_text(l.zoom_to_selection);
        let _ = m.view_menu.set_text(l.view);
        let _ = m.fullscreen.set_text(l.fullscreen);
        let _ = m.copy_sel.set_text(l.copy);
        let _ = m.edit_menu.set_text(l.edit);
        let _ = m.tools_menu.set_text(l.tools);
        let _ = m.shortcuts.set_text(l.shortcuts);
        let _ = m.commands.set_text(l.commands);
        let _ = m.open_source.set_text(l.open_source);
        let _ = m.settings.set_text(l.settings);
        let _ = m.help_menu.set_text(l.help);
    }
    if let Some(m) = app.try_state::<PlatformMenuItems>() {
        let _ = m.new_tab.set_text(l.new_tab);
        let _ = m.doc_info.set_text(l.doc_info);
        let _ = m.print.set_text(l.print);
        let _ = m.password.set_text(l.password);
        let _ = m.save_as.set_text(l.save_as);
        let _ = m.export.set_text(l.export);
        let _ = m.quit.set_text(l.quit);
        let _ = m.file_menu.set_text(l.file);
        let _ = m.tab_menu.set_text(l.tab);
    }
    if let Some(items) = app.try_state::<ViewModeMenuItems>() {
        let labels = [l.view_single, l.view_single_continuous, l.view_two, l.view_two_continuous, l.view_horizontal_continuous];
        for (item, text) in items.0.iter().zip(labels) {
            let _ = item.set_text(text);
        }
    }
    if let Some(items) = app.try_state::<ToolMenuItems>() {
        let labels = [l.pointer, l.text_select, l.highlight_tool];
        for (item, text) in items.0.iter().zip(labels) {
            let _ = item.set_text(text);
        }
    }
    if let Some(items) = app.try_state::<TabMenuItems>() {
        let _ = items.next.set_text(l.next_tab);
        let _ = items.prev.set_text(l.prev_tab);
        let _ = items.reopen.set_text(l.reopen_closed_tab);
    }
    if let Some(item) = app.try_state::<SidebarMenuItem>() {
        let visible = app.try_state::<SidebarVisibleState>().map(|s| s.0.load(Ordering::Relaxed)).unwrap_or(true);
        let _ = item.0.set_text(if visible { l.hide_sidebar } else { l.show_sidebar });
    }
    if let Some(item) = app.try_state::<ToolbarMenuItem>() {
        let visible = app.try_state::<ToolbarVisibleState>().map(|s| s.0.load(Ordering::Relaxed)).unwrap_or(true);
        let _ = item.0.set_text(if visible { l.hide_toolbar } else { l.show_toolbar });
    }
}

#[tauri::command]
fn set_sidebar_visible_menu(app: tauri::AppHandle, visible: bool) {
    if let Some(state) = app.try_state::<SidebarVisibleState>() {
        state.0.store(visible, Ordering::Relaxed);
    }
    if let Some(item) = app.try_state::<SidebarMenuItem>() {
        let lang = app.try_state::<MenuLang>().map(|s| s.0.lock().unwrap().clone()).unwrap_or_else(|| detect_lang().to_string());
        let l = labels_for(&lang);
        let _ = item.0.set_text(if visible { l.hide_sidebar } else { l.show_sidebar });
    }
}

#[tauri::command]
fn set_toolbar_visible_menu(app: tauri::AppHandle, visible: bool) {
    if let Some(state) = app.try_state::<ToolbarVisibleState>() {
        state.0.store(visible, Ordering::Relaxed);
    }
    if let Some(item) = app.try_state::<ToolbarMenuItem>() {
        let lang = app.try_state::<MenuLang>().map(|s| s.0.lock().unwrap().clone()).unwrap_or_else(|| detect_lang().to_string());
        let l = labels_for(&lang);
        let _ = item.0.set_text(if visible { l.hide_toolbar } else { l.show_toolbar });
    }
}

// View 메뉴의 보기 모드 라디오 그룹(한 페이지/한 페이지 연속/두 페이지/두 페이지 연속/가로 연속) —
// 하나를 클릭하면 나머지 체크를 해제해서 라디오처럼 동작하게 만드는 데 쓴다.
struct ViewModeMenuItems(Vec<CheckMenuItem<tauri::Wry>>);

fn update_view_mode_checks(app: &tauri::AppHandle, selected: &str) {
    if let Some(items) = app.try_state::<ViewModeMenuItems>() {
        for item in items.0.iter() {
            let _ = item.set_checked(item.id().as_ref() == selected);
        }
    }
}

// macOS에서 창 리사이즈/전체화면 전환 시 웹뷰만 통째로 리마운트되는 경우가 있는데, 그러면
// 프론트엔드 viewMode state는 'single'로 초기화되는데 네이티브 메뉴 체크 표시는 리마운트
// 전 상태(예: "한 페이지 연속")에 그대로 남아있는다 — 메뉴는 연속인데 실제로는 한 페이지로
// 동작하는 것처럼 보이는 불일치. 프론트엔드가 viewMode를 확정할 때마다 이걸 호출해 메뉴
// 체크 표시를 강제로 다시 맞춘다.
#[tauri::command]
fn set_view_mode_menu(app: tauri::AppHandle, mode: String) {
    update_view_mode_checks(&app, &format!("view-{mode}"));
}

// Tools 메뉴의 포인터/텍스트 선택/하이라이트 라디오 그룹 — 툴바 버튼이나 ⌘1/⌘2/⌘3으로
// 도구를 바꿔도 메뉴 체크 표시가 같이 맞도록, 프론트엔드가 toolMode 바뀔 때마다 호출한다.
struct ToolMenuItems(Vec<CheckMenuItem<tauri::Wry>>);

fn update_tool_checks(app: &tauri::AppHandle, selected: &str) {
    if let Some(items) = app.try_state::<ToolMenuItems>() {
        for item in items.0.iter() {
            let _ = item.set_checked(item.id().as_ref() == selected);
        }
    }
}

// tool: "pointer" | "text-select" | "highlight"
#[tauri::command]
fn set_tool_menu(app: tauri::AppHandle, tool: String) {
    update_tool_checks(&app, &format!("tool-{tool}"));
}

fn update_tab_menu_enabled(app: &tauri::AppHandle, tab_count: usize) {
    if let Some(items) = app.try_state::<TabMenuItems>() {
        let enabled = tab_count > 1;
        let _ = items.next.set_enabled(enabled);
        let _ = items.prev.set_enabled(enabled);
    }
}

// 문서 정보/인쇄/암호/다른 이름으로 저장 — 현재 열린 문서가 있을 때만 활성화.
// hwp는 MuPDF가 아니라 프론트엔드에서 직접 렌더링해서 이 탭 상태(AppState)에 안 잡히므로,
// Rust 쪽에서 자동으로 판단하지 않고 프론트가 filePath를 기준으로 직접 호출해준다.
fn update_document_menu_enabled(app: &tauri::AppHandle, enabled: bool) {
    if let Some(m) = app.try_state::<PlatformMenuItems>() {
        let _ = m.doc_info.set_enabled(enabled);
        let _ = m.print.set_enabled(enabled);
        let _ = m.password.set_enabled(enabled);
        let _ = m.save_as.set_enabled(enabled);
        let _ = m.export.set_enabled(enabled);
    }
}

#[tauri::command]
fn set_document_menu_enabled(app: tauri::AppHandle, enabled: bool) {
    update_document_menu_enabled(&app, enabled);
}

#[tauri::command]
fn cancel_font_analysis(app: tauri::AppHandle) {
    app.state::<CancelFlag>().0.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn get_font_stats(app: tauri::AppHandle) -> Result<Vec<FontStat>, String> {
    app.state::<CancelFlag>().0.store(false, Ordering::Relaxed);
    // 폰트 통계 집계도 무거운 동기 작업이라 메인 스레드에서 돌리면 그동안 웹뷰가 화면을 못 그림
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let s = state.lock().unwrap();
        let doc = s.tab().doc.as_ref().ok_or("No document open")?;
        let count = doc.page_count().map_err(|e| e.to_string())?;
        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        let mut total = 0u32;
        for p in 0..count {
            if app.state::<CancelFlag>().0.load(Ordering::Relaxed) {
                return Err("취소됨".to_string());
            }
            let page = doc.load_page(p).map_err(|e| e.to_string())?;
            let text = page.to_text_page(mupdf::TextPageFlags::empty()).map_err(|e| e.to_string())?;
            for block in text.blocks() {
                for line in block.lines() {
                    for ch in line.chars() {
                        if ch.char().map(|c| c.is_whitespace()).unwrap_or(true) { continue; }
                        if let Some(font) = ch.font() {
                            let raw = safe_font_name(&font);
                            let name = raw.find('+').map(|i| raw[i+1..].to_string()).unwrap_or(raw);
                            *counts.entry(name).or_insert(0) += 1;
                            total += 1;
                        }
                    }
                }
            }
        }
        if total == 0 { return Ok(vec![]); }
        let mut stats: Vec<FontStat> = counts.into_iter().map(|(name, c)| FontStat {
            percent: c as f32 / total as f32 * 100.0, name, count: c,
        }).collect();
        stats.sort_by(|a, b| b.count.cmp(&a.count));
        Ok(stats)
    }).await.map_err(|e| e.to_string())?
}

/// font.name()은 내부에서 unwrap()을 써서 잘못된 UTF-8 폰트명에서 패닉함.
/// catch_unwind로 감싸고 실패 시 "Unknown" 반환.
fn safe_font_name(font: &mupdf::Font) -> String {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| font.name().to_string()));
    std::panic::set_hook(prev);
    result.unwrap_or_else(|_| "Unknown".to_string())
}

fn quad_to_rect(q: &mupdf::Quad) -> [f32; 4] {
    [q.ul.x.min(q.ll.x), q.ul.y.min(q.ur.y), q.ur.x.max(q.lr.x), q.ll.y.max(q.lr.y)]
}

/// vim의 `\c`(대소문자 무시) / `\C`(대소문자 구분)와, Vimong 자체 확장인
/// `\r`(정규식 강제) / `\R`(리터럴 강제)를 검색어 "맨 끝"에서만 옵션으로 인식해
/// 토글 UI의 기본값을 오버라이드한다. 검색어 중간에 있는 `\c` 등은 그냥 리터럴로
/// 취급한다 — 위치를 끝으로 고정해야 검색어 자체에 우연히 같은 문자열이 섞여도
/// 안전하고, vim에서도 관례상 `\c`는 패턴 끝에 붙이므로 그 습관과도 맞는다.
/// `\r`/`\R`는 vim 정식 문법이 아니라 대소문자/정규식 두 토글을 검색어 안에서
/// 즉석으로 바꾸는 용도로 `\c`/`\C`와 같은 관례를 재사용한 것이다.
/// 끝에서부터 여러 개를 겹쳐 쓸 수 있고(`foo\R\c`), 같은 종류가 여러 번 나오면
/// 문자열 끝에 더 가까운(=더 나중에 붙인) 것이 우선한다.
fn parse_search_flags(query: &str, default_case_sensitive: bool) -> (String, bool, bool) {
    let mut case_sensitive = default_case_sensitive;
    // 대소문자 토글(Aa)과 달리 정규식은 토글 상태를 이어받지 않는다 — 검색어 끝에 \r이
    // 없으면 매번 일반 텍스트 검색으로 취급하고, .* 토글도 그 결과를 그대로 따라간다.
    let mut regex = false;
    let mut case_set = false;
    let mut regex_set = false;
    let mut rest = query;
    loop {
        let next = if let Some(p) = rest.strip_suffix("\\c") {
            if !case_set { case_sensitive = false; case_set = true; }
            p
        } else if let Some(p) = rest.strip_suffix("\\C") {
            if !case_set { case_sensitive = true; case_set = true; }
            p
        } else if let Some(p) = rest.strip_suffix("\\r") {
            if !regex_set { regex = true; regex_set = true; }
            p
        } else if let Some(p) = rest.strip_suffix("\\R") {
            if !regex_set { regex = false; regex_set = true; }
            p
        } else {
            break;
        };
        rest = next;
    }
    (rest.to_string(), case_sensitive, regex)
}

#[cfg(test)]
mod parse_search_flags_tests {
    use super::parse_search_flags;

    #[test]
    fn no_flag_keeps_case_default_but_regex_always_off() {
        assert_eq!(parse_search_flags("Hello", false), ("Hello".to_string(), false, false));
        // 대소문자 기본값은 그대로 이어받지만, \r이 없으면 정규식은 토글 상태와 무관하게 항상 꺼진다
        assert_eq!(parse_search_flags("Hello", true), ("Hello".to_string(), true, false));
    }

    #[test]
    fn c_flag_forces_ignore_case() {
        assert_eq!(parse_search_flags("Hello\\c", true), ("Hello".to_string(), false, false));
    }

    #[test]
    fn big_c_flag_forces_case_sensitive() {
        assert_eq!(parse_search_flags("Hello\\C", false), ("Hello".to_string(), true, false));
    }

    #[test]
    fn r_flag_forces_regex() {
        assert_eq!(parse_search_flags("a.b\\r", false), ("a.b".to_string(), false, true));
    }

    #[test]
    fn big_r_flag_forces_literal() {
        assert_eq!(parse_search_flags("a.b\\R", false), ("a.b".to_string(), false, false));
    }

    #[test]
    fn flags_in_the_middle_are_left_as_literal_text() {
        // 끝이 아니면 플래그로 취급하지 않는다 — 검색어 자체로 남아야 함
        assert_eq!(parse_search_flags("a\\Cb", false), ("a\\Cb".to_string(), false, false));
        assert_eq!(parse_search_flags("a\\rb", false), ("a\\rb".to_string(), false, false));
    }

    #[test]
    fn stacked_trailing_flags_of_different_kinds_both_apply() {
        assert_eq!(parse_search_flags("foo\\C\\r", false), ("foo".to_string(), true, true));
        assert_eq!(parse_search_flags("foo\\R\\c", true), ("foo".to_string(), false, false));
    }

    #[test]
    fn closer_to_the_end_wins_for_same_kind() {
        // "foo\c\C" -> 끝에서부터 벗겨내며 \C를 먼저 만나므로 \C가 우선
        assert_eq!(parse_search_flags("foo\\c\\C", false), ("foo".to_string(), true, false));
        assert_eq!(parse_search_flags("foo\\r\\R", false), ("foo".to_string(), false, false));
    }

    #[test]
    fn backslash_before_other_char_is_kept_literally() {
        assert_eq!(parse_search_flags("a\\db", false), ("a\\db".to_string(), false, false));
    }
}

// MuPDF의 page.search()는 항상 대소문자를 무시하도록 하드코딩되어 있어(mupdf-sys의
// fz_search_page 래퍼가 FZ_SEARCH_IGNORE_CASE를 고정으로 넘김), 대소문자 구분 검색과
// 정규식 검색은 문자 단위로 직접 스캔해서 구현한다.
fn extract_chars(page: &mupdf::Page) -> Result<Vec<(char, mupdf::Quad)>, String> {
    let text = page.to_text_page(mupdf::TextPageFlags::DEHYPHENATE).map_err(|e| e.to_string())?;
    let mut chars: Vec<(char, mupdf::Quad)> = Vec::new();
    for block in text.blocks() {
        for line in block.lines() {
            let start = chars.len();
            for ch in line.chars() {
                if let Some(c) = ch.char() {
                    chars.push((c, ch.quad()));
                }
            }
            // 줄바꿈은 공백으로 취급해 줄 경계를 넘는 매치도 찾을 수 있게 한다
            if chars.len() > start {
                let last_quad = chars[chars.len() - 1].1.clone();
                chars.push((' ', last_quad));
            }
        }
    }
    Ok(chars)
}

fn merge_quads(span: &[(char, mupdf::Quad)]) -> [f32; 4] {
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for (_, q) in span {
        x0 = x0.min(q.ul.x.min(q.ll.x));
        y0 = y0.min(q.ul.y.min(q.ur.y));
        x1 = x1.max(q.ur.x.max(q.lr.x));
        y1 = y1.max(q.ll.y.max(q.lr.y));
    }
    [x0, y0, x1, y1]
}

fn search_page_case_sensitive(page: &mupdf::Page, needle: &str, hit_max: usize) -> Result<Vec<[f32; 4]>, String> {
    let chars = extract_chars(page)?;
    let hay: Vec<char> = chars.iter().map(|(c, _)| *c).collect();
    let pat: Vec<char> = needle.chars().collect();
    if pat.is_empty() || hay.len() < pat.len() {
        return Ok(vec![]);
    }
    let mut rects = Vec::new();
    let mut i = 0;
    while i + pat.len() <= hay.len() && rects.len() < hit_max {
        if hay[i..i + pat.len()] == pat[..] {
            rects.push(merge_quads(&chars[i..i + pat.len()]));
            i += pat.len();
        } else {
            i += 1;
        }
    }
    Ok(rects)
}

/// 정규식 매치의 바이트 오프셋(`start..end`)을 `char_starts`(각 글자가 시작하는 바이트
/// 오프셋의 오름차순 목록)에서 글자 인덱스 구간으로 되짚는다. 매치 경계는 항상 유효한
/// UTF-8 문자 경계이므로 `char_starts`에 정확히 존재한다.
fn byte_range_to_char_range(char_starts: &[usize], start: usize, end: usize) -> Option<(usize, usize)> {
    if char_starts.is_empty() || start >= end {
        return None;
    }
    let start_idx = char_starts.partition_point(|&b| b <= start).checked_sub(1)?;
    let end_idx = char_starts.partition_point(|&b| b < end);
    if start_idx < end_idx && end_idx <= char_starts.len() {
        Some((start_idx, end_idx))
    } else {
        None
    }
}

/// vim의 기본 'magic' 모드 정규식 문법을 Rust regex 문법으로 옮긴다.
///
/// 지원: `\+ \? \= \{n,m\} \{-...\} \( \) \| \< \> \zs \ze`,
/// `\d \D \w \W \s \S`(그대로), `\a \A \l \L \u \U \x \X \o \O \h \H`(문자 클래스로 전개).
/// magic 모드에서 리터럴인 `( ) + ? { } |`는 이스케이프해서 그대로 리터럴로 유지한다.
///
/// 미지원(진짜 vim 엔진이 백트래킹으로만 구현하는 기능): `\@= \@! \@<= \@<!` 룩어라운드,
/// `\1`..`\9` 역참조, `\%(...\)`. Rust regex는 선형시간 보장을 위해 이런 기능을 의도적으로
/// 뺐기 때문에 지원하지 않는다 — 해당 시퀀스는 그대로 통과시켜 컴파일 단계에서 에러로 드러난다.
///
/// `\(...\)`는 캡처하지 않는 그룹 `(?:...)`으로 바뀐다 — 역참조를 지원하지 않으므로 굳이
/// 번호를 매길 이유가 없고, `\zs`/`\ze`로 감싼 구간만 캡처 그룹 1번으로 남겨서 실제 매치
/// 범위를 가리키는 데 쓴다. 반환값은 (변환된 패턴, `\zs`/`\ze`가 쓰였으면 그 그룹 번호).
fn vim_magic_to_rust_regex(pattern: &str) -> (String, Option<usize>) {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut zs_at: Option<usize> = None; // out 안에서 '(' 를 끼워 넣을 위치
    let mut ze_at: Option<usize> = None; // out 안에서 ')' 를 끼워 넣을 위치
    let mut chars = pattern.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // magic 모드에서 리터럴인데 Rust regex에서는 특수문자인 것들 -> 이스케이프
            '(' | ')' | '+' | '?' | '{' | '}' | '|' => {
                out.push('\\');
                out.push(c);
            }
            '\\' => match chars.next() {
                Some('+') => out.push('+'),
                Some('?') | Some('=') => out.push('?'),
                Some('(') => out.push_str("(?:"),
                Some(')') => out.push(')'),
                Some('|') => out.push('|'),
                Some('<') | Some('>') => out.push_str(r"\b"),
                Some('{') => {
                    let non_greedy = chars.peek() == Some(&'-');
                    if non_greedy { chars.next(); }
                    let mut body = String::new();
                    loop {
                        match chars.next() {
                            Some('\\') if chars.peek() == Some(&'}') => { chars.next(); break; }
                            Some('}') => break,
                            Some(nc) => body.push(nc),
                            None => break,
                        }
                    }
                    if body.is_empty() {
                        out.push('*');
                    } else {
                        out.push('{');
                        out.push_str(&body);
                        out.push('}');
                    }
                    if non_greedy { out.push('?'); }
                }
                Some('d') => out.push_str(r"\d"),
                Some('D') => out.push_str(r"\D"),
                Some('w') => out.push_str(r"\w"),
                Some('W') => out.push_str(r"\W"),
                Some('s') => out.push_str(r"\s"),
                Some('S') => out.push_str(r"\S"),
                Some('a') => out.push_str("[A-Za-z]"),
                Some('A') => out.push_str("[^A-Za-z]"),
                Some('l') => out.push_str("[a-z]"),
                Some('L') => out.push_str("[^a-z]"),
                Some('u') => out.push_str("[A-Z]"),
                Some('U') => out.push_str("[^A-Z]"),
                Some('x') => out.push_str("[0-9A-Fa-f]"),
                Some('X') => out.push_str("[^0-9A-Fa-f]"),
                Some('o') => out.push_str("[0-7]"),
                Some('O') => out.push_str("[^0-7]"),
                Some('h') => out.push_str("[A-Za-z_]"),
                Some('H') => out.push_str("[^A-Za-z_]"),
                // \p(인쇄 가능 문자)는 Rust regex에 정확히 대응하는 토큰이 없어 "아무 문자나"(.)로
                // 근사한다 — 추출된 PDF 텍스트에는 실질적으로 제어문자/개행이 없으므로 충분히 안전하다.
                Some('p') => out.push('.'),
                Some('P') => out.push_str("[\\x00-\\x1f\\x7f]"),
                // \k(키워드 문자), \i(식별자 문자)는 vim의 'iskeyword'/'isident' 옵션에 따라 달라지지만
                // 기본값 기준으로는 단어 문자와 사실상 같아 \w 로 근사한다.
                Some('k') => out.push_str(r"\w"),
                Some('K') => out.push_str(r"\W"),
                Some('i') => out.push_str(r"\w"),
                Some('I') => out.push_str("[A-Za-z_]"),
                Some('z') => match chars.next() {
                    Some('s') => zs_at = Some(out.len()),
                    Some('e') => ze_at = Some(out.len()),
                    Some(other) => { out.push_str("\\z"); out.push(other); }
                    None => out.push_str("\\z"),
                },
                Some(other) => { out.push('\\'); out.push(other); } // \. \* \\ \^ \$ \[ \] 등은 vim/Rust regex 둘 다 리터럴 이스케이프라 그대로
                None => out.push('\\'),
            },
            _ => out.push(c),
        }
    }

    match (zs_at, ze_at) {
        (None, None) => (out, None),
        _ => {
            let start = zs_at.unwrap_or(0);
            let end = ze_at.unwrap_or(out.len());
            if start > end { return (out, None); } // \ze 가 \zs 보다 앞 -> 잘못된 패턴, 그룹 없이 반환
            let mut wrapped = String::with_capacity(out.len() + 2);
            wrapped.push_str(&out[..start]);
            wrapped.push('(');
            wrapped.push_str(&out[start..end]);
            wrapped.push(')');
            wrapped.push_str(&out[end..]);
            (wrapped, Some(1))
        }
    }
}

#[cfg(test)]
mod vim_magic_to_rust_regex_tests {
    use super::vim_magic_to_rust_regex;
    use regex::Regex;

    fn translate(pattern: &str) -> String { vim_magic_to_rust_regex(pattern).0 }

    #[test]
    fn literal_parens_and_plus_are_escaped() {
        let re = Regex::new(&translate("foo(bar)+")).unwrap();
        assert!(re.is_match("foo(bar)+"));
        assert!(!re.is_match("foobarbarbar")); // Rust regex라면 이렇게 매치했겠지만 vim 리터럴 의미로는 아니어야 함
    }

    #[test]
    fn backslash_plus_question_group_alternation() {
        let re = Regex::new(&translate(r"foo\+\(bar\|baz\)\?")).unwrap();
        assert!(re.is_match("foooo"));
        assert!(re.is_match("foobar"));
        assert!(re.is_match("foobaz"));
    }

    #[test]
    fn word_boundary() {
        let re = Regex::new(&translate(r"\<cat\>")).unwrap();
        assert!(re.is_match("a cat sat"));
        assert!(!re.is_match("category"));
    }

    #[test]
    fn braces_quantifier_and_non_greedy() {
        let re = Regex::new(&translate(r"a\{2,3\}")).unwrap();
        assert_eq!(re.find("aaaa").unwrap().as_str(), "aaa");

        let (pat, _) = vim_magic_to_rust_regex(r"a.\{-}b");
        let re = Regex::new(&pat).unwrap();
        assert_eq!(re.find("a123b456b").unwrap().as_str(), "a123b");
    }

    #[test]
    fn character_class_shorthands() {
        let re = Regex::new(&translate(r"\a\+\d\+")).unwrap();
        assert_eq!(re.find("id42x").unwrap().as_str(), "id42");
    }

    #[test]
    fn printable_and_keyword_classes() {
        // 사용자가 실제로 겪은 케이스: chap\p\pr -> chap + 인쇄가능문자 2개 + r
        let re = Regex::new(&translate(r"chap\p\pr")).unwrap();
        assert_eq!(re.find("chapter").unwrap().as_str(), "chapter");

        let re = Regex::new(&translate(r"\k\+")).unwrap();
        assert_eq!(re.find("foo_bar 123").unwrap().as_str(), "foo_bar");
    }

    #[test]
    fn same_syntax_passthrough() {
        // \d \w \s 는 vim과 Rust regex 표기가 동일하므로 그대로 통과해야 함
        assert_eq!(translate(r"\d\w\s"), r"\d\w\s");
    }

    #[test]
    fn zs_ze_marks_capture_group_for_real_match() {
        let (pat, group) = vim_magic_to_rust_regex(r"foo\zsbar\zebaz");
        assert_eq!(group, Some(1));
        let re = Regex::new(&pat).unwrap();
        let caps = re.captures("foobarbaz").unwrap();
        assert_eq!(caps.get(1).unwrap().as_str(), "bar");
    }

    #[test]
    fn zs_only_matches_from_there_to_end() {
        let (pat, group) = vim_magic_to_rust_regex(r"foo\zsbar");
        let re = Regex::new(&pat).unwrap();
        let caps = re.captures("foobar").unwrap();
        assert_eq!(caps.get(group.unwrap()).unwrap().as_str(), "bar");
    }

    #[test]
    fn no_zs_ze_returns_no_group() {
        assert_eq!(vim_magic_to_rust_regex("plain").1, None);
    }
}

// 정규식 매칭은 바이트 오프셋 기준이라, 페이지 문자열을 만들면서 각 글자가
// 시작하는 바이트 오프셋을 같이 기록해 두고 매치를 다시 글자 구간(quad)으로 되짚는다.
// `group`은 실제 하이라이트할 매치 범위를 담은 캡처 그룹 번호(\zs/\ze가 없으면 0=전체 매치).
fn search_page_regex(page: &mupdf::Page, re: &regex::Regex, group: usize, hit_max: usize) -> Result<Vec<[f32; 4]>, String> {
    let chars = extract_chars(page)?;
    let mut text = String::with_capacity(chars.len());
    let mut char_starts: Vec<usize> = Vec::with_capacity(chars.len());
    for (c, _) in &chars {
        char_starts.push(text.len());
        text.push(*c);
    }

    let mut rects = Vec::new();
    for caps in re.captures_iter(&text) {
        if rects.len() >= hit_max { break; }
        let Some(m) = caps.get(group) else { continue };
        if let Some((start_idx, end_idx)) = byte_range_to_char_range(&char_starts, m.start(), m.end()) {
            rects.push(merge_quads(&chars[start_idx..end_idx]));
        }
    }
    Ok(rects)
}

#[cfg(test)]
mod byte_range_to_char_range_tests {
    use super::byte_range_to_char_range;

    // "a한b" -> 'a'(1바이트, 오프셋0), '한'(3바이트, 오프셋1), 'b'(1바이트, 오프셋4), 끝=5
    fn starts() -> Vec<usize> { vec![0, 1, 4] }

    #[test]
    fn ascii_match() {
        assert_eq!(byte_range_to_char_range(&starts(), 0, 1), Some((0, 1))); // 'a'
    }

    #[test]
    fn multibyte_match() {
        assert_eq!(byte_range_to_char_range(&starts(), 1, 4), Some((1, 2))); // '한'
    }

    #[test]
    fn match_to_end_of_text() {
        assert_eq!(byte_range_to_char_range(&starts(), 4, 5), Some((2, 3))); // 'b'
    }

    #[test]
    fn spans_multiple_chars() {
        assert_eq!(byte_range_to_char_range(&starts(), 0, 5), Some((0, 3))); // "a한b" 전체
    }

    #[test]
    fn empty_match_rejected() {
        assert_eq!(byte_range_to_char_range(&starts(), 2, 2), None);
    }

    #[test]
    fn empty_char_starts() {
        assert_eq!(byte_range_to_char_range(&[], 0, 1), None);
    }
}

fn search_page_rects(page: &mupdf::Page, needle: &str, case_sensitive: bool) -> Result<Vec<[f32; 4]>, String> {
    if case_sensitive {
        search_page_case_sensitive(page, needle, 64)
    } else {
        Ok(page.search(needle, 64).map_err(|e| e.to_string())?.iter().map(quad_to_rect).collect())
    }
}

#[derive(serde::Serialize)]
struct PageHit {
    page: u32,
    hit_count: u32,
    rects: Vec<[f32; 4]>,
}

#[derive(serde::Serialize)]
struct SearchResponse {
    results: Vec<PageHit>,
    // 검색어에 섞인 \c/\C, \r/\R 오버라이드까지 반영한 실제 적용된 설정.
    // UI의 Aa/.* 토글이 이 값을 그대로 보여줘야 검색어 옵션과 아이콘이 어긋나지 않는다.
    case_sensitive: bool,
    regex: bool,
}

#[tauri::command]
fn search_all(query: String, case_sensitive: bool, state: State<Mutex<AppState>>) -> Result<SearchResponse, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.as_ref().ok_or("No document open")?;
    let count = doc.page_count().map_err(|e| e.to_string())?;
    let (query, case_sensitive, regex) = parse_search_flags(&query, case_sensitive);
    if query.is_empty() { return Ok(SearchResponse { results: vec![], case_sensitive, regex }); }

    let compiled_regex = if regex {
        let (translated, group) = vim_magic_to_rust_regex(&query);
        let re = RegexBuilder::new(&translated).case_insensitive(!case_sensitive).build().map_err(|e| e.to_string())?;
        Some((re, group.unwrap_or(0)))
    } else {
        None
    };

    // 일부 PDF는 단어 사이 간격이 MuPDF의 공백 인식 임계값보다 좁아 텍스트 추출 시
    // 공백 글자가 아예 사라진다 (예: "hello world" -> "helloworld"). 이 경우 공백이
    // 포함된 검색어는 절대 매치되지 않으므로, 페이지에서 못 찾으면 공백을 제거한
    // 검색어로 한 번 더 시도한다. (정규식은 공백 매칭을 패턴이 직접 통제하므로 대상 아님)
    let no_space_query: String = query.chars().filter(|c| !c.is_whitespace()).collect();
    let retry_no_space = compiled_regex.is_none() && no_space_query != query && !no_space_query.is_empty();
    let mut results = Vec::new();
    for p in 0..count {
        let page = doc.load_page(p).map_err(|e| e.to_string())?;
        let mut rects = match &compiled_regex {
            Some((re, group)) => search_page_regex(&page, re, *group, 64)?,
            None => search_page_rects(&page, &query, case_sensitive)?,
        };
        if rects.is_empty() && retry_no_space {
            rects = search_page_rects(&page, &no_space_query, case_sensitive)?;
        }
        if !rects.is_empty() {
            results.push(PageHit { page: p as u32, hit_count: rects.len() as u32, rects });
        }
    }
    Ok(SearchResponse { results, case_sensitive, regex })
}

// 로컬 PDF 파일 경로에 의존하는 일회성 성능 측정. 고정된 벤치마크 스위트가 아니라
// "정규식 검색이 기존 검색 대비 얼마나 느려지나"를 확인하기 위한 용도라
// 평소 `cargo test`에는 포함하지 않고 `--ignored`로만 돌린다.
#[cfg(test)]
mod search_perf {
    use std::time::Instant;

    fn time_all_pages<T>(doc: &mupdf::Document, mut f: impl FnMut(&mupdf::Page) -> Result<Vec<T>, String>) -> (u32, std::time::Duration) {
        let count = doc.page_count().unwrap();
        let start = Instant::now();
        let mut hits = 0u32;
        for p in 0..count {
            let page = doc.load_page(p).unwrap();
            hits += f(&page).unwrap().len() as u32;
        }
        (hits, start.elapsed())
    }

    fn run(path: &str, literal: &str, regex_pattern: &str) {
        let doc = mupdf::Document::open(path).expect("failed to open pdf");
        let pages = doc.page_count().unwrap();
        println!("\n=== {path} ({pages} pages) ===");

        let (hits, dur) = time_all_pages(&doc, |page| {
            Ok(page.search(literal, 64).map_err(|e| e.to_string())?.iter().map(super::quad_to_rect).collect())
        });
        println!("case-insensitive (mupdf) \"{literal}\": {hits} hits in {dur:?}");

        let (hits, dur) = time_all_pages(&doc, |page| super::search_page_case_sensitive(page, literal, 64));
        println!("case-sensitive (char scan) \"{literal}\": {hits} hits in {dur:?}");

        let re = regex::Regex::new(regex_pattern).expect("bad regex");
        let (hits, dur) = time_all_pages(&doc, |page| super::search_page_regex(page, &re, 0, 64));
        println!("regex \"{regex_pattern}\": {hits} hits in {dur:?}");
    }

    #[test]
    #[ignore]
    fn slide_deck_246_pages() {
        run(
            "/Volumes/DATA/Downloads/독하게 시작하는 C 프로그래밍 - v1.2 20231001.pdf",
            "컴퓨터",
            r"컴퓨터|포인터|[0-9]+진법",
        );
    }

    #[test]
    #[ignore]
    fn latex_toc_32_pages() {
        run("/Volumes/DATA/Downloads/oblivoir-simpledoc.pdf", "oblivoir", r"oblivoir\w*");
    }
}

#[derive(serde::Serialize)]
struct TocEntry {
    title: String,
    page: Option<u32>,
    // 헤딩이 페이지 안에서 위치한 세로 좌표(위쪽 기준) — 있으면 페이지 이동 후 이
    // 지점이 뷰 맨 위로 오게 스크롤한다. Fit/FitV처럼 세로 위치 정보가 없는
    // 목적지 종류는 None (기존처럼 페이지 맨 위로만 이동).
    y: Option<f32>,
    children: Vec<TocEntry>,
}

fn convert_outline(o: mupdf::Outline) -> TocEntry {
    let (page, y) = match &o.dest {
        Some(d) => {
            let y = match d.kind {
                DestinationKind::XYZ { top, .. } => top,
                DestinationKind::FitH { top } => top,
                DestinationKind::FitBH { top } => top,
                _ => None,
            };
            (Some(d.loc.page_number), y)
        }
        None => (None, None),
    };
    TocEntry {
        title: o.title,
        page,
        y,
        children: o.down.into_iter().map(convert_outline).collect(),
    }
}

#[tauri::command]
fn get_toc(state: State<Mutex<AppState>>) -> Result<Vec<TocEntry>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.as_ref().ok_or("No document open")?;
    let outlines = doc.outlines().map_err(|e| e.to_string())?;
    Ok(outlines.into_iter().map(convert_outline).collect())
}

// PDF 옵셔널 콘텐츠(OCG, Acrobat에서 "레이어" 패널로 보이는 것) — 도면/다국어 문서에서
// 콘텐츠 그룹을 켜고 끈다. 레이어가 아예 없는 일반 문서가 대다수라 빈 배열이면 프론트가
// 사이드바 탭 자체를 숨긴다.
//
// ArcGIS Pro 같은 도구가 내보내는 지도 PDF는 "Map Frame > Map > Light Gray Base"처럼 레이어를
// 중첩시켜(포토샵 그룹처럼) 감싸고, 콘텐츠 스트림도 BDC/OC를 실제로 중첩해서 안쪽 레이어가
// 보이려면 바깥 레이어도 같이 켜져 있어야 하게 만든다. 이 중첩 구조 자체는 optional_content_groups
// (OCGs 배열, 평평함)엔 없고 /OCProperties/D/Order 배열에만 있다 — "OCG 참조 바로 뒤에 배열이
// 오면 그 배열이 앞 OCG의 자식 목록"이라는 PDF 32000-1 8.11.4.3의 관례를 직접 파싱해야 한다.
#[derive(Clone, serde::Serialize)]
struct OptionalContentGroupInfo {
    xref: i32,
    name: Option<String>,
    enabled: bool,
    children: Vec<OptionalContentGroupInfo>,
}

// Order 배열 하나를 재귀적으로 트리로 바꾼다. used에 넣은 xref는 최상위 fallback(아래)에서
// 다시 안 나오게 추적한다. depth 상한은 손상되었거나 악의적으로 순환시킨 PDF에 대한 방어.
fn parse_ocg_order(
    arr: &PdfObject,
    by_xref: &std::collections::HashMap<i32, OptionalContentGroupInfo>,
    used: &mut std::collections::HashSet<i32>,
    depth: u32,
) -> Vec<OptionalContentGroupInfo> {
    if depth > 16 {
        return Vec::new();
    }
    let len = arr.len().unwrap_or(0) as i32;
    let mut out = Vec::new();
    let mut i = 0;
    while i < len {
        let Some(item) = arr.get_array(i).ok().flatten() else { i += 1; continue };
        if item.is_array().unwrap_or(false) {
            // 부모 OCG 없이 등장한 배열(라벨 전용 그룹 등) — 그냥 같은 레벨로 펼친다
            out.extend(parse_ocg_order(&item, by_xref, used, depth + 1));
            i += 1;
            continue;
        }
        let xref = item.as_indirect().unwrap_or(-1);
        let Some(info) = by_xref.get(&xref) else { i += 1; continue }; // 문자열 라벨 등은 건너뜀
        if !used.insert(xref) {
            i += 1; // 이미 나온 xref(순환/중복) — 다시 안 넣는다
            continue;
        }
        // 바로 다음 원소가 배열이면 이 OCG의 자식 목록
        let children = match arr.get_array(i + 1).ok().flatten() {
            Some(next) if next.is_array().unwrap_or(false) => {
                i += 1; // 자식 배열도 같이 소비
                parse_ocg_order(&next, by_xref, used, depth + 1)
            }
            _ => Vec::new(),
        };
        out.push(OptionalContentGroupInfo { children, ..info.clone() });
        i += 1;
    }
    out
}

#[tauri::command]
fn get_optional_content_groups(state: State<Mutex<AppState>>) -> Result<Vec<OptionalContentGroupInfo>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    if !doc.is_pdf() {
        return Ok(vec![]);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let flat = pdf.optional_content_groups().map_err(|e| e.to_string())?;
    let flat: Vec<OptionalContentGroupInfo> = flat
        .into_iter()
        .map(|g| OptionalContentGroupInfo { xref: g.reference.xref(), name: g.name, enabled: g.enabled, children: Vec::new() })
        .collect();
    let by_xref: std::collections::HashMap<i32, OptionalContentGroupInfo> =
        flat.iter().map(|info| (info.xref, info.clone())).collect();

    let order = pdf
        .catalog()
        .ok()
        .and_then(|c| c.get_dict("OCProperties").ok().flatten())
        .and_then(|p| p.get_dict("D").ok().flatten())
        .and_then(|d| d.get_dict("Order").ok().flatten());

    let mut used = std::collections::HashSet::new();
    let mut tree = match &order {
        Some(order) if order.is_array().unwrap_or(false) => parse_ocg_order(order, &by_xref, &mut used, 0),
        _ => Vec::new(),
    };
    // Order에 안 나온(또는 Order 자체가 없는) 나머지 OCG는 등장 순서대로 최상위에 덧붙인다 —
    // PDF 스펙상 Order가 언급하지 않은 그룹도 계속 표시돼야 한다.
    for info in &flat {
        if used.insert(info.xref) {
            tree.push(info.clone());
        }
    }
    Ok(tree)
}

// 체크박스 토글과 같은 패턴(set_form_text_value) — 실패하면 디스크에서 다시 열어 문서를
// 일관된 상태로 되돌린다. 렌더링에 영향을 주는 문서 상태 변경이라 dirty로 표시해 :w로
// 저장할 수 있게 한다(안 그러면 다음에 열 때 켜둔/꺼둔 레이어가 그대로 사라진다).
//
// set_optional_content_enabled는 /OCProperties 딕셔너리만 고쳐 쓸 뿐인데, mupdf의 렌더러는
// 문서 생애주기 동안 딱 한 번(첫 렌더 때) 캐시해두는 pdf_ocg_descriptor를 보고 그린다 — 그
// 캐시는 pdf_enable_layer 같은 전용 API로만 갱신되고 이 크레이트는 그 API를 안 감쌌다. 그래서
// 딕셔너리만 고치면 optional_content_groups()로 다시 읽었을 땐 바뀐 게 보여도(그래서 유닛
// 테스트는 통과) 화면엔 아무 변화가 없다 — save_document(:w)가 저장 후 디스크에서 다시 열어
// tab.doc를 교체하는 것과 같은 이유로, 여기서도 메모리 버퍼에 한 번 써냈다가 그 버퍼로 새
// Document를 열어 캐시를 통째로 새로 만든다. 기본 옵션(do_garbage=0)이라 xref 번호가 그대로
// 유지돼 프론트가 들고 있는 다른 xref(하이라이트/폼 필드 등)와 어긋나지 않는다.
#[tauri::command]
fn toggle_optional_content(state: State<Mutex<AppState>>, xref: i32) -> Result<bool, String> {
    let (doc, path) = {
        let mut s = state.lock().unwrap();
        let tab = s.tab_mut();
        let path = tab.path.clone().ok_or("No document open")?;
        let doc = tab.doc.take().ok_or("No document open")?;
        (doc, path)
    };

    let result: Result<(Document, bool), String> = (|| {
        let mut pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let reference: OptionalContentRef = xref.try_into().map_err(|e: mupdf::Error| e.to_string())?;
        let enabled = !pdf.optional_content_enabled(reference).map_err(|e| e.to_string())?;
        pdf.set_optional_content_enabled(reference, enabled).map_err(|e| e.to_string())?;
        let mut buf: Vec<u8> = Vec::new();
        pdf.write_to(&mut buf).map_err(|e| e.to_string())?;
        let reopened = Document::from_bytes(&buf, "pdf").map_err(|e| e.to_string())?;
        Ok((reopened, enabled))
    })();

    let mut s = state.lock().unwrap();
    let tab = s.tab_mut();
    match result {
        Ok((updated, enabled)) => {
            tab.doc = Some(updated);
            tab.dirty = true;
            Ok(enabled)
        }
        Err(e) => {
            drop(s);
            if let Ok(reopened) = Document::open(&path) {
                state.lock().unwrap().tab_mut().doc = Some(reopened);
            }
            Err(e)
        }
    }
}

// PDF에 첨부된 파일(Acrobat의 "첨부파일" 패널) — 카탈로그 /Names/EmbeddedFiles에 걸린 파일명
// 목록만 보여준다. 크기/다운로드는 없음 — 크기를 구하려면 내용을 읽어야 해서(압축 해제든 raw
// 스트림이든) PDF를 열 때마다 자동으로 비용이 드는데, 그럴 가치가 없다고 판단해 뺐다.
#[tauri::command]
fn get_attachments(state: State<Mutex<AppState>>) -> Result<Vec<String>, String> {
    let s = state.lock().unwrap();
    let doc = s.tab().doc.clone().ok_or("No document open")?;
    drop(s);
    if !doc.is_pdf() {
        return Ok(vec![]);
    }
    let pdf: PdfDocument = doc.try_into().map_err(|e: mupdf::Error| e.to_string())?;
    let files = pdf.embedded_files().map_err(|e| e.to_string())?;
    Ok(files.into_iter().map(|info| info.filename.unwrap_or(info.name)).collect())
}

#[cfg(test)]
mod attachment_tests {
    use super::PdfDocument;
    use mupdf::pdf::EmbeddedFileOptions;

    #[test]
    fn lists_embedded_file_names() {
        let mut doc = PdfDocument::new();
        doc.add_embedded_file("payload", b"hello attachment", EmbeddedFileOptions::new("payload.txt")).unwrap();

        let infos = doc.embedded_files().unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0].filename.as_deref(), Some("payload.txt"));
    }
}

#[cfg(test)]
mod optional_content_tests {
    use super::{OptionalContentGroupInfo, PdfDocument};
    use mupdf::{Colorspace, Document, Matrix};

    fn to_ocg_info(g: mupdf::OptionalContentGroup) -> OptionalContentGroupInfo {
        OptionalContentGroupInfo { xref: g.reference.xref(), name: g.name, enabled: g.enabled, children: Vec::new() }
    }

    #[test]
    fn lists_groups_and_toggle_flips_only_the_targeted_one() {
        let mut doc = PdfDocument::new();
        let a = doc.add_optional_content_group("Layer A").unwrap();
        let b = doc.add_optional_content_group("Layer B").unwrap();

        let infos: Vec<_> = doc.optional_content_groups().unwrap().into_iter().map(to_ocg_info).collect();
        assert_eq!(infos.len(), 2);
        assert_eq!(infos[0].name.as_deref(), Some("Layer A"));
        assert!(infos[0].enabled && infos[1].enabled); // 새 OCG는 기본 켜짐

        let enabled = !doc.optional_content_enabled(a).unwrap();
        doc.set_optional_content_enabled(a, enabled).unwrap();
        assert!(!enabled);

        let infos: Vec<_> = doc.optional_content_groups().unwrap().into_iter().map(to_ocg_info).collect();
        assert!(!infos[0].enabled); // A만 꺼짐
        assert!(infos[1].enabled); // B는 그대로 — 다른 레이어를 건드리지 않는다
        assert_eq!(infos[1].xref, b.xref());
    }

    // ArcGIS Pro류 지도 PDF의 실제 구조("Map Frame > Map > Light Gray Base")에서 확인한
    // /OCProperties/D/Order 관례(OCG 참조 바로 뒤에 배열이 오면 그 OCG의 자식 목록)를
    // 그대로 재현해 parse_ocg_order가 트리를 올바르게 구성하는지 검증한다.
    #[test]
    fn builds_nested_tree_from_order_array() {
        let mut doc = PdfDocument::new();
        let parent = doc.add_optional_content_group("Parent").unwrap();
        let child_a = doc.add_optional_content_group("Child A").unwrap();
        let child_b = doc.add_optional_content_group("Child B").unwrap();

        let mut children = doc.new_array().unwrap();
        children.array_push(doc.new_indirect(child_a.xref(), 0).unwrap()).unwrap();
        children.array_push(doc.new_indirect(child_b.xref(), 0).unwrap()).unwrap();
        let mut order = doc.new_array().unwrap();
        order.array_push(doc.new_indirect(parent.xref(), 0).unwrap()).unwrap();
        order.array_push(children).unwrap();

        let mut d = doc.catalog().unwrap().get_dict("OCProperties").unwrap().unwrap().get_dict("D").unwrap().unwrap();
        d.dict_put("Order", order).unwrap();

        let flat: Vec<_> = doc.optional_content_groups().unwrap().into_iter().map(to_ocg_info).collect();
        let by_xref: std::collections::HashMap<i32, _> = flat.iter().map(|i| (i.xref, i.clone())).collect();
        let order_obj = doc.catalog().unwrap().get_dict("OCProperties").unwrap().unwrap().get_dict("D").unwrap().unwrap().get_dict("Order").unwrap().unwrap();
        let mut used = std::collections::HashSet::new();
        let tree = super::parse_ocg_order(&order_obj, &by_xref, &mut used, 0);

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].name.as_deref(), Some("Parent"));
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].name.as_deref(), Some("Child A"));
        assert_eq!(tree[0].children[1].name.as_deref(), Some("Child B"));
        assert_eq!(used.len(), 3); // 부모 + 자식 둘 다 소비됨 — 최상위 fallback에서 중복 안 됨
    }

    // 손으로 최소 PDF를 만든다 — /OC BDC/EMC로 감싼 빨강(Layer A)/파랑(Layer B) 사각형 하나씩.
    // xref 오프셋 계산이 전부라 이 정도는 직접 쓰는 게 외부 라이브러리보다 빠르고 명확하다.
    fn build_ocg_test_pdf() -> Vec<u8> {
        let content = "/OC /OC1 BDC\n1 0 0 rg 10 10 80 80 re f\nEMC\n/OC /OC2 BDC\n0 0 1 rg 110 110 80 80 re f\nEMC\n";
        let objs = [
            "<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [5 0 R 6 0 R] /D << /ON [5 0 R 6 0 R] /Order [5 0 R 6 0 R] >> >> >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Properties << /OC1 5 0 R /OC2 6 0 R >> >> /Contents 4 0 R >>".to_string(),
            format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
            "<< /Type /OCG /Name (Layer A) >>".to_string(),
            "<< /Type /OCG /Name (Layer B) >>".to_string(),
        ];
        let mut out = String::from("%PDF-1.5\n");
        let mut offsets = Vec::with_capacity(objs.len());
        for (i, body) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", i + 1));
        }
        let xref_offset = out.len();
        out.push_str(&format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1));
        for off in &offsets {
            out.push_str(&format!("{off:010} 00000 n \n"));
        }
        out.push_str(&format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF", objs.len() + 1));
        out.into_bytes()
    }

    fn has_color(pixmap: &mupdf::Pixmap, want: [u8; 3]) -> bool {
        pixmap.samples().chunks_exact(3).any(|p| {
            (p[0] as i32 - want[0] as i32).abs() < 40
                && (p[1] as i32 - want[1] as i32).abs() < 40
                && (p[2] as i32 - want[2] as i32).abs() < 40
        })
    }

    // toggle_optional_content가 set_optional_content_enabled 호출만으로 끝내지 않고 굳이
    // write_to + Document::from_bytes로 다시 여는 이유를 그대로 재현·검증한다: mupdf는 첫
    // 렌더 때 OCG on/off 스냅샷을 캐시해두고, 그 뒤로는 /OCProperties 딕셔너리를 고쳐도 같은
    // Document 인스턴스로는 화면이 안 바뀐다(사용자가 신고한 "레이어 꺼도 아무 차이 없음").
    // 이 회귀 테스트가 없으면 나중에 누가 "간단하게" write_to+reopen을 빼버려도 dict만 보는
    // lists_groups_and_toggle_flips_only_the_targeted_one 테스트는 계속 통과해버린다.
    #[test]
    fn toggling_off_removes_layer_from_render_only_after_reopen() {
        let doc = Document::from_bytes(&build_ocg_test_pdf(), "pdf").unwrap();
        let matrix = Matrix::new_scale(2.0, 2.0);
        let red = [255, 0, 0];
        let blue = [0, 0, 255];

        let page = doc.load_page(0).unwrap();
        let before = page.to_pixmap(&matrix, &Colorspace::device_rgb(), false, true).unwrap();
        assert!(has_color(&before, red) && has_color(&before, blue));

        let mut pdf: PdfDocument = doc.try_into().unwrap();
        let layer_a = pdf.optional_content_groups().unwrap().into_iter().find(|g| g.name.as_deref() == Some("Layer A")).unwrap().reference;
        pdf.set_optional_content_enabled(layer_a, false).unwrap();

        // 같은 Document로는 여전히 빨강이 보여야 한다(버그 재현) — 캐시된 서술자가 그대로라서.
        let stale: Document = (*pdf).clone();
        let stale_pixmap = stale.load_page(0).unwrap().to_pixmap(&matrix, &Colorspace::device_rgb(), false, true).unwrap();
        assert!(has_color(&stale_pixmap, red), "캐시 재사용 시 여전히 빨강이 남아있어야(버그 재현 확인)");

        // toggle_optional_content와 같은 write_to + from_bytes 경로 — 여기서부터 실제 수정.
        let mut buf = Vec::new();
        pdf.write_to(&mut buf).unwrap();
        let reopened = Document::from_bytes(&buf, "pdf").unwrap();
        let after = reopened.load_page(0).unwrap().to_pixmap(&matrix, &Colorspace::device_rgb(), false, true).unwrap();
        assert!(!has_color(&after, red), "재오픈 후엔 꺼진 Layer A(빨강)가 사라져야 함");
        assert!(has_color(&after, blue), "건드리지 않은 Layer B(파랑)는 그대로 보여야 함");
    }
}

#[derive(serde::Serialize)]
struct PageLink {
    rect: [f32; 4],
    page: Option<u32>,
    // convert_outline(TocEntry)와 같은 이유 — 각주처럼 같은/다른 페이지의 특정 세로
    // 위치를 가리키는 링크가 페이지 맨 위가 아니라 실제 목적지로 스크롤되게 한다.
    y: Option<f32>,
    uri: Option<String>,
}

#[tauri::command]
async fn get_page_links(app: tauri::AppHandle, page_num: u32) -> Result<Vec<PageLink>, String> {
    // get_page_size와 같은 이유로 spawn_blocking — render_page가 붙든 Mutex를 메인
    // 스레드에서 동기로 기다리게 두지 않는다.
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Mutex<AppState>>();
        let s = state.lock().unwrap();
        let doc = s.tab().doc.as_ref().ok_or("No document open")?;
        let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
        let links = page.links().map_err(|e| e.to_string())?;
        Ok(links
            .map(|l| {
                let rect = [l.bounds.x0, l.bounds.y0, l.bounds.x1, l.bounds.y1];
                match l.dest {
                    Some(dest) => {
                        let y = match dest.kind {
                            DestinationKind::XYZ { top, .. } => top,
                            DestinationKind::FitH { top } => top,
                            DestinationKind::FitBH { top } => top,
                            _ => None,
                        };
                        PageLink { rect, page: Some(dest.loc.page_number), y, uri: None }
                    }
                    None => PageLink { rect, page: None, y: None, uri: Some(l.uri) },
                }
            })
            .collect())
    }).await.map_err(|e| e.to_string())?
}

#[derive(serde::Serialize)]
pub(crate) struct TextChar {
    ch: char,
    rect: [f32; 4],
    font: Option<String>,
}

// extract_chars(검색용)와 거의 같은 순회지만 폰트까지 뽑는다. 검색 쪽은 폰트가 필요
// 없고 이미 테스트로 검증돼 있어서, 그걸 건드리는 대신 여기서 따로 순회한다.
fn extract_chars_with_font(page: &mupdf::Page) -> Result<Vec<TextChar>, String> {
    let text = page.to_text_page(mupdf::TextPageFlags::DEHYPHENATE).map_err(|e| e.to_string())?;
    let mut chars: Vec<TextChar> = Vec::new();
    for block in text.blocks() {
        for line in block.lines() {
            let start = chars.len();
            for ch in line.chars() {
                if let Some(c) = ch.char() {
                    let font = ch.font().map(|f| safe_font_name(&f));
                    chars.push(TextChar { ch: c, rect: quad_to_rect(&ch.quad()), font });
                }
            }
            // 줄바꿈은 공백으로 취급 — extract_chars와 같은 이유(문자 인덱스 연속성 유지)
            if chars.len() > start {
                let last = &chars[chars.len() - 1];
                chars.push(TextChar { ch: ' ', rect: last.rect, font: last.font.clone() });
            }
        }
    }
    Ok(chars)
}

#[cfg(target_os = "macos")]
enum PageCharsResult {
    Chars(Vec<TextChar>),
    NeedsOcr { png: Vec<u8>, page_w: f32, page_h: f32 },
}

#[tauri::command]
async fn get_page_chars(app: tauri::AppHandle, page_num: u32) -> Result<Vec<TextChar>, String> {
    // get_page_links와 같은 이유로 spawn_blocking
    tauri::async_runtime::spawn_blocking(move || {
        // OCR(수 초 걸림)을 Mutex<AppState> 잠금을 쥔 채로 돌리면, 그동안 다른 페이지의
        // render_page 등 같은 락이 필요한 모든 커맨드가 멈춰버린다(다른 페이지로 넘기면
        // 몇 초간 흰 화면). 그래서 문서 접근이 필요한 부분(텍스트 추출, OCR용 PNG 렌더)만
        // 락을 쥔 채로 끝내고, 느린 OCR 자체는 락을 놓은 뒤에 돌린다.
        #[cfg(target_os = "macos")]
        let outcome = {
            let state = app.state::<Mutex<AppState>>();
            let s = state.lock().unwrap();
            let doc = s.tab().doc.as_ref().ok_or("No document open")?;
            let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
            let chars = extract_chars_with_font(&page)?;
            if chars.is_empty() {
                // 임베드 텍스트가 하나도 없으면(순수 스캔 이미지 PDF) Vision OCR로 폴백.
                // 화면 표시용 render_page가 이미 이 페이지를 그려뒀으면(대개 그렇다 —
                // 화면에 보이는 페이지라 텍스트 선택을 시도하는 것이므로) 그 결과를 그대로
                // 재사용해 똑같은 페이지를 두 번 래스터화하지 않는다.
                let cache = app.state::<Mutex<Option<LastRender>>>();
                let cached = cache.lock().unwrap();
                let png = if let Some(lr) = cached.as_ref().filter(|lr| lr.page == page_num) {
                    let mut pixmap = mupdf::Pixmap::new_with_w_h(&Colorspace::device_rgb(), lr.w as i32, lr.h as i32, true)
                        .map_err(|e| e.to_string())?;
                    pixmap.samples_mut().copy_from_slice(&lr.rgba);
                    let mut png = Vec::new();
                    pixmap.write_to(&mut png, ImageFormat::PNG).map_err(|e| e.to_string())?;
                    png
                } else {
                    drop(cached);
                    // 아직 표시용 렌더가 없었던 페이지(예: 화면 밖 페이지에 검색으로 바로
                    // 이동) — 화면 배율과 무관하게 OCR 품질용 고정 배율(216dpi 상당)로 렌더
                    let matrix = Matrix::new_scale(3.0, 3.0);
                    let pixmap = page.to_pixmap(&matrix, &Colorspace::device_rgb(), false, true).map_err(|e| e.to_string())?;
                    let mut png = Vec::new();
                    pixmap.write_to(&mut png, ImageFormat::PNG).map_err(|e| e.to_string())?;
                    png
                };
                let bounds = page.bounds().map_err(|e| e.to_string())?;
                let (w, h) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
                PageCharsResult::NeedsOcr { png, page_w: w, page_h: h }
            } else {
                PageCharsResult::Chars(chars)
            }
        }; // <- 락이 여기서 풀린다

        #[cfg(target_os = "macos")]
        {
            return match outcome {
                PageCharsResult::Chars(c) => Ok(c),
                PageCharsResult::NeedsOcr { png, page_w, page_h } => ocr_macos::ocr_page_chars(&png, page_w, page_h),
            };
        }

        #[cfg(not(target_os = "macos"))]
        {
            let state = app.state::<Mutex<AppState>>();
            let s = state.lock().unwrap();
            let doc = s.tab().doc.as_ref().ok_or("No document open")?;
            let page = doc.load_page(page_num as i32).map_err(|e| e.to_string())?;
            extract_chars_with_font(&page)
        }
    }).await.map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 이미 떠 있는 인스턴스로 --forward-search와 함께 다시 실행된 경우 —
            // 새 창을 띄우는 대신 이 창으로 이벤트를 보내고 포커스만 가져온다.
            if let Some(fs) = parse_forward_search_args(&args) {
                let _ = app.emit("forward-search", fs);
            }
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(Mutex::new(AppState::new()))
        .manage(Mutex::new(None::<LastRender>))
        .manage(CancelFlag(AtomicBool::new(false)))
        .manage(QuitConfirmed(AtomicBool::new(false)))
        .manage(SavedWindowFrame(Mutex::new(None)))
        .manage(PendingForwardSearch(Mutex::new(parse_forward_search_args(
            &std::env::args().collect::<Vec<_>>(),
        ))))
        .manage(PendingFileOpen(Mutex::new(parse_pending_file_open_arg(
            &std::env::args().collect::<Vec<_>>(),
        ))))
        .setup(|app| {
            let menu_lang = resolve_lang(app.handle());
            let l = labels_for(&menu_lang);
            app.manage(MenuLang(Mutex::new(menu_lang)));
            app.manage(SidebarVisibleState(AtomicBool::new(true)));
            app.manage(ToolbarVisibleState(AtomicBool::new(true)));
            // PredefinedMenuItem::close_window(Cmd+W)은 창 자체를 닫아버린다 —
            // 이 앱은 탭이 여러 개라 Cmd+W는 "현재 탭 닫기"(:q와 동일)여야 한다.
            let close = MenuItem::with_id(app, "close-tab", l.close_tab, true, Some("CmdOrCtrl+W"))?;

            let actual_size = MenuItem::with_id(app, "zoom-actual",    l.actual_size,       true, None::<&str>)?;
            let zoom_fit    = MenuItem::with_id(app, "zoom-fit",        l.zoom_to_fit,        true, None::<&str>)?;
            let zoom_fit_h  = MenuItem::with_id(app, "zoom-fit-height", l.zoom_to_fit_height, true, None::<&str>)?;
            // "CmdOrCtrl+Plus"는 muda가 인식 못 하는 키 이름이라 파싱 실패 — Tauri가 파싱
            // 에러를 조용히 무시(.ok())해서 크래시 없이 그냥 단축키가 안 먹는 상태였다.
            // 크롬처럼 물리적 Equal 키(Shift 없이도 눌리는 =/+ 키)에 바로 건다.
            let zoom_in     = MenuItem::with_id(app, "zoom-in",        l.zoom_in,           true, Some("CmdOrCtrl+="))?;
            let zoom_out    = MenuItem::with_id(app, "zoom-out",       l.zoom_out,          true, Some("CmdOrCtrl+Minus"))?;
            let zoom_sel    = MenuItem::with_id(app, "zoom-selection", l.zoom_to_selection, true, Some("CmdOrCtrl+Shift+8"))?;
            let sep_v       = PredefinedMenuItem::separator(app)?;
            let sep_v2      = PredefinedMenuItem::separator(app)?;
            // 시작 시 사이드바가 보이는 상태이므로 초기 라벨은 "숨기기" — 프론트엔드가
            // sidebarVisible 값이 바뀔 때마다 set_sidebar_visible_menu로 갱신한다.
            let toggle_sb   = MenuItem::with_id(app, "toggle-sidebar", l.hide_sidebar, true, Some("CmdOrCtrl+Shift+T"))?;
            // 시작 시 툴바가 보이는 상태이므로 초기 라벨은 "숨기기" — toggle_sb와 같은 패턴.
            let toggle_tb   = MenuItem::with_id(app, "toggle-toolbar", l.hide_toolbar, true, Some("CmdOrCtrl+Shift+B"))?;
            // close-tab(CmdOrCtrl+W)과 동일한 패턴 — 네이티브 accelerator로 등록해서 메뉴에
            // 단축키가 표시되게 하고, 실제 동작은 on_menu_event → 'menu' 이벤트로 처리한다.
            let fullscreen  = MenuItem::with_id(app, "toggle-fullscreen", l.fullscreen, true, Some("Ctrl+Cmd+F"))?;

            let view_single      = CheckMenuItem::with_id(app, "view-single", l.view_single, true, true, None::<&str>)?;
            let view_single_cont = CheckMenuItem::with_id(app, "view-single-continuous", l.view_single_continuous, true, false, None::<&str>)?;
            let view_two         = CheckMenuItem::with_id(app, "view-two", l.view_two, true, false, None::<&str>)?;
            let view_two_cont    = CheckMenuItem::with_id(app, "view-two-continuous", l.view_two_continuous, true, false, None::<&str>)?;
            let view_horiz_cont  = CheckMenuItem::with_id(app, "view-horizontal-continuous", l.view_horizontal_continuous, true, false, None::<&str>)?;
            let sep_v3           = PredefinedMenuItem::separator(app)?;
            app.manage(ViewModeMenuItems(vec![
                view_single.clone(), view_single_cont.clone(), view_two.clone(),
                view_two_cont.clone(), view_horiz_cont.clone(),
            ]));

            let view_menu   = Submenu::with_items(app, l.view, true,
                &[&actual_size, &zoom_fit, &zoom_fit_h, &sep_v, &zoom_in, &zoom_out, &zoom_sel, &sep_v2, &toggle_sb, &toggle_tb,
                  &fullscreen,
                  &sep_v3, &view_single, &view_single_cont, &view_two, &view_two_cont, &view_horiz_cont])?;

            // 텍스트 선택 도구로 고른 텍스트를 복사하는 용도 — Cmd+C는 여기 메뉴
            // 단축키로 등록해야 잡힌다. WKWebView가 실제 네이티브 DOM 선택이 없으면
            // Cmd+C를 자기 responder chain에서 조용히 삼켜버려서(copy: 셀렉터), 메뉴
            // 단축키 없이는 keydown 이벤트 자체가 JS까지 오지 않는다.
            let copy_sel = MenuItem::with_id(app, "copy-selection", l.copy, true, Some("CmdOrCtrl+C"))?;
            // Cut/Paste/Select All도 같은 이유로 메뉴에 등록해야 잡힌다(바로 위 주석 참고) —
            // 이게 없으면 설정 창 등 일반 텍스트 입력창에서도 Cmd+V/X/A가 아예 안 먹는다.
            // 라벨은 PredefinedMenuItem이 OS 로케일 기준으로 알아서 붙여주므로 Labels에 따로 안 둔다.
            let cut_std = PredefinedMenuItem::cut(app, None)?;
            let paste_std = PredefinedMenuItem::paste(app, None)?;
            let select_all_std = PredefinedMenuItem::select_all(app, None)?;
            let edit_menu = Submenu::with_items(app, l.edit, true, &[&copy_sel, &cut_std, &paste_std, &select_all_std])?;
            app.manage(SidebarMenuItem(toggle_sb.clone()));
            app.manage(ToolbarMenuItem(toggle_tb.clone()));

            // ⌘1/⌘2/⌘3은 JS에서 처리(next_tab/prev_tab과 같은 이유) — 메뉴 항목은 클릭
            // 진입점 + 현재 도구 표시(체크)용으로만 존재한다.
            let tool_pointer = CheckMenuItem::with_id(app, "tool-pointer", l.pointer, true, true, None::<&str>)?;
            let tool_text_select = CheckMenuItem::with_id(app, "tool-text-select", l.text_select, true, false, None::<&str>)?;
            let tool_highlight = CheckMenuItem::with_id(app, "tool-highlight", l.highlight_tool, true, false, None::<&str>)?;
            app.manage(ToolMenuItems(vec![tool_pointer.clone(), tool_text_select.clone(), tool_highlight.clone()]));
            let tools_menu = Submenu::with_items(app, l.tools, true, &[&tool_pointer, &tool_text_select, &tool_highlight])?;

            let shortcuts   = MenuItem::with_id(app, "shortcuts",   l.shortcuts,   true, None::<&str>)?;
            let commands    = MenuItem::with_id(app, "commands",    l.commands,    true, None::<&str>)?;
            let open_source = MenuItem::with_id(app, "open-source", l.open_source, true, None::<&str>)?;
            let settings    = MenuItem::with_id(app, "settings",    l.settings,    true, Some("CmdOrCtrl+,"))?;
            let sep_h       = PredefinedMenuItem::separator(app)?;
            // macOS는 다른 앱들처럼 설정을 App 메뉴(About 바로 아래)에 두고, 그 외
            // 플랫폼은 App 메뉴 자체가 없어서 Help 메뉴에 둔다.
            #[cfg(target_os = "macos")]
            let help_menu = Submenu::with_items(app, l.help, true, &[&shortcuts, &commands, &sep_h, &open_source])?;
            #[cfg(not(target_os = "macos"))]
            let help_menu = {
                let sep_h2 = PredefinedMenuItem::separator(app)?;
                Submenu::with_items(app, l.help, true, &[&shortcuts, &commands, &sep_h, &open_source, &sep_h2, &settings])?
            };

            app.manage(AppMenuItems {
                close: close.clone(),
                actual_size: actual_size.clone(),
                zoom_fit: zoom_fit.clone(),
                zoom_fit_h: zoom_fit_h.clone(),
                zoom_in: zoom_in.clone(),
                zoom_out: zoom_out.clone(),
                zoom_sel: zoom_sel.clone(),
                view_menu: view_menu.clone(),
                fullscreen: fullscreen.clone(),
                copy_sel: copy_sel.clone(),
                edit_menu: edit_menu.clone(),
                tools_menu: tools_menu.clone(),
                shortcuts: shortcuts.clone(),
                commands: commands.clone(),
                open_source: open_source.clone(),
                settings: settings.clone(),
                help_menu: help_menu.clone(),
            });

            #[cfg(target_os = "macos")]
            {
                // 네이티브 About 패널(orderFrontStandardAboutPanel)은 버전 텍스트가 선택
                // 불가능한 정적 라벨이라 Cmd+C로 복사가 안 되고, 짧은 버전/빌드 버전을 따로
                // 안 두면 "Version 0.2.1 (0.2.1)"처럼 같은 값이 중복 표시된다 — 다른 정보
                // 패널들(오픈소스, 단축키 등)과 똑같이 웹뷰 모달로 대체해 둘 다 해결한다.
                let about       = MenuItem::with_id(app, "about", l.about, true, None::<&str>)?;
                let hide        = PredefinedMenuItem::hide(app, None)?;
                let hide_others = PredefinedMenuItem::hide_others(app, None)?;
                let show_all    = PredefinedMenuItem::show_all(app, None)?;
                // PredefinedMenuItem::quit는 macOS의 terminate: 셀렉터로 바로 종료돼서
                // RunEvent::ExitRequested를 거치지 않는다 — 탭 2개 이상일 때 종료 확인을
                // 띄우려면 커스텀 메뉴 아이템으로 만들어 on_menu_event에서 직접 처리해야 한다.
                let quit        = MenuItem::with_id(app, "quit", l.quit, true, Some("CmdOrCtrl+Q"))?;
                let sep_a1      = PredefinedMenuItem::separator(app)?;
                let sep_a2      = PredefinedMenuItem::separator(app)?;
                let sep_a3      = PredefinedMenuItem::separator(app)?;
                let app_menu    = Submenu::with_items(app, "Vimong", true,
                    &[&about, &sep_a1, &settings, &sep_a2, &hide, &hide_others, &show_all, &sep_a3, &quit])?;

                let new_tab   = MenuItem::with_id(app, "new-tab",  l.new_tab,  true, Some("CmdOrCtrl+T"))?;
                let doc_info  = MenuItem::with_id(app, "doc-info", l.doc_info, true, Some("CmdOrCtrl+I"))?;
                let print     = MenuItem::with_id(app, "print",    l.print,    true, Some("CmdOrCtrl+P"))?;
                let password  = MenuItem::with_id(app, "password", l.password, true, None::<&str>)?;
                let save_as   = MenuItem::with_id(app, "save-as",  l.save_as,  true, Some("CmdOrCtrl+Shift+S"))?;
                let export    = MenuItem::with_id(app, "export",   l.export,   true, Some("CmdOrCtrl+Shift+E"))?;
                let sep_f     = PredefinedMenuItem::separator(app)?;
                let file_menu = Submenu::with_items(app, l.file, true,
                    &[&new_tab, &doc_info, &print, &password, &save_as, &export, &sep_f, &close])?;

                // 단축키는 JS에서 처리 (macOS Cocoa 키 이퀴벌런트가 Shift+심볼을 못 잡는
                // muda 버그 때문 — 메뉴 아이템은 클릭 진입점으로만 존재)
                // 시작 시 탭이 1개뿐이라 비활성화 상태로 시작 — new_tab/close_tab에서 갱신
                let next_tab = MenuItem::with_id(app, "next-tab", l.next_tab, false, None::<&str>)?;
                let prev_tab = MenuItem::with_id(app, "prev-tab", l.prev_tab, false, None::<&str>)?;
                let reopen_tab = MenuItem::with_id(app, "reopen-closed-tab", l.reopen_closed_tab, true, Some("CmdOrCtrl+Shift+R"))?;
                let sep_t = PredefinedMenuItem::separator(app)?;
                let tab_menu = Submenu::with_items(app, l.tab, true, &[&next_tab, &prev_tab, &sep_t, &reopen_tab])?;
                app.manage(TabMenuItems { next: next_tab.clone(), prev: prev_tab.clone(), reopen: reopen_tab.clone() });
                app.manage(PlatformMenuItems {
                    new_tab: new_tab.clone(), doc_info: doc_info.clone(), print: print.clone(), password: password.clone(),
                    save_as: save_as.clone(), export: export.clone(),
                    quit: quit.clone(), file_menu: file_menu.clone(), tab_menu: tab_menu.clone(),
                });

                let menu = Menu::with_items(app, &[&app_menu, &file_menu, &edit_menu, &view_menu, &tools_menu, &tab_menu, &help_menu])?;
                app.set_menu(menu)?;
            }

            #[cfg(not(target_os = "macos"))]
            {
                let new_tab   = MenuItem::with_id(app, "new-tab",  l.new_tab,  true, Some("CmdOrCtrl+T"))?;
                let doc_info  = MenuItem::with_id(app, "doc-info", l.doc_info, true, Some("CmdOrCtrl+I"))?;
                let print     = MenuItem::with_id(app, "print",    l.print,    true, Some("CmdOrCtrl+P"))?;
                let password  = MenuItem::with_id(app, "password", l.password, true, None::<&str>)?;
                let save_as   = MenuItem::with_id(app, "save-as",  l.save_as,  true, Some("CmdOrCtrl+Shift+S"))?;
                let export    = MenuItem::with_id(app, "export",   l.export,   true, Some("CmdOrCtrl+Shift+E"))?;
                let quit      = MenuItem::with_id(app, "quit", l.quit, true, Some("CmdOrCtrl+Q"))?;
                let sep_f     = PredefinedMenuItem::separator(app)?;
                let file_menu = Submenu::with_items(app, l.file, true,
                    &[&new_tab, &doc_info, &print, &password, &save_as, &export, &sep_f, &close, &quit])?;

                let next_tab = MenuItem::with_id(app, "next-tab", l.next_tab, false, None::<&str>)?;
                let prev_tab = MenuItem::with_id(app, "prev-tab", l.prev_tab, false, None::<&str>)?;
                let reopen_tab = MenuItem::with_id(app, "reopen-closed-tab", l.reopen_closed_tab, true, Some("CmdOrCtrl+Shift+R"))?;
                let sep_t = PredefinedMenuItem::separator(app)?;
                let tab_menu = Submenu::with_items(app, l.tab, true, &[&next_tab, &prev_tab, &sep_t, &reopen_tab])?;
                app.manage(TabMenuItems { next: next_tab.clone(), prev: prev_tab.clone(), reopen: reopen_tab.clone() });
                app.manage(PlatformMenuItems {
                    new_tab: new_tab.clone(), doc_info: doc_info.clone(), print: print.clone(), password: password.clone(),
                    save_as: save_as.clone(), export: export.clone(),
                    quit: quit.clone(), file_menu: file_menu.clone(), tab_menu: tab_menu.clone(),
                });

                let menu = Menu::with_items(app, &[&file_menu, &edit_menu, &view_menu, &tools_menu, &tab_menu, &help_menu])?;
                app.set_menu(menu)?;
            }

            // 시작 시점엔 열린 문서가 없으니 문서 관련 메뉴는 비활성 상태로 시작
            update_document_menu_enabled(app.handle(), false);

            // `npm run tauri dev`처럼 터미널에서 띄우면 macOS가 창을 자동으로 앞으로 안
            // 가져오는 경우가 있다 — 그 상태로 시작하면 창이 "한 번도 진짜로 포커스를
            // 받아본 적 없는" 상태가 되고, 이후 프로그램적으로 아무리 포커스를 요청해도
            // (실제 클릭 전까지는) 안 먹히는 것으로 보였다. 시작 시점에 명시적으로 한 번
            // 포커스를 줘서 이 상태 자체를 만들지 않는다.
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
                reclaim_webview_focus_retrying(window);
            }

            Ok(())
        })
        .on_menu_event(|app, event| {
            let _ = match event.id().as_ref() {
                "new-tab"        => app.emit("menu", "new-tab"),
                "copy-selection" => app.emit("menu", "copy-selection"),
                "doc-info"       => app.emit("menu", "doc-info"),
                "print"          => app.emit("menu", "print"),
                "password"       => app.emit("menu", "password"),
                "save-as"        => app.emit("menu", "save-as"),
                "export"         => app.emit("menu", "export"),
                "zoom-actual"    => app.emit("menu", "zoom-actual"),
                "zoom-fit"       => app.emit("menu", "zoom-fit"),
                "zoom-fit-height" => app.emit("menu", "zoom-fit-height"),
                "zoom-in"        => app.emit("menu", "zoom-in"),
                "zoom-out"       => app.emit("menu", "zoom-out"),
                "zoom-selection" => app.emit("menu", "zoom-selection"),
                "toggle-sidebar" => app.emit("menu", "toggle-sidebar"),
                "toggle-toolbar" => app.emit("menu", "toggle-toolbar"),
                "toggle-fullscreen" => app.emit("menu", "toggle-fullscreen"),
                id @ ("view-single" | "view-single-continuous" | "view-two" | "view-two-continuous" | "view-horizontal-continuous") => {
                    update_view_mode_checks(app, id);
                    app.emit("menu", id)
                }
                id @ ("tool-pointer" | "tool-text-select" | "tool-highlight") => {
                    update_tool_checks(app, id);
                    app.emit("menu", id)
                }
                "shortcuts"      => app.emit("menu", "shortcuts"),
                "commands"       => app.emit("menu", "commands"),
                "open-source"    => app.emit("menu", "open-source"),
                "about"          => app.emit("menu", "about"),
                "reopen-closed-tab" => app.emit("menu", "reopen-closed-tab"),
                "settings"       => app.emit("menu", "settings"),
                "next-tab"       => app.emit("menu", "next-tab"),
                "prev-tab"       => app.emit("menu", "prev-tab"),
                "close-tab"      => app.emit("menu", "close-tab"),
                "quit" => {
                    let s = app.state::<Mutex<AppState>>();
                    let s = s.lock().unwrap();
                    let tab_count = s.tabs.len();
                    let any_dirty = s.tabs.iter().any(|t| t.dirty);
                    drop(s);
                    if tab_count > 1 || any_dirty { app.emit("quit-requested", tab_count) } else { app.exit(0); Ok(()) }
                }
                _ => Ok(()),
            };
        })
        .invoke_handler(tauri::generate_handler![
            read_file_bytes,
            get_file_mtime,
            open_pdf,
            set_reflow_font_size,
            save_pdf,
            export_document,
            unlock_pdf,
            set_pdf_password,
            remove_pdf_password,
            add_highlight,
            add_shape,
            add_note,
            get_page_notes,
            get_all_notes,
            get_page_highlights,
            page_image_count,
            export_page_images,
            update_note,
            delete_annotation,
            undo_annotation,
            get_page_form_fields,
            set_form_text_value,
            toggle_form_checkbox,
            select_form_radio,
            reset_form_fields,
            save_document,
            render_page,
            render_thumbnail,
            get_file_info,
            get_fonts,
            list_system_fonts,
            get_font_stats,
            cancel_font_analysis,
            get_page_size,
            get_all_page_sizes,
            search_page,
            search_all,
            get_page_links,
            get_page_chars,
            get_toc,
            get_optional_content_groups,
            toggle_optional_content,
            get_attachments,
            new_tab,
            close_tab,
            is_tab_dirty,
            switch_tab,
            reorder_tab,
            quit_app,
            path_exists,
            list_dir_entries,
            set_sidebar_visible_menu,
            set_toolbar_visible_menu,
            set_view_mode_menu,
            set_tool_menu,
            set_app_language,
            print_webview,
            set_fullscreen_and_focus,
            set_document_menu_enabled,
            synctex_forward,
            synctex_inverse,
            open_in_editor,
            take_pending_forward_search,
            take_pending_file_open,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let s = window.state::<Mutex<AppState>>();
                let s = s.lock().unwrap();
                let tab_count = s.tabs.len();
                let any_dirty = s.tabs.iter().any(|t| t.dirty);
                drop(s);
                if tab_count > 1 || any_dirty {
                    api.prevent_close();
                    let _ = window.emit("quit-requested", tab_count);
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                if app_handle.state::<QuitConfirmed>().0.load(Ordering::Relaxed) { return; }
                let s = app_handle.state::<Mutex<AppState>>();
                let s = s.lock().unwrap();
                let tab_count = s.tabs.len();
                let any_dirty = s.tabs.iter().any(|t| t.dirty);
                drop(s);
                if tab_count > 1 || any_dirty {
                    api.prevent_exit();
                    let _ = app_handle.emit("quit-requested", tab_count);
                }
            }
            // Finder에서 파일을 더블클릭(콜드 스타트든, 이미 떠 있는 인스턴스로든)했을 때 macOS가 보내는 이벤트.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Opened { urls } = &event {
                if let Some(path) = urls.first().and_then(|u| u.to_file_path().ok()) {
                    let path = path.to_string_lossy().to_string();
                    *app_handle.state::<PendingFileOpen>().0.lock().unwrap() = Some(path.clone());
                    let _ = app_handle.emit("open-file", path);
                }
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.set_focus();
                }
            }
        });
}
