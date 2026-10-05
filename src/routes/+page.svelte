<script>
  import { invoke } from '@tauri-apps/api/core';
  import { open, save, message, ask, confirm } from '@tauri-apps/plugin-dialog';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWebview } from '@tauri-apps/api/webview';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { readText as clipboardRead, writeText as clipboardWrite } from '@tauri-apps/plugin-clipboard-manager';
  import { Menu, MenuItem } from '@tauri-apps/api/menu';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { getVersion } from '@tauri-apps/api/app';
  import { SvelteSet, SvelteMap } from 'svelte/reactivity';
  import { onMount, onDestroy, tick, flushSync } from 'svelte';
  import { LANGS, LANG_NAMES, loadLangPref, saveLangPref, detectSystemLang, t as t_ } from '$lib/i18n.js';
  import { loadThemePref, saveThemePref, detectSystemTheme } from '$lib/theme.js';
  import Markdown, { buildMdToc, MD_THEMES } from '$lib/Markdown.svelte';

  // MuPDF가 네이티브로 여는 포맷 (mupdf-sys 기본 빌드 기준: img/office/epub/cbz/xps/svg 핸들러)
  const SUPPORTED_EXTENSIONS = ['pdf', 'xps', 'epub', 'cbz', 'svg', 'docx', 'xlsx', 'pptx', 'png', 'gif', 'jpg', 'jpeg', 'bmp', 'tif', 'tiff'];
  // hwp/hwpx는 MuPDF가 못 열어서 @rhwp/core(WASM)로 별도 렌더링 — openHwpFile 참고
  const HWP_EXTENSIONS = ['hwp', 'hwpx'];
  // 마크다운은 MuPDF로 열지 않고 원문 텍스트를 읽어 프론트엔드에서 직접 렌더링 — openMarkdownFile 참고
  const MD_EXTENSIONS = ['md', 'markdown'];
  const ALL_EXTENSIONS = [...SUPPORTED_EXTENSIONS, ...HWP_EXTENSIONS, ...MD_EXTENSIONS];

  function baseName(path) { return path.split('/').pop().split('\\').pop(); }

  // ── 언어 설정 ────────────────────────────────────────────────
  // langPref: 'auto' | 'ko' | 'en' | 'ja' | 'zh-hans' | 'zh-hant' — localStorage에 저장.
  // lang: 화면에 실제로 쓰는 언어 (auto면 시스템 언어로 해석한 값). 네이티브 메뉴바도
  // set_app_language로 즉시 같이 갱신된다(파일로도 저장해 다음 실행에 이어짐).
  let langPref = $state(loadLangPref());
  let lang = $derived(langPref === 'auto' ? detectSystemLang() : langPref);
  function t(key, vars) { return t_(lang, key, vars); }
  function setLangPref(pref) {
    langPref = pref;
    saveLangPref(pref);
    invoke('set_app_language', { lang: pref });
  }

  // ── 테마 설정 ────────────────────────────────────────────────
  // themePref: 'auto' | 'light' | 'dark' — localStorage에 저장. theme: 실제 적용 값(auto면
  // OS 설정을 따라감). body.theme-light 클래스로 CSS 변수(--sh-*)를 스위칭한다.
  let themePref = $state(loadThemePref());
  let theme = $derived(themePref === 'auto' ? detectSystemTheme() : themePref);
  $effect(() => { document.body.classList.toggle('theme-light', theme === 'light'); });

  // ── 프레젠테이션(p) — 파워포인트 슬라이드 쇼처럼 ────────────────
  // 전체화면과 슬라이드형 보기(한 페이지를 화면 가운데에 검은 배경으로 꽉 채움, 스크롤
  // 없이 페이지 단위 이동)를 하나로 합친 단일 모드. Skim처럼 둘을 따로 안 둔다.
  // fullscreenMode: 실제 macOS 전체화면(초록 버튼과 동일한 그 상태) 여부 — 항상
  // presentationMode와 같이 켜지고 꺼지는 내부 구현 상태(사용자가 따로 못 건드림).
  let fullscreenMode = $state(false);
  let presentationMode = $state(false);
  let presentationRestore = null; // 프레젠테이션 진입 전 보기 모드/배율 — 종료 시 복원
  $effect(() => { document.body.classList.toggle('focus-mode', presentationMode); });
  $effect(() => { document.body.classList.toggle('presentation-mode', presentationMode); });

  // setFullscreen()의 promise는 macOS 애니메이션(약 0.3~0.5초)이 끝나기 전에 먼저 resolve돼서,
  // 그 직후 바로 focus()를 줘도 애니메이션이 끝나면서 다시 포커스를 놓쳤다 — 그래서 즉시 한 번,
  // 그리고 실제 리사이즈(mainResizeObserver, 애니메이션이 끝나야 크기가 진짜로 바뀐다)가
  // 감지되는 시점에 한 번 더, 총 두 번 mainEl에 DOM 포커스를 준다.
  let reclaimFocusOnResize = false;
  let reclaimFocusTimer;

  // mainEl.focus()(DOM 레벨)만으로는 안 됐다 — 전체화면 전환이 실제로 놓치는 건 webview
  // (WKWebView 등)의 OS 레벨 first responder 상태라, DOM 쪽 focus()로는 못 되돌린다.
  // Tauri의 Window.setFocus()(창 레벨)도 시도해봤지만 소용없었고, Webview.setFocus()
  // (콘텐츠 뷰 레벨)가 실제로 필요한 API였다 — 클릭했을 때 native가 자동으로 해주는 게
  // 바로 이거라, 클릭 없이도 이걸 직접 호출한다.
  async function reclaimFocus() {
    try { await getCurrentWebview().setFocus(); } catch (_) { /* 미지원 플랫폼 등 */ }
    // 모달 안 입력창(설정의 SyncTeX 편집기 명령 등)에 이미 포커스가 있으면 뺏지 않는다 —
    // setFocus()만으로 webview 자체의 OS 레벨 first responder는 되찾되, 그 안에서 어느
    // DOM 요소가 포커스를 갖는지는 그대로 둔다. 안 그러면 창 전환 후 돌아올 때마다
    // 입력 중이던 필드에서 mainEl로 포커스가 튕겨나갔다.
    const activeTag = document.activeElement?.tagName;
    if (activeTag === 'INPUT' || activeTag === 'TEXTAREA') return;
    mainEl?.focus();
    sidebarEl?.focus();
    // 진짜 클릭이 필요한 부분에 포커스를 뺏기지 않도록, 사이드바 안의 실제 <button>도
    // 하나 시도해본다 — div보다 폼 요소가 webview 쪽 키보드 포커스 알림을 더 확실히
    // 태우는 경우가 있다는 추측(확인은 안 됨).
    document.querySelector('.sidebar-tab-btn.active')?.focus();
    // 위 API들 모두 에러 없이 "성공"하고도 실제로는 안 먹혔다 — macOS가 진짜 사용자
    // 제스처 없는 포커스 탈취를 막고 있을 가능성이 높다. 실제 클릭과 최대한 비슷하게
    // synthetic mousedown/mouseup을 본문/사이드바에 직접 쏴서 webview가 first
    // responder를 갖도록 유도하는 마지막 시도.
    const opts = { bubbles: true, cancelable: true, view: window };
    for (const el of [mainEl, sidebarEl]) {
      if (!el) continue;
      el.dispatchEvent(new MouseEvent('mousedown', opts));
      el.dispatchEvent(new MouseEvent('mouseup', opts));
    }
  }

  async function setFullscreenMode(on) {
    try {
      // JS에서 set_fullscreen과 setFocus를 각각 별도 invoke로 부르면(둘 다 에러 없이
      // "성공"은 해도) webview가 first responder를 못 되찾았다 — 매 invoke가 IPC 왕복을
      // 거치는 비동기 콜백이라 macOS가 "사용자 제스처의 연장"으로 안 쳐줬을 가능성이 있어,
      // 하나의 Rust 커맨드(set_fullscreen_and_focus) 안에서 두 네이티브 호출을 IPC 왕복
      // 없이 곧바로 이어붙이도록 옮겼다.
      await invoke('set_fullscreen_and_focus', { fullscreen: on });
    } catch (_) { /* 전체화면 미지원 플랫폼 등 */ }
    fullscreenMode = on;
    await reclaimFocus();
    reclaimFocusOnResize = true;
    clearTimeout(reclaimFocusTimer);
    reclaimFocusTimer = setTimeout(() => { reclaimFocusOnResize = false; }, 800);
  }

  // 프레젠테이션은 페이지 전체(가로+세로 둘 다)가 화면 안에 들어오도록 맞춘다 — zh/zv(폭/높이
  // 한쪽만 맞춤)와 달리 반드시 둘 다 화면을 넘지 않는 쪽(더 작은 배율)을 쓴다.
  async function fitPresentationPage() {
    if (!filePath) return;
    const [pageW, pageH] = hwpMode ? hwpPageSize(currentPage) : await invoke('get_page_size', { pageNum: currentPage });
    if (!pageW || !pageH) return;
    const availW = (mainEl?.clientWidth ?? window.innerWidth) - 40;
    const availH = (mainEl?.clientHeight ?? window.innerHeight) - 40;
    scale = Math.min(availW / pageW, availH / pageH);
    if (!hwpMode) await rerenderNow();
  }

  async function togglePresentation() {
    if (!filePath && !presentationMode) return;
    if (mdMode) { statusMsg = t('status.md_unsupported_feature'); return; } // 페이지 크기 개념이 없어 프레젠테이션 핏 계산 불가
    if (presentationMode) {
      presentationMode = false;
      // Skim처럼 프레젠테이션을 나가면 전체화면이었든 아니든 항상 완전히 일반 창으로
      // 돌아간다 — 원래 전체화면이었을 때만 끄게 했더니, "이미 전체화면(F) → p로
      // 프레젠테이션 → Esc" 순서에서 전체화면 상태가 그대로 남아 사이드바/툴바가 안 뜨고,
      // (그 전체화면 종료 경로에만 있는) 네이티브 포커스 재확보도 안 타서 키 입력도 안
      // 먹는 문제가 있었다.
      await setFullscreenMode(false);
      if (presentationRestore) {
        const { viewMode: vm, scale: sc } = presentationRestore;
        presentationRestore = null;
        scale = sc;
        await setViewMode(vm);
      }
    } else {
      // 이미 전체화면이어도 그대로 불러도 안전(절대값 설정이라 no-op)하고, 그래야 네이티브
      // 포커스 재확보 로직도 항상 한 번은 타게 된다.
      await setFullscreenMode(true);
      presentationRestore = { viewMode, scale };
      await setViewMode('single');
      presentationMode = true;
      await fitPresentationPage();
    }
  }

  async function toggleFullscreen() {
    if (presentationMode) return; // 프레젠테이션 중 전체화면은 p/Esc로만 제어한다
    await setFullscreenMode(!fullscreenMode);
  }

  // 프레젠테이션 중 페이지가 바뀌면(j/k/gg/G 등 뭘로 넘겼든) 그 페이지 크기에 맞춰 다시 핏
  $effect(() => {
    if (presentationMode) { currentPage; fitPresentationPage(); }
  });

  // ── 설정 modal ──────────────────────────────────────────────
  // 설정 모달은 Apply를 눌러야 실제로 적용된다 — settingsLangDraft/settingsThemeDraft가
  // 모달 안에서만 쓰는 임시 값이고, langPref/themePref(실제 적용된 값)는 apply 시에만 갱신한다.
  let showSettings = $state(false);
  let settingsLangDraft = $state('auto');
  let settingsThemeDraft = $state('auto');
  let settingsMdThemeDraft = $state('auto');
  let settingsMdCodeFontDraft = $state('default');
  const SYNCTEX_EDITOR_KEY = 'vimong-synctex-editor';
  let synctexEditorCmd = $state(localStorage.getItem(SYNCTEX_EDITOR_KEY) ?? '');
  let settingsSynctexDraft = $state('');
  // 탭이 여러 개일 때 종료/:tabonly로 한꺼번에 닫히는 걸 확인할지 여부 — 기본값 켜짐
  const CONFIRM_CLOSE_TABS_KEY = 'vimong-confirm-close-tabs';
  let confirmCloseTabs = $state(localStorage.getItem(CONFIRM_CLOSE_TABS_KEY) !== 'false');
  let settingsConfirmCloseTabsDraft = $state(true);
  // 파일을 다시 열 때 마지막으로 보던 페이지로 이어볼지, 항상 첫 페이지로 열지 — 기본값 켜짐
  const RESUME_LAST_PAGE_KEY = 'vimong-resume-last-page';
  let resumeLastPage = $state(localStorage.getItem(RESUME_LAST_PAGE_KEY) !== 'false');
  let settingsResumeLastPageDraft = $state(true);
  // 앱을 새로 시작할 때 마지막에 열려있던 탭들을 그대로 복원할지 — 기본값 꺼짐(명시적으로 켜야 동작)
  const RESTORE_SESSION_KEY = 'vimong-restore-session';
  let restoreSession = $state(localStorage.getItem(RESTORE_SESSION_KEY) === 'true');
  let settingsRestoreSessionDraft = $state(false);
  // 종료 확인창의 체크박스에서 즉시 반영+저장 — 설정 모달의 draft/Apply 흐름과 별개로,
  // 다음에 설정 모달을 열면 openSettings()가 이 값을 다시 읽어가서 자동으로 동기화된다.
  function setRestoreSession(v) {
    restoreSession = v;
    localStorage.setItem(RESTORE_SESSION_KEY, String(v));
  }
  // 하이라이트 색상 — 설정이 아니라 툴바의 하이라이트 버튼 옆 색상 선택기에서 바로 바꾼다
  // (기본값은 지금까지 쓰던 노란색(1.0, 0.9, 0.0)과 같음). 그래서 draft 없이 즉시 반영+저장.
  const HIGHLIGHT_COLOR_KEY = 'vimong-highlight-color';
  let highlightColor = $state(localStorage.getItem(HIGHLIGHT_COLOR_KEY) ?? '#ffe500');
  function saveHighlightColor(hex) {
    highlightColor = hex;
    localStorage.setItem(HIGHLIGHT_COLOR_KEY, hex);
  }
  const HIGHLIGHT_OPACITY_KEY = 'vimong-highlight-opacity';
  let highlightOpacity = $state(Number(localStorage.getItem(HIGHLIGHT_OPACITY_KEY) ?? '1'));
  function saveHighlightOpacity(value) {
    highlightOpacity = value;
    localStorage.setItem(HIGHLIGHT_OPACITY_KEY, String(value));
  }
  function hexToRgbFloat(hex) {
    const n = parseInt(hex.slice(1), 16);
    return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
  }
  // 도형 도구(선/사각형/rounded 사각형/타원) 스타일 — 하이라이트 색상과 같은 패턴으로
  // 툴바에서 즉시 반영+저장한다. PDF 주석 스펙상 도형 하나에 불투명도는 하나뿐이라
  // (내부색/선 색이 따로여도 CA는 공유) 선/내부 불투명도는 하나로 합쳤다.
  const SHAPE_STROKE_COLOR_KEY = 'vimong-shape-stroke-color';
  let shapeStrokeColor = $state(localStorage.getItem(SHAPE_STROKE_COLOR_KEY) ?? '#ff3b30');
  function saveShapeStrokeColor(hex) {
    shapeStrokeColor = hex;
    localStorage.setItem(SHAPE_STROKE_COLOR_KEY, hex);
  }
  const SHAPE_FILL_COLOR_KEY = 'vimong-shape-fill-color';
  let shapeFillColor = $state(localStorage.getItem(SHAPE_FILL_COLOR_KEY) ?? '#ff3b30');
  function saveShapeFillColor(hex) {
    shapeFillColor = hex;
    localStorage.setItem(SHAPE_FILL_COLOR_KEY, hex);
  }
  const SHAPE_STROKE_WIDTH_KEY = 'vimong-shape-stroke-width';
  let shapeStrokeWidth = $state(Number(localStorage.getItem(SHAPE_STROKE_WIDTH_KEY) ?? '2'));
  function saveShapeStrokeWidth(value) {
    shapeStrokeWidth = value;
    localStorage.setItem(SHAPE_STROKE_WIDTH_KEY, String(value));
  }
  const SHAPE_OPACITY_KEY = 'vimong-shape-opacity';
  let shapeOpacity = $state(Number(localStorage.getItem(SHAPE_OPACITY_KEY) ?? '1'));
  function saveShapeOpacity(value) {
    shapeOpacity = value;
    localStorage.setItem(SHAPE_OPACITY_KEY, String(value));
  }
  // 메모(스티키 노트) 아이콘 색상 — 하이라이트 색상과 같은 패턴
  const NOTE_COLOR_KEY = 'vimong-note-color';
  let noteColor = $state(localStorage.getItem(NOTE_COLOR_KEY) ?? '#ffe033');
  function saveNoteColor(hex) {
    noteColor = hex;
    localStorage.setItem(NOTE_COLOR_KEY, hex);
  }
  // 메모 입력 중 상태 — {page, x, y} | null. 텍스트를 다 쓰기 전까지는 페이지에 아직
  // 아무것도 추가하지 않는다(하이라이트/도형과 달리 내용이 있어야 의미가 있어서).
  let notePending = $state(null);
  let noteText = $state('');
  // Skim의 Sync 프리셋 드롭다운과 동일한 역할 — 선택하면 아래 텍스트 필드를 채워줄 뿐,
  // 그 필드 자체가 항상 실제 값(그리고 텍스샵처럼 직접 명령을 적는 "사용자 지정" 자리)이다.
  // 그래서 별도 상태로 안 두고 현재 필드 내용이 어느 프리셋과 일치하는지에서 값을 역산한다.
  const TEXSHOP_SCRIPT_PATH = '/usr/local/bin/texshop';
  // TeXShop은 SyncTeX용 CLI가 따로 없어서(vimtex/vscode의 code -g와 달리) AppleScript로 대신
  // 호출하는 헬퍼 스크립트가 필요하다 — Skim+TeXShop 연동에 흔히 쓰이는 그 스크립트.
  const TEXSHOP_SCRIPT = '#!/bin/bash\n\n' +
    'file="$1"\n' +
    'line="$2"\n\n' +
    '[ "${file:0:1}" == "/" ] || file="${PWD}/${file}"\n' +
    '[ "${line}" == "" ] && line=1\n\n' +
    'exec osascript << EOF > /dev/null\n' +
    '  set texFile to POSIX file "${file}"\n' +
    '  tell application "TeXShop"\n' +
    '    activate\n' +
    '    open texFile\n' +
    '    delay 0.3\n' +
    '    tell front document\n' +
    '      refreshtext\n' +
    '      goto line ${line}\n' +
    '    end tell\n' +
    '  end tell\n' +
    'EOF\n';
  const SYNCTEX_PRESETS = {
    vscode: 'code -g "%f:%l"',
    texshop: `${TEXSHOP_SCRIPT_PATH} "%f" %l`,
  };
  let settingsSynctexPreset = $derived(
    Object.entries(SYNCTEX_PRESETS).find(([, cmd]) => cmd === settingsSynctexDraft)?.[0] ?? 'custom'
  );
  // /usr/local/bin은 sudo 없인 못 써서(직접 만들어주는 대신) 설치 명령을 안내한다. 스크립트를
  // 클립보드에 담아두고 "pbpaste로 붙여넣는" 명령을 따로 복사하게 했더니, 그 명령 자체를
  // 복사하는 순간 클립보드가 그 명령 텍스트로 덮어써져서 pbpaste가 스크립트 대신 자기
  // 자신을 읽어버렸다 — heredoc으로 스크립트 내용까지 명령 한 덩어리 안에 통째로 담아
  // 클립보드엔 이거 하나만 올리면, 그걸 그대로 붙여넣어 실행하는 것만으로 끝나서 클립보드를
  // 두 번 쓸 일이 없다.
  const TEXSHOP_INSTALL_CMD = (() => {
    const delim = 'VIMONG_TEXSHOP_SCRIPT_EOF';
    return `sudo tee ${TEXSHOP_SCRIPT_PATH} > /dev/null << '${delim}' && sudo chmod +x ${TEXSHOP_SCRIPT_PATH}\n${TEXSHOP_SCRIPT}${delim}\n`;
  })();
  // 프리셋 선택만으로 큰 안내창이 바로 뜨면 설정 모달이 너무 커져서, 선택 시엔 필드만
  // 채우고 안내는 별도 "도움말" 버튼을 눌러야 뜨는 전용 모달로 뺐다.
  let showTexshopHelp = $state(false);
  // statusMsg(하단 상태 표시줄)는 이 모달의 전체화면 오버레이에 가려서 안 보인다 —
  // 모달 안에서 직접 잠깐 나타났다 사라지는 피드백을 띄운다.
  let texshopCopyFeedback = $state(false);
  let texshopCopyFeedbackTimer;
  async function copyTexshopInstallCmd() {
    try {
      await clipboardWrite(TEXSHOP_INSTALL_CMD);
      texshopCopyFeedback = true;
      clearTimeout(texshopCopyFeedbackTimer);
      texshopCopyFeedbackTimer = setTimeout(() => { texshopCopyFeedback = false; }, 1500);
    } catch (_) { /* skip */ }
  }
  function closeTexshopHelp() {
    showTexshopHelp = false;
    texshopCopyFeedback = false;
    clearTimeout(texshopCopyFeedbackTimer);
  }
  function applySynctexPreset(key) {
    if (key === 'custom') { settingsSynctexDraft = ''; return; }
    if (!SYNCTEX_PRESETS[key]) return;
    settingsSynctexDraft = SYNCTEX_PRESETS[key];
  }
  let settingsTab = $state('ui'); // 'ui' | 'markdown' | 'synctex'
  // 탭마다 내용 양이 달라서 크기를 콘텐츠에 맡기면 탭 전환할 때마다 모달이 커졌다 작아졌다
  // 한다 — CSS로 고정 크기를 주고, 네이티브 resize 핸들(모서리 드래그)로 사용자가 직접
  // 바꾼 크기만 기억해뒀다가 다음에 열 때 그대로 적용한다.
  const SETTINGS_MODAL_SIZE_KEY = 'vimong-settings-modal-size';
  function persistModalSize(node) {
    try {
      const saved = JSON.parse(localStorage.getItem(SETTINGS_MODAL_SIZE_KEY));
      if (saved?.width > 0 && saved?.height > 0) {
        node.style.width = `${saved.width}px`;
        node.style.height = `${saved.height}px`;
      }
    } catch (_) { /* 저장된 값이 없거나 깨졌으면 CSS 기본 크기 그대로 */ }
    const observer = new ResizeObserver(() => {
      localStorage.setItem(SETTINGS_MODAL_SIZE_KEY, JSON.stringify({ width: node.offsetWidth, height: node.offsetHeight }));
    });
    observer.observe(node);
    return { destroy: () => observer.disconnect() };
  }
  function openSettings() {
    settingsTab = 'ui';
    settingsLangDraft = langPref;
    settingsThemeDraft = themePref;
    settingsSynctexDraft = synctexEditorCmd;
    settingsConfirmCloseTabsDraft = confirmCloseTabs;
    settingsResumeLastPageDraft = resumeLastPage;
    settingsRestoreSessionDraft = restoreSession;
    settingsMdThemeDraft = mdTheme;
    settingsMdCodeFontDraft = mdCodeFont;
    showSettings = true;
    ensureSystemFontsLoaded(); // 모달은 바로 뜨고, 폰트 목록은 준비되는 대로 select에 반영(반응형)
  }
  function applySettings() {
    setLangPref(settingsLangDraft);
    themePref = settingsThemeDraft;
    saveThemePref(settingsThemeDraft);
    synctexEditorCmd = settingsSynctexDraft;
    localStorage.setItem(SYNCTEX_EDITOR_KEY, synctexEditorCmd);
    confirmCloseTabs = settingsConfirmCloseTabsDraft;
    localStorage.setItem(CONFIRM_CLOSE_TABS_KEY, String(confirmCloseTabs));
    resumeLastPage = settingsResumeLastPageDraft;
    localStorage.setItem(RESUME_LAST_PAGE_KEY, String(resumeLastPage));
    restoreSession = settingsRestoreSessionDraft;
    localStorage.setItem(RESTORE_SESSION_KEY, String(restoreSession));
    // md 테마/폰트는 :md theme 명령(setMdTheme)과 달리 상태표시줄 메시지 없이 조용히 저장한다 —
    // 설정 모달을 닫는 것 자체가 이미 "적용됨" 피드백이라 중복임
    mdTheme = settingsMdThemeDraft;
    try { localStorage.setItem('vimong-md-theme', mdTheme); } catch (_) { /* skip */ }
    mdCodeFont = settingsMdCodeFontDraft;
    try { localStorage.setItem('vimong-md-code-font', mdCodeFont); } catch (_) { /* skip */ }
    showSettings = false;
  }

  // ── 최근 연 파일 (시작 화면용) ──────────────────────────────
  const RECENT_FILES_KEY = 'vimong-recent-files';
  const RECENT_FILES_MAX = 10;
  let recentFiles = $state([]); // string[] — 절대경로, 최근 순
  let recentIndex = $state(0);  // 시작 화면에서 j/k로 고른 최근 파일 인덱스
  let pendingD = false;         // 비반응 — 첫 번째 d를 기다리는 중인지 (dd로 삭제)

  function loadRecentFiles() {
    try { recentFiles = JSON.parse(localStorage.getItem(RECENT_FILES_KEY)) ?? []; }
    catch (_) { recentFiles = []; }
  }

  function addRecentFile(path) {
    recentFiles = [path, ...recentFiles.filter(p => p !== path)].slice(0, RECENT_FILES_MAX);
    localStorage.setItem(RECENT_FILES_KEY, JSON.stringify(recentFiles));
  }

  async function removeRecentFile(path) {
    const ok = await confirm(t('welcome.recent_remove_confirm', { name: baseName(path) }), { title: 'Vimong', kind: 'warning' });
    if (!ok) return;
    recentFiles = recentFiles.filter(p => p !== path);
    localStorage.setItem(RECENT_FILES_KEY, JSON.stringify(recentFiles));
    recentIndex = Math.min(recentIndex, Math.max(recentFiles.length - 1, 0));
    forgetLastPage(path);
  }

  // ── 마지막으로 읽던 페이지 기억 (vim이 파일을 다시 열면 커서 위치로 돌아가는 것과 같은 관습) ──
  // 경로만 키로 쓰면 같은 경로에 완전히 다른 문서가 덮어써져도 예전 페이지로 잘못 점프하니
  // mtime(수정시각, 초)을 같이 저장해뒀다가 달라지면(=다른 파일로 간주) 무시한다.
  const LAST_PAGE_KEY = 'vimong-last-page';
  let currentFileMtime = 0; // 현재 탭에 열린 파일의 mtime — 비반응, hwpDoc과 같은 패턴으로 탭별 저장/복원
  function loadLastPage(path, mtime) {
    try {
      const entry = JSON.parse(localStorage.getItem(LAST_PAGE_KEY))?.[path];
      return entry && entry.mtime === mtime ? entry.page : 0;
    } catch (_) { return 0; }
  }
  function forgetLastPage(path) {
    try {
      const map = JSON.parse(localStorage.getItem(LAST_PAGE_KEY)) ?? {};
      delete map[path];
      localStorage.setItem(LAST_PAGE_KEY, JSON.stringify(map));
    } catch (_) { /* skip */ }
  }
  const saveLastPageTimers = new Map(); // path별 디바운스 — 공용 타이머 하나면 탭을 빠르게
                                         // 전환할 때 방금 떠난 파일의 예약된 저장이 취소돼버린다
  // 스크롤 중엔 currentPage가 계속 바뀌므로(연속 보기 IntersectionObserver) 매번 쓰지 않고
  // 스크롤이 잠깐 멈췄을 때만 기록한다.
  function saveLastPage(path, page, mtime) {
    clearTimeout(saveLastPageTimers.get(path));
    saveLastPageTimers.set(path, setTimeout(() => {
      saveLastPageTimers.delete(path);
      try {
        const map = JSON.parse(localStorage.getItem(LAST_PAGE_KEY)) ?? {};
        if (page > 0) map[path] = { page, mtime }; else delete map[path];
        localStorage.setItem(LAST_PAGE_KEY, JSON.stringify(map));
      } catch (_) { /* skip */ }
    }, 500));
  }
  $effect(() => {
    if (filePath && pageCount > 0) saveLastPage(filePath, currentPage, currentFileMtime);
  });

  // ── 탭 세션 복원 (설정에서 켰을 때만 시작 시 되살림) ──────────
  // 설정이 꺼져 있어도 기록은 계속해둔다 — 나중에 켰을 때 그 시점의 열린 탭부터 바로
  // 반영되게 하려고(resumeLastPage/currentFileMtime과 같은 방식).
  const TAB_SESSION_KEY = 'vimong-tab-session';
  // $effect는 마운트 직후 초기 상태(탭 없음)로 한 번 즉시 실행되므로, restoreTabSession이
  // localStorage를 실시간으로 다시 읽으면 그 즉시-실행이 먼저 빈 배열로 덮어써버린 뒤라 항상
  // 빈 목록만 보게 된다 — 이펙트가 한 번도 안 돈 시점(스크립트 최상단)에 스냅샷을 떠서 쓴다.
  const EMPTY_TAB_SESSION = { paths: [], activePath: '' };
  let savedTabSession = (() => {
    try { return JSON.parse(localStorage.getItem(TAB_SESSION_KEY)) ?? EMPTY_TAB_SESSION; }
    catch (_) { return EMPTY_TAB_SESSION; }
  })();
  function saveTabSession() {
    const paths = tabLabels
      .map((_, i) => (i === currentTabIdx ? filePath : tabStates[i]?.filePath) || '')
      .filter(Boolean);
    try { localStorage.setItem(TAB_SESSION_KEY, JSON.stringify({ paths, activePath: filePath })); }
    catch (_) { /* skip */ }
  }
  $effect(() => {
    tabLabels.join('|'); // 값 자체는 안 씀 — 탭 개수/순서/내용 변경을 전부 잡기 위한 의존성
    saveTabSession();
  });
  async function restoreTabSession() {
    const { paths, activePath } = savedTabSession;
    for (const path of paths) {
      await newTab(path); // 파일이 없어졌으면 openFile 내부에서 에러 다이얼로그만 뜨고 계속 진행됨
    }
    // newTab을 순서대로 열면 항상 마지막 탭이 활성 상태로 남으므로, 원래 활성 탭이었던
    // 파일로 다시 전환해준다.
    if (activePath) {
      const idx = findOpenTab(activePath);
      if (idx !== -1) await switchToTab(idx);
    }
  }

  // ── PDF state ──────────────────────────────────────────────
  let pageCount = $state(0);
  let currentPage = $state(0);
  let scale = $state(1.0);
  // 0/90/180/270 — r/R로 "현재 보고 있는 페이지"만 90도씩 회전(Preview/Acrobat과 같은 방식).
  // 뷰 전용이라 문서 자체는 안 바뀜(:w 저장 시에만 각 페이지에 실제로 반영). PDF/HWP 공용.
  let pageRotations = $state({}); // { [pageIndex]: 0|90|180|270 }
  function getPageRotation(i) { return pageRotations[i] ?? 0; }
  let filePath = $state('');
  // 콜드 스타트 때 세션 복원/파일 연결 열기 중 뭔가 자동으로 열릴지 아직 판단 전이면 true —
  // 이 동안은 "새 탭" 웰컴 화면을 띄우지 않는다. 안 그러면 파일이 열리기 전 잠깐 웰컴 화면이
  // 보였다가 문서로 바뀌는 깜빡임이 생긴다(Finder/브라우저에서 PDF를 열어 콜드 스타트할 때 특히
  // 눈에 띔). onMount에서 자동으로 열릴 만한 걸 다 확인한 뒤 false로 내린다.
  let bootLoading = $state(true);

  // ── hwp state (최소 뷰어 — @rhwp/core WASM으로 SVG 렌더링. 썸네일/텍스트 선택/줌 핏은 미지원) ──
  let hwpMode = $state(false);
  let hwpPages = $state([]); // svg 문자열[] — 페이지 전체를 열 때 한 번에 미리 렌더링(가상 스크롤 없음)
  let hwpDoc = null; // 현재 탭의 live HwpDocument — 탭별로 tabStates에 같이 저장/복원(비반응성, pageCanvasRefs와 같은 패턴)
  let hwpFileBytes = 0; // 문서 정보 패널의 크기(KB) 표시용 — HWP는 Rust tab.doc가 없어 get_file_info로 못 가져옴
  let hwpInited = false;

  // ── markdown state (원문 텍스트를 그대로 들고 코드/미리보기/분할로 렌더링 — 페이지 개념 없음) ──
  let mdMode = $state(false);
  let mdSource = $state(''); // 파일 원문(UTF-8 디코딩)
  let mdScale = $state(1.0); // zi/zo/z0 — 페이지 스케일(scale)과 달리 폰트 크기 배율. 탭별로 저장/복원
  let mdViewMode = $state(loadMdViewModePref()); // 'code' | 'preview' | 'split' — gc/gp/gs, :md 명령

  // EPUB/HTML처럼 mupdf가 재배치(layout)로 페이지를 만드는 문서 — mdMode의 mdScale과 같은
  // 이유로, zi/zo가 캔버스 확대 대신 이 em(pt)을 바꿔서 다시 페이지를 나누게 한다.
  // 탭별로 저장/복원(마지막에 쓴 크기는 새 문서를 열 때도 기본값으로 이어감).
  const DEFAULT_EPUB_EM = 12;
  // 일반 PDF의 zi/zo(scale ±0.25 = 25%)에 비해 em ±1(기준 12pt 대비 8.3%p)은 눈으로 커졌다/
  // 작아졌다 구분하기엔 너무 작은 변화였다 — 2pt(16.7%p)로 늘려 한 번만 눌러도 체감되게 한다.
  const EPUB_EM_STEP = 2;
  let epubReflowable = $state(false);
  let epubFontSize = $state(Number(localStorage.getItem('vimong-epub-font-size')) || DEFAULT_EPUB_EM);
  // relayout(문서 전체를 다시 줄바꿈) 자체가 다국어 EPUB에서는 100~수백ms 걸릴 수 있어서, zi/zo를
  // 연타(또는 z를 쥔 채로 i/o 연타)하면 그때마다 겹쳐서 부를 수는 없다 — 그렇다고 매번 일정
  // 시간을 죽였다가(디바운스) 마지막 값 하나만 처리하면, 누르는 동안은 화면이 전혀 안
  // 바뀌다가 손을 뗀 뒤에야 한 번에 바뀌어서 "즉각 반응"하는 느낌이 없다. 그래서 대기 없이
  // 바로 시작하되 절대 겹쳐 부르지 않고, 처리 중에 더 눌러서 쌓인 목표 em은 지금 처리 중인
  // 게 끝나자마자 이어서 처리한다 — 쥐고 있는 동안 relayout이 끝나는 대로 계속 단계적으로
  // 화면이 갱신된다(그 사이 더 빠르게 눌린 중간값들은 자동으로 건너뛰어져 낭비가 없다).
  // $state여야 한다 — 상태 표시줄 배율 표시가 이 값을 그대로 보여줘서 사용자가 "지금 계산
  // 중인지" 바로 알 수 있게 한다(아래 reflowRunning도 같은 이유).
  let pendingEpubEm = $state(null); // 아직 처리 안 한 목표 em — null이면 대기 중인 요청 없음
  let reflowRunning = $state(false); // setReflowFontSize가 지금 실행 중인지
  function requestReflowFontSize(em) {
    pendingEpubEm = Math.max(6, Math.min(36, em));
    if (!reflowRunning) runPendingReflow();
  }
  // 이전 relayout이 아직 안 끝났는데 또 부르면 겹쳐서 호출하게 된다 — 백엔드
  // set_reflow_font_size는 문서를 잠깐 꺼내놨다(take) 되돌리는 방식이라, 두 번째 호출이 그
  // 사이에 끼면 "열린 문서가 없습니다"로 실패하고, 그 실패 전에 이미 부른
  // resetMainViewState()가 첫 번째 호출의 pageVersion을 밀어버려 첫 번째 결과가 그려지다
  // 만 채로 버려진다(zi/zo를 반복할 때 홀수 번째가 반응 없는 것처럼 보인 원인이었다) —
  // reflowRunning으로 겹침을 막고, 끝난 직후 새로 쌓인 목표가 있으면 곧바로 이어서 돈다.
  async function runPendingReflow() {
    if (pendingEpubEm === null) return;
    const target = pendingEpubEm;
    pendingEpubEm = null;
    reflowRunning = true;
    try {
      await setReflowFontSize(target);
    } finally {
      reflowRunning = false;
      if (pendingEpubEm !== null) runPendingReflow();
    }
  }
  function loadMdViewModePref() { try { return localStorage.getItem('vimong-md-view-mode') || 'preview'; } catch (_) { return 'preview'; } }
  function setMdViewMode(m) {
    mdViewMode = m;
    try { localStorage.setItem('vimong-md-view-mode', m); } catch (_) { /* skip */ }
    statusMsg = t(`status.md_view_${m}`);
  }
  let mdComponentRef = $state(null); // <Markdown bind:this> — 스크롤/검색 하이라이트는 이 컴포넌트가 직접 처리
  let mdSearchResults = $state([]); // [{start,end,lineText}] — 원문 문자 오프셋 기준, 화면엔 없음(:e한 문서마다 새로 계산)

  const MD_THEME_LABELS = { 'github-light': 'GitHub Light', 'github-dark': 'GitHub Dark', dracula: 'Dracula', nord: 'Nord', 'solarized-light': 'Solarized Light' };
  let mdTheme = $state(loadMdThemePref()); // 'auto' | 'github-light' | 'github-dark' | 'dracula' | 'nord' | 'solarized-light'
  function loadMdThemePref() { try { return localStorage.getItem('vimong-md-theme') || 'auto'; } catch (_) { return 'auto'; } }
  function setMdTheme(name) {
    mdTheme = name;
    try { localStorage.setItem('vimong-md-theme', name); } catch (_) { /* skip */ }
    statusMsg = t('status.md_theme_changed', { name });
  }

  let mdCodeFont = $state(loadMdCodeFontPref()); // 'default' | 시스템 폰트 패밀리 이름(list_system_fonts로 스캔)
  function loadMdCodeFontPref() { try { return localStorage.getItem('vimong-md-code-font') || 'default'; } catch (_) { return 'default'; } }
  // 앱 시작할 때 한 번만 스캔해서 캐싱(onMount에서 미리 던져둠) — 디스크/레지스트리를
  // 훑고 고정폭 판별을 위해 폰트를 하나하나 실제로 로드해봐야 해서 폰트가 많으면 몇 초
  // 걸릴 수 있다. 실패해도(권한 등) 목록만 비어있을 뿐 '기본값'은 항상 남아있다.
  let systemFonts = $state([]);
  let systemFontsLoading = $state(false);
  let systemFontsLoaded = false;
  async function ensureSystemFontsLoaded() {
    if (systemFontsLoaded) return;
    systemFontsLoaded = true;
    systemFontsLoading = true;
    try { systemFonts = await invoke('list_system_fonts'); } catch (_) { systemFonts = []; }
    systemFontsLoading = false;
  }

  async function ensureRhwpInit() {
    if (hwpInited) return;
    const [rhwp, { default: wasmUrl }] = await Promise.all([
      import('@rhwp/core'),
      import('@rhwp/core/rhwp_bg.wasm?url'),
    ]);
    // WASM이 줄바꿈/정렬을 계산하려면 브라우저 Canvas로 글자 폭을 재야 한다 — init 전에 등록 필수
    let ctx = null, lastFont = '';
    globalThis.measureTextWidth = (font, text) => {
      if (!ctx) ctx = document.createElement('canvas').getContext('2d');
      if (font !== lastFont) { ctx.font = font; lastFont = font; }
      return ctx.measureText(text).width;
    };
    await rhwp.default({ module_or_path: wasmUrl });
    hwpInited = true;
  }

  // rhwp StructureNode({level,kind,marker,heading,section,paragraph,children}) → 앱의 toc 형식
  // ({title,page,y,children}). y(페이지 내 세로 위치)는 rhwp가 제공하지 않아 항상 null —
  // gotoTocItem은 y가 null이면 페이지 이동까지만 하고 세로 위치 보정은 건너뛴다.
  function buildHwpToc(nodes) {
    return nodes.map(n => {
      let page = null;
      try { page = JSON.parse(hwpDoc.getPageOfPosition(n.section, n.paragraph)).page; } catch (_) { /* 위치 정보 없음 */ }
      return { title: n.heading, page, y: null, children: buildHwpToc(n.children ?? []) };
    });
  }

  // openFile/openHwpFile/openMarkdownFile(아래 세 함수) 공용 에러 처리. 파일이 이동/삭제된
  // 경우(최근 파일 목록의 오래된 항목을 열 때 흔하다)는 MuPDF/OS가 돌려주는 날것의 에러
  // 문자열("MuPDF error, code: 2, message: cannot open ...: No such file or directory")을
  // 그대로 보여주면 마치 앱이 고장난 것처럼 보인다 — 실제로는 흔하고 가벼운 상황이라
  // 아이콘도 경고(error)가 아닌 안내(info)로, 문구도 경로만 짚어주는 쪽으로 부드럽게 바꾼다.
  // 그 외의 진짜 에러는 원래대로 자세한 메시지를 보여준다.
  async function showOpenFileError(path, e) {
    statusMsg = `Error: ${e}`;
    if (/no such file or directory/i.test(String(e))) {
      await message(t('status.file_not_found', { path }), { title: 'Vimong', kind: 'info' });
    } else {
      await message(t('status.open_file_error', { e }), { title: 'Vimong', kind: 'error' });
    }
  }

  async function openHwpFile(path) {
    mdMode = false;
    mdSource = '';
    mdSearchResults = [];
    mdScale = 1.0;
    epubReflowable = false;
    try {
      ++thumbVersion;
      resetMainViewState();
      await ensureRhwpInit();
      const bytes = new Uint8Array(await invoke('read_file_bytes', { path }));
      hwpFileBytes = bytes.length;
      currentFileMtime = await invoke('get_file_mtime', { path }).catch(() => 0);
      const rhwp = await import('@rhwp/core');
      hwpDoc = new rhwp.HwpDocument(bytes);
      const count = hwpDoc.pageCount();
      hwpPages = Array.from({ length: count }, (_, i) => hwpDoc.renderPageSvg(i));
      hwpMode = true;
      viewMode = 'single-continuous'; // 최소 뷰어라 페이지/두쪽 모드는 없음 — 연속 스크롤 하나로 고정
      pageCount = count;
      currentPage = 0;
      pageRotations = {};
      // PDF와 동일하게 열 때 폭 맞춤 — 이전 문서에서 남은 scale이 그대로 새 문서에 적용되는 것을 방지
      const [pageW0] = hwpPageSize(0);
      if (pageW0) scale = Math.round((((mainEl?.clientWidth ?? window.innerWidth) - 40) / pageW0) * 100) / 100;
      filePath = path;
      invoke('set_document_menu_enabled', { enabled: true });
      sessionStorage.setItem('vimong-file', path);
      addRecentFile(path);
      const label = baseName(path);
      statusMsg = t('status.hwp_minimal_viewer', { label });
      tabLabels[currentTabIdx] = label;
      await closeOtherEmptyTabs();
      thumbnails = [];
      allSearchResults = [];
      mdSearchResults = [];
      pageHighlights = [];
      lastSearch = ''; searchQuery = '';
      selectedTocId = null;
      marks = {};
      try { toc = buildHwpToc(JSON.parse(hwpDoc.getStructure('auto')).roots); } catch (_) { toc = []; }
      collapsed = collectParentIds(toc, '', new SvelteSet());
      sidebarView = tocIsMeaningful ? 'toc' : 'thumbs';
      const resumePage = resumeLastPage ? loadLastPage(path, currentFileMtime) : 0;
      if (resumePage > 0 && resumePage < count) {
        await gotoPage(resumePage);
      } else {
        await tick();
        pageWrapEls[0]?.scrollIntoView({ block: 'start', inline: 'start' });
      }
    } catch (e) {
      await showOpenFileError(path, e);
    }
  }

  // 마크다운은 페이지 개념이 없는 순수 텍스트 문서라 MuPDF를 거치지 않고 원문만 읽어서
  // 프론트엔드(Markdown.svelte)가 직접 코드/미리보기/분할로 그린다. pageCount=1로 고정해두면
  // gg/G 등 페이지 기반 로직이 자연스럽게 no-op이 된다(gotoPage의 mdMode 분기 참고).
  async function openMarkdownFile(path) {
    hwpMode = false;
    hwpPages = [];
    hwpDoc = null;
    try {
      ++thumbVersion;
      resetMainViewState();
      const bytes = new Uint8Array(await invoke('read_file_bytes', { path }));
      mdSource = new TextDecoder('utf-8').decode(bytes);
      currentFileMtime = await invoke('get_file_mtime', { path }).catch(() => 0);
      mdMode = true;
      pageCount = 1;
      currentPage = 0;
      pageRotations = {};
      scale = 1.0;
      mdScale = 1.0;
      epubReflowable = false;
      filePath = path;
      invoke('set_document_menu_enabled', { enabled: true });
      sessionStorage.setItem('vimong-file', path);
      addRecentFile(path);
      const label = baseName(path);
      statusMsg = label;
      tabLabels[currentTabIdx] = label;
      await closeOtherEmptyTabs();
      thumbnails = [];
      allSearchResults = [];
      pageHighlights = [];
      lastSearch = ''; searchQuery = '';
      selectedTocId = null;
      marks = {};
      mdSearchResults = [];
      toc = buildMdToc(mdSource);
      collapsed = collectParentIds(toc, '', new SvelteSet()); // PDF/HWP처럼 챕터(최상위)만 펼친 채 시작
      sidebarView = toc.length > 0 ? 'toc' : 'search';
    } catch (e) {
      await showOpenFileError(path, e);
    }
  }

  // hwp SVG 문자열에 검색 하이라이트 <rect>를 직접 끼워 넣는다 — 좌표를 CSS px로 환산하는
  // 대신 SVG 자체 좌표계(viewBox 단위)에 그대로 그려서, getSelectionRects가 renderPageSvg와
  // 같은 내부 레이아웃에서 나온 좌표라는 사실에만 의존하고 별도 배율 변환은 하지 않는다.
  function injectHwpHighlights(svg, rects, activeRect) {
    const marks = rects.map(r => {
      const active = r === activeRect;
      const fill = active ? 'rgba(255,230,0,0.65)' : 'rgba(255,200,0,0.25)';
      const stroke = active ? ' stroke="#ffd400" stroke-width="2"' : '';
      return `<rect x="${r[0]}" y="${r[1]}" width="${r[2] - r[0]}" height="${r[3] - r[1]}" fill="${fill}"${stroke}/>`;
    }).join('');
    return svg.replace('</svg>', marks + '</svg>');
  }

  // Rust의 parse_search_flags(lib.rs)와 동일한 규칙 — 끝에서부터 \c\C\r\R을 벗겨내며 적용
  function parseSearchFlags(query, defaultCaseSensitive) {
    let caseSensitive = defaultCaseSensitive, regex = false; // \r 없으면 항상 일반 텍스트 검색
    let caseSet = false, regexSet = false, rest = query;
    for (;;) {
      if (rest.endsWith('\\c')) { if (!caseSet) { caseSensitive = false; caseSet = true; } rest = rest.slice(0, -2); }
      else if (rest.endsWith('\\C')) { if (!caseSet) { caseSensitive = true; caseSet = true; } rest = rest.slice(0, -2); }
      else if (rest.endsWith('\\r')) { if (!regexSet) { regex = true; regexSet = true; } rest = rest.slice(0, -2); }
      else if (rest.endsWith('\\R')) { if (!regexSet) { regex = false; regexSet = true; } rest = rest.slice(0, -2); }
      else break;
    }
    return { query: rest, caseSensitive, regex };
  }

  // Rust의 vim_magic_to_rust_regex(lib.rs)를 JS RegExp용으로 옮긴 것 — vim 기본(magic) 정규식
  // 문법을 그대로 지원한다. \w \W \< \>는 Rust regex의 유니코드 인식 매칭에 맞추려고 [A-Za-z0-9_]가
  // 아니라 \p{L}\p{N}(u 플래그 필요) 기반으로 옮겨서, 한글 문서에서도 단어 경계가 실제로 동작한다.
  function vimMagicToRegex(pattern) {
    const chars = Array.from(pattern);
    let out = '', zsAt = null, zeAt = null, i = 0;
    const WORD = '\\p{L}\\p{N}_';
    while (i < chars.length) {
      const c = chars[i++];
      if ('()+?{}|'.includes(c)) { out += '\\' + c; continue; }
      if (c !== '\\') { out += c; continue; }
      const n = chars[i++];
      switch (n) {
        case '+': out += '+'; break;
        case '?': case '=': out += '?'; break;
        case '(': out += '(?:'; break;
        case ')': out += ')'; break;
        case '|': out += '|'; break;
        case '<': out += `(?<![${WORD}])(?=[${WORD}])`; break;
        case '>': out += `(?<=[${WORD}])(?![${WORD}])`; break;
        case '{': {
          const nonGreedy = chars[i] === '-';
          if (nonGreedy) i++;
          let body = '';
          for (;;) {
            const bc = chars[i++];
            if (bc === undefined) break;
            if (bc === '\\' && chars[i] === '}') { i++; break; }
            if (bc === '}') break;
            body += bc;
          }
          out += body === '' ? '*' : `{${body}}`;
          if (nonGreedy) out += '?';
          break;
        }
        case 'd': out += '\\d'; break;
        case 'D': out += '\\D'; break;
        case 'w': out += `[${WORD}]`; break;
        case 'W': out += `[^${WORD}]`; break;
        case 's': out += '\\s'; break;
        case 'S': out += '\\S'; break;
        case 'a': out += '[A-Za-z]'; break;
        case 'A': out += '[^A-Za-z]'; break;
        case 'l': out += '[a-z]'; break;
        case 'L': out += '[^a-z]'; break;
        case 'u': out += '[A-Z]'; break;
        case 'U': out += '[^A-Z]'; break;
        case 'x': out += '[0-9A-Fa-f]'; break;
        case 'X': out += '[^0-9A-Fa-f]'; break;
        case 'o': out += '[0-7]'; break;
        case 'O': out += '[^0-7]'; break;
        case 'h': out += '[A-Za-z_]'; break;
        case 'H': out += '[^A-Za-z_]'; break;
        case 'p': out += '.'; break;
        case 'P': out += '[\\x00-\\x1f\\x7f]'; break;
        case 'k': out += `[${WORD}]`; break;
        case 'K': out += `[^${WORD}]`; break;
        case 'i': out += `[${WORD}]`; break;
        case 'I': out += '[A-Za-z_]'; break;
        case 'z': {
          const zc = chars[i++];
          if (zc === 's') zsAt = out.length;
          else if (zc === 'e') zeAt = out.length;
          else out += zc === undefined ? '\\z' : '\\z' + zc;
          break;
        }
        case undefined: out += '\\'; break;
        default: out += '\\' + n;
      }
    }
    if (zsAt === null && zeAt === null) return { source: out, groupIdx: null };
    const start = zsAt ?? 0, end = zeAt ?? out.length;
    if (start > end) return { source: out, groupIdx: null };
    return { source: out.slice(0, start) + '(' + out.slice(start, end) + ')' + out.slice(end), groupIdx: 1 };
  }

  // rhwp에는 정규식 검색 API가 없어서 문단 텍스트를 직접 뽑아 JS RegExp로 훑는다 — 검색 자체를
  // 문서 전체 문단 순회로 구현하는 대신, searchAllText와 같은 {sec,para,charOffset,length} 모양의
  // hit 배열을 만들어 아래 좌표 변환 로직을 문자열/정규식 검색이 그대로 공유하게 한다.
  function hwpRegexSearch(pattern, caseSensitive) {
    const { source, groupIdx } = vimMagicToRegex(pattern);
    const re = new RegExp(source, 'gdu' + (caseSensitive ? '' : 'i'));
    const hits = [];
    const sectionCount = hwpDoc.getSectionCount();
    for (let sec = 0; sec < sectionCount; sec++) {
      const paraCount = hwpDoc.getParagraphCount(sec);
      for (let para = 0; para < paraCount; para++) {
        const len = hwpDoc.getParagraphLength(sec, para);
        if (len === 0) continue;
        const text = hwpDoc.getTextRange(sec, para, 0, len);
        re.lastIndex = 0;
        let m;
        while ((m = re.exec(text)) !== null) {
          if (m[0].length === 0) { re.lastIndex++; continue; } // 빈 매치 무한루프 방지
          const [start, end] = (groupIdx !== null && m.indices[groupIdx]) ? m.indices[groupIdx] : m.indices[0];
          if (end > start) hits.push({ sec, para, charOffset: start, length: end - start });
        }
      }
    }
    return hits;
  }

  async function runHwpSearch(rawQuery) {
    const { query, caseSensitive: cs, regex } = parseSearchFlags(rawQuery, caseSensitive);
    caseSensitive = cs; // \c / \C 오버라이드를 Aa 아이콘에도 반영
    regexMode = regex;  // \r / \R 오버라이드를 .* 아이콘에도 반영
    try {
      const hits = regex ? hwpRegexSearch(query, cs) : JSON.parse(hwpDoc.searchAllText(query, cs, false));
      const pageRects = new Map(); // page -> [x0,y0,x1,y1][] — getSelectionRects 자체 pageIndex 기준(문단이 페이지 경계에 걸치면 hit 하나가 여러 페이지에 rect를 남길 수 있음)
      for (const h of hits) {
        let rects = [];
        try {
          rects = JSON.parse(hwpDoc.getSelectionRects(h.sec, h.para, h.charOffset, h.para, h.charOffset + h.length));
        } catch (_) { /* 이 매치는 좌표 없이 건너뜀 */ }
        if (rects.length === 0) {
          const page = JSON.parse(hwpDoc.getPageOfPosition(h.sec, h.para)).page;
          rects = [{ pageIndex: page, x: 0, y: 0, width: 0, height: 0 }];
        }
        for (const r of rects) {
          if (!pageRects.has(r.pageIndex)) pageRects.set(r.pageIndex, []);
          pageRects.get(r.pageIndex).push([r.x, r.y, r.x + r.width, r.y + r.height]);
        }
      }
      const results = [...pageRects.entries()]
        .sort((a, b) => a[0] - b[0])
        .map(([page, rects]) => ({ page, hit_count: rects.length, rects }));
      allSearchResults = results;
      if (results.length > 0) {
        searchResultIdx = 0;
        sidebarView = 'search';
        const totalHits = results.reduce((s, r) => s + r.hit_count, 0);
        statusMsg = `/${rawQuery}  [${totalHits} hit${totalHits > 1 ? 's' : ''} on ${results.length} page${results.length > 1 ? 's' : ''}]`;
        await gotoPage(results[0].page);
        await tick();
        focusNearestItem(searchResultEls, 0);
      } else {
        statusMsg = `Pattern not found: ${rawQuery}`;
      }
    } catch (e) {
      allSearchResults = [];
      searchError = String(e);
      statusMsg = `Search error: ${e}`;
    }
  }

  // mdSource(원문) 전체를 정규식으로 훑어 {start,end}(문자 오프셋) 배열을 만든다 — PDF/HWP처럼
  // 페이지 개념이 없어 결과를 페이지별로 묶지 않고 원문 순서 그대로 하나의 flat 목록으로 둔다.
  function mdTextSearch(query, cs, regex) {
    let source, groupIdx = null;
    if (regex) ({ source, groupIdx } = vimMagicToRegex(query));
    else source = query.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'); // 리터럴 검색은 정규식 특수문자를 그대로 문자로 취급
    const re = new RegExp(source, 'gdu' + (cs ? '' : 'i'));
    const hits = [];
    let m;
    while ((m = re.exec(mdSource)) !== null) {
      if (m[0].length === 0) { re.lastIndex++; continue; } // 빈 매치 무한루프 방지
      const [start, end] = (groupIdx !== null && m.indices[groupIdx]) ? m.indices[groupIdx] : m.indices[0];
      if (end > start) hits.push({ start, end });
    }
    return hits;
  }

  // 주어진 원문 오프셋보다 앞에 있는 헤딩 중 가장 가까운 것의 앵커 — 검색 매치가 미리보기에서
  // 정확히 어디인지는 알 수 없어도(렌더링되며 마크업이 사라짐), 최소한 그 섹션까진 데려간다.
  function nearestMdHeadingAnchor(offset) {
    let anchor = null;
    for (const item of flattenToc(toc, 0, '', [], true)) {
      if (item.mdOffset !== null && item.mdOffset <= offset) anchor = item.mdAnchor;
      else break;
    }
    return anchor;
  }

  // 사이드바 검색 결과 목록에 보여줄 한 줄 미리보기 — 매치가 속한 줄에서 앞뒤로 조금만 잘라온다
  function mdHitContext(hit) {
    const lineStart = mdSource.lastIndexOf('\n', hit.start - 1) + 1;
    let lineEnd = mdSource.indexOf('\n', hit.end);
    if (lineEnd === -1) lineEnd = mdSource.length;
    const MAX = 40;
    let before = mdSource.slice(lineStart, hit.start);
    let after = mdSource.slice(hit.end, lineEnd);
    if (before.length > MAX) before = '…' + before.slice(-MAX);
    if (after.length > MAX) after = after.slice(0, MAX) + '…';
    return { before: before.trimStart(), match: mdSource.slice(hit.start, hit.end), after: after.trimEnd() };
  }

  function jumpToMdSearchHit(idx) {
    const hit = mdSearchResults[idx];
    if (!hit) return;
    mdComponentRef?.scrollToOffset(hit.start, nearestMdHeadingAnchor(hit.start));
  }

  async function runMdSearch(rawQuery) {
    const { query, caseSensitive: cs, regex } = parseSearchFlags(rawQuery, caseSensitive);
    caseSensitive = cs; // \c / \C 오버라이드를 Aa 아이콘에도 반영
    regexMode = regex;  // \r / \R 오버라이드를 .* 아이콘에도 반영
    try {
      mdSearchResults = mdTextSearch(query, cs, regex);
      if (mdSearchResults.length > 0) {
        searchResultIdx = 0;
        sidebarView = 'search';
        statusMsg = `/${rawQuery}  [${mdSearchResults.length} hit${mdSearchResults.length > 1 ? 's' : ''}]`;
        jumpToMdSearchHit(0);
        await tick();
        focusNearestItem(searchResultEls, 0);
      } else {
        statusMsg = `Pattern not found: ${rawQuery}`;
      }
    } catch (e) {
      mdSearchResults = [];
      searchError = String(e);
      statusMsg = `Search error: ${e}`;
    }
  }

  // ── 보기 모드 (Skim 스타일: 한 페이지/한 페이지 연속/두 페이지/두 페이지 연속/가로 연속) ──
  // 모든 모드가 같은 지연 렌더링 체계(pageSizes/pageCanvasRefs + IntersectionObserver)를
  // 공유한다 — 'two'도 매번 딱 2개 페이지만 DOM에 붙는 것뿐이고, 'single'도 트랙패드로
  // 페이지 경계를 넘을 때 옆 페이지를 잠깐 더 붙이는 것뿐이라 따로 파이프라인을 만들
  // 필요가 없다.
  let viewMode = $state('single');
  // View 메뉴의 보기 모드 체크 표시를 동기화 — macOS에서 창 리사이즈/전체화면 전환 시
  // 웹뷰만 리마운트되면 viewMode는 'single'로 초기화되는데 네이티브 메뉴 체크는 리마운트
  // 전 상태 그대로 남아 "메뉴는 연속인데 실제로는 한 페이지" 같은 불일치가 생긴다
  $effect(() => { invoke('set_view_mode_menu', { mode: viewMode }); });
  function isContinuous(m) { return m === 'single-continuous' || m === 'two-continuous' || m === 'horizontal-continuous'; }
  function isTwoUp(m) { return m === 'two' || m === 'two-continuous'; }

  let pageSizes = $state([]);       // [w,h][] — 'single'은 필요한 페이지만(ensurePageSize), 나머지는 문서 열 때 한 번에
  let pageCanvasRefs = [];          // bind:this — 페이지별 canvas (비반응성, thumbEls와 같은 패턴)
  let pageWrapEls = [];             // bind:this — 페이지별 wrap div (gotoPage 스크롤 대상)
  let visiblePageSet = new Set();   // 현재 뷰포트에 걸쳐 있는 페이지 인덱스
  // $state여야 한다 — {#key pageVersion}이 이 값의 변화를 감지해서 탭 전환 시 캔버스를
  // 강제로 새로 만들게 하는데, 일반 let이면 반응성이 없어 감지가 안 된다. 같은 페이지
  // 인덱스로 다른 문서 탭을 오가면(둘 다 1페이지 등) windowLo/windowHi 값이 안 바뀌어서
  // 키 없는 each 블록은 canvas DOM 노드를 그대로 재사용했고, 그 canvas의 dataset.scale은
  // "이전 문서에서 이 배율로 이미 그렸다"로 남아있어 재렌더를 건너뛰어 다른 문서의 화면이
  // 그대로 남아있었다 (사이드바/탭은 새 문서인데 본문만 이전 문서인 버그의 원인).
  let pageVersion = $state(0);
  let mainPageObserver = null;    // 넉넉한 여백으로 미리 렌더(prefetch)만 담당
  let currentPageObserver = null; // 여백 없이 실제 화면에 걸친 페이지만 보고 currentPage를 갱신

  let pageSizePromises = new Map(); // 진행 중인 get_page_size 요청 중복 방지용 (아래 fetchPageBitmap과 같은 이유)

  async function ensurePageSize(i) {
    if (pageSizes[i]) return;
    if (pageSizePromises.has(i)) return pageSizePromises.get(i);
    const promise = invoke('get_page_size', { pageNum: i })
      .then((size) => { pageSizes[i] = size; pageSizePromises.delete(i); })
      .catch((e) => { pageSizePromises.delete(i); throw e; });
    pageSizePromises.set(i, promise);
    return promise;
  }

  // get_page_size는 회전 없는 원본 크기를 준다 — 90/270도로 돌면 화면에서 가로/세로가
  // 뒤바뀌니 레이아웃(page-wrap 크기)에 쓸 땐 여기서 맞바꿔서 쓴다.
  function pageDims(i) {
    const s = pageSizes[i];
    if (!s) return [0, 0];
    const r = getPageRotation(i);
    return (r === 90 || r === 270) ? [s[1], s[0]] : [s[0], s[1]];
  }

  // render_page 응답(원시 픽셀 버퍼) 캐시 — "page:scale" 단위. single 모드에서 인접 페이지를
  // 화면에 붙이기 전에 미리 받아둬서, 붙는 순간 IPC 왕복 없이 바로 칠할 수 있게 한다.
  // (붙고 나서 그리면 그 사이 캔버스가 아직 아무것도 안 칠해진 채로 노출되는데, 이때 캔버스
  // 뒷배경이 흰색 CSS로도 완전히 가려지지 않는 경우가 있어 검게 깜빡이는 것으로 보였다.)
  // 원본 비압축 픽셀 버퍼(zoom/리사이즈로 scale이 바뀔 때마다 새 키)라 개수 제한 없이 두면
  // 줌/리사이즈를 반복하는 세션에서 GB 단위로 무한정 쌓인다 — 오래된 항목부터 축출(LRU).
  // "페이지 개수"가 아니라 "바이트 합"으로 상한을 두는 이유: 페이지 크기는 문서마다 천차만별이라
  // (A4 문서는 페이지당 몇 MB, 큰 스캔본은 몇십 MB) 개수 상한은 큰 페이지 문서에서 그대로
  // GB 단위가 되어버린다 — 바이트 합으로 재면 문서 종류와 무관하게 실제 메모리 상한이 지켜진다.
  // 칠해진 페이지는 loadMainPageAt이 바로 여기서 지우므로(아래) 이 캐시엔 "칠하기 전" 프리페치
  // 중인 몇 페이지만 남는다 — 상한은 그 대비 여유만 있으면 되니 작게 잡는다.
  const PAGE_BITMAP_BUDGET = 100 * 1024 * 1024;
  let pageBitmapCache = new Map(); // key → { promise, bytes } (bytes는 응답 오기 전엔 0)
  let pageBitmapBytes = 0;

  // pageBitmapCache에서 지울 때 항상 이 함수를 거쳐야 pageBitmapBytes 합이 실제 캐시 내용과
  // 어긋나지 않는다 — 직접 .delete()만 하면 그 항목의 바이트가 합계에 유령처럼 남아서, 시간이
  // 지날수록(예: 하이라이트 추가할 때마다 refreshPageBitmap이 지우는 것) 상한을 실제보다 일찍
  // 넘긴 걸로 착각해 필요 이상으로 축출하게 된다.
  function deletePageBitmapEntry(key) {
    const entry = pageBitmapCache.get(key);
    if (!entry) return;
    pageBitmapCache.delete(key);
    pageBitmapBytes -= entry.bytes;
  }

  function evictPageBitmapOverBudget() {
    if (pageBitmapBytes <= PAGE_BITMAP_BUDGET) return;
    for (const [key, entry] of pageBitmapCache) {
      if (pageBitmapBytes <= PAGE_BITMAP_BUDGET) break;
      if (!entry.bytes) continue; // 아직 응답 안 온 항목은 크기를 몰라 건드리지 않는다
      deletePageBitmapEntry(key);
    }
  }

  // 연속 스크롤 모드(single-continuous 등)는 문서의 모든 페이지에 캔버스가 미리 깔려 있고
  // IntersectionObserver가 화면 근처에 온 것만 지연해서 칠하는데(loadMainPageAt), 한 번 칠한
  // 캔버스는 화면 밖으로 나가도 안 지워져서 큰 문서를 끝까지 스크롤하면 그린 페이지 수만큼
  // 캔버스 픽셀 데이터가 무한정 쌓인다(수백 페이지짜리 문서면 GB 단위). 최근에 칠한 페이지의
  // 바이트 합이 상한을 넘으면 오래된 것부터 캔버스를 비운다(재방문 시 loadMainPageAt이 다시
  // 그림) — 위 pageBitmapCache와 같은 이유로 페이지 개수가 아니라 바이트 기준.
  const PAINTED_PAGE_BUDGET = 150 * 1024 * 1024;
  let paintedPages = new Map(); // page index → 그 페이지 버퍼의 바이트 수, 삽입 순서 = LRU
  let paintedPagesBytes = 0;

  function markPagePainted(i, bytes) {
    if (paintedPages.has(i)) paintedPagesBytes -= paintedPages.get(i);
    paintedPages.delete(i); paintedPages.set(i, bytes); paintedPagesBytes += bytes; // MRU로 재배치
    if (paintedPagesBytes <= PAINTED_PAGE_BUDGET) return;
    for (const [key, sz] of paintedPages) {
      if (paintedPagesBytes <= PAINTED_PAGE_BUDGET) break;
      if (visiblePageSet.has(key)) continue; // 지금 화면에 보이는 페이지는 놔둔다
      const canvas = pageCanvasRefs[key];
      if (canvas) { canvas.width = 0; canvas.height = 0; delete canvas.dataset.scale; }
      paintedPages.delete(key); paintedPagesBytes -= sz;
    }
  }

  async function fetchPageBitmap(i) {
    const key = `${i}:${scale}:${getPageRotation(i)}`;
    if (pageBitmapCache.has(key)) {
      const entry = pageBitmapCache.get(key);
      pageBitmapCache.delete(key); pageBitmapCache.set(key, entry); // MRU로 재배치
      return entry.promise;
    }
    const dpr = window.devicePixelRatio || 1;
    // invoke를 부르는 즉시(await 전에) 진행 중인 프라미스 자체를 캐시에 넣는다 — 그래야
    // 이 응답이 오기 전에 같은 페이지가 또 요청되면(연속 스크롤 중 prefetch와 실제 렌더가
    // 겹치는 경우) 새 invoke를 또 쏘지 않고 이 프라미스를 같이 기다린다. render_page는
    // Rust 쪽 Mutex<AppState>로 직렬화되는 커맨드라, 중복 요청이 쌓이면 뒤로 갈수록 큐가
    // 길어져서(빠르게 여러 페이지를 훑고 지나가면 특히) 몇 초씩 밀리는 것으로 나타났다.
    const promise = invoke('render_page', { pageNum: i, scale: scale * dpr, rotation: getPageRotation(i) });
    const entry = { promise, bytes: 0 };
    pageBitmapCache.set(key, entry);
    try {
      const buf = await promise;
      entry.bytes = buf.byteLength;
      pageBitmapBytes += buf.byteLength;
      evictPageBitmapOverBudget();
      return buf;
    } catch (e) {
      deletePageBitmapEntry(key); // 실패하면 캐시에서 지워서 나중에 다시 시도할 수 있게
      throw e;
    }
  }

  function paintCanvasFromBuffer(canvas, buf, i) {
    const view = new DataView(buf);
    const w = view.getUint32(0, true);
    const h = view.getUint32(4, true);
    const pixels = new Uint8ClampedArray(buf, 8);
    canvas.width = w; canvas.height = h;
    canvas.getContext('2d').putImageData(new ImageData(pixels, w, h), 0, 0);
    canvas.dataset.scale = `${scale}:${getPageRotation(i)}`;
  }

  // 현재 페이지 앞/뒤를 미리 받아둔다(크기 + 픽셀) — single 모드 트랙패드 스크롤이 경계에
  // 닿았을 때 IPC 왕복 없이 바로 붙고 바로 칠할 수 있어야, 그 사이 들어오는 휠 이벤트를 못
  // 쓰고 흘려버리거나 안 칠해진 캔버스가 잠깐 노출되는 일이 없다.
  function prefetchAdjacentSizes(page) {
    if (page > 0) { ensurePageSize(page - 1); fetchPageBitmap(page - 1); }
    if (page < pageCount - 1) { ensurePageSize(page + 1); fetchPageBitmap(page + 1); }
  }


  // ── single/two(비연속) 모드: 화면에는 항상 currentPage(두 페이지 모드는 그 스프레드)만
  // 붙어 있다 — windowLo === windowHi === currentPage가 항상 성립. 트랙패드/키보드로 페이지
  // 안에서는 자연스럽게 스크롤되다가, 맨 위/맨 아래에 닿으면 한 번에 딱 한 페이지(two는 한
  // 스프레드)만 넘어간다 — onMainWheel 참고.
  let windowLo = $state(0); // 지금 DOM에 붙어 있는 페이지 범위의 시작(포함)
  let windowHi = $state(0); // 끝(포함) — 항상 windowLo === windowHi === currentPage

  // ── 링크 호버 커서 / 클릭 이동 ──────────────────────────────
  let pageLinkCache = new Map(); // page index -> link[] — hover 판정용, 문서/뷰모드 바뀌면 비움
  let hoverLink = $state(null);

  async function ensureLinksLoaded(i) {
    if (pageLinkCache.has(i)) return;
    pageLinkCache.set(i, []); // 동시에 여러 번 요청되는 것 방지용 placeholder
    try { pageLinkCache.set(i, await invoke('get_page_links', { pageNum: i })); } catch (_) { /* skip */ }
  }

  function linkAt(page, x, y) {
    const links = pageLinkCache.get(page);
    return links?.find(l => x >= l.rect[0] && x <= l.rect[2] && y >= l.rect[1] && y <= l.rect[3]) ?? null;
  }

  // ── 메모 아이콘 호버 커서 / 클릭 판정 — pageLinkCache와 같은 패턴 ──
  let pageNoteCache = new Map(); // page index -> note[]
  let hoverNote = $state(null);

  async function ensureNotesLoaded(i) {
    if (pageNoteCache.has(i)) return;
    pageNoteCache.set(i, []); // 동시에 여러 번 요청되는 것 방지용 placeholder
    try { pageNoteCache.set(i, await invoke('get_page_notes', { pageNum: i })); } catch (_) { /* skip */ }
  }

  function noteAt(page, x, y) {
    const notes = pageNoteCache.get(page);
    return notes?.find(n => x >= n.rect[0] && x <= n.rect[2] && y >= n.rect[1] && y <= n.rect[3]) ?? null;
  }

  // ── 우클릭 메뉴에서 "기존 하이라이트를 우클릭했는지" 판정 — pageNoteCache와 같은 패턴 ──
  // 페이지가 로드될 때 loadMainPageAt이 미리 채워두므로(ensureLinksLoaded/ensureNotesLoaded와 같은
  // 자리), onPageContextMenu에서는 await 없이 동기로 조회할 수 있다 — contextmenu 핸들러는
  // e.preventDefault()를 동기적으로 호출해야 OS 기본 메뉴(Reload/Inspect Element)를 막을 수
  // 있어서, 그 전에 await를 넣으면 이 판정 이전에 기본 메뉴가 먼저 떠 버린다.
  let pageHighlightAnnotCache = new Map(); // page index -> {xref, rect}[]

  async function ensureHighlightsLoaded(i) {
    if (pageHighlightAnnotCache.has(i)) return;
    pageHighlightAnnotCache.set(i, []); // 동시에 여러 번 요청되는 것 방지용 placeholder
    try { pageHighlightAnnotCache.set(i, await invoke('get_page_highlights', { pageNum: i })); } catch (_) { /* skip */ }
  }

  function highlightAt(page, x, y) {
    const hls = pageHighlightAnnotCache.get(page);
    return hls?.find(h => x >= h.rect[0] && x <= h.rect[2] && y >= h.rect[1] && y <= h.rect[3]) ?? null;
  }

  // ── 폼 필드(AcroForm) 호버 커서 / 클릭 판정 — pageLinkCache와 같은 패턴. 텍스트/체크박스만
  // 상호작용 대상이고(kind !== 'other'), 나머지(라디오/콤보/리스트/버튼/서명)는 조회만 된다.
  // $state(SvelteMap)로 둔다 — 나머지 캐시(pageLinkCache 등)와 달리 이건 hit-test뿐 아니라
  // 템플릿에서 직접 읽어 필드 박스 오버레이를 그리는 데도 쓰므로(아래 .field-box), .set()이
  // 화면 갱신을 트리거해야 한다. 일반 Map이면 Svelte 5가 변경을 감지 못해 페이지를 다시
  // 열어야만 박스가 보였을 것이다.
  let pageFieldCache = $state(new SvelteMap()); // page index -> field[]
  let hoverField = $state(null);

  async function ensureFieldsLoaded(i) {
    if (pageFieldCache.has(i)) return;
    pageFieldCache.set(i, []); // 동시에 여러 번 요청되는 것 방지용 placeholder
    try { pageFieldCache.set(i, await invoke('get_page_form_fields', { pageNum: i })); } catch (_) { /* skip */ }
  }

  // 필드 값을 수정한 뒤 캐시를 다시 채울 때 쓴다 — ensureFieldsLoaded처럼 delete()로 비웠다가
  // 다시 채우면, 그 사이(await 도중) 배열이 순간적으로 비어서 {#each ... (f.xref)} 블록이
  // 모든 필드 DOM을 지웠다 새로 만든다. 다른 필드(예: Notes)에 포커스가 가 있는 도중 이게
  // 일어나면 그 DOM 엘리먼트가 통째로 사라지면서 포커스가 날아간다 — 실제로 겪은 버그
  // (FullName 편집 커밋 → Notes 클릭 시 포커스가 안 잡힘). delete 없이 한 번에 덮어써서
  // 중간에 빈 상태가 아예 안 생기게 한다(같은 xref는 Svelte가 DOM을 그대로 재사용).
  async function refreshFieldCache(page) {
    try { pageFieldCache.set(page, await invoke('get_page_form_fields', { pageNum: page })); } catch (_) { /* 실패하면 이전 값 유지 */ }
  }

  function fieldAt(page, x, y) {
    const fields = pageFieldCache.get(page);
    return fields?.find(f => x >= f.rect[0] && x <= f.rect[2] && y >= f.rect[1] && y <= f.rect[3]) ?? null;
  }

  // kind: "text" | "checkbox" | "radio" | "combobox" | "listbox" | "reset_button" | "other".
  // "other"는 서명/서브밋 버튼 등 아직(또는 절대) 상호작용을 안 붙이는 것들 — 클릭해도 무시한다.
  // text/combobox/listbox는 여기 없다 — field-box(박스만 그리고 클릭은 onPageClick이 처리)
  // 대신 실제 <input>/<textarea>/<select>를 필드 자리 위에 그대로 얹어두는 방식이라(아래
  // .field-input-inline/.field-select-inline) 그 엘리먼트 자신이 클릭/커서를 다 처리한다.
  const INTERACTIVE_FIELD_KINDS = new Set(['checkbox', 'radio', 'reset_button']);
  function isInteractiveField(f) { return !!f && !f.readonly && INTERACTIVE_FIELD_KINDS.has(f.kind); }

  function onPageMouseMove(e, page) {
    if (getPageRotation(page) !== 0) return; // 링크/텍스트 좌표가 회전 없는 페이지 기준이라 여기선 못 씀
    const rect = e.currentTarget.getBoundingClientRect();
    const x = (e.clientX - rect.left) / scale;
    const y = (e.clientY - rect.top) / scale;
    hoverLink = linkAt(page, x, y);
    hoverNote = toolMode === 'note' ? noteAt(page, x, y) : null;
    hoverField = toolMode === 'pointer' ? fieldAt(page, x, y) : null;
  }

  function onPageMouseLeave() { hoverLink = null; hoverNote = null; hoverField = null; }

  // ── SyncTeX 역방향 검색(PDF → 소스) — Shift+Cmd(⇧⌘)+클릭, Skim과 동일 ──
  // https://www.sumatrapdfreader.org/docs/LaTeX-integration 참고. 좌표는 스케일을 뺀
  // 페이지 point 좌표(좌상단 기준) — synctex CLI가 그대로 받는 단위와 동일하다.
  async function synctexInverseSearch(page, x, y) {
    if (!filePath || hwpMode) return;
    try {
      const hit = await invoke('synctex_inverse', { pdfPath: filePath, page: page + 1, x, y });
      statusMsg = t('status.synctex_jumped', { name: baseName(hit.file), line: hit.line });
      if (synctexEditorCmd) {
        await invoke('open_in_editor', { cmdTemplate: synctexEditorCmd, file: hit.file, line: hit.line, column: hit.column });
      }
    } catch (e) {
      statusMsg = t('status.synctex_error', { e });
    }
  }

  async function onPageClick(e, page) {
    if (getPageRotation(page) !== 0) return;
    if ((e.metaKey || e.ctrlKey) && e.shiftKey) {
      const rect = e.currentTarget.getBoundingClientRect();
      await synctexInverseSearch(page, (e.clientX - rect.left) / scale, (e.clientY - rect.top) / scale);
      return;
    }
    if (toolMode === 'note' && !hwpMode) {
      const rect = e.currentTarget.getBoundingClientRect();
      const x = (e.clientX - rect.left) / scale;
      const y = (e.clientY - rect.top) / scale;
      // 기존 메모 아이콘을 클릭했으면 새로 만들지 않고 그 메모를 편집한다
      await ensureNotesLoaded(page);
      const hit = noteAt(page, x, y);
      if (hit) {
        notePending = { page, x: hit.rect[0], y: hit.rect[1], xref: hit.xref };
        noteText = hit.contents;
      } else {
        notePending = { page, x, y, xref: null };
        noteText = '';
      }
      return;
    }
    if (toolActive) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const x = (e.clientX - rect.left) / scale;
    const y = (e.clientY - rect.top) / scale;
    const field = fieldAt(page, x, y);
    if (isInteractiveField(field)) {
      if (field.kind === 'checkbox') { await toggleFormCheckbox(page, field.xref); return; }
      if (field.kind === 'radio') { await selectFormRadio(page, field.xref); return; }
      if (field.kind === 'reset_button') { await resetFormFields(page, field.xref); return; }
      return;
    }
    const link = linkAt(page, x, y);
    if (link) { await followLink(link); return; }
    // 포인터 모드에서도 클릭으로 vim 커서를 놓을 수 있게 한다 — ga나 w/b/e 같은
    // cursor 기반 기능을 쓰려고 매번 텍스트 선택 모드로 전환할 필요가 없도록.
    await ensureCharsLoaded(page);
    const idx = charIndexAt(page, x, y);
    if (idx !== null) { anchorIdx = null; cursor = { page, idx }; }
  }

  // ── 텍스트 선택 ──────────────────────────────────────────────
  let toolMode = $state('pointer'); // 'pointer' | 'text' | 'highlight' | 'line' | 'rect' | 'rounded_rect' | 'ellipse' | 'note'
  // 도형 도구는 텍스트 선택과 전혀 무관하게 좌표만으로 자유롭게 드래그해야 해서 별도 플래그로 뺀다.
  const SHAPE_KINDS = ['line', 'rect', 'rounded_rect', 'ellipse'];
  let shapeMode = $derived(SHAPE_KINDS.includes(toolMode));
  // 하이라이트 도구도 텍스트 선택과 똑같이 드래그로 글자를 골라야 해서(다른 건 mouseup 때
  // 바로 하이라이트로 바뀐다는 것뿐), 문자 선택 관련 로직은 전부 이 값 하나로 같이 켠다.
  let textSelectMode = $derived(toolMode === 'text' || toolMode === 'highlight');
  // pointer 모드가 아닌 도구가 하나라도 켜져 있는지 — 링크 클릭/커서 배치 같은 "포인터 전용"
  // 동작을 막을 때 텍스트 도구/도형 도구를 한꺼번에 가리키는 용도
  let toolActive = $derived(toolMode !== 'pointer');
  $effect(() => { invoke('set_tool_menu', { tool: toolMode === 'text' ? 'text-select' : toolMode }); });
  let pageCharCache = new Map(); // page index -> {ch, rect}[] — pageLinkCache와 같은 패턴
  let anchorIdx = null;      // 비반응 — 선택이 시작된 문자 인덱스 (cursor와 다르면 selection이 생김)
  let cursor = $state(null); // {page, idx} | null — vim 스타일 커서. 클릭/검색 이동으로 놓이고
                              // w/W로 옮겨간다. anchorIdx와 같은 위치면 캐럿만, 다르면 선택 범위.
  let visualMode = $state(false); // v로 켜짐 — 켜져 있을 때만 w/W가 anchor를 고정한 채 선택을
                                   // 늘린다. 꺼져 있으면 실제 vim처럼 w/W는 커서만(anchor도 같이) 옮긴다.
  // 도형 드래그 — 텍스트 선택과 달리 글자 인덱스가 아니라 페이지 point 좌표(스케일 제외)를
  // 그대로 기록한다. {page, x0, y0, x1, y1, shift} | null. shift는 마지막 mousemove 시점의
  // Shift 키 상태 — 타원 도구에서 정원으로 제약할지 판단하는 데 쓴다.
  let shapeDrag = $state(null);
  let selecting = false;
  let mouseButtonDown = false;   // 텍스트 선택 mousedown이 아직 안 떼진 상태인지
  let pendingMouseDown = $state(null); // {page, clientX, clientY} — 그 페이지 글자가 아직 로딩(OCR
                                  // 포함) 중이라 드래그를 못 시작했을 때, 로딩이 끝나면 이
                                  // 위치에서 이어서 시작하려고 기억해두는 용도
  let pendingMouseUp = null;     // {clientX, clientY} — 로딩 중에 이미 손을 뗀 경우, 뗀 위치도
                                  // 같이 기억해뒀다가 로딩이 끝나면 시작~끝 구간을 한 번에 확정
  // cursor.idx는 항상 "선택에 포함할 마지막 글자"를 가리키도록 만들어서 온다 — 드래그는
  // 그 글자 자체, w/W 모션은 motionWordForward/Backward가 이미 경계 보정까지 끝낸 값.
  // 그래서 여기서는 별도 보정 없이 그대로 startIdx/endIdx로 쓴다.
  let selection = $derived.by(() => {
    if (!cursor || anchorIdx === null || anchorIdx === cursor.idx) return null;
    return { page: cursor.page, startIdx: anchorIdx, endIdx: cursor.idx };
  });

  async function ensureCharsLoaded(i) {
    if (pageCharCache.has(i)) return;
    pageCharCache.set(i, []); // 중복 요청 방지용 placeholder
    // 텍스트 레이어가 없는 페이지는 macOS에서 OCR로 폴백되는데 몇 초 걸릴 수 있다 — 그동안
    // 드래그해도 조용히 아무 반응이 없어 보여서(캐시가 비어 있으니) 오래 걸릴 때만 안내한다
    const slowTimer = setTimeout(() => { statusMsg = t('status.ocr_running'); }, 400);
    try {
      pageCharCache.set(i, await invoke('get_page_chars', { pageNum: i }));
    } catch (_) { /* skip */ }
    finally {
      clearTimeout(slowTimer);
      if (statusMsg === t('status.ocr_running')) statusMsg = '';
    }
    // 로딩되는 동안 눌러놓고 기다리던 드래그가 있으면 이어서 처리한다 — 사용자가 다시
    // 드래그할 필요 없이. 아직 손을 안 뗐으면 그 위치에서 드래그를 시작하고, 이미 뗐으면
    // (드래그가 OCR보다 빨리 끝난 경우) 시작~끝 구간을 한 번에 선택으로 확정한다.
    if (pendingMouseDown && pendingMouseDown.page === i) {
      const chars = pageCharCache.get(i);
      const wrap = pageWrapEls[i];
      if (chars && chars.length > 0 && wrap) {
        const rect = wrap.getBoundingClientRect();
        const startIdx = charIndexAt(i, (pendingMouseDown.clientX - rect.left) / scale, (pendingMouseDown.clientY - rect.top) / scale);
        if (startIdx !== null) {
          if (pendingMouseUp) {
            const endIdx = charIndexAt(i, (pendingMouseUp.clientX - rect.left) / scale, (pendingMouseUp.clientY - rect.top) / scale);
            anchorIdx = startIdx;
            cursor = { page: i, idx: endIdx ?? startIdx };
            visualMode = false;
          } else if (mouseButtonDown) {
            selecting = true;
            anchorIdx = null;
            cursor = null;
            visualMode = false;
            pendingStart = { page: i, idx: startIdx };
          }
        }
      }
      pendingMouseDown = null;
      pendingMouseUp = null;
    }
  }

  // HWP/워드 변환 PDF 등에는 문단 간격을 맞추려고 넣은 "안 보이는" 스페이서 문자가
  // 있는데, 그 문자의 rect가 페이지 폭만큼 넓거나 비정상적으로 커서 그대로 쓰면 드래그
  // 스냅 대상도 되고 하이라이트 박스도 페이지를 뒤덮게 그려진다. 그 페이지의 보통 글자
  // 너비보다 훨씬 넓은 문자만 선택 후보/렌더링에서 제외한다. (예전엔 공백도 무조건
  // 제외했는데, OCR 결과는 한 줄을 글자 수로 균등 배분해서 공백도 정상 크기 박스를 갖고
  // 있어 그걸 감추면 단어 사이가 뚝뚝 끊겨 보였다 — 폭 기준 하나로 충분하다.)
  function isRealChar(c, pageW) {
    return (c.rect[2] - c.rect[0]) <= pageW * 0.15;
  }

  // 문자 rect 안에 정확히 들어가면 그 글자, 아니면 "같은 줄(y가 가장 가까운 줄)"로
  // 먼저 좁힌 다음 그 줄 안에서 x가 가장 가까운 글자를 고른다. 줄 제한 없이 전체 페이지에서
  // 유클리드 최단거리로 고르면 문단을 건너뛴 엉뚱한 글자가 선택될 수 있어서 이렇게 나눈다.
  function charIndexAt(page, x, y) {
    const chars = pageCharCache.get(page);
    if (!chars || chars.length === 0) return null;
    const pageW = pageSizes[page]?.[0] ?? Infinity;
    for (let idx = 0; idx < chars.length; idx++) {
      if (!isRealChar(chars[idx], pageW)) continue;
      const [x0, y0, x1, y1] = chars[idx].rect;
      if (x >= x0 && x <= x1 && y >= y0 && y <= y1) return idx;
    }
    let lineIdx = null, bestDy = Infinity;
    for (let idx = 0; idx < chars.length; idx++) {
      if (!isRealChar(chars[idx], pageW)) continue;
      const [, y0, , y1] = chars[idx].rect;
      const dy = Math.abs(y - (y0 + y1) / 2);
      if (dy < bestDy) { bestDy = dy; lineIdx = idx; }
    }
    if (lineIdx === null) return null;
    const [, lineY0, , lineY1] = chars[lineIdx].rect;
    const lineMid = (lineY0 + lineY1) / 2;
    const lineTol = (lineY1 - lineY0) * 0.6 || 4;
    let best = lineIdx, bestDx = Infinity;
    for (let idx = 0; idx < chars.length; idx++) {
      if (!isRealChar(chars[idx], pageW)) continue;
      const [x0, y0, x1, y1] = chars[idx].rect;
      if (Math.abs((y0 + y1) / 2 - lineMid) > lineTol) continue;
      const dx = Math.abs(x - (x0 + x1) / 2);
      if (dx < bestDx) { bestDx = dx; best = idx; }
    }
    return best;
  }

  // 검색 이동 등 "이 페이지 좌표에 커서를 놓는다" 용도의 공통 진입점 — x/y는 charIndexAt과
  // 같은 PDF 포인트 좌표계. 이후 w/W로 그 지점부터 단어 선택을 이어갈 수 있다.
  async function placeCursorAt(page, x, y) {
    await ensureCharsLoaded(page);
    const idx = charIndexAt(page, x, y);
    if (idx === null) return;
    anchorIdx = idx;
    cursor = { page, idx };
    visualMode = false; // 검색 이동은 실제 vim처럼 커서만 옮긴다 — 선택은 v로 시작해야 함
  }

  let selectionRects = $derived.by(() => {
    if (!selection) return [];
    const chars = pageCharCache.get(selection.page) ?? [];
    const pageW = pageSizes[selection.page]?.[0] ?? Infinity;
    const [lo, hi] = selection.startIdx <= selection.endIdx ? [selection.startIdx, selection.endIdx] : [selection.endIdx, selection.startIdx];
    return chars.slice(lo, hi + 1).filter(c => isRealChar(c, pageW)).map(c => c.rect);
  });

  // 선택 범위 없이 캐럿만 있을 때(클릭/검색 이동 직후) 보여줄 커서 표시
  let caretRect = $derived.by(() => {
    if (!cursor || selection) return null;
    const chars = pageCharCache.get(cursor.page);
    return chars?.[cursor.idx]?.rect ?? null;
  });

  function selectionText() {
    if (!selection) return '';
    const chars = pageCharCache.get(selection.page) ?? [];
    const [lo, hi] = selection.startIdx <= selection.endIdx ? [selection.startIdx, selection.endIdx] : [selection.endIdx, selection.startIdx];
    return chars.slice(lo, hi + 1).map(c => c.ch).join('');
  }

  // selectionRects는 글자 하나당 사각형 하나라, 여러 줄에 걸친 선택을 그대로 하이라이트
  // 주석에 넘기면 자잘한 사각형이 잔뜩 생긴다 — 실제 PDF 하이라이트 관례처럼 같은 줄(y0/y1이
  // 거의 같은)끼리는 하나의 넓은 사각형으로 합친다. rects는 읽기 순서(위→아래, 왼쪽→오른쪽)로
  // 들어온다고 가정한다(selectionRects 자체가 그렇게 만들어짐).
  function mergeRectsByLine(rects) {
    const lines = [];
    for (const [x0, y0, x1, y1] of rects) {
      const last = lines.at(-1);
      if (last && Math.abs(last[1] - y0) < 0.5 && Math.abs(last[3] - y1) < 0.5) {
        last[0] = Math.min(last[0], x0);
        last[2] = Math.max(last[2], x1);
      } else {
        lines.push([x0, y0, x1, y1]);
      }
    }
    return lines;
  }

  // 선택 영역을 형광펜 하이라이트로 바꿔 원본 PDF에 저장한다 — H 키 진입점.
  // 해당 페이지의 캔버스만 다시 그린다 — resetMainViewState()는 pageVersion을 올려서
  // {#key pageVersion}이 캔버스 전체를 새로 마운트해버리는데(문서를 새로 열 때는 맞는
  // 동작이지만), 하이라이트 하나 추가한 걸로 화면 전체가 깜빡였다. 캐시와 canvas의
  // "이미 이 배율로 그렸다" 표시만 지워서 그 페이지 캔버스에 그대로 다시 그려 넣는다.
  async function refreshPageBitmap(i) {
    deletePageBitmapEntry(`${i}:${scale}:${getPageRotation(i)}`);
    const canvas = pageCanvasRefs[i];
    if (canvas) delete canvas.dataset.scale;
    await loadMainPageAt(i, pageVersion);
  }

  // 레이어(옵셔널 콘텐츠) on/off는 한 페이지가 아니라 문서 전체 렌더링에 영향을 주므로,
  // refreshPageBitmap 하나가 아니라 이미 그려둔 모든 캔버스를 무효화하고 다시 그린다.
  async function refreshAllPageBitmaps() {
    pageBitmapCache.clear();
    pageBitmapBytes = 0;
    const indices = [...paintedPages.keys()];
    for (const i of indices) {
      const canvas = pageCanvasRefs[i];
      if (canvas) delete canvas.dataset.scale;
    }
    await Promise.all(indices.map((i) => loadMainPageAt(i, pageVersion)));
    ++thumbVersion; // 썸네일도 레이어 반영해서 다시 그리기
  }

  // 사이드바 레이어 패널의 체크박스 — 클릭 즉시 토글(폼 체크박스와 같은 UX)
  async function toggleOptionalContentLayer(xref) {
    try {
      await invoke('toggle_optional_content', { xref });
      layers = await invoke('get_optional_content_groups'); // refreshFieldCache와 같은 패턴 — 토글 후 서버 쪽 상태를 다시 읽어온다
      await refreshAllPageBitmaps();
      statusMsg = t('status.layer_toggled');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  async function addHighlightToSelection() {
    if (!selection || hwpMode) return;
    const rects = mergeRectsByLine(selectionRects);
    if (rects.length === 0) return;
    const page = selection.page;
    try {
      await invoke('add_highlight', { pageNum: page, rects, color: hexToRgbFloat(highlightColor), opacity: highlightOpacity });
      anchorIdx = null; cursor = null; visualMode = false; // 선택 해제
      pageHighlightAnnotCache.delete(page);
      ++thumbVersion;
      await refreshPageBitmap(page);
      statusMsg = t('status.highlight_added');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // 우클릭 메뉴 "하이라이트 삭제"
  async function deleteHighlight(page, xref) {
    try {
      await invoke('delete_annotation', { pageNum: page, xref });
      pageHighlightAnnotCache.delete(page);
      ++thumbVersion;
      await refreshPageBitmap(page);
      statusMsg = t('status.highlight_deleted');
    } catch (err) {
      statusMsg = `Error: ${err}`;
    }
  }

  // u / ⌘Z — 하이라이트든 도형이든, 가장 최근에 추가한 주석 하나를 되돌린다(LIFO).
  async function undoLastAnnotation() {
    if (hwpMode || mdMode) return;
    try {
      const page = await invoke('undo_annotation');
      if (page === null) { statusMsg = t('status.no_highlight_to_undo'); return; }
      pageNoteCache.delete(page); // 되돌린 게 메모였을 수도 있으니 호버 캐시도 같이 비운다
      pageHighlightAnnotCache.delete(page); // 되돌린 게 하이라이트였을 수도 있으니 같이 비운다
      if (sidebarView === 'notes') await refreshNotesList();
      ++thumbVersion;
      await refreshPageBitmap(page);
      statusMsg = t('status.highlight_undone');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  function constrainSquareDrag(x0, y0, x1, y1) {
    const size = Math.max(Math.abs(x1 - x0), Math.abs(y1 - y0));
    return [x0, y0, x0 + (x1 < x0 ? -size : size), y0 + (y1 < y0 ? -size : size)];
  }

  // Rust rounded_rect_vertices와 같은 공식으로 미리보기용 바깥쪽 모서리 반지름을 구한다
  // (문서 point 단위) — 공식이 다르면 드래그 중엔 다른 모양으로 보이다가 실제 도형이
  // 그려지는 순간 반지름이 바뀌는 것처럼 보인다. 백엔드는 선 두께 절반만큼 안쪽으로 들인
  // 사각형을 기준으로 반지름을 계산한 뒤 그 위에 선을 두르므로, 바깥쪽(미리보기 기준)
  // 반지름은 안쪽 반지름 + 들인 만큼(strokeWidth/2)에 근사한다.
  function roundedRectPreviewRadius(w, h, strokeWidth) {
    const inset = Math.min(strokeWidth / 2, w / 2, h / 2);
    const innerW = w - 2 * inset, innerH = h - 2 * inset;
    const rInner = Math.min(0.15 * Math.min(innerW, innerH), 16, innerW / 2, innerH / 2);
    return rInner + inset;
  }

  // 도형 도구 드래그 확정 — 마우스를 놓는 순간 바로 주석으로 만든다(하이라이트와 동일한 UX).
  async function addShapeFromDrag(drag) {
    if (hwpMode) return;
    let { page, x0, y0, x1, y1, shift } = drag;
    if (toolMode === 'ellipse' && shift) [x0, y0, x1, y1] = constrainSquareDrag(x0, y0, x1, y1);
    if (Math.abs(x1 - x0) < 2 && Math.abs(y1 - y0) < 2) return; // 드래그 없이 그냥 클릭 — 무시
    try {
      await invoke('add_shape', {
        pageNum: page,
        kind: toolMode,
        x0, y0, x1, y1,
        strokeColor: hexToRgbFloat(shapeStrokeColor),
        fillColor: hexToRgbFloat(shapeFillColor),
        strokeWidth: shapeStrokeWidth,
        opacity: shapeOpacity,
      });
      ++thumbVersion;
      await refreshPageBitmap(page);
      statusMsg = t('status.shape_added');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // 체크박스는 모달 없이 클릭 즉시 토글(하이라이트/도형처럼 바로 반영되는 UX). "켜짐" 값
  // 이름은 백엔드가 필드 외관에서 직접 찾아 계산해서 돌려준다.
  async function toggleFormCheckbox(page, xref) {
    try {
      await invoke('toggle_form_checkbox', { pageNum: page, xref });
      ++thumbVersion;
      await Promise.all([refreshFieldCache(page), refreshPageBitmap(page)]);
      statusMsg = t('status.field_updated');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // 라디오도 체크박스처럼 모달 없이 클릭 즉시 반영 — 이미 선택된 걸 다시 눌러도 안 꺼진다
  // (NO_TOGGLE_TO_OFF가 보통 붙어 있음), 같은 그룹의 다른 버튼은 백엔드가 알아서 꺼준다.
  async function selectFormRadio(page, xref) {
    try {
      await invoke('select_form_radio', { pageNum: page, xref });
      ++thumbVersion;
      await Promise.all([refreshFieldCache(page), refreshPageBitmap(page)]);
      statusMsg = t('status.field_updated');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // 초기화 버튼 — 문서의 모든 페이지·모든 필드를 기본값으로 되돌린다. 지금 보고 있는
  // 페이지는 refreshFieldCache로 끊김 없이 새로 채우고(다른 필드 조작과 같은 이유 —
  // delete()로 비웠다가 채우면 그 사이 포커스가 날아간다), 화면에 없는 다른 페이지는
  // delete만 해서 다음에 그 페이지로 갈 때 자연히 새로 로드되게 한다(안 보이는 페이지라
  // 잠깐 비는 것 자체는 문제가 안 됨).
  async function resetFormFields(page, xref) {
    try {
      await invoke('reset_form_fields', { pageNum: page, xref });
      for (const p of [...pageFieldCache.keys()]) { if (p !== page) pageFieldCache.delete(p); }
      combDrafts.clear();
      ++thumbVersion;
      await Promise.all([refreshFieldCache(page), refreshPageBitmap(page)]);
      statusMsg = t('status.field_updated');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // 텍스트/콤보박스/리스트박스 공통 — 값 하나를 문자열로 저장. 모달 없이 브라우저 폼처럼
  // 필드 자리에 얹어둔 실제 <input>/<textarea>/<select>가 바로 값을 반영한다(콤보/리스트
  // 박스는 onchange, 텍스트는 onblur). 리스트박스가 스펙상 다중 선택(MULTI_SELECT)이어도
  // 여기선 단일 선택만 지원한다 — set_form_text_value가 문자열 하나만 받으므로 고르면 그
  // 값 하나로 덮어쓴다. original은 텍스트 입력에서 "아무것도 안 바꾸고 그냥 클릭했다 벗어난"
  // 경우까지 dirty로 표시하지 않으려는 가드 — select는 onchange 자체가 값이 실제로 바뀔
  // 때만 불려서 필요 없지만 같은 함수를 쓰니 항상 넘겨받는다.
  async function commitFieldValue(page, xref, value, original) {
    if (value === (original ?? '')) return;
    try {
      await invoke('set_form_text_value', { pageNum: page, xref, value });
      ++thumbVersion;
      await Promise.all([refreshFieldCache(page), refreshPageBitmap(page)]);
      statusMsg = t('status.field_updated');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // comb 필드(칸 하나당 글자 하나, 예: ID)의 타이핑 중 상태 — xref -> { value, caret }
  // (caret: 코드포인트 인덱스). 박스 렌더링이 이걸 읽어서 typing 중에도 실시간으로 칸을
  // 채워 보여준다(blur 전 커밋 전에는 pageFieldCache의 f.value가 안 바뀌므로 이게 따로
  // 필요하다). focusedCombXref는 지금 포커스된 comb 필드 — 이 필드일 때만 커스텀 캐럿을
  // 그린다(아래 .field-comb-caret 참고).
  let combDrafts = $state(new SvelteMap());
  let focusedCombXref = $state(null);

  // String.length는 UTF-16 코드 유닛 단위라 서로게이트 페어(이모지 등)가 있으면 글자 수를
  // 실제보다 많게 센다 — 스프레드 연산자([...str])는 코드포인트 단위로 쪼개줘서 정확하다.
  function clampCodepoints(value, maxLen) {
    const chars = [...value];
    return chars.length > maxLen ? chars.slice(0, maxLen).join('') : value;
  }

  function combChars(f) {
    const raw = combDrafts.get(f.xref)?.value ?? f.value ?? '';
    return [...raw];
  }

  function combCaretIndex(f) {
    const entry = combDrafts.get(f.xref);
    return entry ? entry.caret : [...(f.value ?? '')].length;
  }

  // 이 입력창은 글자 자체가 투명이라(.field-comb-input) 브라우저가 지 맘대로 그리는 네이티브
  // 캐럿을 쓰면 안 된다 — 투명 글자의 폭(기본 폰트 기준)과 칸 폭(flex로 균등분배)이 서로
  // 안 맞아서, 네이티브 캐럿 위치와 실제로 글자가 들어갈 칸이 어긋난다(한글처럼 라틴 문자보다
  // 넓은 글자에서 특히 심하게 벌어짐 — 실제로 겪은 버그: 캐럿은 7번째 칸에 보이는데 실제
  // 삭제는 10번째 글자부터 됨). 그래서 네이티브 캐럿은 투명하게 감추고(caret-color:
  // transparent), selectionStart를 직접 읽어 칸 경계에 우리가 그린 막대를 놓는다.
  // selectionStart는 UTF-16 코드 유닛 기준이라 서로게이트 페어가 섞이면 코드포인트 인덱스와
  // 어긋날 수 있지만, ID류 필드엔 사실상 안 나오니 무시한다.
  function syncCombCaret(e, f) {
    // 항상 지금 DOM의 실제 값을 읽는다 — 예전엔 combDrafts에 이미 값이 있으면(포커스 때
    // 한 번 넣어둔 빈 문자열 등) 그걸 계속 우선시하는 버그가 있어서(""도 "값 있음"으로
    // 처리돼 ?? 가 안 걸림), 타이핑을 해도 칸 표시가 그 자리에서 얼어붙고 blur 후 다시
    // 포커스해야만 뒤늦게 따라잡혔다 — 실제로 겪은 버그.
    combDrafts.set(f.xref, { value: e.currentTarget.value, caret: e.currentTarget.selectionStart });
  }

  function onCombFocus(e, f) {
    focusedCombXref = f.xref;
    if (!combDrafts.has(f.xref)) combDrafts.set(f.xref, { value: f.value ?? '', caret: e.currentTarget.selectionStart });
    else syncCombCaret(e, f);
  }

  // 클릭한 x좌표를 칸 폭으로 나눠 "몇 번째 칸 경계"인지 직접 계산해 커서를 그리로 옮긴다 —
  // syncCombCaret과 같은 이유로, 투명 글자 기준의 네이티브 클릭 판정을 그대로 두면 칸
  // 그리드와 안 맞아서 클릭 위치 자체를 우리가 다시 계산해 강제로 덮어쓴다.
  function onCombClick(e, f) {
    e.stopPropagation();
    const rect = e.currentTarget.getBoundingClientRect();
    const boxW = rect.width / f.max_len;
    const idx = Math.max(0, Math.min([...e.currentTarget.value].length, Math.round((e.clientX - rect.left) / boxW)));
    e.currentTarget.setSelectionRange(idx, idx);
    syncCombCaret(e, f);
  }

  function onCombInput(e, f) {
    // 한글 등 IME 조합 도중엔 안 자른다 — 조합 중간에 value를 강제로 바꾸면 IME 후보 상태가
    // 깨질 수 있다(실제로 이 문제 때문에 "눈엔 10칸인데 계속 입력되는" 버그가 있었음 —
    // 대부분의 브라우저가 조합 중엔 native maxlength 검사를 안 하기 때문).
    if (e.isComposing) { syncCombCaret(e, f); return; }
    const clamped = clampCodepoints(e.currentTarget.value, f.max_len);
    if (clamped !== e.currentTarget.value) {
      e.currentTarget.value = clamped;
      // value를 강제로 바꾸면 브라우저가 커서를 0으로 되돌리므로 끝으로 다시 옮겨준다
      e.currentTarget.setSelectionRange(clamped.length, clamped.length);
    }
    syncCombCaret(e, f);
  }

  function onCombCompositionEnd(e, f) {
    const clamped = clampCodepoints(e.currentTarget.value, f.max_len);
    if (clamped !== e.currentTarget.value) {
      e.currentTarget.value = clamped;
      e.currentTarget.setSelectionRange(clamped.length, clamped.length);
    }
    syncCombCaret(e, f);
  }

  function onCombBlur(page, f, e) {
    const value = clampCodepoints(e.currentTarget.value, f.max_len);
    focusedCombXref = null;
    combDrafts.delete(f.xref);
    commitFieldValue(page, f.xref, value, f.value);
  }

  function cancelNote() {
    notePending = null;
    noteText = '';
  }

  async function confirmNote() {
    if (!notePending) return;
    const { page, x, y, xref } = notePending;
    if (xref != null) {
      // 기존 메모 편집 — 내용을 비워서 저장하면 삭제로 취급한다(빈 메모를 남겨둘 이유가 없음)
      if (!noteText.trim()) { await deleteNote(); return; }
      try {
        await invoke('update_note', { pageNum: page, xref, text: noteText, color: hexToRgbFloat(noteColor) });
        cancelNote();
        pageNoteCache.delete(page);
        if (sidebarView === 'notes') await refreshNotesList();
        await refreshPageBitmap(page);
        statusMsg = t('status.note_updated');
      } catch (e) {
        statusMsg = `Error: ${e}`;
      }
      return;
    }
    if (!noteText.trim()) { cancelNote(); return; }
    try {
      await invoke('add_note', { pageNum: page, x, y, text: noteText, color: hexToRgbFloat(noteColor) });
      cancelNote();
      pageNoteCache.delete(page);
      if (sidebarView === 'notes') await refreshNotesList();
      ++thumbVersion;
      await refreshPageBitmap(page);
      statusMsg = t('status.note_added');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  async function deleteNote() {
    if (!notePending?.xref) { cancelNote(); return; }
    const { page, xref } = notePending;
    try {
      await invoke('delete_annotation', { pageNum: page, xref });
      cancelNote();
      pageNoteCache.delete(page);
      if (sidebarView === 'notes') await refreshNotesList();
      ++thumbVersion;
      await refreshPageBitmap(page);
      statusMsg = t('status.note_deleted');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // vim의 :w — 하이라이트 등 지금까지 메모리에만 있던 변경을 원본 파일에 실제로 저장한다.
  // idx를 안 주면 현재 탭(:w). 저장 안 된 다른 탭을 닫기 확인창에서 "저장"으로 고를 때는
  // 그 탭 인덱스를 직접 넘긴다 — 그 탭으로 전환하지 않고도 저장하기 위함.
  async function saveDocument(idx = null) {
    const label = baseName(idx === null || idx === currentTabIdx ? filePath : tabStates[idx]?.filePath ?? '');
    if (!label) { statusMsg = t('status.no_document'); return; }
    // pageRotations(r/R)은 지금까지 화면 표시 전용이었는데, 저장할 때는 실제 페이지에 반영한다.
    // 지금 보고 있는 탭이 아니면(백그라운드 탭을 "저장 안 함/저장" 확인창에서 저장하는 경우)
    // 이 화면 회전은 그 탭 것이 아니므로 적용하지 않는다.
    const isCurrentTab = idx === null || idx === currentTabIdx;
    const rotationsToSave = isCurrentTab
      ? Object.entries(pageRotations).filter(([, deg]) => deg !== 0).map(([page, deg]) => [Number(page), deg])
      : [];
    try {
      await invoke('save_document', { idx, pageRotations: rotationsToSave });
      if (rotationsToSave.length > 0) {
        // 파일에 이미 회전이 구워졌으니 화면 회전을 그대로 두면 두 번 겹쳐 돌아간다 —
        // 비우고 페이지 크기/비트맵 캐시까지 전부 다시 읽어온다(회전으로 페이지
        // 가로/세로가 바뀔 수 있어 비트맵만 지우는 걸로는 부족하다).
        pageRotations = {};
        resetMainViewState();
        await renderCurrentView();
      }
      statusMsg = t('status.saved', { label });
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // 종료 확인 큐에 남은 저장 안 한 탭을 하나씩 확인창으로 띄운다. 다 끝나면(취소 없이)
  // 기존 종료 흐름(탭 여러 개 확인 또는 바로 종료)으로 이어간다.
  async function processNextQuitDirtyTab() {
    if (quitDirtyQueue.length === 0) {
      if (!confirmCloseTabs || quitTabCount <= 1) { invoke('quit_app'); return; }
      showQuitConfirm = true;
      return;
    }
    pendingQuitTabIdx = quitDirtyQueue[0];
    pendingQuitTabName = baseName(pendingQuitTabIdx === currentTabIdx ? filePath : tabStates[pendingQuitTabIdx]?.filePath ?? '');
    showQuitDiscardConfirm = true;
  }

  // mousedown 안에서 await로 로딩을 기다리면, 그 사이 사용자가 이미 mouseup까지 끝내버린
  // 경우(첫 드래그가 아무 반응 없던 증상) 뒤늦게 도착한 응답이 selecting/selection을
  // "다음" 드래그 도중에 덮어써서 엉뚱한 곳이 선택되는 레이스가 생긴다. 그래서 여기서는
  // await 없이 이미 캐시된 것만 동기로 쓰고, 로딩 자체는 loadMainPageAt/setToolMode에서
  // 미리 해둔다(ensureLinksLoaded와 같은 패턴).
  let pendingStart = null; // {page, idx} — mousedown 지점. 실제로 다른 글자로 드래그되기 전엔
                            // selection을 만들지 않는다 — 만들었다가 클릭으로 끝나면 바로 지워야
                            // 해서 하이라이트가 깜빡였다.

  function onPageMouseDown(e, page) {
    if ((e.metaKey || e.ctrlKey) && e.shiftKey) return; // SyncTeX 역방향 검색(⇧⌘/⇧⌃+클릭) — onPageClick이 처리
    if (getPageRotation(page) !== 0) return;
    if (e.button !== 0) return; // 우클릭(컨텍스트 메뉴)까지 여기로 들어와 기존 선택을 지워버렸다
    if (shapeMode) {
      e.preventDefault();
      const rect = e.currentTarget.getBoundingClientRect();
      const x = (e.clientX - rect.left) / scale;
      const y = (e.clientY - rect.top) / scale;
      shapeDrag = { page, x0: x, y0: y, x1: x, y1: y, shift: e.shiftKey, dragging: true };
      return;
    }
    if (!textSelectMode) return;
    e.preventDefault();
    mouseButtonDown = true;
    pendingMouseDown = null; // 새 mousedown이 왔으니 이전에 기다리던 위치는 무효화
    ensureCharsLoaded(page);
    const chars = pageCharCache.get(page);
    if (!chars || chars.length === 0) {
      // 아직 로딩(OCR 포함) 중 — 끝나면 ensureCharsLoaded가 이 위치에서 이어서 시작해준다
      pendingMouseDown = { page, clientX: e.clientX, clientY: e.clientY };
      return;
    }
    const rect = e.currentTarget.getBoundingClientRect();
    const idx = charIndexAt(page, (e.clientX - rect.left) / scale, (e.clientY - rect.top) / scale);
    if (idx === null) return;
    selecting = true;
    anchorIdx = null;
    cursor = null;
    visualMode = false;
    pendingStart = { page, idx };
  }

  function onSelectionDragMove(e) {
    // dragging이 꺼진 뒤(마우스를 뗀 직후, add_shape 저장이 끝나길 기다리는 동안)엔 무시해야
    // 마우스를 뗀 자리에 도형이 바로 고정된다 — 안 그러면 저장이 끝나기 전에 커서가 움직인
    // 만큼 미리보기가 계속 따라와서 늘었다 줄어드는 것처럼 보인다.
    if (shapeDrag?.dragging) {
      const wrap = pageWrapEls[shapeDrag.page];
      if (!wrap) return;
      const rect = wrap.getBoundingClientRect();
      shapeDrag = {
        ...shapeDrag,
        x1: (e.clientX - rect.left) / scale,
        y1: (e.clientY - rect.top) / scale,
        shift: e.shiftKey,
      };
      return;
    }
    if (!selecting || !pendingStart) return;
    const wrap = pageWrapEls[pendingStart.page];
    if (!wrap) return;
    const rect = wrap.getBoundingClientRect();
    const idx = charIndexAt(pendingStart.page, (e.clientX - rect.left) / scale, (e.clientY - rect.top) / scale);
    if (idx === null) return;
    if (anchorIdx === null && idx === pendingStart.idx) return; // 아직 시작 글자 그대로 — 클릭인지 드래그인지 미정
    anchorIdx = pendingStart.idx;
    cursor = { page: pendingStart.page, idx };
    visualMode = true; // 드래그로 실제 선택이 생겼으니 이어서 w/W로 확장할 수 있게
  }

  async function copySelection() {
    if (!selection) return;
    const text = selectionText();
    if (text.trim()) {
      await clipboardWrite(text);
      statusMsg = t('status.copied_chars', { n: text.length });
    }
  }

  // Cmd+C는 메뉴 단축키로만 잡혀서(Rust lib.rs의 copy_sel 주석 참고 — WKWebView가 실제
  // 네이티브 선택 없인 조용히 삼켜버리기 때문) 포커스가 어디 있든 이 경로 하나로 들어온다.
  // 그래서 뭘 복사할지 여기서 직접 판단해야 한다 — 안 그러면 설정 창 입력란 등 일반 UI
  // 텍스트를 선택하고 Cmd+C를 눌러도 항상 PDF 텍스트 선택만 복사돼버린다.
  async function copyActiveSelection() {
    const active = document.activeElement;
    if (active && (active.tagName === 'INPUT' || active.tagName === 'TEXTAREA') && active.selectionStart !== active.selectionEnd) {
      await clipboardWrite(active.value.slice(active.selectionStart, active.selectionEnd));
      return;
    }
    const domSelection = window.getSelection()?.toString();
    if (domSelection) {
      await clipboardWrite(domSelection);
      return;
    }
    await copySelection();
  }

  function showCharValue() {
    const chars = cursor ? pageCharCache.get(cursor.page) : null;
    const ch = chars?.[cursor?.idx]?.ch;
    if (ch === undefined) { statusMsg = t('status.no_cursor'); return; }
    const cp = ch.codePointAt(0);
    statusMsg = t('status.char_value', { char: ch, dec: cp, hex: cp.toString(16), oct: cp.toString(8) });
  }

  async function showSelectionFont() {
    if (!selection) { statusMsg = t('status.no_selection'); return; }
    const chars = pageCharCache.get(selection.page) ?? [];
    const [lo, hi] = selection.startIdx <= selection.endIdx ? [selection.startIdx, selection.endIdx] : [selection.endIdx, selection.startIdx];
    const fonts = [...new Set(chars.slice(lo, hi + 1).map(c => c.font).filter(Boolean))];
    await message(fonts.length ? fonts.join('\n') : t('status.no_font_info'), { title: t('status.selection_font_title'), kind: 'info' });
  }

  async function stopSelectionDrag(e) {
    if (shapeDrag) {
      // 마우스를 뗀 그 자리에 바로 고정: dragging을 꺼서 onSelectionDragMove가 더는
      // x1/y1을 갱신하지 못하게 한다(안 그러면 저장 끝날 때까지 커서를 따라 계속 늘어난다).
      // 그 다음, 실제 주석이 반영된 새 비트맵이 그려지기 전까지는 이 마지막 모습 그대로
      // 미리보기를 켜둔다 — 먼저 지워버리면 add_shape invoke + refreshPageBitmap이 끝나는
      // 그 짧은 사이에 도형이 화면에서 잠깐 사라졌다 나타나는 것처럼 보인다.
      const drag = { ...shapeDrag, dragging: false };
      shapeDrag = drag;
      await addShapeFromDrag(drag);
      shapeDrag = null;
      return;
    }
    mouseButtonDown = false;
    if (pendingMouseDown) {
      // 아직 로딩(OCR 포함) 중에 손을 뗐다 — 뗀 위치를 기억해두면 ensureCharsLoaded가
      // 로딩이 끝나는 대로 시작~끝 구간을 한 번에 확정해준다
      if (e) pendingMouseUp = { clientX: e.clientX, clientY: e.clientY };
      return;
    }
    if (!selecting) return;
    selecting = false;
    // 드래그였든 그냥 클릭이었든 도착 지점에 캐럿을 놓는다 — 클릭이면 선택 범위 없이 캐럿만,
    // 드래그였으면 이미 selection이 있는 채로. 복사는 y/Y를 눌러야 한다.
    if (pendingStart && !selection) {
      anchorIdx = pendingStart.idx;
      cursor = { page: pendingStart.page, idx: pendingStart.idx };
    }
    pendingStart = null;
    // 하이라이트 도구는 드래그를 놓는 순간 바로 하이라이트로 확정한다 — H를 따로 누를 필요 없음
    if (toolMode === 'highlight' && selection) await addHighlightToSelection();
  }

  function isWordChar(ch) {
    return /[\p{L}\p{N}]/u.test(ch);
  }

  // ── vim 스타일 w/W/b/B/e/E 단어 이동 ──
  // w/b/e: 문자/숫자/_ 뭉치와 구두점 뭉치를 서로 다른 "단어"로 구분.
  // W/B/E(대문자, big=true): 공백만 경계로 본다.
  function charClass(ch) {
    if (/\s/.test(ch)) return 0;
    if (/[\p{L}\p{N}_]/u.test(ch)) return 1;
    return 2;
  }

  // 실제 vim의 "다음 단어 시작"에 착지하는 원래 값 — 커서만 옮길 때(선택 없이) 쓴다
  function vimWordLanding(chars, idx, big) {
    const n = chars.length;
    const cls = big ? (c => (/\s/.test(c) ? 0 : 1)) : charClass;
    let i = Math.min(Math.max(idx, 0), n - 1);
    const startCls = cls(chars[i].ch);
    if (startCls !== 0) {
      while (i < n - 1 && cls(chars[i + 1].ch) === startCls) i++;
    }
    while (i < n - 1 && cls(chars[i + 1].ch) === 0) i++;
    return Math.min(i + 1, n - 1);
  }

  // visual mode에서 선택을 늘릴 때 쓰는 값 — 다음 단어 시작 "앞 글자"까지만 포함(exclusive)
  function motionWordForward(chars, idx, big) {
    const n = chars.length;
    if (n === 0 || idx >= n - 1) return idx;
    const landing = vimWordLanding(chars, idx + 1, big);
    if (landing >= n - 1) return n - 1; // 다음 단어가 없음 — 페이지 끝까지 선택
    return Math.max(idx, landing - 1);
  }

  // 이전 단어의 시작으로 이동 — 커서 단독 이동/선택 확장 둘 다 이 값을 그대로 쓴다
  // (뒤로 갈 땐 착지 지점 자체가 이미 "그 단어의 시작"이라 exclusive 보정이 필요 없다)
  function motionWordBackward(chars, idx, big) {
    const cls = big ? (c => (/\s/.test(c) ? 0 : 1)) : charClass;
    let i = idx - 1;
    if (i < 0) return 0;
    while (i > 0 && cls(chars[i].ch) === 0) i--;
    if (cls(chars[i].ch) === 0) return 0; // 여기까지 전부 공백 — 문서 시작
    const runCls = cls(chars[i].ch);
    while (i > 0 && cls(chars[i - 1].ch) === runCls) i--;
    return i;
  }

  // 다음 단어의 끝으로 이동 — b와 마찬가지로 착지 지점 자체가 이미 "포함할 마지막 글자"라
  // 커서 단독 이동/선택 확장 둘 다 이 값을 그대로 쓴다(exclusive 보정 불필요)
  function motionWordEnd(chars, idx, big) {
    const n = chars.length;
    const cls = big ? (c => (/\s/.test(c) ? 0 : 1)) : charClass;
    let i = idx + 1;
    if (i >= n) return n - 1;
    while (i < n && cls(chars[i].ch) === 0) i++;
    if (i >= n) return n - 1;
    const runCls = cls(chars[i].ch);
    while (i + 1 < n && cls(chars[i + 1].ch) === runCls) i++;
    return i;
  }

  // dir: 'w'(다음 단어 시작) | 'b'(이전 단어 시작) | 'e'(다음 단어 끝)
  function wordMotion(big, dir) {
    if (!cursor) return;
    const chars = pageCharCache.get(cursor.page);
    if (!chars || chars.length === 0) return;
    if (visualMode) {
      // visual mode: anchor는 그대로 두고 커서만 옮겨서 선택을 늘리거나 줄인다
      if (anchorIdx === null) anchorIdx = cursor.idx;
      const idx = dir === 'b' ? motionWordBackward(chars, cursor.idx, big)
        : dir === 'e' ? motionWordEnd(chars, cursor.idx, big)
        : motionWordForward(chars, cursor.idx, big);
      cursor = { page: cursor.page, idx };
    } else {
      // 실제 vim처럼 그냥 커서만(선택 없이) 이동 — anchor도 같이 따라간다
      const idx = dir === 'b' ? motionWordBackward(chars, cursor.idx, big)
        : dir === 'e' ? motionWordEnd(chars, cursor.idx, big)
        : vimWordLanding(chars, cursor.idx + 1, big);
      anchorIdx = idx;
      cursor = { page: cursor.page, idx };
    }
  }

  // 커서가 있을 때 h/l로 한 글자씩 이동 — dir: -1(왼쪽/뒤) | 1(오른쪽/앞)
  function charMotion(dir) {
    if (!cursor) return;
    const chars = pageCharCache.get(cursor.page);
    if (!chars || chars.length === 0) return;
    const idx = Math.min(Math.max(cursor.idx + dir, 0), chars.length - 1);
    if (visualMode) {
      if (anchorIdx === null) anchorIdx = cursor.idx;
    } else {
      anchorIdx = idx;
    }
    cursor = { page: cursor.page, idx };
  }

  // ── 우클릭 복사 메뉴 ── macOS WKWebView가 실제 네이티브 선택 없이는 Cmd+C를
  // 자체 responder에서 삼켜버려 메뉴 단축키조차 못 뚫는 경우가 있어, 우클릭으로
  // 확실하게 복사할 수 있는 경로를 따로 둔다. HTML로 직접 그리면 macOS 기본
  // 컨텍스트 메뉴와 모양이 달라서 어색하니, 네이티브 메뉴를 그 자리에 띄운다.
  async function onPageContextMenu(e, page) {
    // contextmenu 핸들러는 e.preventDefault()를 동기적으로(await 이전에) 호출해야 OS 기본
    // 메뉴를 막을 수 있어서, 하이라이트 캐시가 아직 없는 경우를 대비한 예비 로드만 비동기로
    // 걸어두고(다음 우클릭부터 반영됨) 판정 자체는 이미 채워진 캐시로 동기 처리한다 —
    // loadMainPageAt이 페이지 렌더 때마다 미리 채워두므로 보통은 바로 준비돼 있다.
    let hlHit = null;
    if (getPageRotation(page) === 0) {
      const rect = e.currentTarget.getBoundingClientRect();
      const x = (e.clientX - rect.left) / scale;
      const y = (e.clientY - rect.top) / scale;
      hlHit = highlightAt(page, x, y);
      ensureHighlightsLoaded(page);
    }
    // 텍스트 선택 도구가 꺼져 있어도 검색/키보드로 커서가 놓여 있으면(vim 모션으로 고른
    // 선택 포함) 이 메뉴를 띄운다 — 안 그러면 기본 메뉴(Reload/Inspect Element)만 나온다
    if (!textSelectMode && !cursor && !hlHit) return;
    e.preventDefault();
    const copyItem = await MenuItem.new({
      text: t('context.copy'),
      enabled: !!selection,
      action: () => { copySelection(); },
    });
    const googleItem = await MenuItem.new({
      text: t('context.google_search'),
      enabled: !!selection,
      action: () => { googleSearch(selectionText()); },
    });
    const fontItem = await MenuItem.new({
      text: t('context.font_info'),
      enabled: !!selection,
      action: () => { showSelectionFont(); },
    });
    const items = [copyItem, googleItem, fontItem];
    if (hlHit) {
      items.push(await MenuItem.new({
        text: t('context.delete_highlight'),
        action: () => { deleteHighlight(page, hlHit.xref); },
      }));
    }
    const menu = await Menu.new({ items });
    await menu.popup();
  }

  function onPageDoubleClick(e, page) {
    if (!textSelectMode || getPageRotation(page) !== 0) return;
    const chars = pageCharCache.get(page);
    if (!chars || chars.length === 0) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const idx = charIndexAt(page, (e.clientX - rect.left) / scale, (e.clientY - rect.top) / scale);
    if (idx === null || !isWordChar(chars[idx].ch)) return;
    let lo = idx, hi = idx;
    while (lo > 0 && isWordChar(chars[lo - 1].ch)) lo--;
    while (hi < chars.length - 1 && isWordChar(chars[hi + 1].ch)) hi++;
    anchorIdx = lo;
    cursor = { page, idx: hi };
    visualMode = true; // 단어 하나가 이미 선택됐으니 이어서 w/W로 확장할 수 있게
  }

  function setToolMode(mode) {
    toolMode = mode;
    anchorIdx = null;
    cursor = null;
    visualMode = false;
    pendingMouseDown = null;
    pendingMouseUp = null;
    shapeDrag = null;
    notePending = null;
    noteText = '';
    hoverNote = null;
    hoverField = null;
    // 이미 화면에 떠 있는 페이지는 loadMainPageAt이 다시 불려주지 않으니 켜는 시점에 직접 미리 로드
    if (textSelectMode) {
      for (let i = 0; i < pageWrapEls.length; i++) {
        if (pageWrapEls[i]) ensureCharsLoaded(i);
      }
    }
  }

  let twoPageIndices = $derived.by(() => {
    if (!isTwoUp(viewMode) || pageCount === 0) return [];
    const start = Math.floor(currentPage / 2) * 2;
    return start + 1 < pageCount ? [start, start + 1] : [start];
  });
  let pageRows = $derived.by(() => {
    if (viewMode !== 'two-continuous') return [];
    const rows = [];
    for (let i = 0; i < pageCount; i += 2) rows.push([i, i + 1 < pageCount ? i + 1 : null]);
    return rows;
  });

  function resetMainViewState() {
    ++pageVersion;
    pageCanvasRefs = [];
    pageWrapEls = [];
    visiblePageSet.clear();
    pageSizes = [];
    pageSizePromises.clear();
    pageLinkCache.clear();
    pageNoteCache.clear();
    pageHighlightAnnotCache.clear();
    pageFieldCache.clear();
    combDrafts.clear();
    focusedCombXref = null;
    pageBitmapCache.clear();
    pageBitmapBytes = 0;
    paintedPages.clear();
    paintedPagesBytes = 0;
    pageCharCache.clear();
    anchorIdx = null;
    cursor = null;
    visualMode = false;
    pendingMouseDown = null;
    pendingMouseUp = null;
    selectedThumbs = new SvelteSet();
    thumbSelectAnchor = null;
    // windowLo/windowHi는 여기서 currentPage 기준으로 정하지 않는다 — 탭 전환 시
    // resetMainViewState()가 currentPage 갱신보다 먼저 호출되므로, 최종 currentPage가
    // 정해진 뒤 각 호출부(gotoPage/openFile/setViewMode/renderCurrentView)에서 직접 맞춘다.
  }

  // prefetch용 observer는 여백(400px)을 둬서 스크롤하기 전에 미리 그려두는데, 그 여백 안에
  // 든 "화면 밖" 페이지까지 currentPage 계산에 섞이면 사이드바/상태표시줄 페이지 번호가
  // 실제 스크롤 위치보다 한참 앞서 나가버린다 — 그래서 currentPage는 여백이 전혀 없는
  // 별도 observer로, 진짜 화면에 걸쳐 있는 페이지만 보고 갱신한다.
  // (여백은 원래 800px이었는데, 렌더링이 무거운 문서(고해상도 스캔 PDF 등)에서 한 번에
  // 여러 페이지가 동시에 렌더 요청되면 MuPDF 단일 락 때문에 뒤로 밀린 페이지가 몇 초씩
  // 대기하는 문제가 있어 동시 프리페치 범위를 줄였다.)
  // 리사이즈(특히 전체화면 토글)로 뷰포트가 한 번에 커지면 이 여백(400px) 안에 페이지가
  // 왕창 들어와 IntersectionObserver가 전부 한꺼번에 발동한다. MuPDF 문서 접근은 Mutex
  // 하나로 직렬화돼 있어서(위 rootMargin을 800→400으로 줄인 이유와 같은 문제) 이게 몇 초짜리
  // 정체를 만들고, 그동안 쌓인 키 입력이 몰아서 처리되며 오래된 입력이 그 시점엔 안 맞는
  // currentPage를 기준으로 실행되는(예: f로 놓은 링크 힌트가 엉뚱한 페이지 링크를 여는)
  // 문제까지 이어졌다. 동시 요청 수를 클라이언트에서 직접 제한해서 완화한다.
  let activePageLoads = 0;
  const pendingPageLoads = [];
  const MAX_CONCURRENT_PAGE_LOADS = 2;
  function queuePageLoad(task) {
    pendingPageLoads.push(task);
    pumpPageLoadQueue();
  }
  function pumpPageLoadQueue() {
    while (activePageLoads < MAX_CONCURRENT_PAGE_LOADS && pendingPageLoads.length > 0) {
      const task = pendingPageLoads.shift();
      activePageLoads++;
      task().finally(() => { activePageLoads--; pumpPageLoadQueue(); });
    }
  }

  // 썸네일 전용 큐 — 메인 페이지 큐(pendingPageLoads)와 같은 걸 쓰면, 연속 보기에서 스크롤로
  // 메인 페이지 로드가 계속 쌓일 때 뒤에 밀린 썸네일이 한참(페이지 수가 많은 문서면 몇 초~수십
  // 초) 안 뜨는 것처럼 보인다 — 둘을 분리해서 사이드바가 메인 뷰 스크롤에 안 밀리게 한다.
  let activeThumbLoads = 0;
  const pendingThumbLoads = [];
  const MAX_CONCURRENT_THUMB_LOADS = 2;
  function queueThumbLoad(task) {
    pendingThumbLoads.push(task);
    pumpThumbLoadQueue();
  }
  function pumpThumbLoadQueue() {
    while (activeThumbLoads < MAX_CONCURRENT_THUMB_LOADS && pendingThumbLoads.length > 0) {
      const task = pendingThumbLoads.shift();
      activeThumbLoads++;
      task().finally(() => { activeThumbLoads--; pumpThumbLoadQueue(); });
    }
  }

  function setupMainPageObservers() {
    const isH = viewMode === 'horizontal-continuous';
    mainPageObserver = new IntersectionObserver((entries) => {
      for (const en of entries) {
        if (en.isIntersecting) {
          const i = Number(en.target.dataset.page);
          queuePageLoad(() => loadMainPageAt(i, pageVersion));
        }
      }
    }, { root: mainEl, rootMargin: isH ? '0px 400px' : '400px 0px' });

    currentPageObserver = new IntersectionObserver((entries) => {
      for (const en of entries) {
        const i = Number(en.target.dataset.page);
        if (en.isIntersecting) visiblePageSet.add(i);
        else visiblePageSet.delete(i);
      }
      if ((isContinuous(viewMode) || windowLo !== windowHi) && visiblePageSet.size > 0) {
        const min = Math.min(...visiblePageSet);
        if (min !== currentPage) {
          currentPage = min;
          // 스크롤로 currentPage가 바뀔 때는 gotoPage를 거치지 않으니 사이드바 썸네일/toc도
          // 따로 따라가게 해줘야 한다 (포커스는 뺏지 않도록 scrollIntoView·펼침만)
          thumbEls[min]?.scrollIntoView({ block: 'nearest' });
          ensureTocPageVisible();
        }
      }
    }, { root: mainEl, rootMargin: '0px' });
  }

  async function loadMainPageAt(i, version) {
    const canvas = pageCanvasRefs[i];
    // "이미 이 배율로 그려졌나"를 페이지 인덱스가 아니라 canvas 노드 자체(dataset)에 표시한다 —
    // 'two'/'single' 모드에서 페이지를 넘기면 이전 canvas는 사라지고 새 빈 canvas가 그 자리에
    // 붙는데, 배열을 인덱스로만 체크하면 "이미 그렸다"고 착각해 새 캔버스를 비워둔 채 건너뛴다.
    if (!canvas) return;
    ensureLinksLoaded(i);
    ensureNotesLoaded(i);
    ensureHighlightsLoaded(i);
    ensureFieldsLoaded(i);
    if (textSelectMode) ensureCharsLoaded(i);
    if (canvas.dataset.scale === `${scale}:${getPageRotation(i)}`) return;
    try {
      const buf = await fetchPageBitmap(i);
      if (version !== pageVersion || pageCanvasRefs[i] !== canvas) return; // 그 사이 캔버스가 교체됨
      paintCanvasFromBuffer(canvas, buf, i);
      markPagePainted(i, buf.byteLength);
      // 캔버스에 이미 칠해졌으니 pageBitmapCache의 원본 버퍼는 더 필요 없다(그 목적은 "칠하기
      // 전" prefetch뿐) — 안 지우면 같은 페이지의 바이트가 두 캐시(이 캐시 + paintedPages)에
      // 동시에 남아 상한을 두 배로 쓰는 꼴이 된다.
      deletePageBitmapEntry(`${i}:${scale}:${getPageRotation(i)}`);
    } catch (_) { /* skip broken page */ }
  }

  function lazyMainPage(node, i) {
    node.dataset.page = i;
    if (!mainPageObserver) setupMainPageObservers();
    mainPageObserver.observe(node);
    currentPageObserver.observe(node);
    // unobserve()는 마지막으로 "안 보임" 콜백을 보장하지 않으므로, 'two'/'single' 모드처럼
    // 페이지가 넘어가며 wrap div가 통째로 사라질 때 여기서 직접 visiblePageSet을 정리해야
    // 다음 페이지에 있던 stale 인덱스가 계속 남아있지 않는다.
    return {
      destroy: () => {
        mainPageObserver?.unobserve(node);
        currentPageObserver?.unobserve(node);
        visiblePageSet.delete(i);
      },
    };
  }

  let lazyZoomTimer;
  function refreshLazyView() {
    clearTimeout(lazyZoomTimer);
    lazyZoomTimer = setTimeout(() => {
      for (const i of visiblePageSet) loadMainPageAt(i, pageVersion);
      // 줌이 바뀌면 이전 배율로 미리 받아둔 인접 페이지 캐시가 무효가 된다 — single 모드에서
      // 페이지를 넘기면(gotoPage) 그제서야 새 배율로 다시 받게 되는데, 그 왕복 시간만큼
      // (메인 스레드는 안 막혀도) 화면에 빈 배경이 잠깐 비친다. 줌이 가라앉은 시점에 미리
      // 새 배율로 받아두면 다음 페이지 넘김이 캐시 히트가 된다.
      if (viewMode === 'single') prefetchAdjacentSizes(currentPage);
    }, 120);
  }

  // zi/zo처럼 연타되는 게 아니라 한 번의 명확한 동작(줌 맞춤 등)이라 디바운스 없이 바로 그린다.
  async function rerenderNow() {
    await tick();
    for (const i of visiblePageSet) await loadMainPageAt(i, pageVersion);
  }

  function triggerRerender() {
    refreshLazyView();
  }

  // View 메뉴에서 보기 모드를 바꿨을 때 — 지금까지 그려둔 페이지 캔버스/observer를 버리고
  // 새 모드에 맞는 렌더링을 다시 시작한다.
  const VIEW_MODES = ['single', 'single-continuous', 'two', 'two-continuous', 'horizontal-continuous'];

  async function setViewMode(mode) {
    if (mode === viewMode) return;
    viewMode = mode;
    localStorage.setItem('vimong-view-mode', mode);
    mainPageObserver?.disconnect();
    mainPageObserver = null;
    currentPageObserver?.disconnect();
    currentPageObserver = null;
    resetMainViewState();
    if (!filePath || mdMode) return;
    if (hwpMode) { await tick(); pageWrapEls[currentPage]?.scrollIntoView({ block: 'start', inline: 'start' }); return; }
    if (mode === 'single') {
      await ensurePageSize(currentPage);
      windowLo = currentPage;
      windowHi = currentPage;
      prefetchAdjacentSizes(currentPage);
    } else {
      pageSizes = await invoke('get_all_page_sizes');
    }
    await tick();
    pageWrapEls[currentPage]?.scrollIntoView({ block: 'start', inline: 'start' });
  }

  // 탭 전환/닫기로 다른 문서가 활성화됐을 때 현재 보기 모드에 맞게 다시 그린다.
  // (문서가 바뀌었을 뿐 모드 자체는 그대로이므로 observer는 유지하고 페이지 자료만 새로 받는다)
  async function renderCurrentView() {
    if (!filePath || mdMode) return;
    if (hwpMode) { await tick(); pageWrapEls[currentPage]?.scrollIntoView({ block: 'start', inline: 'start' }); return; }
    if (viewMode === 'single') {
      await ensurePageSize(currentPage);
      windowLo = currentPage;
      windowHi = currentPage;
      prefetchAdjacentSizes(currentPage);
    } else {
      pageSizes = await invoke('get_all_page_sizes');
    }
    await tick();
    pageWrapEls[currentPage]?.scrollIntoView({ block: 'start', inline: 'start' });
  }

  // ── 인쇄 ───────────────────────────────────────────────────
  // 별도 확인창 없이 시스템 인쇄 패널 하나로 통합 — 방향은 Layout의 "Reverse Page
  // Orientation", 배율은 Paper Handling의 "Scale to Fit Paper Size"로 그 패널 안에서
  // 바로 조절된다(패널에 프린터가 안 잡혀 있으면 일부 항목이 안 보일 수 있음 — macOS가
  // 프린터 드라이버(PPD)에서 용지 정보를 가져오는 구조라 앱에서 어찌할 수 없는 부분).
  // render_page는 scale=1일 때 PDF 좌표(1pt=1/72in)를 그대로 픽셀에 매핑하는데, 브라우저
  // 인쇄는 CSS px를 1/96in로 해석한다 — 그 차이(96/72)만큼 배율을 올려야 실제 인쇄물이
  // 원본 페이지의 물리적 크기(예: A4/Letter)와 맞게 나온다.
  const PRINT_SCALE = 96 / 72;
  let printRestore = null; // 인쇄 직전 보기 모드/배율 — afterprint에서 복원, null이면 복원할 것 없음

  // afterprint가 안 뜨는 환경(webview가 print()를 지원 안 하거나 조용히 무시하는 경우)에도
  // "인쇄 준비 중"에 영영 멈춰있지 않도록, window.print() 호출 직후에도 곧장 한 번 더 부른다.
  // printRestore가 null이면(이미 복원됐으면) 아무 일도 안 해서 두 번 불러도 안전하다.
  async function restorePrintState() {
    if (!printRestore) return;
    const { viewMode: vm, scale: sc, statusMsg: sm } = printRestore;
    printRestore = null;
    scale = sc;
    await setViewMode(vm);
    statusMsg = sm;
  }

  // Tauri 문서는 window.print()가 "모든 플랫폼에서 동작"한다고 하지만 webview 구현(wry)에 따라
  // 조용히 아무 것도 안 하는 경우가 있다 — 같은 문서에서 "현재 macOS만 지원"이라는 네이티브
  // print_webview 커맨드를 먼저 시도하고, 안 되면(Windows/Linux 등) window.print()로 폴백한다.
  async function triggerPrint() {
    try { await invoke('print_webview'); }
    catch (_) { window.print(); }
  }

  async function printDocument() {
    if (!filePath) return;
    printRestore = { viewMode, scale, statusMsg };
    let error = null;
    try {
      if (hwpMode) {
        // hwp는 이미 모든 페이지가 항상 DOM에 그려져 있어(지연 로딩 없음) 배율만 1(원본
        // 크기)로 되돌리면 된다 — SVG 좌표가 이미 CSS px 기준이라 PRINT_SCALE 변환 불필요
        scale = 1;
        await tick();
        await triggerPrint();
      } else {
        // 인쇄는 뷰 모드와 무관하게 한 페이지씩 세로로 쭉 이어붙인 레이아웃이어야 브라우저의
        // 세로 페이지 나눔(@media print의 break-after)이 제대로 먹는다 — 'two'/'horizontal-continuous'
        // 그대로 인쇄하면 페이지가 가로로 잘리거나 두 장씩 겹쳐 나온다.
        scale = PRINT_SCALE;
        await setViewMode('single-continuous');
        // 지연 로딩(IntersectionObserver)은 화면에 보이는 페이지만 그리므로, 인쇄 전에 전 페이지를
        // 강제로 한 번씩 그려둬야 스크롤 안 해본 뒤쪽 페이지가 빈 캔버스로 인쇄되지 않는다. 페이지
        // 수가 많으면 몇 초 걸릴 수 있어 진행 중임을 상태 표시줄에 남긴다.
        statusMsg = t('status.print_preparing');
        for (let i = 0; i < pageCount; i++) await loadMainPageAt(i, pageVersion);
        await tick();
        await triggerPrint();
      }
    } catch (e) {
      error = e;
    }
    // afterprint가 안 뜨는 환경(webview가 print()를 조용히 무시하는 경우 등)에도 "인쇄 준비
    // 중"에 영영 멈춰있지 않도록 여기서도 곧장 복원한다 — printRestore가 이미 null이면(정상적으로
    // afterprint가 먼저 왔으면) restorePrintState는 아무 일도 안 해서 두 번 불러도 안전하다.
    await restorePrintState();
    if (error) statusMsg = t('status.print_error', { e: error });
  }

  // :images — 현재 페이지에 박혀 있는 이미지를 전부 골라둔 폴더에 파일로 저장한다.
  async function exportPageImages() {
    await exportImagesForPages([currentPage]);
  }

  // 사이드바 썸네일 우클릭 메뉴에서도 쓴다 — 여러 페이지를 선택했으면 전부 같은 폴더에 저장.
  async function exportImagesForPages(pages) {
    if (!filePath || hwpMode || mdMode) { statusMsg = t('status.no_document'); return; }
    try {
      const counts = await Promise.all(pages.map((p) => invoke('page_image_count', { pageNum: p })));
      if (counts.every((c) => c === 0)) { flashToast(t('status.images_none')); return; }
    } catch (e) {
      statusMsg = `Error: ${e}`;
      return;
    }
    const dir = await open({ directory: true, title: t('images.pick_folder') });
    if (!dir) return;
    try {
      let total = 0;
      for (const p of pages) {
        total += await invoke('export_page_images', { pageNum: p, dir });
      }
      if (total > 0) {
        statusMsg = t('status.images_exported', { n: total });
      } else {
        flashToast(t('status.images_none'));
      }
    } catch (e) {
      statusMsg = `Error: ${e}`;
    }
  }

  // ── UI state ───────────────────────────────────────────────
  let statusMsg = $state('No file open.  :e to open');
  // 상태표시줄만으로는 놓치기 쉬운 메시지를 잠깐 눈에 띄게 띄운다(자동으로 사라짐).
  let toastMsg = $state('');
  let toastTimer = 0;
  function flashToast(msg) {
    toastMsg = msg;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => { toastMsg = ''; }, 2500);
  }
  let showFileInfo = $state(false);
  let showShortcuts = $state(false);
  let showCommands = $state(false);
  let showOpenSource = $state(false);
  let showAbout = $state(false);
  // 정보성 모달(도움말류) — 탭 전환/생성/닫기 등 문서 상태를 바꾸는 동작과 부딪히면
  // 모달만 붕 뜬 채 남으니, 열려있는 동안은 그런 동작을 막는 데 같이 쓴다.
  function blockingHelpModalOpen() {
    return showFileInfo || showShortcuts || showCommands || showOpenSource || showSettings || showAbout;
  }
  let appVersion = $state('');
  let showQuitConfirm = $state(false);
  let quitTabCount = $state(0);
  let showTabOnlyConfirm = $state(false);
  let tabOnlyCount = $state(0);
  let showDiscardConfirm = $state(false);
  let pendingCloseTabIdx = null; // 비반응 — 확인창에서 어느 탭을 닫으려던 건지만 기억
  let pendingCloseTabName = $state(''); // 확인창 제목에 파일명을 보여주려고 반응형으로 둠
  // 앱 종료(⌘Q/창 닫기 버튼) 직전에 저장 안 한 탭이 있으면 하나씩 순서대로 확인한다 —
  // 탭 닫기(showDiscardConfirm)와 달리 확인이 끝나도 탭을 지우지 않고(어차피 앱이 곧
  // 종료되므로) 다음 저장 안 한 탭으로 넘어가거나, 다 끝나면 실제로 종료한다.
  let showQuitDiscardConfirm = $state(false);
  let quitDirtyQueue = []; // 비반응 — 종료 전에 확인해야 할 남은 탭 인덱스들
  let pendingQuitTabIdx = null; // 비반응
  let pendingQuitTabName = $state('');
  let showPasswordPrompt = $state(false);
  let passwordInput = $state('');
  let passwordError = $state('');
  let passwordResolve;
  let showPasswordManage = $state(false);
  let passwordManageTab = $state('set'); // 'set' | 'remove'
  let newPassword1 = $state('');
  let newPassword2 = $state('');
  let passwordManageError = $state('');
  let passwordManageBusy = $state(false);
  let showWritePasswordPrompt = $state(false);
  let writePassword1 = $state('');
  let writePassword2 = $state('');
  let writePasswordError = $state('');
  let writePasswordBusy = $state(false);
  let pendingWrite = null; // :w -p 또는 :export -p 로 암호 입력 대기 중인 요청
    // { outPath, pageSpec } (:w — 항상 pdf/merge 고정) 또는 { outPath, pageSpec, merge, format, kind: 'export' } (:export)
  let showExportModal = $state(false);
  let exportFormat = $state('png'); // 'png' | 'jpg' | 'pdf' — 드롭박스로만 고르므로 항상 유효
  let exportFilename = $state(''); // 확장자 제외 — exportFormat과 합쳐 원본 PDF와 같은 폴더에 저장
  let exportPassword = $state(''); // PDF 내보내기 전용, 비우면 암호 없음
  let exportPageSpec = $state('');
  let exportMerge = $state(false); // 여러 페이지를 파일 하나로 합칠지 — png/jpg에서만 의미 있음(pdf는 원래도 한 파일)
  let exportError = $state('');
  let exportBusy = $state(false);
  let showExportManyConfirm = $state(false);
  let exportManyCount = $state(0);
  let exportManyMerge = $state(false); // 합치기 중 뜬 확인창인지 — 안내 문구를 파일 개수 대신 처리량 경고로 바꾸는 데 쓴다
  let exportManyResolve = null; // 열려 있는 확인창의 resolver — 네이티브 ask() 대신 커스텀 모달을 써서 취소 버튼에 기본 포커스를 줄 수 있게 한다
  let fileInfo = $state(null);
  let fileFonts = $state([]);
  let fontsLoaded = $state(false); // fileFonts.length === 0 만으로는 "로딩 중"과 "폰트 없음"을 구분 못 해서 별도로 둠
  let fontStats = $state([]);
  let fontView = $state('list');    // 'list' | 'stats'
  let analyzingFonts = $state(false);
  let fileInfoTab = $state('general');   // 'general' | 'fonts'
  let shortcutsTab = $state('nav');      // 'nav' | 'zoom' | 'file' | 'search' | 'tab'
  let commandsTab = $state('file');      // 'file' | 'tab' | 'search'
  let thumbnails = $state([]); // { src: string, loaded: bool }[]

  // ── Table of contents ────────────────────────────────────────
  let toc = $state([]);              // { title, page, children }[] (nested)
  let sidebarView = $state('thumbs'); // 'thumbs' | 'toc' | 'search' | 'notes' | 'layers'

  // ── Optional content (OCG, Acrobat의 "레이어" 패널과 같은 개념) ──────────
  // 대부분의 PDF엔 없어서 빈 배열이면 사이드바 탭 자체를 숨긴다(disabled로만 두지 않음).
  // ArcGIS류 지도 PDF는 "Map Frame > Map > Light Gray Base"처럼 레이어를 포토샵 그룹마냥
  // 중첩시키므로(안쪽이 보이려면 바깥도 같이 켜져 있어야 함) 트리 그대로 받는다.
  let layers = $state([]); // { xref, name, enabled, children }[] (nested)
  let collapsedLayers = $state(new SvelteSet()); // 접힌 레이어 그룹 id — toc의 collapsed와 별개(id 스킴이 같아 섞이면 안 됨)

  // ── PDF 첨부파일(카탈로그 /Names/EmbeddedFiles) ──────────────────────────
  // 목록(파일명/크기)만 보여준다 — 다운로드/저장은 없음. 대부분의 PDF엔 없어서 layers와
  // 같이 탭 자체를 숨긴다.
  let attachments = $state([]); // string[] — 파일명만, 크기/다운로드 없음
  let collapsed = $state(new SvelteSet()); // collapsed toc node ids
  let selectedTocId = $state(null); // id of the toc entry the user explicitly clicked
  let sidebarWidth = $state(150);
  let sidebarVisible = $state(true);
  let resizingSidebar = false;
  // View 메뉴의 사이드바 항목 라벨을 "보이기"/"숨기기"로 동기화
  $effect(() => { invoke('set_sidebar_visible_menu', { visible: sidebarVisible }); });

  let toolbarVisible = $state(true);
  // View 메뉴의 툴바 항목 라벨을 "보이기"/"숨기기"로 동기화
  $effect(() => { invoke('set_toolbar_visible_menu', { visible: toolbarVisible }); });
  // 툴바는 position:fixed라 숨겨도 레이아웃이 저절로 안 줄어든다 — .app의
  // margin-top/height를 조정해야 해서 focus-mode와 같은 body 클래스 패턴을 쓴다.
  $effect(() => { document.body.classList.toggle('toolbar-hidden', !toolbarVisible); });

  // ── Search state ───────────────────────────────────────────
  let allSearchResults = $state([]); // { page, hit_count, rects }[]
  let allNotes = $state([]); // { page, xref, rect, contents }[] — 사이드바 "메모" 탭
  let pageHighlights = $state([]);   // [x0, y0, x1, y1][] for current page
  let hlsearchOn = $state(true);     // :nohlsearch(:noh)로 끔 — 검색 결과/커서는 그대로 두고
                                      // 하이라이트 표시만 숨긴다. 새 검색이나 n/N에서 다시 켠다.
  let activeMatchRect = $derived.by(() => flatSearchMatches()[searchResultIdx]?.rect ?? null);

  // ── Link hints (easy-motion style) ──────────────────────────
  let pageLinks = $state([]); // { rect, page, uri, label }[] for current page
  let hintMode = $state(false);
  let hintBuf = '';

  // ── Vim mode ───────────────────────────────────────────────
  let mode = $state('normal'); // 'normal' | 'command' | 'search'
  let cmdInput = $state('');

  // execCommand이 인식하는 명령어 이름 전체 — Tab 자동완성 후보 목록.
  const COMMAND_NAMES = ['q', 'tabclose', 'tabc', 'tabnew', 'tabe', 'tabonly', 'tabo', 'tabmove', 'tabm', 'tabreopen', 'tabre', 'e', 'w', 'export', 'images', 'nohlsearch', 'noh', 'docinfo', 'print', 'p', 'font', 'set', 'settings', 'cmd', 'oss', 'google', 'g', 'md', 'view', 'attachments', 'att'];
  const PATH_ARG_COMMANDS = ['e', 'w', 'export']; // 인자로 경로를 받는 명령 — 여기만 경로 자동완성 대상
  const MD_SUBCOMMANDS = ['code', 'preview', 'split', 'theme']; // :md 서브명령 Tab 자동완성 후보
  /** @type {{kind: string, candidates: string[], idx: number, prefix?: string} | null} */
  let cmdCompletion = $state(null); // Tab 순환 상태 + 상태표시줄 위 후보 목록 표시용
  let cmdSuggestEls = [];

  // Tab/Shift-Tab으로 순환하다가 후보가 목록 표시 범위(cmd-suggest의 max-height) 밖으로
  // 나가면 안 보이는 채로 계속 순환하는 것처럼 보이므로 활성 항목을 항상 스크롤해 보여준다
  $effect(() => {
    if (mode === 'command' && cmdCompletion) cmdSuggestEls[cmdCompletion.idx]?.scrollIntoView({ block: 'nearest' });
  });

  // 목록에서 클릭으로 후보를 고른다 — 파일이면 그걸로 확정하고 목록을 닫지만, 디렉터리는
  // "선택 완료"가 아니라 그 안으로 들어가는 동작이라(파일이 아니라 디렉터리를 :e로 열 수는
  // 없다) 목록을 닫지 않고 그 안의 목록으로 다시 채운다 — →/Enter로 들어가는 것과 동일하게.
  /** @param {number} i */
  async function pickCmdCompletion(i) {
    if (!cmdCompletion) return;
    const picked = cmdCompletion.candidates[i];
    cmdInput = (cmdCompletion.prefix ?? '') + picked;
    if (cmdCompletion.kind === 'path' && picked.endsWith('/')) {
      await descendCmdDirCompletion();
      return;
    }
    cmdCompletion = null;
  }

  // ↑/↓로 자동완성 목록 안에서 선택만 옮긴다(목록을 새로 계산하지 않음) — Tab의 순환 로직과
  // 동일하게 idx를 이동하고 입력창 텍스트도 그 후보로 갱신한다.
  /** @param {number} dir */
  function moveCmdCompletionSelection(dir) {
    if (!cmdCompletion) return;
    cmdCompletion.idx = (cmdCompletion.idx + dir + cmdCompletion.candidates.length) % cmdCompletion.candidates.length;
    cmdInput = (cmdCompletion.prefix ?? '') + cmdCompletion.candidates[cmdCompletion.idx];
  }

  // vim처럼 Tab/Shift-Tab으로 자동완성 후보를 순환한다. 공백 앞이면 명령어 이름을,
  // 공백 뒤(인자 자리)면 경로를 받는 명령(:e/:w/:export)에 한해 파일시스템 경로를 완성한다.
  // cmdInput이 마지막으로 채워넣은 후보와 다르면(사용자가 그새 직접 타이핑) 새로 계산한다.
  async function cycleCmdCompletion(dir) {
    const spIdx = cmdInput.indexOf(' ');
    if (spIdx === -1) {
      if (!cmdCompletion || cmdCompletion.kind !== 'name' || cmdInput !== cmdCompletion.candidates[cmdCompletion.idx]) {
        const candidates = COMMAND_NAMES.filter(c => c.startsWith(cmdInput));
        if (candidates.length === 0) { cmdCompletion = null; return; }
        cmdCompletion = { kind: 'name', candidates, idx: 0 };
      } else {
        cmdCompletion.idx = (cmdCompletion.idx + dir + cmdCompletion.candidates.length) % cmdCompletion.candidates.length;
      }
      cmdInput = cmdCompletion.candidates[cmdCompletion.idx];
      return;
    }

    // ":md <서브명령>" — code/preview/split/theme 자동완성 (아직 서브명령 뒤에 공백을 안 쳤을 때만.
    // 공백을 쳤다면 그 서브명령은 이미 확정된 것이므로 아래 인자 완성 분기로 넘어간다)
    const mdSubMatch = /^md\s+(\S*)$/.exec(cmdInput);
    if (mdSubMatch) {
      const prefix = cmdInput.slice(0, cmdInput.length - mdSubMatch[1].length);
      const partial = mdSubMatch[1];
      if (!cmdCompletion || cmdCompletion.kind !== 'md-sub' || cmdInput !== prefix + cmdCompletion.candidates[cmdCompletion.idx]) {
        const candidates = MD_SUBCOMMANDS.filter(name => name.startsWith(partial));
        if (candidates.length === 0) { cmdCompletion = null; return; }
        cmdCompletion = { kind: 'md-sub', candidates, idx: 0, prefix };
      } else {
        cmdCompletion.idx = (cmdCompletion.idx + dir + cmdCompletion.candidates.length) % cmdCompletion.candidates.length;
      }
      cmdInput = cmdCompletion.prefix + cmdCompletion.candidates[cmdCompletion.idx];
      return;
    }

    // ":md theme <이름>" — 테마 이름 자동완성 (이름 자리를 아직 하나도 안 썼어도 Tab만 누르면
    // 전체 목록이 후보가 되어 순환된다 — 이름을 몰라도 Tab만 계속 눌러서 둘러볼 수 있게)
    const mdThemeMatch = /^md\s+theme\s+(.*)$/.exec(cmdInput);
    if (mdThemeMatch) {
      const prefix = cmdInput.slice(0, cmdInput.length - mdThemeMatch[1].length);
      const partial = mdThemeMatch[1];
      if (!cmdCompletion || cmdCompletion.kind !== 'md-theme' || cmdInput !== prefix + cmdCompletion.candidates[cmdCompletion.idx]) {
        const candidates = MD_THEMES.filter(name => name.startsWith(partial));
        if (candidates.length === 0) { cmdCompletion = null; return; }
        cmdCompletion = { kind: 'md-theme', candidates, idx: 0, prefix };
      } else {
        cmdCompletion.idx = (cmdCompletion.idx + dir + cmdCompletion.candidates.length) % cmdCompletion.candidates.length;
      }
      cmdInput = cmdCompletion.prefix + cmdCompletion.candidates[cmdCompletion.idx];
      return;
    }

    if (!PATH_ARG_COMMANDS.includes(cmdInput.slice(0, spIdx))) return;
    // 형제 후보가 여러 개면(예: Apple/, Boo/, Charlie/) 지금 고른 게 디렉터리라도 아직 그
    // 형제들을 순환하는 중인 것 — 여기서 더 들어가면 안 된다. 반대로 후보가 정확히 하나뿐이고
    // 그게 디렉터리였다면(예: "Downloads/"만 유일하게 매칭) 더 순환할 형제가 없으므로 다음
    // Tab은 텍스트가 그대로여도 그 디렉터리 안으로 들어가 다시 조회한다.
    const descendIntoDir = cmdCompletion && cmdCompletion.kind === 'path' &&
      cmdCompletion.candidates.length === 1 &&
      cmdCompletion.candidates[0].endsWith('/') &&
      cmdInput === cmdCompletion.prefix + cmdCompletion.candidates[0];
    if (!cmdCompletion || cmdCompletion.kind !== 'path' || descendIntoDir || cmdInput !== cmdCompletion.prefix + cmdCompletion.candidates[cmdCompletion.idx]) {
      const result = await fetchPathCandidates(cmdInput);
      if (!result) { cmdCompletion = null; return; }
      cmdCompletion = { kind: 'path', candidates: result.candidates, idx: 0, prefix: result.prefix };
    } else {
      cmdCompletion.idx = (cmdCompletion.idx + dir + cmdCompletion.candidates.length) % cmdCompletion.candidates.length;
    }
    cmdInput = cmdCompletion.prefix + cmdCompletion.candidates[cmdCompletion.idx];
  }

  // ":e /dir/" 부분까지 완성된 텍스트에서 명령/경로를 나눠 그 디렉터리 안의 항목을 조회한다.
  // cycleCmdCompletion(Tab 순환)과 descendCmdDirCompletion(→/Enter로 명시적 진입) 둘 다 쓴다.
  async function fetchPathCandidates(text) {
    const lastSpace = text.lastIndexOf(' ');
    const prefix = text.slice(0, lastSpace + 1);
    const segment = text.slice(lastSpace + 1);
    const slashIdx = segment.lastIndexOf('/');
    const dirPart = slashIdx === -1 ? '' : segment.slice(0, slashIdx + 1);
    const namePrefix = slashIdx === -1 ? segment : segment.slice(slashIdx + 1);
    let entries;
    try {
      entries = await invoke('list_dir_entries', { dir: dirPart, docPath: filePath || null });
    } catch (_) {
      return null;
    }
    // zsh처럼 대소문자 그대로 매칭되는 후보가 없으면 대소문자 무시하고 다시 찾는다
    let candidates = entries.filter(name => name.startsWith(namePrefix));
    if (candidates.length === 0 && namePrefix) {
      const lower = namePrefix.toLowerCase();
      candidates = entries.filter(name => name.toLowerCase().startsWith(lower));
    }
    if (!namePrefix.startsWith('.')) candidates = candidates.filter(name => !name.startsWith('.'));
    if (candidates.length === 0) return null;
    return { prefix, candidates: candidates.map(name => dirPart + name) };
  }

  // 지금 커맨드라인이 정확히 "디렉터리 후보 하나"로 완성된 상태인지 — →/Enter로 그 안에
  // 들어갈 수 있는지 판단하는 데 쓴다 (Tab 순환 중인 형제 후보와는 다른 조건).
  function cmdDirCandidateActive() {
    return !!(cmdCompletion && cmdCompletion.kind === 'path' &&
      cmdCompletion.candidates[cmdCompletion.idx]?.endsWith('/') &&
      cmdInput === cmdCompletion.prefix + cmdCompletion.candidates[cmdCompletion.idx]);
  }

  // →나 Enter로 지금 선택된 디렉터리 후보 안으로 명시적으로 들어간다 — Tab 순환과 달리
  // 형제가 몇 개든 상관없이 "이 디렉터리로 확정"이라는 분명한 사용자 의도이므로 바로 조회한다.
  async function descendCmdDirCompletion() {
    const result = await fetchPathCandidates(cmdInput);
    if (!result) { cmdCompletion = null; return; }
    cmdCompletion = { kind: 'path', candidates: result.candidates, idx: 0, prefix: result.prefix };
    cmdInput = cmdCompletion.prefix + cmdCompletion.candidates[0];
  }

  // : 명령 / 검색어 기록 — vim처럼 localStorage에 저장해서 앱을 재시작해도 남고,
  // ↑/↓로 훑어본다. 두 입력창(명령/검색)이 완전히 같은 동작이라 하나로 묶어서 공유한다.
  function createHistory(key, max) {
    let list;
    try { list = JSON.parse(localStorage.getItem(key)) ?? []; } catch (_) { list = []; }
    let idx = -1;   // -1: 기록을 훑어보는 중이 아니라 직접 입력 중
    let draft = ''; // ↑로 훑어보기 시작하기 직전 입력 — ↓로 끝까지 내려오면 복원
    return {
      push(entry) {
        if (!entry.trim() || list[list.length - 1] === entry) return;
        list.push(entry);
        if (list.length > max) list.shift();
        localStorage.setItem(key, JSON.stringify(list));
        idx = -1;
      },
      up(current) {
        if (list.length === 0) return current;
        if (idx === -1) { draft = current; idx = list.length - 1; }
        else if (idx > 0) idx--;
        return list[idx];
      },
      down(current) {
        if (idx === -1) return current;
        if (idx < list.length - 1) { idx++; return list[idx]; }
        idx = -1;
        return draft;
      },
    };
  }
  const cmdHistory = createHistory('vimong-cmd-history', 50);
  const searchHistory = createHistory('vimong-search-history', 50);
  let searchQuery = $state('');
  let caseSensitive = $state(false); // 검색 대소문자 구분 (\c, \C 로 검색어별 오버라이드 가능)
  let regexMode = $state(false); // 검색어를 정규식으로 해석
  let searchError = $state(''); // 정규식 컴파일 실패 등 검색 자체가 실패했을 때의 오류 메시지

  let numBuf = '';          // digit prefix for nG
  let pendingPrefix = '';   // 'g' or 'z' waiting for next key
  let pendingN = null;      // numeric prefix saved when setting pendingPrefix
  let zHeld = false;        // true while z key is physically held down — z를 쥔 채로 i/o를
                             // 반복해서 눌러도(PDF 뷰어 관례) 매번 z를 다시 안 눌러도 되게 한다
  let marks = {};           // m{a-z}로 저장한 위치 — { [letter]: { page, y } }, 탭별로 저장/복원
  let lastSearch = '';
  let searchResultIdx = $state(0);  // current index in flatSearchMatches()
  let thumbVersion = 0;     // cancel old thumbnail loads on new file (global across tabs)

  // ── 멀티탭 ────────────────────────────────────────────────
  function makeEmptyTabState() {
    return {
      filePath: '', pageCount: 0, currentPage: 0, scale: 1.0, pageRotations: {},
      thumbnails: [], allSearchResults: [], pageHighlights: [],
      lastSearch: '', searchResultIdx: 0,
      statusMsg: 'No file open.  :e to open',
      toc: [], layers: [], attachments: [], sidebarView: 'thumbs',
      hwpMode: false, hwpPages: [], hwpDoc: null, hwpFileBytes: 0, currentFileMtime: 0,
      mdMode: false, mdSource: '', mdSearchResults: [], mdScale: 1.0,
      epubReflowable: false, epubFontSize,
      marks: {},
    };
  }

  let tabStates = [makeEmptyTabState()]; // 비반응성 — 탭 전환 시 저장/복원용
  let tabLabels = $state(['New Tab']);   // 탭바 표시용
  let currentTabIdx = $state(0);

  function saveTab() {
    tabStates[currentTabIdx] = {
      filePath, pageCount, currentPage, scale, pageRotations,
      thumbnails, allSearchResults, pageHighlights,
      lastSearch, searchResultIdx,
      statusMsg, toc, layers, attachments, sidebarView,
      hwpMode, hwpPages, hwpDoc, hwpFileBytes, currentFileMtime,
      mdMode, mdSource, mdSearchResults, mdScale,
      epubReflowable, epubFontSize,
      marks,
    };
  }

  function loadTab(idx) {
    const s = tabStates[idx];
    filePath = s.filePath; pageCount = s.pageCount;
    invoke('set_document_menu_enabled', { enabled: !!s.filePath });
    currentPage = s.currentPage; scale = s.scale; pageRotations = s.pageRotations ?? {};
    thumbnails = s.thumbnails; allSearchResults = s.allSearchResults;
    pageHighlights = s.pageHighlights;
    lastSearch = s.lastSearch; searchResultIdx = s.searchResultIdx;
    searchQuery = s.lastSearch; // 탭별 검색어 — 안 하면 검색창이 이전 탭의 입력 텍스트를 그대로 보여줌
    statusMsg = s.statusMsg;
    toc = s.toc; layers = s.layers ?? []; attachments = s.attachments ?? []; collapsedLayers = new SvelteSet(); sidebarView = s.sidebarView;
    hwpMode = s.hwpMode ?? false; hwpPages = s.hwpPages ?? []; hwpDoc = s.hwpDoc ?? null;
    hwpFileBytes = s.hwpFileBytes ?? 0;
    mdMode = s.mdMode ?? false; mdSource = s.mdSource ?? ''; mdSearchResults = s.mdSearchResults ?? [];
    mdScale = s.mdScale ?? 1.0;
    epubReflowable = s.epubReflowable ?? false;
    epubFontSize = s.epubFontSize ?? DEFAULT_EPUB_EM;
    currentFileMtime = s.currentFileMtime ?? 0;
    marks = s.marks ?? {};
    collapsed = collectParentIds(s.toc, '', new SvelteSet());
    selectedTocId = null;
    // PNG 캐시(Blob URL)를 안 쓰기로 하면서 탭별 렌더 결과도 안 들고 있는다 —
    // 래스터화가 이제 가벼워서(PNG 인코딩 없음) 전환할 때마다 다시 그려도 충분히 빠르다
  }

  async function switchToTab(idx) {
    idx = Math.max(0, Math.min(idx, tabLabels.length - 1));
    if (idx === currentTabIdx) return;
    hintMode = false; pageLinks = [];
    ++thumbVersion; // 현재 탭 썸네일 로드 중단
    resetMainViewState();
    saveTab();
    await invoke('switch_tab', { idx });
    currentTabIdx = idx;
    loadTab(idx);
    await renderCurrentView();
    if (showFileInfo) await loadDocInfo();
  }

  // vim의 실제 :tabmove 문법. 인자 N은 "N번 탭 뒤로 옮긴다"는 뜻이라(N=0은 맨 앞이라는
  // 특례) 0-indexed 배열 인덱스와 그대로 맞아떨어진다 — clamp만 해주면 된다.
  // +N/-N은 현재 위치 기준 상대 이동, 숫자 없이 +나 -만 쓰면 1칸으로 취급한다.
  async function execTabMove(arg) {
    const count = tabLabels.length;
    let target;
    if (/^[+-]\d*$/.test(arg)) {
      const n = arg.length > 1 ? parseInt(arg.slice(1), 10) : 1;
      target = arg[0] === '+' ? currentTabIdx + n : currentTabIdx - n;
    } else if (/^\d+$/.test(arg)) {
      target = parseInt(arg, 10);
    } else {
      statusMsg = t('status.tabmove_bad_arg', { arg });
      return;
    }
    await reorderTab(currentTabIdx, Math.max(0, Math.min(target, count - 1)));
  }

  async function reorderTab(from, to) {
    if (from === to) return;
    const newCurrent = await invoke('reorder_tab', { from, to });
    const [label] = tabLabels.splice(from, 1);
    tabLabels.splice(to, 0, label);
    const [state] = tabStates.splice(from, 1);
    tabStates.splice(to, 0, state);
    currentTabIdx = newCurrent;
  }

  // ── 탭바 드래그 재정렬(트랙패드/마우스 공용) ──────────────────
  // 라이브러리 없이 pointer 이벤트로 직접 구현. 드래그 중엔 잡은 탭만 포인터를 따라
  // 옆으로 움직이고(다른 탭은 그대로 둬서 로직을 단순하게 유지), 놓는 순간 포인터가
  // 어느 탭의 자리 위에 있는지로 최종 위치를 정해서 한 번에 재정렬한다.
  let draggingTabIdx = $state(null);
  let dragTabDeltaX = $state(0);
  let dragTabStartX = 0;
  let dragTabMoved = false;

  function startTabDrag(e, i) {
    if (e.button !== 0 || tabLabels.length < 2) return;
    draggingTabIdx = i;
    dragTabStartX = e.clientX;
    dragTabDeltaX = 0;
    dragTabMoved = false;
  }

  function onTabDragMove(e) {
    if (draggingTabIdx === null) return;
    dragTabDeltaX = e.clientX - dragTabStartX;
    if (Math.abs(dragTabDeltaX) > 4) dragTabMoved = true;
  }

  async function endTabDrag(e) {
    if (draggingTabIdx === null) return;
    const from = draggingTabIdx;
    const moved = dragTabMoved;
    draggingTabIdx = null;
    dragTabDeltaX = 0;
    dragTabMoved = false;
    if (!moved) return;
    const tabs = [...document.querySelectorAll('.tabbar-tab')];
    let to = from;
    for (let j = 0; j < tabs.length; j++) {
      if (j === from) continue;
      const r = tabs[j].getBoundingClientRect();
      if (e.clientX > r.left && e.clientX < r.right) { to = j; break; }
    }
    await reorderTab(from, to);
  }

  // path를 이미 연 탭이 있으면 그 인덱스를, 없으면 -1을 반환한다. 활성 탭의 filePath는
  // saveTab() 전까진 tabStates에 반영이 안 돼 있어 currentTabIdx만 따로 봐야 한다.
  function findOpenTab(path) {
    for (let i = 0; i < tabLabels.length; i++) {
      if ((i === currentTabIdx ? filePath : tabStates[i].filePath) === path) return i;
    }
    return -1;
  }

  async function newTab(path = null) {
    if (path) {
      const existing = findOpenTab(path);
      if (existing !== -1) { await switchToTab(existing); return; }
    }
    // Cmd+T는 네이티브 메뉴 단축키라 검색/명령 입력 중에 눌러도 그대로 실행된다 — mode를
    // 안 되돌리면 새 탭에서 mode가 'search'/'command'로 남아 handleKeydown이 맨 앞에서 바로
    // return 해버려서 키보드가 완전히 안 먹는다(마우스는 이 검사를 안 거치니 멀쩡해 보임).
    mode = 'normal';
    hintMode = false; pageLinks = [];
    ++thumbVersion;
    resetMainViewState();
    saveTab();
    const idx = await invoke('new_tab');
    tabStates.push(makeEmptyTabState());
    tabLabels.push('New Tab');
    currentTabIdx = idx;
    // 상태 초기화
    filePath = ''; invoke('set_document_menu_enabled', { enabled: false });
    pageCount = 0; currentPage = 0; scale = 1.0;
    thumbnails = []; allSearchResults = []; mdSearchResults = []; pageHighlights = []; allNotes = [];
    lastSearch = ''; searchResultIdx = 0;
    statusMsg = 'No file open.  :e to open';
    toc = []; sidebarView = 'thumbs'; collapsed = new SvelteSet(); selectedTocId = null;
    hwpMode = false; hwpPages = []; hwpDoc = null;
    mdMode = false; mdSource = ''; mdScale = 1.0;
    epubReflowable = false;
    marks = {};
    if (path) {
      await openFile(path);
      // openFile은 실패해도 내부에서 에러를 삼키므로(다이얼로그만 띄움) filePath로 성공 여부 판단
      if (!filePath) await closeTab(currentTabIdx);
    }
  }

  // 닫은 탭 다시 열기(Cmd+Shift+T) — 최근 닫은 파일 경로 스택. 세션 한정, 저장 안 함
  const CLOSED_TABS_MAX = 20;
  let closedTabs = [];
  function rememberClosedTab(path) {
    if (!path) return;
    closedTabs.push(path);
    if (closedTabs.length > CLOSED_TABS_MAX) closedTabs.shift();
  }
  async function reopenClosedTab() {
    const path = closedTabs.pop();
    if (!path) { statusMsg = t('status.no_closed_tabs'); return; }
    await newTab(path);
  }

  // 탭을 실제로 닫기 전에, 하이라이트 등 :w로 아직 저장 안 한 변경사항이 있으면 확인창을 띄운다.
  async function closeTab(idx) {
    if (await invoke('is_tab_dirty', { idx })) {
      pendingCloseTabIdx = idx;
      pendingCloseTabName = baseName(idx === currentTabIdx ? filePath : tabStates[idx]?.filePath ?? '');
      showDiscardConfirm = true;
      return;
    }
    await closeTabConfirmed(idx);
  }

  async function closeTabConfirmed(idx) {
    // newTab()과 같은 이유(Cmd+W도 네이티브 메뉴 단축키) — mode 스택 문제
    mode = 'normal';
    hintMode = false; pageLinks = [];
    ++thumbVersion;
    resetMainViewState();
    if (tabLabels.length === 1) {
      // 마지막 탭: 문서만 닫기
      rememberClosedTab(filePath);
      await invoke('close_tab', { idx: 0 });
      closeDocument();
      tabLabels[0] = 'New Tab';
      tabStates[0] = makeEmptyTabState();
      return;
    }
    rememberClosedTab(idx === currentTabIdx ? filePath : tabStates[idx].filePath);
    const newIdx = await invoke('close_tab', { idx });
    tabStates.splice(idx, 1);
    tabLabels.splice(idx, 1);
    currentTabIdx = newIdx;
    loadTab(newIdx);
    await renderCurrentView();
  }

  // 문서가 하나라도 열렸으면 아직 아무것도 안 연 다른 빈 탭은 정리한다.
  // closeTab()과 달리 loadTab()을 호출하지 않는다 — 지금 보고 있는(방금 문서를 연) 탭의
  // 상태를 건드리면 안 되기 때문
  async function closeOtherEmptyTabs() {
    for (let i = tabLabels.length - 1; i >= 0; i--) {
      if (i === currentTabIdx || tabLabels[i] !== 'New Tab') continue;
      await invoke('close_tab', { idx: i });
      tabStates.splice(i, 1);
      tabLabels.splice(i, 1);
      if (i < currentTabIdx) currentTabIdx--;
    }
  }

  // vim의 :tabonly — 현재 탭만 남기고 나머지 전부 닫는다.
  async function closeOtherTabs() {
    for (let i = tabLabels.length - 1; i >= 0; i--) {
      if (i === currentTabIdx) continue;
      rememberClosedTab(tabStates[i].filePath);
      await invoke('close_tab', { idx: i });
      tabStates.splice(i, 1);
      tabLabels.splice(i, 1);
      if (i < currentTabIdx) currentTabIdx--;
    }
  }

  // :tabonly 진입점 — 설정(confirmCloseTabs)이 켜져 있으면 닫기 전에 확인창을 띄운다.
  async function requestCloseOtherTabs() {
    if (tabLabels.length <= 1) return;
    if (!confirmCloseTabs) { await closeOtherTabs(); return; }
    tabOnlyCount = tabLabels.length - 1;
    showTabOnlyConfirm = true;
  }

  // ── DOM refs ───────────────────────────────────────────────
  let mainEl;
  let sidebarEl;
  let thumbEls = [];
  let tocItemEls = [];
  let searchResultEls = [];
  let notesResultEls = [];
  let layersResultEls = [];
  let attachmentsResultEls = [];
  let searchInputEl;
  let fileInfoScrollEl;
  let shortcutsScrollEl;
  let commandsScrollEl;
  let pagespecSectionEl;
  let wantSearchFocus = false; // focusSearchTab()에서만 true로 설정 — [ / ] 탭 순환으로 마운트될 땐 포커스하지 않음

  function focusOnMount(node) { node.focus(); }

  function focusOnSearchMount(node) {
    if (wantSearchFocus) { node.focus(); wantSearchFocus = false; }
  }

  // ── 모달 탭 내비게이션 (gt / gT / ngt) ───────────────────
  // 여러 모달에서 공유하는 키 상태
  let modalG = false;
  let modalN = '';

  function resetModalNav() { modalG = false; modalN = ''; }

  // 문서 정보 모달 내용을 (다시) 불러온다 — ⌘I로 처음 열 때, 그리고 모달이 열려 있는
  // 채로 탭을 전환했을 때 갱신하는 용도로 공유
  async function loadDocInfo() {
    fileInfoTab = 'general';
    fontView = 'list';
    fontStats = [];
    fileInfo = null;
    fileFonts = [];
    fontsLoaded = false;
    resetModalNav();
    // HWP는 Rust tab.doc가 없어(전체가 프론트엔드 WASM으로 열림) get_file_info가
    // "No document open"으로 실패한다 — rhwp의 getDocumentInfo()로 직접 채운다.
    if (hwpMode && hwpDoc) {
      try {
        const info = JSON.parse(hwpDoc.getDocumentInfo());
        fileInfo = {
          file_name: baseName(filePath), path: filePath,
          page_count: info.pageCount, file_size_kb: hwpFileBytes / 1024,
          is_pdf: false, is_hwp: true,
          hwp_version: info.version, encrypted: info.encrypted,
          section_count: info.sectionCount,
        };
        // rhwp는 PDF처럼 글자 수 기준 비율/굵기·기울임 정보는 안 줘서 이름 목록만 채운다
        fileFonts = (info.fontsUsed ?? []).map(name => ({ name }));
      } catch (e) {
        statusMsg = `Error: ${e}`;
      } finally {
        fontsLoaded = true;
      }
      return;
    }
    // 마크다운도 Rust tab.doc가 없어 get_file_info가 실패한다 — 원문 바이트 길이로 직접 채운다.
    if (mdMode) {
      fileInfo = {
        file_name: baseName(filePath), path: filePath,
        page_count: 1, file_size_kb: new TextEncoder().encode(mdSource).length / 1024,
        is_pdf: false, is_hwp: false,
      };
      fontsLoaded = true;
      return;
    }
    try {
      const f = await invoke('get_file_info');
      fileInfo = f;
      if (f.is_pdf) fileFonts = await invoke('get_fonts');
    } catch (e) {
      statusMsg = `Error: ${e}`;
    } finally {
      fontsLoaded = true;
    }
  }

  /**
   * gt / gT / ngt 키를 처리. 소비한 경우 true 반환.
   * tabs: 탭 id 배열, current: 현재 탭 id, setTab: 탭 변경 콜백
   */
  // ]/[(또는 gt/gT)로 탭을 바꿔도 setTab은 상태만 바꿀 뿐 DOM 포커스는 그대로라, 실제
  // 포커스가 눈에 보이는 탭과 따로 놀았다 — 방금 활성화된 탭 버튼으로 포커스를 직접 옮긴다.
  function focusModalTab(modalSelector, tabs, t) {
    const buttons = document.querySelectorAll(`${modalSelector} .modal-tabs button`);
    buttons[tabs.indexOf(t)]?.focus();
  }

  function handleTabNav(key, tabs, current, setTab) {
    const i = tabs.indexOf(current);
    if (key === ']') { setTab(tabs[(i + 1) % tabs.length]); return true; }
    if (key === '[') { setTab(tabs[(i - 1 + tabs.length) % tabs.length]); return true; }
    if (key >= '1' && key <= '9') { modalN += key; return true; }
    if (key === '0' && modalN)    { modalN += key; return true; }
    if (modalG) {
      modalG = false;
      const n   = modalN ? parseInt(modalN, 10) - 1 : null;
      modalN = '';
      const i   = tabs.indexOf(current);
      if (key === 't') {
        setTab(n !== null
          ? tabs[Math.max(0, Math.min(n, tabs.length - 1))]
          : tabs[(i + 1) % tabs.length]);
        return true;
      }
      if (key === 'T') {
        setTab(tabs[(i - 1 + tabs.length) % tabs.length]);
        return true;
      }
      return false;
    }
    if (key === 'g') { modalG = true; return true; }
    modalN = '';
    return false;
  }

  let FILE_INFO_TABS = $derived((fileInfo?.is_pdf || fileInfo?.is_hwp) ? ['general', 'fonts'] : ['general']);
  const SHORTCUTS_TABS    = ['nav', 'zoom', 'file', 'search', 'markdown', 'tab'];
  const COMMANDS_TABS     = ['file', 'tab', 'search'];

  // hwp 페이지는 렌더된 SVG 자체 <svg width=".." height=".."> (renderPageSvg가 내놓는 1배율
  // 좌표계, injectHwpHighlights의 rect와 같은 단위)에서 원본 크기를 읽는다 — rhwp에 별도
  // 페이지 크기 API가 없다.
  function hwpPageSize(i) {
    const m = hwpPages[i]?.match(/<svg[^>]*\bwidth="([\d.]+)"[^>]*\bheight="([\d.]+)"/);
    return m ? [parseFloat(m[1]), parseFloat(m[2])] : [0, 0];
  }

  // Ctrl-D/Ctrl-U 한 번에 옮길 거리 — 뷰포트 절반이 아니라 "현재 페이지 렌더 높이"의 절반으로
  // 잡아야, 확대해서 페이지가 뷰포트보다 훨씬 커진 경우에도 페이지 하나를 두 번이면 다 훑는다
  // (뷰포트 절반 기준이면 배율이 커질수록 필요한 입력 횟수가 그만큼 늘어나 버린다).
  // 페이지 크기를 아직 모르면(로딩 중 등) 뷰포트 절반으로 대체.
  function halfPageStep() {
    const ph = (hwpMode ? hwpPageSize(currentPage) : pageDims(currentPage))[1];
    return (ph ? ph * scale : mainEl.clientHeight) / 2;
  }

  // rhwp는 회전된 SVG를 못 내놓으니 CSS로 돌린다 — transform-origin:top left에서
  // rotate 후 자기 자신의 폭/높이(%)만큼 되돌려 밀면, 원래 있던 자리(좌상단)를 기준으로
  // 회전된 박스가 다시 채워진다("rotate and reflow" 관용구).
  function hwpRotateTransform(rot) {
    if (rot === 90) return 'rotate(90deg) translateY(-100%)';
    if (rot === 180) return 'rotate(180deg) translate(-100%,-100%)';
    if (rot === 270) return 'rotate(270deg) translateX(-100%)';
    return 'none';
  }

  async function fitToWidth() {
    if (!filePath || mdMode) return; // 마크다운은 페이지 크기 개념이 없음
    if (hwpMode) {
      const [pageW] = hwpPageSize(currentPage);
      if (!pageW) return;
      const available = (mainEl?.clientWidth ?? window.innerWidth) - 40;
      scale = Math.round((available / pageW) * 100) / 100;
      return;
    }
    const [pageW] = await invoke('get_page_size', { pageNum: currentPage });
    // 'two'/'two-continuous'는 한 행에 페이지 2장 + 사이 여백(.viewer-two/.page-row의
    // gap:4px)이 나란히 들어가니 그 폭 기준으로 맞춰야 페이지 한 장 기준으로 계산했을 때
    // 두 장이 합쳐 뷰포트를 넘치는 문제가 없다.
    const cols = isTwoUp(viewMode) ? 2 : 1;
    const gap = isTwoUp(viewMode) ? 4 : 0;
    const available = (mainEl?.clientWidth ?? window.innerWidth) - 40 - gap;
    scale = Math.round((available / (pageW * cols)) * 100) / 100;
    await rerenderNow();
  }

  // 세로 연속 모드(한 페이지 연속/두 페이지 연속)는 창이나 사이드바 크기가 바뀌면 폭을
  // 다시 맞춰야 페이지가 잘리거나 좁은 여백에 덩그러니 남지 않는다. 가로 연속은 스크롤
  // 축 자체가 가로라 폭 맞춤이 의미 없어서 제외.
  let resizeFitTimer;
  function scheduleResizeFit() {
    if (reclaimFocusOnResize) reclaimFocus();
    if (!filePath) return;
    // 프레젠테이션 중엔(예: F로 전체화면 전환하는 애니메이션 도중/직후) 창 크기가 바뀔 때마다
    // 다시 화면에 꽉 차게 맞춘다 — setFullscreen()이 resolve돼도 실제 리사이즈는 애니메이션
    // 뒤에 따라오는 경우가 있어, 페이지 전환 없이도 리사이즈만으로 재계산이 필요하다.
    if (presentationMode) { clearTimeout(resizeFitTimer); resizeFitTimer = setTimeout(fitPresentationPage, 150); return; }
    if (viewMode !== 'single-continuous' && viewMode !== 'two-continuous') return;
    clearTimeout(resizeFitTimer);
    resizeFitTimer = setTimeout(fitToWidth, 150);
  }

  async function fitToHeight() {
    if (!filePath || mdMode) return; // 마크다운은 페이지 크기 개념이 없음
    if (hwpMode) {
      const [, pageH] = hwpPageSize(currentPage);
      if (!pageH) return;
      const available = (mainEl?.clientHeight ?? window.innerHeight) - 40;
      scale = Math.round((available / pageH) * 100) / 100;
      return;
    }
    const [, pageH] = await invoke('get_page_size', { pageNum: currentPage });
    const available = (mainEl?.clientHeight ?? window.innerHeight) - 40;
    scale = Math.round((available / pageH) * 100) / 100;
    await rerenderNow();
  }

  // macOS에서 전체화면 전환 시 webview가 remount되는 경우가 있어(아래 openFile 복원 주석
  // 참고), onMount가 다시 돌 때마다 리스너를 계속 쌓기만 하면 전체화면을 반복할수록 메뉴/
  // 드롭/포커스 이벤트가 여러 번씩 겹쳐 발동해 갈수록 느려지고 결국 먹통이 된다. 컴포넌트가
  // 파괴될 때(remount로 인한 것 포함) 여기 모아둔 것들을 전부 정리한다.
  let mainResizeObserver;
  const unlistenFns = [];
  onDestroy(() => {
    mainResizeObserver?.disconnect();
    window.removeEventListener('afterprint', restorePrintState);
    for (const fn of unlistenFns) fn?.();
  });

  onMount(async () => {
    const savedWidth = parseInt(localStorage.getItem('vimong-sidebar-width'), 10);
    if (!isNaN(savedWidth)) sidebarWidth = Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, savedWidth));

    const savedViewMode = localStorage.getItem('vimong-view-mode');
    if (VIEW_MODES.includes(savedViewMode)) viewMode = savedViewMode;

    loadRecentFiles();
    if (!appVersion) appVersion = await getVersion();
    // 메뉴바 언어 파일이 저장된 langPref와 어긋나 있을 수 있으니(예: 이 기능이 막 추가된
    // 직후) 시작할 때 한 번 맞춰준다 — 이후로는 설정에서 Apply를 눌러야만 갱신된다.
    invoke('set_app_language', { lang: langPref });
    // 시스템 폰트 스캔(고정폭 필터링 때문에 폰트 하나하나를 실제로 로드해봐야 해서 느림 —
    // 폰트가 많은 실사용 환경에선 수 초 걸릴 수 있다)을 여기서 미리 시작해둔다. 설정 모달을
    // 열자마자 바로 눌러도(=openSettings에서도 한 번 더 부름, 중복 호출은 안전) 대개 이미
    // 끝나 있게 하려는 것 — 기다리지 않고 던져만 둔다.
    ensureSystemFontsLoaded();

    mainResizeObserver = new ResizeObserver(scheduleResizeFit);
    if (mainEl) mainResizeObserver.observe(mainEl);

    // 인쇄 다이얼로그가 닫히면(취소 포함) printDocument가 잠깐 바꿔둔 보기 모드/배율을 되돌린다.
    // printDocument도 window.print() 호출 직후 같은 복원을 한 번 더 시도하므로(afterprint를
    // 지원 안 하거나 print()를 조용히 무시하는 webview 대비), 이 리스너는 그보다 늦게(다이얼로그가
    // 실제로 뜬 뒤) 닫혔을 때를 위한 것 — restorePrintState는 두 번 불려도 안전하다.
    window.addEventListener('afterprint', restorePrintState);

    // 전체화면 진입/이탈(특히 macOS Space 전환)로 창이 OS 포커스를 다시 얻는 정확한 시점에
    // webview 쪽 DOM 포커스도 같이 잡는다 — 리사이즈 기반 재시도(reclaimFocusOnResize)보다
    // 더 정확한 신호라 이걸 우선한다. 명령/검색 입력 중엔 그쪽 포커스를 뺏지 않는다.
    unlistenFns.push(await getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (focused && mode !== 'command' && mode !== 'search') reclaimFocus();
    }));

    // Restore file after remount (window resize / fullscreen toggle on macOS)
    const saved = sessionStorage.getItem('vimong-file');
    if (saved) {
      await openFile(saved);
      // sessionStorage는 이 프로세스가 살아있는 동안만 남아있어서, saved가 있다는 건 완전히
      // 새로 켠 게 아니라 리마운트라는 뜻 — 이때는 탭 세션 복원을 하면 안 된다(중복으로 열림).
    } else if (restoreSession) {
      await restoreTabSession();
    }

    // SyncTeX 정방향 검색 — 이 프로세스 자체가 --forward-search로 실행됐다면(콜드 스타트) 여기서
    // 가져온다. 이미 떠 있는 인스턴스로 두 번째 실행이 들어온 경우는 아래 이벤트로 온다.
    const pendingForwardSearch = await invoke('take_pending_forward_search');
    if (pendingForwardSearch) await handleForwardSearch(pendingForwardSearch);
    unlistenFns.push(await listen('forward-search', (e) => handleForwardSearch(e.payload)));

    // Finder/브라우저에서 PDF를 더블클릭해 열었을 때 — forward-search와 같은 패턴. 이 시점에
    // 세션 복원(위)이 이미 탭을 채워놨을 수 있으니, 지금 탭이 비어 있을 때만(진짜 콜드 스타트로
    // 빈 탭 하나뿐일 때만) openFile로 바로 채우고, 이미 문서가 열려 있으면(세션 복원됐거나 이미
    // 떠 있는 상태로 또 열린 경우) 그 문서를 덮어쓰지 않게 새 탭에 연다(openFile이 내부에서
    // 남는 빈 탭은 정리해준다).
    const pendingFileOpen = await invoke('take_pending_file_open');
    if (pendingFileOpen) {
      if (filePath) await newTab(pendingFileOpen); else await openFile(pendingFileOpen);
    }
    unlistenFns.push(await listen('open-file', (e) => newTab(e.payload)));

    // 세션 복원/파일 연결 열기 중 자동으로 열릴 만한 걸 다 확인했다 — 이제 웰컴 화면을 띄워도 된다.
    bootLoading = false;

    unlistenFns.push(await listen('menu', async (e) => {
      switch (e.payload) {
        case 'new-tab':
          // close-tab과 같은 이유(Cmd+T도 네이티브 메뉴 단축키라 JS keydown에서 못 막는다) —
          // 도움말류 모달이 떠 있는 동안 탭이 생기면 모달은 그대로인데 뒤 문서만 바뀌어 버린다.
          if (blockingHelpModalOpen()) break;
          await newTab();
          break;
        case 'copy-selection':
          await copyActiveSelection();
          break;
        case 'tool-pointer':
          setToolMode('pointer');
          break;
        case 'tool-text-select':
          setToolMode('text');
          break;
        case 'tool-highlight':
          setToolMode('highlight');
          break;
        case 'doc-info':
          if (!filePath) return;
          showFileInfo = true;
          await loadDocInfo();
          break;
        case 'print':
          await printDocument();
          break;
        case 'password':
          if (!filePath) return;
          passwordManageTab = 'set';
          newPassword1 = ''; newPassword2 = ''; passwordManageError = '';
          showPasswordManage = true;
          break;
        case 'save-as':
          if (!filePath) return;
          await doSaveAs();
          break;
        case 'export':
          if (!filePath) return;
          exportFormat = 'png';
          exportFilename = defaultExportFilename();
          exportPageSpec = defaultExportSpec() ?? String(currentPage + 1);
          exportMerge = false; exportPassword = ''; exportError = '';
          showExportModal = true;
          break;
        case 'zoom-actual':
          scale = 1.0;
          triggerRerender();
          break;
        case 'zoom-fit':
          await fitToWidth();
          break;
        case 'zoom-fit-height':
          await fitToHeight();
          break;
        case 'zoom-in':
          scale = Math.min(+(scale + 0.25).toFixed(2), 12);
          triggerRerender();
          break;
        case 'zoom-out':
          scale = Math.max(+(scale - 0.25).toFixed(2), 0.25);
          triggerRerender();
          break;
        case 'shortcuts':
          shortcutsTab = 'nav';
          resetModalNav();
          showShortcuts = true;
          break;
        case 'commands':
          commandsTab = 'file';
          resetModalNav();
          showCommands = true;
          break;
        case 'settings':
          openSettings();
          break;
        case 'open-source':
          showOpenSource = true;
          break;
        case 'about':
          if (!appVersion) appVersion = await getVersion();
          showAbout = true;
          break;
        case 'reopen-closed-tab':
          await reopenClosedTab();
          break;
        case 'toggle-sidebar':
          sidebarVisible = !sidebarVisible;
          break;
        case 'toggle-toolbar':
          toolbarVisible = !toolbarVisible;
          break;
        case 'toggle-fullscreen':
          await toggleFullscreen();
          break;
        case 'view-single':
          await setViewMode('single');
          break;
        case 'view-single-continuous':
          await setViewMode('single-continuous');
          break;
        case 'view-two':
          await setViewMode('two');
          break;
        case 'view-two-continuous':
          await setViewMode('two-continuous');
          break;
        case 'view-horizontal-continuous':
          await setViewMode('horizontal-continuous');
          break;
        case 'next-tab':
          await switchToTab((currentTabIdx + 1) % tabLabels.length);
          break;
        case 'prev-tab':
          await switchToTab((currentTabIdx - 1 + tabLabels.length) % tabLabels.length);
          break;
        case 'close-tab':
          // Cmd+W는 네이티브 메뉴 단축키라 JS keydown에서 못 막는다 — 도움말류 모달을 보는
          // 도중 탭이 사라지면 모달만 붕 뜨는 이상한 상태가 되니 여기서 직접 막는다.
          if (blockingHelpModalOpen()) { statusMsg = t('status.close_tab_blocked_fileinfo'); break; }
          await closeTab(currentTabIdx);
          break;
      }
    }));

    unlistenFns.push(await listen('quit-requested', async (e) => {
      quitTabCount = e.payload;
      // 탭 개수와 무관하게, 저장 안 한 변경사항이 있으면 몇 개가 열려 있든 먼저 확인한다 —
      // confirmCloseTabs(탭 여러 개 닫을 때만 물어보는 설정)와는 별개의 데이터 손실 방지라
      // 그 설정이 꺼져 있어도 건너뛰면 안 된다.
      const dirty = [];
      for (let i = 0; i < tabLabels.length; i++) {
        if (await invoke('is_tab_dirty', { idx: i })) dirty.push(i);
      }
      if (dirty.length > 0) {
        quitDirtyQueue = dirty;
        await processNextQuitDirtyTab();
        return;
      }
      if (!confirmCloseTabs) { invoke('quit_app'); return; }
      showQuitConfirm = true;
    }));

    unlistenFns.push(await getCurrentWebview().onDragDropEvent(async (e) => {
      if (e.payload.type !== 'drop') return;
      const isSupported = p => ALL_EXTENSIONS.includes(p.split('.').pop().toLowerCase());
      const paths = e.payload.paths.filter(isSupported);
      const rejected = e.payload.paths.filter(p => !isSupported(p));
      if (rejected.length > 0) {
        const names = rejected.map(baseName).join(', ');
        await message(t('status.unsupported_format', { names }), { title: 'Vimong', kind: 'warning' });
      }
      if (e.payload.paths.length >= 3 && paths.length > 0) {
        const ok = await ask(t('status.confirm_open_tabs', { n: paths.length }), { title: 'Vimong', kind: 'warning' });
        if (!ok) return;
      }
      for (const p of paths) await newTab(p);
    }));
  });

  // 암호 걸린 PDF — 모달을 띄우고 제출/취소될 때까지 openFile()을 멈춰 세운다
  function askPassword() {
    passwordInput = '';
    passwordError = '';
    showPasswordPrompt = true;
    return new Promise(resolve => { passwordResolve = resolve; });
  }

  async function submitPassword() {
    if (!passwordInput) return;
    try {
      const count = await invoke('unlock_pdf', { password: passwordInput });
      showPasswordPrompt = false;
      passwordResolve({ ok: true, count });
    } catch (e) {
      passwordError = t('password.incorrect');
      passwordInput = '';
    }
  }

  function cancelPassword() {
    showPasswordPrompt = false;
    passwordResolve({ ok: false });
  }

  async function submitSetPassword() {
    if (!newPassword1) return;
    if (newPassword1 !== newPassword2) { passwordManageError = t('password.mismatch'); return; }
    passwordManageBusy = true;
    passwordManageError = '';
    try {
      await invoke('set_pdf_password', { password: newPassword1 });
      showPasswordManage = false;
      statusMsg = t('status.password_set');
    } catch (e) {
      passwordManageError = String(e);
    } finally {
      passwordManageBusy = false;
    }
  }

  async function submitRemovePassword() {
    passwordManageBusy = true;
    passwordManageError = '';
    try {
      await invoke('remove_pdf_password');
      showPasswordManage = false;
      statusMsg = t('status.password_removed');
    } catch (e) {
      passwordManageError = String(e);
    } finally {
      passwordManageBusy = false;
    }
  }

  // ── PDF operations ─────────────────────────────────────────
  async function openFile(path) {
    const ext = path.split('.').pop().toLowerCase();
    if (HWP_EXTENSIONS.includes(ext)) {
      await openHwpFile(path);
      return;
    }
    if (MD_EXTENSIONS.includes(ext)) {
      await openMarkdownFile(path);
      return;
    }
    hwpMode = false;
    hwpPages = [];
    hwpDoc = null;
    mdMode = false;
    mdSource = '';
    mdSearchResults = [];
    mdScale = 1.0;
    epubReflowable = false;
    try {
      ++thumbVersion;
      resetMainViewState();
      thumbScale = 0;
      thumbScaleReady = new Promise(r => { resolveThumbScaleReady = r; });
      // EPUB/HTML은 페이지 수를 세려면 mupdf가 문서 전체를 한 번 흘려봐야 해서, 스크립트가
      // 여러 개 섞인 문서는 수십 초 넘게 걸릴 수 있다 — 그동안 화면이 그냥 멈춘 것처럼
      // 보이지 않게 최소한의 진행 표시를 해둔다.
      statusMsg = t('status.opening_file');
      const result = await invoke('open_pdf', { path });
      currentFileMtime = await invoke('get_file_mtime', { path }).catch(() => 0);
      let count = result.page_count;
      if (result.needs_password) {
        const r = await askPassword();
        if (!r.ok) return; // 취소 — 이 탭은 그냥 비워둔다 (newTab이면 호출부에서 롤백)
        count = r.count;
      } else if (result.is_reflowable) {
        // EPUB/HTML — mupdf 내부 기본 레이아웃 대신 우리가 쓰던(또는 기본) 폰트 크기로
        // 바로 다시 페이지를 나눈다. 암호 걸린 문서는 실사용 사례가 없다시피 해서 위 분기와
        // 겹치는 경우는 그냥 재배치를 건너뛴다(고정 페이지처럼 열림).
        epubReflowable = true;
        count = await invoke('set_reflow_font_size', { em: epubFontSize });
      }
      pageCount = count;
      currentPage = 0;
      pageRotations = {};
      filePath = path;
      invoke('set_document_menu_enabled', { enabled: true });
      sessionStorage.setItem('vimong-file', path);
      addRecentFile(path);
      const label = baseName(path);
      statusMsg = label;
      tabLabels[currentTabIdx] = label;
      await closeOtherEmptyTabs();
      thumbnails = Array.from({ length: count }, () => ({ src: '', loaded: false }));
      allSearchResults = [];
      mdSearchResults = [];
      pageHighlights = [];
      lastSearch = ''; searchQuery = '';
      selectedTocId = null;
      marks = {};
      toc = await invoke('get_toc');
      collapsed = collectParentIds(toc, '', new SvelteSet()); // 챕터만 보이게 하위 항목은 기본 접힌 채로 시작
      layers = await invoke('get_optional_content_groups');
      collapsedLayers = new SvelteSet();
      attachments = await invoke('get_attachments');
      sidebarView = tocIsMeaningful ? 'toc' : 'thumbs';

      // fit to window height(zz와 동일) — 이 페이지 크기를 initThumbScale에도 그대로 넘겨서
      // 같은 get_page_size(0) IPC 왕복을 열 때마다 반복하지 않게 한다
      const pageSize0 = await invoke('get_page_size', { pageNum: 0 });
      const [, pageH] = pageSize0;
      const available = (mainEl?.clientHeight ?? window.innerHeight) - 40;
      scale = Math.round((available / pageH) * 100) / 100;

      if (viewMode === 'single') {
        pageSizes[0] = pageSize0;
        windowLo = 0;
        windowHi = 0;
        prefetchAdjacentSizes(0);
      } else {
        pageSizes = await invoke('get_all_page_sizes');
      }
      await initThumbScale(pageSize0);
      const resumePage = resumeLastPage ? loadLastPage(path, currentFileMtime) : 0;
      if (resumePage > 0 && resumePage < count) await gotoPage(resumePage);
    } catch (e) {
      await showOpenFileError(path, e);
    }
  }

  // EPUB/HTML(재배치 문서)의 폰트 크기를 바꿔서 mupdf에 다시 페이지를 나누게 한다 — 페이지
  // 수 자체가 바뀌므로(내용은 같아도 몇 페이지로 쪼개지는지가 변함) openFile의 "문서를 새로
  // 연 직후" 시퀀스를 그대로 재사용한다(캐시 전부 무효화 + 페이지 크기/썸네일 다시 계산).
  // mupdf에 relayout 후 같은 읽던 위치를 찾아주는 API가 없어서, 이전 페이지/전체 쪽수 비율로
  // 근사해 이동한다(정확히 같은 문장으로 돌아가진 않지만 대충 비슷한 위치는 유지된다).
  async function setReflowFontSize(em) {
    em = Math.max(6, Math.min(36, em));
    const oldPage = currentPage, oldCount = pageCount;
    epubFontSize = em;
    localStorage.setItem('vimong-epub-font-size', String(em));
    ++thumbVersion; // 이전 폰트 크기 기준으로 아직 로딩 중이던 썸네일이 새 배열에 잘못 꽂히지 않게
    resetMainViewState();
    statusMsg = t('status.opening_file'); // open_pdf와 같은 이유로 relayout도 오래 걸릴 수 있다
    const count = await invoke('set_reflow_font_size', { em });
    pageCount = count;
    // relayout하면 챕터별 시작 페이지가 통째로 밀려서, open_pdf 때 받아둔 toc의 page 번호가
    // 전부 옛 레이아웃 기준으로 남는다 — 그대로 두면 사이드바 하이라이트가 실제 보고 있는
    // 페이지와 다른 챕터를 가리키게 된다(currentTocItem이 toc.page로 계산되므로).
    toc = await invoke('get_toc');
    thumbnails = Array.from({ length: count }, () => ({ src: '', loaded: false }));
    const pageSize0 = await invoke('get_page_size', { pageNum: 0 });
    // openFile은 항상 0페이지(표지)부터 시작하니 windowLo/windowHi를 0으로 맞춰두는 게
    // 맞지만, 여기는 비율 근사로 계산한 target으로 바로 gotoPage할 거라 0으로 맞춰봤자
    // 곧바로 덮어써진다 — 그 사이(await initThumbScale 동안) Svelte가 이 임시 0페이지
    // 상태를 실제로 한 번 그려버려서, zi/zo 할 때마다 필요도 없는 1페이지 렌더가 매번
    // 끼어들었다. pageSizes[0]만 캐싱해두고 windowLo/windowHi는 아래 gotoPage에 맡긴다.
    pageSizes[0] = pageSize0;
    if (viewMode !== 'single') {
      pageSizes = await invoke('get_all_page_sizes');
    }
    await initThumbScale(pageSize0);
    const target = oldCount > 0 ? Math.round((oldPage / oldCount) * (count - 1)) : 0;
    await gotoPage(Math.max(0, Math.min(target, count - 1)));
    // gotoPage는 스크롤/포커스만 맞추고 실제 페이지 래스터화는 IntersectionObserver가
    // 비동기로(fire-and-forget) 트리거해서, 지금 시점엔 아직 화면에 안 그려져 있다 —
    // 여기서 안 기다리고 바로 "여는 중..."을 지우면, 실제 렌더링이 끝나기 한참 전에
    // 메시지만 먼저 사라져서 "언제 끝난 거지" 하고 헷갈리게 된다. 지금 페이지의 래스터화
    // (fetchPageBitmap)만 명시적으로 기다렸다가 지운다 — observer가 나중에 같은 페이지를
    // 또 요청해도 진행 중인 프라미스를 그대로 공유하니 중복 렌더는 안 생긴다.
    await fetchPageBitmap(currentPage).catch(() => {});
    statusMsg = baseName(filePath);
  }

  // 페이지 wrap div가 mainEl 스크롤 컨테이너 기준으로 세로 몇 px 지점에 있는지 —
  // offsetTop은 중간에 position:relative 조상이 끼면 틀어질 수 있어 getBoundingClientRect로 계산
  function pageOffsetInMain(el) {
    if (!el || !mainEl) return 0;
    return el.getBoundingClientRect().top - mainEl.getBoundingClientRect().top + mainEl.scrollTop;
  }

  async function gotoPage(n) {
    const target = Math.max(0, Math.min(n, pageCount - 1));
    if (hwpMode) {
      currentPage = target;
      const hit = allSearchResults.find(r => r.page === target);
      pageHighlights = hit ? hit.rects : [];
      await tick();
      pageWrapEls[target]?.scrollIntoView({ block: 'start', inline: 'start' });
      return;
    }
    if (mdMode) { currentPage = target; return; } // 페이지 개념이 없으니 그냥 no-op
    if (viewMode === 'single') {
      await ensurePageSize(target);
      windowLo = target;
      windowHi = target;
      prefetchAdjacentSizes(target);
    }
    currentPage = target;
    hintMode = false;
    pageLinks = [];
    // 사이드바 강조/스크롤은 무거운 본문 렌더링을 기다리지 않고 즉시 반영
    thumbEls[target]?.scrollIntoView({ block: 'nearest' });
    const hit = allSearchResults.find(r => r.page === target);
    pageHighlights = hit ? hit.rects : [];
    ensureTocPageVisible();
    // 포커스가 이미 사이드바 안에 있을 때만 toc/썸네일 항목을 따라 옮긴다 — 안 그러면 본문
    // (mainEl)에 포커스가 있는 채로 j/k 스크롤하다 페이지가 넘어갈 때마다 사이드바로 포커스를
    // 뺏어가서, 그 다음부터 j/k가 본문이 아니라 사이드바 목록 안에서 움직이는 문제가 생긴다.
    // 썸네일 쪽을 안 맞춰주면, gg/G로 currentPage만 바뀌고 document.activeElement는 여전히
    // 이전 페이지의 썸네일에 남아있어서, 그 다음 사이드바 j/k(= thumbEls.indexOf(activeElement)
    // 기준)가 "이전에 보던 페이지 ± 1"로 계산돼버린다.
    if (isFocusInSidebar()) {
      if (sidebarView === 'toc') syncTocFocus(target);
      else if (sidebarView === 'thumbs') focusNearestItem(thumbEls, target);
    }
    if (isContinuous(viewMode)) {
      await tick();
      pageWrapEls[target]?.scrollIntoView({ block: 'start', inline: 'start' });
    } else {
      mainEl?.scrollTo(0, 0);
      // 'two'/'single' 모두 currentPage가 바뀌면 새 wrap div가 붙고, lazyMainPage의
      // IntersectionObserver가 이미 화면에 들어와 있는 그 div를 바로 감지해서 그려주므로
      // 여기서 따로 렌더 호출을 할 필요가 없다.
    }
  }

  // ── single/two(비연속) 모드: 스크롤은 항상 네이티브로 자연스럽게 흐르게 두고(경계에서도
  // 안 막는다) → 브라우저가 스크롤 물리 계산을 끝낸 뒤 보내는 표준 scrollend에서 경계인지
  // 확인해 "넘어갈 준비"만 해둔다 → 그 다음 wheel에서 넘긴다. 넘긴 직후에는 같은 스와이프의
  // 남은 관성이 새 페이지까지 밀고 들어가 또 넘기지 않도록, overflow를 잠깐 hidden으로
  // 바꿔 스크롤할 대상 자체를 없앤다(트랙패드 관성은 e.preventDefault()로는 못 막는다 —
  // 실측 확인함). 이 뒤로 더 정교하게(유휴시간 연장, cancelable 등) 다듬어보려 했지만 전부
  // 반대쪽 부작용(응답 없음)이 더 컸다 — 고정된 짧은 잠금 하나가 제일 낫다는 결론.
  let readyToFlip = null; // 'up' | 'down' | null — scrollend 시점에 이 방향 경계였다(다음 wheel에 넘어간다)
  let flipCooldownUntil = 0;
  const FLIP_COOLDOWN_MS = 300;

  function onMainScrollEnd() {
    if (isContinuous(viewMode) || !filePath || performance.now() < flipCooldownUntil) return;
    const atBottom = mainEl.scrollTop + mainEl.clientHeight >= mainEl.scrollHeight - 1;
    const atTop = mainEl.scrollTop <= 0;
    if (atBottom && currentPage < pageCount - 1) readyToFlip = 'down';
    else if (atTop && currentPage > 0) readyToFlip = 'up';
    else readyToFlip = null;
  }

  function flipPage(dir) {
    readyToFlip = null;
    const pageStep = isTwoUp(viewMode) ? 2 : 1;
    const target = currentPage + (dir === 'down' ? pageStep : -pageStep);
    gotoPage(target);
    if (dir === 'up') {
      // 위로 스크롤하다 이전 페이지로 넘어간 경우, 그 페이지의 끝에서부터 이어지는 게
      // 자연스럽다(키보드 k와 동일한 동작) — gotoPage가 스크롤을 (0,0)으로 리셋한 뒤에
      // 다시 맨 아래로 내려야 하니 렌더 타이밍을 살짝 기다린다.
      setTimeout(() => mainEl.scrollTo(0, mainEl.scrollHeight), 50);
    }
    flipCooldownUntil = performance.now() + FLIP_COOLDOWN_MS;
    mainEl.style.overflow = 'hidden';
    setTimeout(() => { mainEl.style.overflow = ''; }, FLIP_COOLDOWN_MS);
  }

  function onMainWheel(e) {
    // 트랙패드 핀치 제스처는 브라우저/웹뷰가 ctrlKey를 켠 wheel 이벤트로 합성해 보낸다 —
    // 실제 Ctrl 키 입력과 구분이 안 되지만 트랙패드 핀치가 훨씬 흔한 경우라 그대로 줌으로 매핑한다.
    if (e.ctrlKey) {
      if (!filePath) return;
      e.preventDefault();
      const zoomFactor = e.deltaY > 0 ? 0.02 : 0.018; // 축소(deltaY>0)가 확대보다 살짝 더 빠르다
      const newScale = Math.min(12, Math.max(0.25, +(scale - e.deltaY * zoomFactor).toFixed(2)));
      if (newScale === scale) return;
      // 커서 아래의 콘텐츠 지점(스크롤 오프셋 + 뷰포트 내 커서 위치)이 줌 후에도 같은
      // 화면 위치에 남도록, 배율 비율만큼 그 지점을 이동시키고 그 차이를 scrollLeft/Top에 반영한다.
      const rect = mainEl.getBoundingClientRect();
      const viewportX = e.clientX - rect.left;
      const viewportY = e.clientY - rect.top;
      const contentX = mainEl.scrollLeft + viewportX;
      const contentY = mainEl.scrollTop + viewportY;
      const ratio = newScale / scale;
      flushSync(() => { scale = newScale; });
      mainEl.scrollLeft = contentX * ratio - viewportX;
      mainEl.scrollTop = contentY * ratio - viewportY;
      triggerRerender();
      return;
    }
    if (isContinuous(viewMode) || !filePath || Math.abs(e.deltaY) <= Math.abs(e.deltaX)) return;

    const dir = e.deltaY > 0 ? 'down' : e.deltaY < 0 ? 'up' : null;
    if (!dir) return;

    // .viewer가 min-height:100%라, 축소해서 페이지가 뷰포트보다 작아지면 scrollHeight가
    // clientHeight로 뻥튀기되어 스크롤 자체가 발생하지 않는다 — 이 경우 scrollend가 안
    // 오므로 별도로 처리한다(항상 경계이므로 flipCooldownUntil만으로 다중 페이지 넘김을 막는다).
    if (mainEl.scrollHeight <= mainEl.clientHeight + 1) {
      if (performance.now() < flipCooldownUntil) { e.preventDefault(); return; }
      if ((dir === 'down' && currentPage < pageCount - 1) || (dir === 'up' && currentPage > 0)) {
        e.preventDefault();
        flipPage(dir);
      }
      return;
    }

    // 스크롤 여지가 있는 일반적인 경우 — 절대 가로막지 않는다. scrollend로 확인된 경계에서만,
    // 그것도 "그다음" wheel이 와야 넘어간다.
    if (readyToFlip === dir) {
      e.preventDefault();
      flipPage(dir);
    }
  }

  // toc 항목 선택 시 페이지 이동 + 해당 헤딩 위치로 스크롤. gotoPage가 스크롤을 (0,0)으로
  // 리셋하므로 렌더링(및 DOM 갱신)이 끝난 뒤에 y 위치로 다시 스크롤해야 한다.
  // 페이지 + 페이지 내 세로 위치(y, 배율 적용 전 좌표)로 이동 — TOC 항목 점프와 마크 점프가 공유
  async function gotoPageAndY(page, y) {
    await gotoPage(page);
    // 가로 연속 모드는 스크롤 축이 가로라 세로 위치 보정이 의미 없어 건너뛴다
    if (y != null && viewMode !== 'horizontal-continuous') {
      await tick();
      // 연속 모드는 gotoPage가 이미 페이지 맨 위로 스크롤 정렬해뒀으니 그 위치를 기준으로 더
      // 내리고, 그 외 모드는 gotoPage가 (0,0)으로 리셋해뒀으니 절대 위치로 바로 스크롤한다
      if (isContinuous(viewMode)) mainEl?.scrollTo(0, pageOffsetInMain(pageWrapEls[page]) + y * scale);
      else mainEl?.scrollTo(0, y * scale);
    }
  }

  // ── SyncTeX 정방향 검색(소스 → PDF) ──────────────────────────
  // 에디터가 `vimong --forward-search file.tex 3 out.pdf`로 실행하면(이미 떠 있는 인스턴스면
  // 재사용, Rust의 tauri_plugin_single_instance) 이 이벤트/pending 값으로 들어온다.
  async function handleForwardSearch(fs) {
    try {
      if (filePath !== fs.pdf_path) {
        // 지금 탭에 이미 다른 문서가 열려 있으면(세션 복원 직후 등) openFile로 덮어쓰지 않고
        // 새 탭에 연다 — 빈 탭일 때만 그 자리에 바로 채운다.
        if (filePath) await newTab(fs.pdf_path); else await openFile(fs.pdf_path);
      }
      const hit = await invoke('synctex_forward', { texFile: fs.tex_file, line: fs.line, column: 0, pdfPath: fs.pdf_path });
      await gotoPageAndY(hit.page, hit.y);
      pageHighlights = [hit.rect];
      await getCurrentWindow().setFocus();
    } catch (e) {
      statusMsg = t('status.synctex_error', { e });
    }
  }

  async function gotoTocItem(item) {
    if (mdMode) { selectedTocId = item.id; mdComponentRef?.scrollToOffset(item.mdOffset, item.mdAnchor); return; }
    if (item.page === null) return;
    selectedTocId = item.id;
    await gotoPageAndY(item.page, item.y);
  }

  // 지금 스크롤 위치를 페이지+y(배율 적용 전 좌표)로 — gotoPageAndY의 역연산
  function currentPageY() {
    const base = isContinuous(viewMode) ? pageOffsetInMain(pageWrapEls[currentPage]) : 0;
    return (mainEl.scrollTop - base) / scale;
  }

  async function gotoMark(letter) {
    const m = marks[letter];
    if (m) await gotoPageAndY(m.page, m.y);
  }

  // 사이드바 항목 목록(els)에서 idx번째로 포커스를 옮기고 보이게 스크롤 — 썸네일/목차/
  // 검색결과 세 패널이 전부 같은 방식으로 j/k 이동을 처리하므로 공유한다.
  function focusNearestItem(els, idx) {
    els[idx]?.focus();
    els[idx]?.scrollIntoView({ block: 'nearest' });
  }

  // 항목 목록(items)에서 page에 해당하는 toc 항목을 찾는다 — 정확히 그 페이지를 가리키는
  // 항목이 여러 개면 selectedTocId 우선, 없으면(표지 등) 그 페이지 이전의 가장 가까운 항목
  function findNearestTocItem(items, page) {
    const exact = items.filter(it => it.page === page);
    if (exact.length > 0) return exact.find(it => it.id === selectedTocId) ?? exact[0];
    let item = null;
    for (const it of items) {
      if (it.page !== null && it.page <= page) item = it;
    }
    return item ?? items[0] ?? null;
  }

  // TOC가 보이는 상태에서 페이지가 바뀌면(j/k, 검색, 링크 등 경로 무관) 해당 페이지의
  // 헤딩으로 포커스를 옮겨서 화살표 이동이 항상 현재 페이지 기준으로 이어지게 한다.
  function syncTocFocus(page) {
    if (sidebarView !== 'toc' || flatToc.length === 0) return;
    const item = findNearestTocItem(flatToc, page);
    const idx = flatToc.indexOf(item);
    focusNearestItem(tocItemEls, idx);
  }

  // 조상 노드가 접혀있으면 id 경로(예: "0-2-1")를 따라 전부 펼친다 — 자기 자신은 그대로 두고
  // 부모들만 편다(자기 자신을 펴면 그 항목의 자식들까지 불필요하게 드러난다)
  function expandAncestors(id) {
    const parts = id.split('-');
    for (let i = 1; i < parts.length; i++) collapsed.delete(parts.slice(0, i).join('-'));
  }

  // 현재 페이지의 헤딩이 접힌 상위 항목 밑에 숨어 있어도 toc에서 보이도록 조상을 펼친다 —
  // flatToc는 접힌 항목의 자식을 아예 안 담으므로 collapsed를 무시하고 전체 트리에서 찾는다
  function ensureTocPageVisible() {
    if (currentTocItem) expandAncestors(currentTocItem.id);
  }

  // ── Table of contents (flattened for rendering) ──────────────
  // 문서를 열면 챕터(최상위 항목)만 보이고 하위 항목은 접힌 채로 시작하도록,
  // 자식이 있는 모든 노드의 id를 미리 collapsed에 채워 넣는다
  function collectParentIds(nodes, path, out) {
    nodes.forEach((n, i) => {
      const id = path ? `${path}-${i}` : `${i}`;
      if (n.children.length > 0) {
        out.add(id);
        collectParentIds(n.children, id, out);
      }
    });
    return out;
  }

  function flattenToc(nodes, depth, path, out, all = false) {
    nodes.forEach((n, i) => {
      const id = path ? `${path}-${i}` : `${i}`;
      out.push({ title: n.title, page: n.page, y: n.y, mdOffset: n.mdOffset, mdAnchor: n.mdAnchor, depth, id, hasChildren: n.children.length > 0 });
      if (n.children.length > 0 && (all || !collapsed.has(id))) flattenToc(n.children, depth + 1, id, out, all);
    });
    return out;
  }

  // flatToc와 같은 방식으로 레이어 트리를 평평하게 편다 — toc과 id 스킴은 같아도
  // collapsedLayers가 따로 있어 서로 접힘 상태가 섞이지 않는다. 목차와 달리 문서를 열 때
  // 기본적으로 전부 펼쳐둔다(중첩이 보통 얕고, 이 기능의 목적 자체가 구조를 드러내는 것).
  function flattenLayers(nodes, depth, path, out) {
    nodes.forEach((n, i) => {
      const id = path ? `${path}-${i}` : `${i}`;
      out.push({ xref: n.xref, name: n.name, enabled: n.enabled, depth, id, hasChildren: n.children.length > 0 });
      if (n.children.length > 0 && !collapsedLayers.has(id)) flattenLayers(n.children, depth + 1, id, out);
    });
    return out;
  }
  let flatLayers = $derived(flattenLayers(layers, 0, '', []));

  function toggleLayerNode(id) {
    if (collapsedLayers.has(id)) collapsedLayers.delete(id);
    else collapsedLayers.add(id);
  }
  let flatToc = $derived(flattenToc(toc, 0, '', []));
  // 현재 페이지를 "담고 있는" toc 항목 — 정확히 그 페이지를 가리키는 헤딩이 없으면(섹션
  // 내용이 길어 중간 페이지에는 헤딩이 없는 경우) 그 페이지 이전의 가장 가까운 헤딩이 계속
  // 현재 항목으로 하이라이트되게 한다. 전체 트리 기준이라 접힌 항목도 후보에 포함된다.
  let currentTocItem = $derived(toc.length ? findNearestTocItem(flattenToc(toc, 0, '', [], true), currentPage) : null);

  function tocPages(nodes, out) {
    nodes.forEach(n => { if (n.page !== null) out.push(n.page); tocPages(n.children, out); });
    return out;
  }

  // 항목 대부분이 "페이지 1개당 항목 1개씩, 페이지 번호가 1씩 증가하며 순서대로"
  // 나열돼 있으면 목차가 아니라 스캔/슬라이드 도구가 페이지마다 자동 생성한 목록이다
  // (내지1, 내지2... 같은 스캔본이나 슬라이드 N: ... 같은 PPT 변환본).
  // 반대로 실제 목차는 챕터/섹션이 페이지를 건너뛰거나 한 페이지에 여러 항목이
  // 몰리는 등 불규칙하므로, 항목 수 대 페이지 수 비율만으로는 구분할 수 없다.
  let tocIsMeaningful = $derived.by(() => {
    if (toc.length === 0) return false;
    const pages = tocPages(toc, []);
    if (pages.length < 5) return true; // 표본이 적으면 그냥 목차로 취급
    let sequential = 0;
    for (let i = 1; i < pages.length; i++) {
      if (pages[i] - pages[i - 1] === 1) sequential++;
    }
    return sequential / (pages.length - 1) <= 0.8;
  });

  // toc 항목이 커버하는 페이지 범위 — 다음에 나오는 "같거나 얕은 depth" 항목 직전까지.
  // 하위 항목이 접혀 있어도 포함되도록 collapsed 무시한 전체 트리(all=true) 기준으로 찾는다.
  function tocSectionRange(item) {
    if (item.page === null) return null;
    const all = flattenToc(toc, 0, '', [], true);
    const idx = all.findIndex(it => it.id === item.id);
    let end = pageCount - 1;
    for (let i = idx + 1; i < all.length; i++) {
      if (all[i].depth <= item.depth && all[i].page !== null) { end = all[i].page - 1; break; }
    }
    return { start: item.page, end: Math.max(item.page, end) };
  }

  // 위 범위를 :export/:w 페이지 지정 문법(1-indexed, "a" 또는 "a-b")으로 변환
  function tocSectionSpec(item) {
    const r = tocSectionRange(item);
    if (!r) return null;
    return r.start === r.end ? `${r.start + 1}` : `${r.start + 1}-${r.end + 1}`;
  }

  // 내보내기 페이지 스펙 기본값: 썸네일 다중 선택 > toc에서 선택해둔 헤딩(그 섹션 전체) > 없음(호출부가 현재 페이지/전체로 처리)
  function defaultExportSpec() {
    if (selectedThumbs.size > 1) return selectedThumbsToSpec();
    if (sidebarView === 'toc' && selectedTocId) {
      const item = flatToc.find(it => it.id === selectedTocId);
      const spec = item && tocSectionSpec(item);
      if (spec) return spec;
    }
    return null;
  }

  // toc 항목 우클릭 — 그 헤딩(하위 섹션 포함)의 페이지 범위를 내보내기 스펙으로 미리 채운다
  async function onTocContextMenu(e, item) {
    e.preventDefault();
    if (mdMode || item.page === null) return; // 마크다운은 페이지 PNG로 내보낼 개념이 없음
    selectedTocId = item.id;
    const exportItem = await MenuItem.new({
      text: t('context.export'),
      action: () => {
        exportFormat = 'png';
        exportFilename = defaultExportFilename();
        exportPageSpec = tocSectionSpec(item) ?? String(item.page + 1);
        exportMerge = false; exportPassword = ''; exportError = '';
        showExportModal = true;
      },
    });
    const menu = await Menu.new({ items: [exportItem] });
    await menu.popup();
  }

  function toggleTocNode(id) {
    if (collapsed.has(id)) collapsed.delete(id);
    else collapsed.add(id);
  }

  // ── Sidebar resize ────────────────────────────────────────────
  const SIDEBAR_MIN = 100;
  const SIDEBAR_MAX = 500;

  function startSidebarResize(e) {
    e.preventDefault();
    resizingSidebar = true;
    document.body.style.userSelect = 'none';
  }

  function onSidebarResizeMove(e) {
    if (!resizingSidebar) return;
    // 최소 폭보다 더 왼쪽으로 끌면 cmd+shift+t로 숨긴 것처럼 사이드바를 닫는다
    if (e.clientX < SIDEBAR_MIN) {
      resizingSidebar = false;
      document.body.style.userSelect = '';
      sidebarVisible = false;
      return;
    }
    sidebarWidth = Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, e.clientX));
  }

  function stopSidebarResize() {
    if (!resizingSidebar) return;
    resizingSidebar = false;
    document.body.style.userSelect = '';
    localStorage.setItem('vimong-sidebar-width', String(sidebarWidth));
    // initThumbScale은 문서를 열 때 한 번만 계산돼서, 연 뒤에 사이드바를 넓혀도 이미
    // 그려둔 낮은 해상도 그대로였다 — 리사이즈가 끝나면 새 폭 기준으로 다시 계산해서
    // 이후에 (지연 로딩으로) 새로 그려질 썸네일은 실제 표시 크기에 맞게 선명하게 나온다
    if (filePath) initThumbScale();
  }

  // ── Link hints (f) ───────────────────────────────────────────
  const HINT_CHARS = 'asdfghjklqwertyuiopzxcvbnm';

  function makeHintLabels(n) {
    if (n <= HINT_CHARS.length) return HINT_CHARS.slice(0, n).split('');
    const labels = [];
    for (const a of HINT_CHARS) {
      for (const b of HINT_CHARS) {
        labels.push(a + b);
        if (labels.length === n) return labels;
      }
    }
    return labels;
  }

  async function activateLinkHints() {
    if (hwpMode) { statusMsg = t('status.hwp_unsupported_feature'); return; }
    if (mdMode) { statusMsg = t('status.md_unsupported_feature'); return; }
    // 연속/두 페이지 보기에서는 currentPage 말고도 다른 페이지가 동시에 화면에 보이므로
    // (예: '한 페이지 연속보기'에서 스크롤이 페이지 경계에 걸쳐 있는 경우), visiblePageSet에
    // 잡히는 페이지 전부에서 링크를 모은다 — 안 그러면 currentPage가 아닌 화면의 다른
    // 페이지에 있는 링크는 f를 눌러도 안 잡힌다.
    const pages = [...visiblePageSet].sort((a, b) => a - b);
    if (pages.length === 0) pages.push(currentPage);
    const perPage = await Promise.all(pages.map(async (p) => {
      await ensureLinksLoaded(p);
      return (pageLinkCache.get(p) ?? []).map(l => ({ ...l, srcPage: p }));
    }));
    const links = perPage.flat();
    if (links.length === 0) { statusMsg = 'No links in view'; return; }
    const labels = makeHintLabels(links.length);
    pageLinks = links.map((l, i) => ({ ...l, label: labels[i] }));
    hintBuf = '';
    hintMode = true;
  }

  // 목적지를 못 찾은 내부 참조(각주 하이퍼타겟이 문서에 아예 등록 안 돼 있는 경우 등)의
  // 최선의 대안 — 정확한 좌표는 알 길이 없지만, 책 형태 PDF는 각주가 거의 항상 같은
  // 페이지 하단에 있으므로 그쪽으로 스크롤해 보이게 한다.
  async function scrollToPageBottom(page) {
    // 연속 보기가 아니면 화면엔 currentPage(스프레드)만 붙어 있으니, 링크가 실제로
    // 있던 페이지가 그게 아니면 먼저 그 페이지로 이동해야 pageWrapEls[page]가 존재한다.
    if (!isContinuous(viewMode) && page !== currentPage) await gotoPage(page);
    await ensurePageSize(page);
    const [, h] = pageDims(page);
    await tick();
    if (!mainEl || viewMode === 'horizontal-continuous') return;
    if (isContinuous(viewMode)) {
      const bottom = pageOffsetInMain(pageWrapEls[page]) + h * scale;
      mainEl.scrollTo(0, bottom - mainEl.clientHeight);
    } else {
      mainEl.scrollTo(0, h * scale - mainEl.clientHeight);
    }
  }

  async function followLink(link) {
    hintMode = false;
    pageLinks = [];
    if (link.page !== null && link.page !== undefined) {
      await gotoPageAndY(link.page, link.y);
    } else if (link.uri && !link.uri.startsWith('#')) {
      await openUrl(link.uri);
    } else if (link.uri) {
      // '#'로 시작하는 uri는 MuPDF가 명명된 대상(named destination)을 못 찾았을 때
      // 돌려주는 미해결 내부 참조(예: "#nameddest=...")다 — PDF 자체에 해당 대상이
      // 없는 경우(예: 각주 하이퍼타겟 미등록)라 정확히는 못 가지만, 같은 페이지
      // 하단으로라도 스크롤해 최선을 다한다. currentPage가 아니라 링크가 실제로
      // 있던 페이지(srcPage) 기준 — 연속 보기에서 화면에 currentPage 외 다른 페이지도
      // 같이 보일 때, 그 다른 페이지의 각주를 눌러도 엉뚱하게 currentPage로 안 가게.
      await scrollToPageBottom(link.srcPage ?? currentPage);
      statusMsg = t('status.link_target_missing');
    }
  }

  // 문서를 열자마자 페이지 수만큼(수십~수백 개) 순차로 render_thumbnail을 다 부르면
  // 안 보이는 썸네일까지 굽느라 Mutex<AppState>를 오래 붙잡아서, 메인 페이지는 빨리 떠도
  // 그 직후 스크롤/페이지 이동 같은 조작이 그 뒤에 밀려 버벅인다. 그래서 사이드바에
  // 실제로 스크롤돼서 보이는 썸네일만, 보이는 시점에 하나씩 렌더링한다 (lazyThumb 액션).
  // ── 썸네일 다중 선택 (Cmd/Shift+클릭) — 내보내기 페이지 지정에 쓴다 ──
  let selectedThumbs = $state(new SvelteSet()); // 0-indexed 페이지 번호
  let thumbSelectAnchor = null; // 비반응 — Shift+클릭 범위의 시작점

  function onThumbClick(e, i) {
    if (e.shiftKey && thumbSelectAnchor !== null) {
      // Finder처럼 기존 선택은 그대로 두고 앵커~i 범위만 더한다(합집합) — 앵커는 안 바꿔서
      // Shift+클릭을 이어서 눌러도 항상 마지막 cmd/일반 클릭 지점 기준으로 범위가 계산된다
      const [lo, hi] = thumbSelectAnchor <= i ? [thumbSelectAnchor, i] : [i, thumbSelectAnchor];
      for (let p = lo; p <= hi; p++) selectedThumbs.add(p);
      return;
    }
    if (e.metaKey || e.ctrlKey) {
      if (selectedThumbs.has(i)) selectedThumbs.delete(i); else selectedThumbs.add(i);
      thumbSelectAnchor = i;
      return;
    }
    // 그냥 클릭 — 이동은 기존처럼 하되, 그 페이지 하나는 선택된 상태로 시작한다
    // (Finder처럼 첫 클릭도 선택으로 잡혀야 이어서 Cmd+클릭으로 추가할 수 있다)
    selectedThumbs = new SvelteSet([i]);
    thumbSelectAnchor = i;
    gotoPage(i);
  }

  // 우클릭한 페이지가 이미 선택돼 있으면 선택 전체를 대상으로, 아니면(선택 밖을 우클릭)
  // Finder처럼 그 페이지 하나만 선택한 걸로 바꾸고 메뉴를 띄운다.
  async function onThumbContextMenu(e, i) {
    e.preventDefault();
    if (!selectedThumbs.has(i)) { selectedThumbs = new SvelteSet([i]); thumbSelectAnchor = i; }
    const exportItem = await MenuItem.new({
      text: t('context.export'),
      action: () => {
        exportFormat = 'png';
        exportFilename = defaultExportFilename();
        exportPageSpec = selectedThumbs.size > 1 ? selectedThumbsToSpec() : String(i + 1);
        exportMerge = false; exportPassword = ''; exportError = '';
        showExportModal = true;
      },
    });
    const saveImagesItem = await MenuItem.new({
      text: t('context.save_images'),
      action: () => exportImagesForPages(selectedThumbs.size > 1 ? [...selectedThumbs] : [i]),
    });
    const menu = await Menu.new({ items: [exportItem, saveImagesItem] });
    await menu.popup();
  }

  // 선택한 페이지(0-indexed)를 :export 페이지 지정 문법(1-indexed, 연속 구간은 a-b)으로 압축
  function selectedThumbsToSpec() {
    const pages = [...selectedThumbs].sort((a, b) => a - b);
    const parts = [];
    let start = pages[0], prev = pages[0];
    for (let i = 1; i <= pages.length; i++) {
      const p = pages[i];
      if (p === prev + 1) { prev = p; continue; }
      parts.push(start === prev ? `${start + 1}` : `${start + 1}-${prev + 1}`);
      start = prev = p;
    }
    return parts.join(',');
  }

  let thumbScale = 0;
  // 문서를 열자마자 초기 화면에 보이는 썸네일들의 IntersectionObserver가 initThumbScale()의
  // IPC 왕복이 끝나기도 전에 먼저 발동해버려서, thumbScale이 아직 0일 때 로딩을 시도하고
  // 그대로 disconnect돼 다시는 안 불리는 문제가 있었다 — 앞쪽 페이지 썸네일이 영영 안 뜨던
  // 원인. loadThumbAt이 이 Promise를 기다리게 해서 준비될 때까지 확실히 대기하게 한다.
  let resolveThumbScaleReady;
  let thumbScaleReady = new Promise(r => { resolveThumbScaleReady = r; });

  let thumbPlaceholderHeight = $state(90); // 로딩 전 자리 표시용 — 실제 페이지 비율로 갱신됨
  // 170(레티나 스크린샷에서 잰 픽셀 값) ÷ 2 = 실제 CSS px — 스크린샷은 물리 해상도(2배)라서
  const THUMB_MAX_WIDTH = 85; // .thumb-item img / .thumb-placeholder의 width와 반드시 맞출 것

  async function initThumbScale(knownSize) {
    const dpr = window.devicePixelRatio || 1;
    const [pageW, pageH] = knownSize ?? await invoke('get_page_size', { pageNum: 0 });
    // sidebar-content/thumb-item 패딩 + 페이지 번호 폭을 대략 뺀 값 — 사이드바를 최소
    // 크기까지 줄여도(SIDEBAR_MIN=100) 썸네일이 몇 픽셀짜리로 쪼그라들어 안 보이지
    // 않도록 최소 폭을 보장하고, 반대로 사이드바를 넓혀도 Skim처럼 계속 커지지 않게
    // THUMB_MAX_WIDTH(아래 .thumb-item img/.thumb-placeholder의 max-width와 맞춰야 함)로 상한선을 둔다
    const targetWidth = Math.min(Math.max(sidebarWidth - 70, 24), THUMB_MAX_WIDTH);
    thumbScale = (targetWidth * dpr) / pageW;
    thumbPlaceholderHeight = targetWidth * (pageH / pageW); // Skim처럼 실제 페이지 비율에 맞춤
    resolveThumbScaleReady();
  }

  async function loadThumbAt(i, version) {
    await thumbScaleReady;
    if (version !== thumbVersion || thumbnails[i]?.loaded) return;
    try {
      const src = await invoke('render_thumbnail', { pageNum: i, scale: thumbScale });
      if (version !== thumbVersion) return; // 로딩 도중 다른 문서로 바뀌었으면 버림
      thumbnails[i] = { src: `data:image/png;base64,${src}`, loaded: true };
    } catch (e) { console.error(`thumbnail render failed for page ${i}:`, e); }
  }

  function lazyThumb(node, i) {
    const version = thumbVersion;
    const observer = new IntersectionObserver((entries) => {
      if (entries[0].isIntersecting) {
        queueThumbLoad(() => loadThumbAt(i, version));
        observer.disconnect();
      }
    }, { root: node.closest('.sidebar-content'), rootMargin: '300px 0px' });
    observer.observe(node);
    return { destroy: () => observer.disconnect() };
  }

  // ── Key handling ───────────────────────────────────────────
  async function handleKeydown(e) {
    const key = e.key;
    if (mode === 'command' || mode === 'search') return; // <input>이 직접 처리
    // 폼 필드(텍스트/콤보박스/리스트박스)는 페이지 위에 얹은 실제 <input>/<textarea>/<select>라
    // notePending 같은 상태 플래그가 따로 없다 — 지금 포커스가 그 안에 있으면 타이핑이 vim
    // 단축키로 새지 않게 여기서 막는다. Escape만 넘겨서 포커스를 내려놓을 수 있게 한다.
    if (document.activeElement?.classList?.contains('field-live-input')) {
      if (key === 'Escape') { document.activeElement.blur(); e.preventDefault(); }
      return;
    }

    // ── link hint mode: 'f' 로 진입, 라벨 입력으로 이동 ──
    if (hintMode) {
      if (['Shift','Control','Alt','Meta'].includes(key)) return;
      e.preventDefault();
      if (key === 'Escape') { hintMode = false; pageLinks = []; return; }
      hintBuf += key;
      const matches = pageLinks.filter(l => l.label.startsWith(hintBuf));
      const exact = matches.find(l => l.label === hintBuf);
      if (exact) await followLink(exact);
      else if (matches.length === 0) { hintMode = false; pageLinks = []; }
      return;
    }

    // ESC closes any open modal or search panel
    if (key === 'Escape') {
      // preventDefault 없이 두면 WKWebView가 네이티브 cancelOperation:으로 넘겨서,
      // 전체화면 상태일 때 macOS가 이걸 "전체화면 종료"로 처리해버린다.
      e.preventDefault();
      if (presentationMode) { await togglePresentation(); return; }
      // 설정 모달 위에 겹쳐 뜨는 팝업이라, 한 번 누르면 이것부터 닫히고 설정 모달은 남는다
      if (showTexshopHelp) { closeTexshopHelp(); return; }
      if (blockingHelpModalOpen()) {
        showFileInfo = showShortcuts = showCommands = showOpenSource = showSettings = showAbout = false; resetModalNav(); return;
      } else if (showQuitConfirm) {
        showQuitConfirm = false; return;
      } else if (showTabOnlyConfirm) {
        showTabOnlyConfirm = false; return;
      } else if (showDiscardConfirm) {
        showDiscardConfirm = false; pendingCloseTabIdx = null; return;
      } else if (showQuitDiscardConfirm) {
        showQuitDiscardConfirm = false; quitDirtyQueue = []; pendingQuitTabIdx = null; return;
      } else if (notePending) {
        cancelNote(); return;
      } else if (showPasswordPrompt) {
        cancelPassword(); return;
      } else if (showPasswordManage) {
        showPasswordManage = false; return;
      } else if (showWritePasswordPrompt) {
        cancelWritePassword(); return;
      } else if (showExportModal) {
        cancelExport(); return;
      } else if (showExportManyConfirm) {
        resolveExportMany(false); return;
      } else if (visualMode) {
        // 검색 결과가 남아있어도 선택 취소가 먼저 — 아래 검색 결과 지우기보다 우선한다
        visualMode = false;
        anchorIdx = cursor.idx; // 선택은 접고 캐럿만 남긴다
      } else if (cursor) {
        anchorIdx = null;
        cursor = null;
      } else if (mdMode && mdSearchResults.length > 0) {
        mdSearchResults = [];
        if (sidebarView === 'search') sidebarView = toc.length > 0 ? 'toc' : 'search';
      } else if (allSearchResults.length > 0) {
        allSearchResults = [];
        pageHighlights = [];
        if (sidebarView === 'search') sidebarView = tocIsMeaningful ? 'toc' : 'thumbs';
      }
      return;
    }

    // 프레젠테이션 중엔 스페이스/백스페이스로도 다음/이전 페이지(Skim 등 프레젠테이션 관례) —
    // j/k도 이미 되지만(한 페이지가 화면에 꽉 차서 스크롤 여지가 없어 바로 페이지 전환됨),
    // 발표 중 익숙한 키도 같이 받아준다.
    if (presentationMode && (key === ' ' || key === 'Backspace')) {
      e.preventDefault();
      if (key === ' ') { if (currentPage < pageCount - 1) await gotoPage(currentPage + 1); }
      else { if (currentPage > 0) await gotoPage(currentPage - 1); }
      return;
    }

    // 문서 정보 모달이 열려 있으면 모달 전용 키만 처리
    if (showFileInfo) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return; // Cmd+Q 등 메뉴 단축키는 네이티브로 넘긴다
      if (key === 'Tab') {
        e.preventDefault();
        const modal = document.querySelector('.fileinfo-modal');
        const tabSel  = [...modal.querySelectorAll('.modal-tabs button')];
        const content = [...modal.querySelectorAll('.modal-body [tabindex], .modal-body button:not(:disabled), .fileinfo-modal > button')];
        const all = [...tabSel, ...content]; // 탭 선택자 → 콘텐츠 → 닫기 순 (실제 화면 순서와 일치)
        const idx = all.indexOf(document.activeElement);
        const next = e.shiftKey
          ? all[(idx - 1 + all.length) % all.length]
          : all[(idx + 1) % all.length];
        next?.focus();
        return;
      }
      e.preventDefault();
      if (key === 'j' || key === 'ArrowDown') { fileInfoScrollEl?.scrollBy(0, 60); return; }
      if (key === 'k' || key === 'ArrowUp') { fileInfoScrollEl?.scrollBy(0, -60); return; }
      handleTabNav(key, FILE_INFO_TABS, fileInfoTab, t => { fileInfoTab = t; focusModalTab('.fileinfo-modal', FILE_INFO_TABS, t); });
      return;
    }
    if (showShortcuts) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return; // Cmd+Q 등 메뉴 단축키는 네이티브로 넘긴다
      e.preventDefault();
      if (key === 'Tab') {
        // 그냥 두면(preventDefault 없이) 브라우저 기본 포커스 이동이 모달 밖 뒤 문서로 새 나간다.
        const modal = document.querySelector('.shortcuts-modal');
        const focusable = [...modal.querySelectorAll('.modal-tabs button'), ...modal.querySelectorAll(':scope > button')];
        const idx = focusable.indexOf(document.activeElement);
        const next = e.shiftKey
          ? focusable[(idx - 1 + focusable.length) % focusable.length]
          : focusable[(idx + 1) % focusable.length];
        next?.focus();
        return;
      }
      if (key === 'j') { shortcutsScrollEl?.scrollBy(0, 60); return; }
      if (key === 'k') { shortcutsScrollEl?.scrollBy(0, -60); return; }
      handleTabNav(key, SHORTCUTS_TABS, shortcutsTab, t => { shortcutsTab = t; focusModalTab('.shortcuts-modal', SHORTCUTS_TABS, t); });
      return;
    }
    if (showCommands) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return; // Cmd+Q 등 메뉴 단축키는 네이티브로 넘긴다
      e.preventDefault();
      if (key === 'Tab') {
        const modal = document.querySelector('.commands-modal');
        const focusable = [...modal.querySelectorAll('.modal-tabs button'), ...modal.querySelectorAll(':scope > button')];
        const idx = focusable.indexOf(document.activeElement);
        const next = e.shiftKey
          ? focusable[(idx - 1 + focusable.length) % focusable.length]
          : focusable[(idx + 1) % focusable.length];
        next?.focus();
        return;
      }
      if (key === 'j') { commandsScrollEl?.scrollBy(0, 60); return; }
      if (key === 'k') { commandsScrollEl?.scrollBy(0, -60); return; }
      handleTabNav(key, COMMANDS_TABS, commandsTab, t => { commandsTab = t; focusModalTab('.commands-modal', COMMANDS_TABS, t); });
      return;
    }
    if (showOpenSource) return;
    if (showAbout) return; // Cmd+C로 버전 텍스트를 복사할 수 있어야 해서 아무 것도 가로채지 않는다
    if (showSettings) {
      // Enter/문자 입력 등은 모달 안 요소 자체 기본 동작에 맡긴다(SyncTeX 명령 입력창이 있어서
      // j/k나 문자 단축키를 가로채면 타이핑과 충돌한다) — Tab만 모달 밖으로 안 새게 직접 가둔다.
      // 안 그러면(트래핑 없이 그냥 두면) 브라우저 기본 포커스 이동이 뒤에 깔린 페이지(최근
      // 파일 목록 등)까지 다 훑고 지나가야 다시 모달로 돌아온다.
      if (key === 'Tab') {
        e.preventDefault();
        const modal = document.querySelector('.settings-modal');
        const focusable = [...modal.querySelectorAll('button, select, input')];
        const idx = focusable.indexOf(document.activeElement);
        const next = e.shiftKey
          ? focusable[(idx - 1 + focusable.length) % focusable.length]
          : focusable[(idx + 1) % focusable.length];
        next?.focus();
      }
      return;
    }
    if (notePending) return; // 타이핑/Enter는 textarea 자체가 처리, Escape는 위 ESC 통합 처리에서 이미 끝남
    if (showPasswordPrompt) return; // Enter/Escape는 입력창 자체 onkeydown에서 처리
    if (showPasswordManage) return; // 위와 동일
    if (showWritePasswordPrompt) return; // 위와 동일
    if (showExportModal) return; // 위와 동일
    if (showQuitConfirm) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return;
      if (key === 'Tab') {
        e.preventDefault();
        const buttons = [...document.querySelectorAll('.quit-modal button')];
        const idx = buttons.indexOf(document.activeElement);
        const next = e.shiftKey
          ? buttons[(idx - 1 + buttons.length) % buttons.length]
          : buttons[(idx + 1) % buttons.length];
        next?.focus();
      }
      return;
    }
    if (showTabOnlyConfirm) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return;
      if (key === 'Tab') {
        e.preventDefault();
        const buttons = [...document.querySelectorAll('.quit-modal button')];
        const idx = buttons.indexOf(document.activeElement);
        const next = e.shiftKey
          ? buttons[(idx - 1 + buttons.length) % buttons.length]
          : buttons[(idx + 1) % buttons.length];
        next?.focus();
      }
      return;
    }
    if (showDiscardConfirm || showQuitDiscardConfirm) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return;
      if (key === 'Tab') {
        e.preventDefault();
        const buttons = [...document.querySelectorAll('.quit-modal button')];
        const idx = buttons.indexOf(document.activeElement);
        const next = e.shiftKey
          ? buttons[(idx - 1 + buttons.length) % buttons.length]
          : buttons[(idx + 1) % buttons.length];
        next?.focus();
      }
      return;
    }
    if (showExportManyConfirm) {
      if (['Shift','Control','Alt','Meta','Enter'].includes(key)) return;
      if (e.metaKey || e.ctrlKey) return;
      if (key === 'Tab') {
        e.preventDefault();
        const buttons = [...document.querySelectorAll('.export-many-modal button')];
        const idx = buttons.indexOf(document.activeElement);
        const next = e.shiftKey
          ? buttons[(idx - 1 + buttons.length) % buttons.length]
          : buttons[(idx + 1) % buttons.length];
        next?.focus();
      }
      return;
    }

    // ── 시작 화면(파일 없음)에서 j/k로 최근 파일 고르고 Enter/l로 열기, dd로 삭제 ──
    if (!filePath && recentFiles.length > 0 && ['ArrowDown','ArrowUp','j','k','Enter','l','d'].includes(key)) {
      e.preventDefault();
      if (key !== 'd') pendingD = false;
      if (key === 'j' || key === 'ArrowDown') recentIndex = Math.min(recentIndex + 1, recentFiles.length - 1);
      else if (key === 'k' || key === 'ArrowUp') recentIndex = Math.max(recentIndex - 1, 0);
      else if (key === 'd') {
        if (pendingD) { pendingD = false; await removeRecentFile(recentFiles[recentIndex]); }
        else pendingD = true;
      }
      else await openFile(recentFiles[recentIndex]);
      return;
    }
    pendingD = false;

    // Ignore modifier-only events
    if (['Shift','Control','Alt','Meta'].includes(key)) return;

    // ── ⌘⇧] / ⌘⇧[ (크롬처럼): gt / gT와 동일. e.code로 판단 — e.key는
    // 키보드 레이아웃/입력 소스(한글 등)에 따라 '}'가 아닌 다른 문자가 나올 수 있다 ──
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'BracketRight') {
      e.preventDefault(); await switchToTab((currentTabIdx + 1) % tabLabels.length); return;
    }
    if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'BracketLeft') {
      e.preventDefault(); await switchToTab((currentTabIdx - 1 + tabLabels.length) % tabLabels.length); return;
    }

    // ── ⌘1: 포인터, ⌘2: 텍스트 선택, ⌘3: 하이라이트, ⌘4: 메모 (툴바 도구 전환) ──
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === 'Digit1') {
      e.preventDefault(); setToolMode('pointer'); return;
    }
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === 'Digit2') {
      e.preventDefault(); setToolMode('text'); return;
    }
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === 'Digit3') {
      e.preventDefault(); setToolMode('highlight'); return;
    }
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === 'Digit4') {
      e.preventDefault(); setToolMode('note'); return;
    }

    // ── ⌘Z: 하이라이트/도형 실행 취소 (u와 동일) — macOS 관례상 다들 이 단축키부터 눌러본다 ──
    if ((e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey && e.code === 'KeyZ') {
      e.preventDefault(); await undoLastAnnotation(); return;
    }

    // ── ⌘F: 검색 (/ 와 동일) — macOS 관례상 다들 이 단축키부터 눌러본다.
    // Ctrl까지 같이 눌리면(⌃⌘F) 전체화면 토글(네이티브 단축키)이라 여기서 가로채면 안 된다 ──
    if (e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey && e.code === 'KeyF') {
      e.preventDefault(); await focusSearchTab(); return;
    }

    // ── 사이드바 항목에 포커스가 있을 때 j/k(또는 화살표)로 이동 — {n}j/{n}k처럼 숫자를
    // 먼저 치면 numBuf에 쌓이는데(아래 "digit accumulation" 참고), 이 블록이 그걸 안 읽고
    // 매번 한 칸만 이동하면 숫자가 그대로 numBuf에 남아 다음 입력(예: 나중의 {n}G)에
    // 엉뚱하게 섞여 들어간다 — 그래서 실제로 이동할 때(idx가 유효할 때)만 소비하고 비운다.
    if (['ArrowDown', 'ArrowUp', 'j', 'k'].includes(key)) {
      const down = key === 'ArrowDown' || key === 'j';
      const n = numBuf ? parseInt(numBuf, 10) : 1;
      if (sidebarView === 'toc') {
        const idx = tocItemEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          const nextIdx = down ? Math.min(idx + n, flatToc.length - 1) : Math.max(idx - n, 0);
          const item = flatToc[nextIdx];
          focusNearestItem(tocItemEls, nextIdx);
          await gotoTocItem(item);
          return;
        }
      } else if (sidebarView === 'thumbs') {
        const idx = thumbEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          // 이동 기준은 "포커스가 놓인 썸네일"이 아니라 항상 currentPage다 — 포커스는
          // 클릭이나 스크롤(currentPageObserver)로 페이지가 바뀌는 경로를 못 따라오는
          // 경우가 있어서, idx를 기준으로 삼으면 "썸네일 클릭 → gg/G → j"처럼 중간에
          // 다른 경로로 페이지가 바뀐 뒤에는 예전에 보던 페이지 ±1로 튄다. idx는 "지금
          // 포커스가 썸네일 목록 안에 있나"(= 사이드바 이동이냐 본문 스크롤이냐)를
          // 가리는 데만 쓴다.
          const nextIdx = down ? Math.min(currentPage + n, thumbnails.length - 1) : Math.max(currentPage - n, 0);
          focusNearestItem(thumbEls, nextIdx);
          await gotoPage(nextIdx);
          return;
        }
      } else if (sidebarView === 'search' && mdMode) {
        const idx = searchResultEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          const nextIdx = down ? Math.min(idx + n, mdSearchResults.length - 1) : Math.max(idx - n, 0);
          focusNearestItem(searchResultEls, nextIdx);
          searchResultIdx = nextIdx;
          jumpToMdSearchHit(nextIdx);
          return;
        }
      } else if (sidebarView === 'search') {
        const idx = searchResultEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          const nextIdx = down ? Math.min(idx + n, allSearchResults.length - 1) : Math.max(idx - n, 0);
          focusNearestItem(searchResultEls, nextIdx);
          const page = allSearchResults[nextIdx].page;
          searchResultIdx = flatSearchMatches().findIndex(m => m.page === page);
          await gotoPage(page);
          return;
        }
      } else if (sidebarView === 'notes') {
        const idx = notesResultEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          const nextIdx = down ? Math.min(idx + n, allNotes.length - 1) : Math.max(idx - n, 0);
          focusNearestItem(notesResultEls, nextIdx);
          await gotoPage(allNotes[nextIdx].page);
          return;
        }
      } else if (sidebarView === 'layers') {
        const idx = layersResultEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          const nextIdx = down ? Math.min(idx + n, flatLayers.length - 1) : Math.max(idx - n, 0);
          focusNearestItem(layersResultEls, nextIdx);
          return;
        }
      } else if (sidebarView === 'attachments') {
        const idx = attachmentsResultEls.indexOf(document.activeElement);
        if (idx !== -1) {
          e.preventDefault(); numBuf = '';
          const nextIdx = down ? Math.min(idx + n, attachments.length - 1) : Math.max(idx - n, 0);
          focusNearestItem(attachmentsResultEls, nextIdx);
          return;
        }
      }
    }

    // ── TOC 항목에 포커스가 있을 때 ←/→(또는 h/l)로 접기/펼치기 (표준 트리뷰 관례) ──
    // → / l : 접혀있으면 펼침, 이미 펼쳐져 있으면 첫 자식으로 이동
    // ← / h : 펼쳐져 있으면 접음, 접혀있거나 자식이 없으면 부모로 이동
    if (sidebarView === 'toc' && ['ArrowLeft', 'ArrowRight', 'h', 'l'].includes(key)) {
      const idx = tocItemEls.indexOf(document.activeElement);
      if (idx !== -1) {
        e.preventDefault();
        const item = flatToc[idx];
        if (key === 'ArrowRight' || key === 'l') {
          if (item.hasChildren && collapsed.has(item.id)) {
            collapsed.delete(item.id);
          } else if (item.hasChildren) {
            const nextIdx = Math.min(idx + 1, flatToc.length - 1);
            const nextItem = flatToc[nextIdx];
            focusNearestItem(tocItemEls, nextIdx);
            await gotoTocItem(nextItem);
          }
        } else if (item.hasChildren && !collapsed.has(item.id)) {
          collapsed.add(item.id);
        } else {
          const lastDash = item.id.lastIndexOf('-');
          const parentId = lastDash === -1 ? null : item.id.slice(0, lastDash);
          const parentIdx = parentId === null ? -1 : flatToc.findIndex(it => it.id === parentId);
          if (parentIdx !== -1) {
            const parentItem = flatToc[parentIdx];
            focusNearestItem(tocItemEls, parentIdx);
            await gotoTocItem(parentItem);
          }
        }
        return;
      }
    }

    // Absorb browser defaults for vim nav keys + mode-trigger keys
    if (['j','k','h','l','G','/',' ',':','[',']'].includes(key)) e.preventDefault();

    // ── z-prefix: zi / zo / z0 / zh / zv ──
    if (pendingPrefix === 'z') {
      // z를 쥔 채로 있으면(zHeld) pendingPrefix를 'z'로 유지해서 i/o를 여러 번 다시 눌러도
      // 매번 z를 새로 안 눌러도 되게 한다(PDF 뷰어 관례) — 그런데 z를 뗀 뒤에도 이걸 안
      // 지워두면, 한참 뒤에 완전히 새로 누르는 z(다음 zi/zo 시도의 첫 키)가 "zz(높이 맞춤)"의
      // 두 번째 z로 오인되어 소비되고 뒤따르는 i/o는 프리픽스 없이 허공에 뜨는 버그가 있었다
      // (zi/zo를 반복할 때 홀수 번째가 반응 없는 것처럼 보인 원인) — 그래서 z를 뗄 때(아래
      // svelte:window의 onkeyup)도 pendingPrefix를 같이 지운다.
      pendingPrefix = zHeld ? 'z' : '';
      if (mdMode) {
        // 마크다운은 페이지 폭/높이 개념이 없어 zh/zv/zz(핏)는 zi/zo/z0와 같은 "폰트 확대/축소"로
        // 대신한다 — scale(캔버스 렌더 배율)과 별개로 CSS 폰트 크기 배율만 바꾼다(triggerRerender 불필요).
        if (key === 'i') { mdScale = Math.min(+(mdScale + 0.1).toFixed(2), 3); return; }
        if (key === 'o') { mdScale = Math.max(+(mdScale - 0.1).toFixed(2), 0.5); return; }
        if (['0', 'h', 'v', 'z'].includes(key)) { mdScale = 1.0; return; }
        return;
      }
      if (epubReflowable) {
        // EPUB/HTML — 캔버스를 확대하는 대신 폰트 크기(em)를 바꿔서 다시 페이지를 나눈다.
        // zh/zv/zz(맞춤)는 페이지 자체 크기(REFLOW_WIDTH x REFLOW_HEIGHT)가 폰트 크기와
        // 무관하게 고정이라 아래 일반 scale 맞춤 로직을 그대로 써도 의미가 있다 — 그래서
        // 여기서 안 가로채고 흘려보낸다.
        if (key === 'i') { requestReflowFontSize((pendingEpubEm ?? epubFontSize) + EPUB_EM_STEP); return; }
        if (key === 'o') { requestReflowFontSize((pendingEpubEm ?? epubFontSize) - EPUB_EM_STEP); return; }
        if (key === '0') { requestReflowFontSize(DEFAULT_EPUB_EM); return; }
      }
      if (key === 'i') { scale = Math.min(+(scale + 0.25).toFixed(2), 12); triggerRerender(); return; }
      if (key === 'o') { scale = Math.max(+(scale - 0.25).toFixed(2), 0.25); triggerRerender(); return; }
      if (key === '0') { scale = 1.0; triggerRerender(); return; }
      if (key === 'h') { await fitToWidth(); return; }
      if (key === 'v' || key === 'z') { await fitToHeight(); return; }
    }

    // ── g-prefix: gg / gt / gT ──
    if (pendingPrefix === 'g') {
      pendingPrefix = '';
      const savedN = pendingN; pendingN = null;
      if (key === 'g') { if (mdMode) mdComponentRef?.scrollToTop(); else await gotoPage(0); return; }
      if (key === 'a') { showCharValue(); return; }
      // gc/gp/gs — 마크다운 코드/미리보기/분할 뷰 전환 (:md 명령과 동일, 마크다운 문서에서만 동작)
      if (mdMode && key === 'c') { setMdViewMode('code'); return; }
      if (mdMode && key === 'p') { setMdViewMode('preview'); return; }
      if (mdMode && key === 's') { setMdViewMode('split'); return; }
      if (key === 't') {
        if (savedN !== null) await switchToTab(savedN - 1); // 1-indexed
        else await switchToTab((currentTabIdx + 1) % tabLabels.length);
        return;
      }
      if (key === 'T') {
        await switchToTab((currentTabIdx - 1 + tabLabels.length) % tabLabels.length);
        return;
      }
    }

    // ── m-prefix: m{a-z} 현재 위치를 마크로 저장 ──
    if (pendingPrefix === 'm') {
      pendingPrefix = '';
      if (/^[a-z]$/.test(key)) {
        marks[key] = { page: currentPage, y: currentPageY() };
        statusMsg = t('status.mark_set', { letter: key }); // 실제로 저장됐는지 눈에 보이게
        return;
      }
    }

    // ── '-prefix: '{a-z} 저장해둔 마크로 이동 ──
    if (pendingPrefix === "'") {
      pendingPrefix = '';
      if (/^[a-z]$/.test(key)) {
        if (marks[key]) await gotoMark(key);
        else statusMsg = t('status.mark_not_found', { letter: key });
        return;
      }
    }

    // ── digit accumulation for nG ──
    if (key >= '1' && key <= '9') { numBuf += key; return; }
    if (key === '0' && numBuf.length > 0) { numBuf += key; return; }

    const n = numBuf ? parseInt(numBuf, 10) : null;
    numBuf = '';

    // ── normal-mode commands ──
    const pageStep = isTwoUp(viewMode) ? 2 : 1;
    if (key === 'j') {
      if (mdMode) { mdComponentRef?.scrollBy(60); }
      // 연속 모드는 페이지 경계에서 안 끊기고 그대로 흘러가는 게 핵심이라 스냅 없이 스크롤만
      else if (isContinuous(viewMode)) { mainEl.scrollBy(0, 60); }
      else {
        const atBottom = mainEl.scrollTop + mainEl.clientHeight >= mainEl.scrollHeight - 4;
        if (atBottom && currentPage < pageCount - 1) await gotoPage(currentPage + pageStep);
        else mainEl.scrollBy(0, 60);
      }
    } else if (key === 'J') {
      if (!mdMode && currentPage < pageCount - 1) await gotoPage(currentPage + pageStep);
    } else if (key === 'K') {
      if (!mdMode && currentPage > 0) await gotoPage(currentPage - pageStep);
    } else if (key === 'k') {
      if (mdMode) { mdComponentRef?.scrollBy(-60); }
      else if (isContinuous(viewMode)) { mainEl.scrollBy(0, -60); }
      else {
        const atTop = mainEl.scrollTop <= 4;
        if (atTop && currentPage > 0) {
          await gotoPage(currentPage - pageStep);
          setTimeout(() => mainEl.scrollTo(0, mainEl.scrollHeight), 50);
        } else {
          mainEl.scrollBy(0, -60);
        }
      }
    } else if (key === 'l') {
      if (mdMode) mdComponentRef?.scrollByX(60);
      else if (cursor) charMotion(1);
      else mainEl.scrollBy(60, 0);
    } else if (key === 'h') {
      if (mdMode) mdComponentRef?.scrollByX(-60);
      else if (cursor) charMotion(-1);
      else mainEl.scrollBy(-60, 0);
    } else if ((e.ctrlKey || e.metaKey) && key === 'd') {
      e.preventDefault(); // 브라우저 기본 단축키(북마크 등)와 겹치니 확실히 막는다
      if (mdMode) { mdComponentRef?.halfPageScrollBy(1); }
      else if (isContinuous(viewMode)) { mainEl.scrollBy(0, halfPageStep()); }
      else {
        // single/two 모드는 화면에 페이지가 딱 하나(쌍)뿐이라, "이 스크롤 컨테이너가 움직일
        // 수 있는 전체 거리"의 절반을 그대로 쓰면 된다 — .viewer의 padding까지 자동으로
        // 포함되어서, halfPageStep()의 페이지 높이 추정치(패딩 미포함)로는 두 번 눌러도
        // 살짝 못 미쳐 세 번째 입력이 더 필요했던 문제가 없다.
        const half = (mainEl.scrollHeight - mainEl.clientHeight) / 2;
        const atBottom = mainEl.scrollTop + mainEl.clientHeight >= mainEl.scrollHeight - 4;
        if (atBottom && currentPage < pageCount - 1) await gotoPage(currentPage + pageStep);
        else mainEl.scrollBy(0, half);
      }
    } else if ((e.ctrlKey || e.metaKey) && key === 'u') {
      e.preventDefault();
      if (mdMode) { mdComponentRef?.halfPageScrollBy(-1); }
      else if (isContinuous(viewMode)) { mainEl.scrollBy(0, -halfPageStep()); }
      else {
        const half = (mainEl.scrollHeight - mainEl.clientHeight) / 2;
        const atTop = mainEl.scrollTop <= 4;
        if (atTop && currentPage > 0) {
          await gotoPage(currentPage - pageStep);
          setTimeout(() => mainEl.scrollTo(0, mainEl.scrollHeight), 50);
        } else {
          mainEl.scrollBy(0, -half);
        }
      }
    } else if (key === 'r') {
      // 문서 전체가 아니라 지금 보고 있는 페이지만 돌린다(Preview/Acrobat과 같은 방식) —
      // 페이지 크기 스왑(pageDims)은 pageRotations를 그대로 읽으니 자동으로 반영되고,
      // 비트맵만 그 페이지에 한해 다시 그리면 된다(pageSizes는 회전 무관 원본 크기라
      // 그대로 둬도 됨).
      pageRotations[currentPage] = (getPageRotation(currentPage) + 90) % 360;
      await refreshPageBitmap(currentPage);
    } else if (key === 'R') {
      pageRotations[currentPage] = (getPageRotation(currentPage) + 270) % 360; // -90 대신 +270 — JS %는 음수를 그대로 돌려준다
      await refreshPageBitmap(currentPage);
    } else if (key === 'H' && selection) {
      await addHighlightToSelection();
    } else if (key === 'u') {
      await undoLastAnnotation();
    } else if (key === 'p') {
      await togglePresentation();
    } else if (key === 'G') {
      if (mdMode) mdComponentRef?.scrollToBottom();
      else if (n !== null) await gotoPage(n - 1);
      else            await gotoPage(pageCount - 1);
    } else if (key === 'g') {
      pendingN = n; pendingPrefix = 'g';
    } else if (key === 'z') {
      zHeld = true;
      pendingPrefix = 'z';
    } else if (key === 'm') {
      pendingPrefix = 'm';
    } else if (key === "'" || key === '`' || e.code === 'Quote' || e.code === 'Backquote') {
      // 실제 vim은 '와 `(백틱) 둘 다 마크 이동으로 쓴다 — 이 앱엔 "정확한 컬럼" 개념이
      // 없어 vim처럼 구분할 필요가 없어서 둘 다 같은 프리픽스로 취급한다. 둘 다 여러
      // 키보드 레이아웃(예: US-International)에서 억양 합성용 데드키라 e.key가 "'"/"`"가
      // 아니라 "Dead"로 올 수 있어 물리 키 위치 기준인 e.code도 같이 본다.
      pendingPrefix = "'";
    } else if (key === 'f') {
      await activateLinkHints();
    } else if (key === ':') {
      // : 입력창도 상태표시줄 안에 있어서 / 검색과 같은 이유로 프레젠테이션 중엔 막는다
      // (focusSearchTab 주석 참고).
      if (presentationMode) { statusMsg = t('status.presentation_unsupported_feature'); }
      else { mode = 'command'; cmdInput = ''; cmdCompletion = null; }
    } else if (key === '/') {
      await focusSearchTab();
    } else if (key === 'n' && (mdMode ? mdSearchResults.length > 0 : allSearchResults.length > 0)) {
      await gotoSearchMatch(1);
    } else if (key === 'N' && (mdMode ? mdSearchResults.length > 0 : allSearchResults.length > 0)) {
      await gotoSearchMatch(-1);
    } else if (key === ']') {
      await cycleSidebarView(1);
    } else if (key === '[') {
      await cycleSidebarView(-1);
    } else if (key === 'Tab') {
      e.preventDefault(); // 기본 동작(다음 포커스 가능 요소로 이동)이 겹쳐 끼어들지 않게
      toggleSidebarFocus();
    } else if (key === 'w' && cursor) {
      // 검색 이동은 텍스트 선택 모드가 아니어도 커서를 놓아주므로, w/W/b/B/e/E/y도 모드가
      // 아니라 커서 존재 여부로만 게이트한다 — 안 그러면 검색 후 바로 못 쓴다
      wordMotion(false, 'w');
    } else if (key === 'W' && cursor) {
      wordMotion(true, 'w');
    } else if (key === 'b' && cursor) {
      wordMotion(false, 'b');
    } else if (key === 'B' && cursor) {
      wordMotion(true, 'b');
    } else if (key === 'e' && cursor) {
      wordMotion(false, 'e');
    } else if (key === 'E' && cursor) {
      wordMotion(true, 'e');
    } else if (key === 'v' && cursor) {
      visualMode = !visualMode;
      anchorIdx = cursor.idx; // 켜질 땐 지금 위치를 anchor로 고정, 꺼질 땐 선택을 캐럿으로 접음
    } else if ((key === 'y' || key === 'Y') && selection) {
      await copySelection();
    }
  }

  function isFocusInSidebar() {
    return !!document.querySelector('.sidebar-content')?.contains(document.activeElement);
  }

  // 현재 sidebarView에서 현재 페이지에 해당하는 항목에 포커스 — Tab 토글과 탭 전환
  // (아래 switchSidebarView) 둘 다에서 쓴다
  function focusCurrentSidebarItem() {
    if (sidebarView === 'toc') {
      syncTocFocus(currentPage);
    } else if (sidebarView === 'thumbs') {
      focusNearestItem(thumbEls, currentPage);
    } else if (sidebarView === 'search' && mdMode) {
      focusNearestItem(searchResultEls, searchResultIdx);
    } else if (sidebarView === 'search') {
      const idx = allSearchResults.findIndex(r => r.page === currentPage);
      focusNearestItem(searchResultEls, idx !== -1 ? idx : 0);
    } else if (sidebarView === 'notes') {
      const idx = allNotes.findIndex(n => n.page === currentPage);
      focusNearestItem(notesResultEls, idx !== -1 ? idx : 0);
    } else if (sidebarView === 'layers') {
      focusNearestItem(layersResultEls, 0);
    } else if (sidebarView === 'attachments') {
      focusNearestItem(attachmentsResultEls, 0);
    }
  }

  // 포커스가 옮겨간 순간에만 굵고 진한 링이 반짝였다가 옅어지도록 .focus-flash를 붙인다
  // (CSS keyframes: focus-flash-item/-inset/-page). 이미 붙어있는 상태에서 또 호출되면
  // 클래스를 뗐다가 강제 리플로우 후 다시 붙여야 애니메이션이 처음부터 재생된다.
  function flashFocusRing(el) {
    if (!el) return;
    el.classList.remove('focus-flash');
    void el.offsetWidth;
    el.classList.add('focus-flash');
  }

  // w: 사이드바 ↔ 본문 포커스 토글. 사이드바 쪽으로 갈 땐 현재 페이지에 해당하는
  // 항목(썸네일/목차/검색결과)에 포커스해서 곧바로 j/k 등으로 이동을 이어갈 수 있게 한다.
  // j/k로 사이드바 안에서 항목만 옮길 때는(focusNearestItem 직접 호출) 반짝임 없이
  // 조용한 상태 링만 보이고, w로 넘어오는 "그 순간"에만 반짝인다.
  function toggleSidebarFocus() {
    if (!sidebarVisible) return;
    if (isFocusInSidebar()) {
      mainEl?.focus();
      flashFocusRing(mainEl);
      return;
    }
    focusCurrentSidebarItem();
    flashFocusRing(document.activeElement);
    flashFocusRing(sidebarEl);
  }

  // 내용이 있는 탭만 대상으로 [/]로 순환 전환 (검색 탭은 결과 없어도 항상 활성)
  // [/]는 키보드로 명시적으로 사이드바 탭을 고르는 동작이라, 본문에 포커스가 있었어도 항상
  // 사이드바(방금 고른 탭의 현재 항목)로 옮겨야 자연스럽다 — switchSidebarView는 마우스
  // 클릭 등 다른 경로와도 같이 쓰여서 원래 포커스가 사이드바 안에 있었을 때만 옮겨주므로
  // (클릭으로 훑어볼 땐 포커스를 안 뺏으려고), 여기서 한 번 더 강제로 포커스한다. 사이드바가
  // 숨겨져 있었으면 보여준다(안 그러면 옮길 DOM 자체가 없다).
  async function cycleSidebarView(dir) {
    if (pageCount === 0) return;
    const views = mdMode
      ? ['toc', 'search'].filter(v => v !== 'toc' || toc.length > 0)
      : ['thumbs', 'toc', 'search', 'notes', 'layers', 'attachments'].filter(v =>
          (v !== 'toc' || toc.length > 0) && (v !== 'layers' || layers.length > 0) &&
          (v !== 'attachments' || attachments.length > 0));
    const idx = views.indexOf(sidebarView);
    const next = views[(idx + dir + views.length) % views.length];
    sidebarVisible = true;
    if (next === 'notes') await switchToNotesTab(); else await switchSidebarView(next);
    await tick();
    focusCurrentSidebarItem();
    flashFocusRing(document.activeElement);
    flashFocusRing(sidebarEl);
  }

  // 사이드바 "메모" 탭 — 문서 전체 메모를 한 번에 모아 보여준다(search_all과 같은 목적).
  // 하이라이트/도형과 달리 탭을 열 때마다 다시 긁어와야 하므로(캐시가 페이지 단위라 여러
  // 페이지를 한 번에 볼 땐 의미가 없다) 탭에 진입할 때, 그리고 메모를 추가/수정/삭제/
  // 실행취소할 때마다 새로 불러온다.
  async function refreshNotesList() {
    if (hwpMode) { allNotes = []; return; }
    try { allNotes = await invoke('get_all_notes'); } catch (_) { allNotes = []; }
  }
  async function switchToNotesTab() {
    await switchSidebarView('notes');
    await refreshNotesList();
  }

  // 탭 전환 — j/k로 사이드바 항목을 보던 중(포커스가 사이드바 안에 있던 상태)에 탭을
  // 바꾸면, 이전 탭의 포커스된 항목이 통째로 사라지면서 포커스가 본문으로 튕겨나가던
  // 문제가 있었다. 원래 사이드바에 포커스가 있었을 때만 새 탭의 해당 항목으로 옮겨준다
  // (그냥 클릭으로 탭만 훑어보는 중이면 포커스를 강제로 뺏지 않는다).
  async function switchSidebarView(view) {
    const wasInSidebar = isFocusInSidebar();
    sidebarView = view;
    if (wasInSidebar) {
      await tick(); // 새 탭의 항목 DOM(썸네일/목차 엘리먼트)이 실제로 그려질 때까지 대기
      focusCurrentSidebarItem();
    }
  }

  // '/' 키 또는 검색 탭 클릭 전용: 검색 탭으로 들어가서 입력 필드에 포커스한다.
  // "새로 진입했는지"는 sidebarView 값이 아니라 입력창이 실제로 마운트돼 있는지
  // (searchInputEl)로 판단해야 한다 — 사이드바를 숨겼다 켜면 sidebarView는 그대로
  // 'search'인데 입력창은 언마운트돼 있어서, sidebarView만 보면 이미 떠 있다고
  // 착각해 stale한 searchInputEl.focus()를 부르고 실제 포커스는 안 가는 채로
  // mode='search'만 남아 이후 모든 vim 키가 막혀버리는 문제가 있었다.
  async function focusSearchTab() {
    if (pageCount === 0) return;
    // 프레젠테이션은 사이드바를 CSS로 숨기기만 하고 DOM은 그대로 마운트돼 있어서, 검색
    // 입력창이 이미 "마운트는 됐지만 display:none이라 실제로 포커스가 안 되는" 상태가
    // 된다 — mode만 'search'로 남고 아무도 키를 못 받아 그 뒤 모든 vim 키가 막혀버렸다.
    // 검색 결과는 사이드바 목록으로 보여주는 구조라 사이드바 없이는 반쪽짜리 기능이고,
    // 파워포인트 슬라이드 쇼도 발표 중엔 편집/검색 같은 조작을 안 주는 것과 같은 맥락으로
    // 프레젠테이션 중엔 검색 자체를 그냥 막는다(모드 전환으로 흐름을 끊지 않음).
    if (presentationMode) { statusMsg = t('status.presentation_unsupported_feature'); return; }
    sidebarVisible = true;
    sidebarView = 'search';
    mode = 'search';
    if (searchInputEl) {
      searchInputEl.focus(); // 이미 마운트돼 있으면 바로 포커스
    } else {
      searchQuery = lastSearch;
      wantSearchFocus = true; // 새로 마운트될 때 use:focusOnSearchMount 가 포커스
    }
  }

  // 페이지별 rects를 낱개 매치로 펼쳐서 n/N이 페이지 단위가 아니라
  // 매치 단위로 이동하게 한다 (같은 페이지 매치를 다 돌기 전엔 페이지 이동 안 함).
  function flatSearchMatches() {
    return allSearchResults.flatMap(r => r.rects.map(rect => ({ page: r.page, rect })));
  }

  async function gotoSearchMatch(dir) {
    if (mdMode) {
      if (mdSearchResults.length === 0) return;
      hlsearchOn = true;
      searchResultIdx = (searchResultIdx + dir + mdSearchResults.length) % mdSearchResults.length;
      jumpToMdSearchHit(searchResultIdx);
      await tick();
      focusNearestItem(searchResultEls, searchResultIdx);
      return;
    }
    const flat = flatSearchMatches();
    if (flat.length === 0) return;
    hlsearchOn = true;
    searchResultIdx = (searchResultIdx + dir + flat.length) % flat.length;
    const m = flat[searchResultIdx];
    if (m.page !== currentPage) await gotoPage(m.page);
    // hwp는 rect가 실제 좌표가 아닌 더미값(문자 단위 커서/선택 좌표를 rhwp가 아직 제공하지 않음)
    // 이라 커서 배치/세로 위치 보정 없이 페이지 이동까지만 한다
    if (hwpMode) return;
    await placeCursorAt(m.page, m.rect[0], m.rect[1]); // w/W로 이어서 단어 선택할 수 있게 매치 위치에 커서
    if (viewMode === 'horizontal-continuous') return; // 가로 스크롤 모드는 세로 위치 보정 불필요
    await tick();
    const y = Math.max(0, m.rect[1] * scale - 80);
    if (isContinuous(viewMode)) mainEl?.scrollTo(0, pageOffsetInMain(pageWrapEls[m.page]) + y);
    else mainEl?.scrollTo(0, y);
  }

  // ── Command input keydown (status bar <input> 전용) ────────
  function handleCmdInputKey(e) {
    // mode를 'normal'로 되돌리기 전에 버블링을 끊어야 한다 — 안 그러면 이 keydown이
    // svelte:window의 handleKeydown까지 올라갔을 때 이미 바뀐 mode를 보고(예: Enter로
    // 시작 화면의 최근 파일 열기 같은) 엉뚱한 동작을 같이 실행해버린다.
    e.stopPropagation();
    if (e.key === 'Escape') { e.preventDefault(); mode = 'normal'; cmdInput = ''; cmdCompletion = null; }
    else if (e.key === 'Enter') {
      e.preventDefault();
      // 지금 고른 후보가 디렉터리면(:e로 열 수 없는 대상) 실행 대신 그 안으로 들어간다 —
      // Tab 순환 중 원하는 디렉터리에서 바로 확정하는 용도.
      if (cmdDirCandidateActive()) { descendCmdDirCompletion(); return; }
      cmdHistory.push(cmdInput);
      execCommand(cmdInput); mode = 'normal'; cmdInput = ''; cmdCompletion = null;
    }
    // 자동완성 목록이 떠 있으면(cmdCompletion) ↑/↓는 그 목록 안에서 선택을 옮기고,
    // 안 떠 있을 때만 원래 하던 명령 기록 훑어보기로 쓴다.
    else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (cmdCompletion && cmdCompletion.candidates.length > 0) moveCmdCompletionSelection(-1);
      else cmdInput = cmdHistory.up(cmdInput);
    }
    else if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (cmdCompletion && cmdCompletion.candidates.length > 0) moveCmdCompletionSelection(1);
      else cmdInput = cmdHistory.down(cmdInput);
    }
    else if (e.key === 'Tab') { e.preventDefault(); cycleCmdCompletion(e.shiftKey ? -1 : 1); }
    // → — 커서가 맨 끝에 있고 지금 고른 후보가 디렉터리일 때만 "안으로 들어가기"로 가로채고,
    // 그 외엔 평소처럼 텍스트 커서 이동에 맡긴다(preventDefault 안 함).
    else if (e.key === 'ArrowRight' && cmdDirCandidateActive() &&
             e.currentTarget.selectionStart === cmdInput.length && e.currentTarget.selectionEnd === cmdInput.length) {
      e.preventDefault();
      descendCmdDirCompletion();
    }
    else if ((e.metaKey || e.ctrlKey) && e.key === 'v') {
      e.preventDefault();
      clipboardRead().then(text => { if (text) cmdInput += text.trimEnd(); });
    }
    // ⌘⇧] / ⌘⇧[ 탭 전환 — handleSearchInputKey와 같은 이유로 여기도 안 먹었다
    else if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'BracketRight') {
      e.preventDefault(); switchToTab((currentTabIdx + 1) % tabLabels.length);
    }
    else if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'BracketLeft') {
      e.preventDefault(); switchToTab((currentTabIdx - 1 + tabLabels.length) % tabLabels.length);
    }
  }

  function closeDocument() {
    sessionStorage.removeItem('vimong-file');
    resetMainViewState();
    pageCount = 0;
    currentPage = 0;
    filePath = '';
    invoke('set_document_menu_enabled', { enabled: false });
    hwpMode = false;
    hwpPages = [];
    hwpDoc = null;
    mdMode = false;
    mdSource = '';
    mdSearchResults = [];
    mdScale = 1.0;
    epubReflowable = false;
    thumbnails = [];
    allSearchResults = [];
    pageHighlights = [];
    lastSearch = ''; searchQuery = '';
    statusMsg = 'No file open.  :e to open';
    tabLabels[currentTabIdx] = 'New Tab';
    recentIndex = 0;
    toc = [];
    sidebarView = 'thumbs';
    collapsed = new SvelteSet(); selectedTocId = null;
  }

  // 터미널에서 드래그&드롭 등으로 복사된 경로는 따옴표로 감싸지거나
  // 특수문자가 백슬래시로 이스케이프(\(, \~ 등)되어 있을 수 있음
  // PDF Info 딕셔너리 날짜는 "D:YYYYMMDDHHmmSS+HH'mm'" 형식 — 타임존은 버리고 보기 좋게만 표시
  function formatPdfDate(raw) {
    const m = /^D:(\d{4})(\d{2})(\d{2})(\d{2})?(\d{2})?(\d{2})?/.exec(raw || '');
    if (!m) return raw || '';
    const [, y, mo, d, h = '00', mi = '00', se = '00'] = m;
    return `${y}-${mo}-${d} ${h}:${mi}:${se}`;
  }

  function parsePathArg(raw) {
    let path = raw.trim().replace(/\0/g, '');
    if ((path.startsWith("'") && path.endsWith("'")) ||
        (path.startsWith('"') && path.endsWith('"'))) {
      path = path.slice(1, -1).trim();
    }
    return path.replace(/\\(.)/g, '$1');
  }

  // ":w 1-3,7-8 out.pdf"에서 앞쪽 토큰이 페이지 스펙인지 판별 — "1", "1-3", "1,3", "1-3,7-8" 형태만 허용
  const PAGE_SPEC_RE = /^\d+(-\d+)?(,\d+(-\d+)?)*$/;

  async function doSavePdf(outPath, pageSpec, password) {
    await invoke('save_pdf', { outPath, pageSpec, password });
    statusMsg = t('status.write_done', { path: outPath });
  }

  async function execWrite(arg) {
    arg = arg.trim();
    let withPassword = false;
    const flag = /^-p\s+/.exec(arg);
    if (flag) { withPassword = true; arg = arg.slice(flag[0].length); }
    if (!arg) { statusMsg = t('status.write_no_path'); return; }
    const sp = arg.indexOf(' ');
    let pageSpec = null, pathArg = arg;
    if (sp !== -1 && PAGE_SPEC_RE.test(arg.slice(0, sp))) {
      pageSpec = arg.slice(0, sp);
      pathArg = arg.slice(sp + 1);
    }
    const outPath = parsePathArg(pathArg);
    if (!outPath) { statusMsg = t('status.write_no_path'); return; }
    if (withPassword) {
      pendingWrite = { outPath, pageSpec };
      writePassword1 = ''; writePassword2 = ''; writePasswordError = '';
      showWritePasswordPrompt = true;
      return;
    }
    try {
      // :w는 :export의 짧은 버전 — 항상 PDF로, 항상 한 파일로 합쳐서 저장한다. export처럼
      // 확장자로 포맷을 추론하지 않고 항상 pdf로 고정하고, runExport(썸네일/toc 선택을 pagespec
      // 생략 시 자동으로 대신 쓰는 편의 기능이 있음)를 거치지 않고 바로 호출한다 — :w는 pagespec
      // 없으면 항상 "전체 문서"를 뜻해야지, 사이드바에 남아있는 선택으로 조용히 바뀌면 안 된다.
      await invoke('export_document', { outPath, format: 'pdf', pageSpec, merge: true, password: null });
      statusMsg = t('status.write_done', { path: outPath });
    } catch (e) {
      statusMsg = t('status.write_error', { e });
    }
  }

  async function submitWritePassword() {
    if (!writePassword1) return;
    if (writePassword1 !== writePassword2) { writePasswordError = t('password.mismatch'); return; }
    writePasswordBusy = true;
    writePasswordError = '';
    try {
      if (pendingWrite.kind === 'export') {
        const { outPath, format, pageSpec, merge } = pendingWrite;
        if (await runExport(outPath, format, pageSpec, merge, writePassword1)) {
          statusMsg = t('status.export_done', { path: outPath });
        }
      } else {
        // :w -p — 위 execWrite와 같은 이유로 runExport를 거치지 않고 직접 호출한다
        const { outPath, pageSpec } = pendingWrite;
        await invoke('export_document', { outPath, format: 'pdf', pageSpec, merge: true, password: writePassword1 });
        statusMsg = t('status.write_done', { path: outPath });
      }
      showWritePasswordPrompt = false;
      pendingWrite = null;
    } catch (e) {
      writePasswordError = String(e);
    } finally {
      writePasswordBusy = false;
    }
  }

  function cancelWritePassword() {
    showWritePasswordPrompt = false;
    pendingWrite = null;
  }

  // 파일 메뉴의 "다른 이름으로 저장" — 다른 프로그램처럼 페이지 지정 없이 문서 전체를
  // 다른 경로/이름으로 그대로 저장한다. 일부 페이지만 뽑고 싶으면 :export/:w를 쓰면 된다.
  async function doSaveAs() {
    const target = await save({
      title: t('save_as.title'),
      filters: [{ name: 'PDF', extensions: ['pdf'] }],
      defaultPath: filePath || undefined,
    });
    if (!target) return;
    try {
      await doSavePdf(target, null, null);
      statusMsg = t('status.write_done', { path: target });
    } catch (e) {
      statusMsg = t('status.write_error', { e });
    }
  }

  // 저장/내보내기 창의 페이지 범위 물음표 버튼 — 자세한 문법은 명령어 도움말(파일 탭)로 뺐다.
  // 지금 열려있던 창을 닫고 도움말을 여는데, 입력해둔 값은 state에 남아있어서 나중에
  // 다시 열면 그대로 있다.
  async function openPageSpecHelp() {
    showExportModal = false;
    commandsTab = 'file';
    showCommands = true;
    await tick(); // 모달/탭이 실제로 DOM에 그려질 때까지 기다린 뒤에 스크롤해야 위치가 맞는다
    pagespecSectionEl?.scrollIntoView({ block: 'start' });
  }

  function dirName(path) {
    return path.slice(0, path.length - baseName(path).length);
  }

  // 원본 문서와 같은 폴더에, 확장자 없는 기본 파일명 (포맷은 드롭박스가 별도로 관리)
  function defaultExportFilename() {
    return filePath ? baseName(filePath).replace(/\.[^.]+$/, '') : 'export';
  }

  // "1-3,7-8,20-22" 형태의 페이지 스펙이 가리키는 페이지 수를 센다 — 호출부에서 이미
  // PAGE_SPEC_RE로 검증한 문자열만 넘어온다는 게 전제.
  function countSpecPages(spec) {
    return spec.split(',').reduce((sum, part) => {
      const [a, b] = part.split('-');
      return sum + ((b === undefined ? +a : +b) - +a + 1);
    }, 0);
  }

  // 페이지가 많은 내보내기(5장 초과)를 확인시키는 모달 — 네이티브 ask()는 기본 포커스
  // 버튼을 못 정해서(대부분 OK 쪽) 실수로 Enter를 누르면 그대로 진행돼버리므로, 취소 버튼에
  // 기본 포커스를 주는 커스텀 모달을 쓴다. merge 여부에 따라 안내 문구만 다르게 보여준다
  // ("파일이 여러 개 생김" vs "합치는 데 시간/메모리가 많이 듦") — 합쳐도 페이지 수가 많으면
  // 렌더링·합성 자체가 무거워서 시스템이 멎을 수 있으므로 파일 개수와 무관하게 확인이 필요하다.
  function confirmExportMany(n, merge) {
    return new Promise(resolve => {
      exportManyCount = n;
      exportManyMerge = merge;
      exportManyResolve = resolve;
      showExportManyConfirm = true;
    });
  }

  function resolveExportMany(ok) {
    showExportManyConfirm = false;
    exportManyResolve?.(ok);
    exportManyResolve = null;
  }

  // 모달(doExport)과 명령(execExport) 양쪽이 여기로 모인다 — 이미지 포맷에서 페이지 스펙이
  // 없으면 "전체 페이지"가 아니라 "현재 페이지 한 장"이 기본값이다. 그렇지 않으면 :export a.png
  // 처럼 스펙을 깜빡했을 때 문서 전체가 페이지마다 파일 하나씩 통째로 쏟아진다. 페이지 수가
  // 많으면(5장 초과) merge 여부와 상관없이 확인창을 띄운다 — 합치기도 페이지가 많으면 렌더링/
  // 합성 부하가 커서 시스템이 멎을 수 있다.
  // 반환값 false는 사용자가 확인창에서 취소했다는 뜻 — 호출부는 "저장 완료" 메시지를 건너뛴다.
  async function runExport(outPath, format, pageSpec, merge = false, password = null) {
    if (!pageSpec) {
      // 페이지 스펙을 안 밝혔으면(모달 필드도 비어있고, :export 명령에도 스펙이 없으면)
      // 썸네일에서 여러 페이지를 선택해뒀는지부터 본다 — 포맷 상관없이 선택이 우선.
      // 선택이 없으면 기존처럼 이미지 포맷만 현재 페이지 한 장(PDF는 전체 문서).
      pageSpec = defaultExportSpec();
      if (!pageSpec && format !== 'pdf') pageSpec = String(currentPage + 1);
    }
    // PDF는 합치면(merge) 그래프트 한 번이라 페이지가 많아도 가볍지만, 안 합치면 이미지처럼
    // 페이지마다 파일이 생기니 똑같이 파일 개수 확인이 필요하다.
    const n = pageSpec ? countSpecPages(pageSpec) : pageCount;
    const manyFiles = format === 'pdf' ? (!merge && n > 5) : (n > 5);
    if (manyFiles && !(await confirmExportMany(n, merge))) return false;
    await invoke('export_document', { outPath, format, pageSpec, merge, password });
    return true;
  }

  // 파일 메뉴 "내보내기" — 포맷(png/jpg/pdf), 파일명, 페이지 스펙(비우면 전체 문서, 이미지는
  // 페이지마다 별도 파일)을 지정해 현재 문서를 내보낸다. 원본 문서와 같은 폴더에 저장하고,
  // 네이티브 저장 다이얼로그가 자동으로 해주던 "이미 있는 파일" 경고는 직접 확인한다.
  async function doExport() {
    const spec = exportPageSpec.trim();
    if (spec && !PAGE_SPEC_RE.test(spec)) {
      exportError = t('export.bad_page_spec');
      return;
    }
    const filename = exportFilename.trim();
    if (!filename) {
      exportError = t('export.no_filename');
      return;
    }
    exportError = '';
    const target = dirName(filePath) + filename + '.' + exportFormat;
    // merge를 껐고 페이지가 여러 장이면 파일마다 번호가 붙어 target 이름 그대로는 안 쓰이므로
    // (export_document 참고) 그 경우엔 덮어쓰기 확인이 필요 없다.
    const n = spec ? countSpecPages(spec) : pageCount;
    const willUseExactFilename = exportMerge || n <= 1;
    if (willUseExactFilename && await invoke('path_exists', { path: target })) {
      const ok = await confirm(t('status.export_overwrite_confirm', { name: baseName(target) }), { title: 'Vimong', kind: 'warning' });
      if (!ok) return;
    }
    exportBusy = true;
    try {
      const password = exportFormat === 'pdf' ? (exportPassword.trim() || null) : null;
      if (await runExport(target, exportFormat, spec || null, exportMerge, password)) {
        statusMsg = t('status.export_done', { path: target });
        showExportModal = false;
      }
    } catch (e) {
      exportError = String(e);
    } finally {
      exportBusy = false;
    }
  }

  function cancelExport() {
    showExportModal = false;
  }

  // ":export [-m] [pagespec] <path>" — :w와 같은 인자 문법, 포맷은 경로 확장자로 정한다.
  // -m은 여러 페이지를 파일 하나로 합친다(:w -p의 -p와 같은 자리).
  async function execExport(arg) {
    arg = arg.trim();
    // -m(합치기)과 -p(암호, pdf 전용)는 순서 상관없이 앞에 붙는다
    let merge = false, withPassword = false;
    for (;;) {
      const mFlag = /^-m\s+/.exec(arg);
      if (mFlag) { merge = true; arg = arg.slice(mFlag[0].length); continue; }
      const pFlag = /^-p\s+/.exec(arg);
      if (pFlag) { withPassword = true; arg = arg.slice(pFlag[0].length); continue; }
      break;
    }
    if (!arg) { statusMsg = t('status.export_no_path'); return; }
    const sp = arg.indexOf(' ');
    let pageSpec = null, pathArg = arg;
    if (sp !== -1 && PAGE_SPEC_RE.test(arg.slice(0, sp))) {
      pageSpec = arg.slice(0, sp);
      pathArg = arg.slice(sp + 1);
    }
    // 경로 자리에 "-x " 형태가 남아있으면 인식 못 하는 옵션을 조용히 파일명으로 삼키는 대신
    // 에러를 낸다 — 안 그러면 예: ":export -m 1-10 -x kkk.png"가 "-x kkk.png"라는 파일을
    // 만들어버린다(지원 옵션 -m/-p는 위에서 이미 소비되고 남았다면 인식 못 하는 옵션이다).
    const strayFlag = /^(-\S+)(?:\s|$)/.exec(pathArg);
    if (strayFlag) { statusMsg = t('status.export_unknown_flag', { flag: strayFlag[1] }); return; }
    const outPath = parsePathArg(pathArg);
    if (!outPath) { statusMsg = t('status.export_no_path'); return; }
    const ext = outPath.split('.').pop().toLowerCase();
    const format = ext === 'jpg' || ext === 'jpeg' ? 'jpg' : ext === 'png' ? 'png' : ext === 'pdf' ? 'pdf' : null;
    if (!format) { statusMsg = t('status.export_bad_format', { ext }); return; }
    if (withPassword && format !== 'pdf') { statusMsg = t('status.export_password_pdf_only'); return; }
    if (withPassword) {
      pendingWrite = { outPath, pageSpec, merge, format, kind: 'export' };
      writePassword1 = ''; writePassword2 = ''; writePasswordError = '';
      showWritePasswordPrompt = true;
      return;
    }
    try {
      if (await runExport(outPath, format, pageSpec, merge)) {
        statusMsg = t('status.export_done', { path: outPath });
      }
    } catch (e) {
      statusMsg = t('status.export_error', { e });
    }
  }

  // ":md code" / ":md preview" / ":md split" (짧게 ":md c/p/s"도 허용) — gc/gp/gs와 동일한 동작.
  // ":md theme [이름]" — 이름 없이 쓰면 MD_THEMES 목록을 순환, 있으면 그 테마로 바로 지정.
  const MD_VIEW_ALIASES = { code: 'code', c: 'code', preview: 'preview', p: 'preview', split: 'split', s: 'split' };
  async function execMdCommand(arg) {
    if (!mdMode) { statusMsg = t('status.no_document'); return; }
    const [verb, ...rest] = arg.split(/\s+/);
    if (verb === 'theme') {
      const name = rest.join(' ').trim();
      if (!name) { setMdTheme(MD_THEMES[(MD_THEMES.indexOf(mdTheme) + 1) % MD_THEMES.length]); return; }
      if (!MD_THEMES.includes(name)) { statusMsg = t('status.unknown_command', { cmd: `md theme ${name}` }); return; }
      setMdTheme(name);
      return;
    }
    const target = MD_VIEW_ALIASES[verb];
    if (!target) { statusMsg = t('status.unknown_command', { cmd: `md ${arg}` }); return; }
    setMdViewMode(target);
  }

  // ":view {mode}" — [보기] 메뉴의 다섯 레이아웃을 명령으로 전환. 1/2/h는 짧은 별칭.
  const VIEW_MODE_ALIASES = {
    single: 'single', '1': 'single',
    'single-continuous': 'single-continuous', '1c': 'single-continuous',
    two: 'two', '2': 'two',
    'two-continuous': 'two-continuous', '2c': 'two-continuous',
    'horizontal-continuous': 'horizontal-continuous', horizontal: 'horizontal-continuous', h: 'horizontal-continuous',
  };

  async function execCommand(cmd) {
    cmd = cmd.trim();
    if (cmd === 'q' || cmd === 'tabclose' || cmd === 'tabc') {
      await closeTab(currentTabIdx);
    } else if (cmd === 'tabnew' || cmd === 'tabe') {
      await newTab();
    } else if (cmd.startsWith('tabnew ')) {
      await newTab(parsePathArg(cmd.slice(7)));
    } else if (cmd.startsWith('tabe ')) {
      await newTab(parsePathArg(cmd.slice(5)));
    } else if (cmd === 'tabonly' || cmd === 'tabo') {
      await requestCloseOtherTabs();
    } else if (cmd === 'tabreopen' || cmd === 'tabre') {
      await reopenClosedTab();
    } else if (cmd === 'tabmove' || cmd === 'tabm') {
      await reorderTab(currentTabIdx, tabLabels.length - 1);
    } else if (cmd.startsWith('tabmove ') || cmd.startsWith('tabm ')) {
      await execTabMove(cmd.slice(cmd.indexOf(' ') + 1).trim());
    } else if (cmd === 'e') {
      const path = await open({ filters: [{ name: 'Documents', extensions: ALL_EXTENSIONS }] });
      if (path) await openFile(path);
    } else if (cmd.startsWith('e ')) {
      await openFile(parsePathArg(cmd.slice(2)));
    } else if (cmd === 'w') {
      await saveDocument();
    } else if (cmd.startsWith('w ')) {
      await execWrite(cmd.slice(2));
    } else if (cmd === 'export') {
      statusMsg = t('status.export_no_path');
    } else if (cmd.startsWith('export ')) {
      await execExport(cmd.slice(7));
    } else if (cmd === 'nohlsearch' || cmd === 'noh') {
      hlsearchOn = false;
    } else if (cmd === '?') {
      shortcutsTab = 'nav';
      resetModalNav();
      showShortcuts = true;
    } else if (cmd === 'cmd') {
      commandsTab = 'file';
      resetModalNav();
      showCommands = true;
    } else if (cmd === 'oss') {
      resetModalNav();
      showOpenSource = true;
    } else if (cmd === 'google' || cmd === 'g') {
      await googleSearch(selection ? selectionText() : '');
    } else if (cmd.startsWith('google ')) {
      await googleSearch(cmd.slice(7));
    } else if (cmd.startsWith('g ')) {
      await googleSearch(cmd.slice(2));
    } else if (cmd === 'attachments' || cmd === 'att') {
      if (attachments.length === 0) { statusMsg = t('status.no_attachments'); return; }
      sidebarVisible = true;
      await switchSidebarView('attachments');
    } else if (cmd === 'docinfo') {
      if (!filePath) return;
      showFileInfo = true;
      await loadDocInfo();
    } else if (cmd === 'print' || cmd === 'p') {
      await printDocument();
    } else if (cmd === 'images') {
      await exportPageImages();
    } else if (cmd.startsWith('md ')) {
      await execMdCommand(cmd.slice(3).trim());
    } else if (cmd.startsWith('view ')) {
      const target = VIEW_MODE_ALIASES[cmd.slice(5).trim()];
      if (!target) { statusMsg = t('status.unknown_command', { cmd }); return; }
      await setViewMode(target);
    } else if (cmd === 'font') {
      await showSelectionFont();
    } else if (cmd === 'set' || cmd === 'settings') {
      openSettings();
    } else if (cmd.startsWith('set lang=')) {
      setLangPref(cmd.slice(9).trim());
    } else {
      statusMsg = t('status.unknown_command', { cmd });
    }
  }

  async function googleSearch(query) {
    query = query.trim();
    if (!query) { statusMsg = t('status.google_no_text'); return; }
    await openUrl(`https://www.google.com/search?q=${encodeURIComponent(query)}`);
  }

  // ── Search input keydown (status bar <input> 전용) ────────
  function handleSearchInputKey(e) {
    // handleCmdInputKey와 같은 이유 — mode를 되돌리기 전에 버블링을 끊는다.
    e.stopPropagation();
    if (e.key === 'Escape') { e.preventDefault(); mode = 'normal'; searchQuery = ''; e.target.blur(); }
    else if (e.key === 'Enter' && !e.isComposing) {
      e.preventDefault();
      if (searchQuery) { searchHistory.push(searchQuery); runSearch(searchQuery); }
      mode = 'normal';
      e.target.blur();
    }
    else if (e.key === 'ArrowUp') { e.preventDefault(); searchQuery = searchHistory.up(searchQuery); }
    else if (e.key === 'ArrowDown') { e.preventDefault(); searchQuery = searchHistory.down(searchQuery); }
    else if ((e.metaKey || e.ctrlKey) && e.key === 'v') {
      // handleCmdInputKey와 같은 이유 — 이 입력창에서도 네이티브 붙여넣기가 안 먹는다
      e.preventDefault();
      clipboardRead().then(text => { if (text) searchQuery += text.trimEnd(); });
    }
    // ⌘⇧] / ⌘⇧[ 탭 전환 — 위 stopPropagation 때문에 전역 핸들러까지 안 올라가서 이 입력창에
    // 포커스가 있는 동안엔 안 먹혔다. handleKeydown의 같은 분기와 동일하게 처리한다.
    else if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'BracketRight') {
      e.preventDefault(); switchToTab((currentTabIdx + 1) % tabLabels.length);
    }
    else if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.code === 'BracketLeft') {
      e.preventDefault(); switchToTab((currentTabIdx - 1 + tabLabels.length) % tabLabels.length);
    }
  }

  async function runSearch(query) {
    hlsearchOn = true;
    lastSearch = query;
    searchError = '';
    if (hwpMode) { await runHwpSearch(query); return; } // \c/\C/\r/\R, vim magic 정규식(\zs\ze 포함) 지원 — runHwpSearch 참고
    if (mdMode) { await runMdSearch(query); return; }
    try {
      const { results, case_sensitive, regex } = await invoke('search_all', { query, caseSensitive });
      caseSensitive = case_sensitive; // \c / \C 오버라이드를 Aa 아이콘에도 반영
      regexMode = regex; // \r / \R 오버라이드를 .* 아이콘에도 반영
      allSearchResults = results;
      if (results.length > 0) {
        searchResultIdx = 0;
        sidebarView = 'search';
        const totalHits = results.reduce((s, r) => s + r.hit_count, 0);
        statusMsg = `/${query}  [${totalHits} hit${totalHits > 1 ? 's' : ''} on ${results.length} page${results.length > 1 ? 's' : ''}]`;
        await gotoPage(results[0].page);
        // 검색 직후 바로 j/k로 결과를 훑을 수 있게 사이드바 첫 결과에 포커스 —
        // allSearchResults가 방금 바뀌어서 검색결과 DOM(searchResultEls)이 아직 그
        // 전 상태일 수 있으니 렌더링이 실제로 반영될 때까지 기다린 뒤에 포커스한다
        await tick();
        focusNearestItem(searchResultEls, 0);
      } else {
        pageHighlights = [];
        statusMsg = `Pattern not found: ${query}`;
      }
    } catch (e) {
      allSearchResults = [];
      searchError = String(e);
      statusMsg = `Search error: ${e}`;
    }
  }
</script>

<svelte:window
  onkeydown={handleKeydown}
  onkeyup={(e) => {
    // z를 떼는 순간 "z-prefix 세션"을 확실히 끝낸다 — 안 그러면 다음에 완전히 새로 누르는
    // z가 이전 세션이 남긴 pendingPrefix='z' 때문에 zz(높이 맞춤)의 두 번째 z로 오인된다.
    if (e.key === 'z') { zHeld = false; if (pendingPrefix === 'z') pendingPrefix = ''; }
  }}
  onmousemove={(e) => { onSidebarResizeMove(e); onSelectionDragMove(e); }}
  onmouseup={(e) => { stopSidebarResize(); stopSelectionDrag(e); }}
  onpointermove={onTabDragMove}
  onpointerup={endTabDrag}
/>

<!-- ── 탭바 ── -->
<div class="tabbar">
  {#each tabLabels as label, i}
    <div
      class="tabbar-tab"
      class:active={i === currentTabIdx}
      class:dragging={draggingTabIdx === i}
      style={draggingTabIdx === i ? `transform: translateX(${dragTabDeltaX}px)` : ''}
      title={(i === currentTabIdx ? filePath : tabStates[i]?.filePath) || label}
      role="button"
      tabindex="-1"
      onclick={() => { if (!dragTabMoved) switchToTab(i); }}
      onkeydown={(e) => e.key === 'Enter' && switchToTab(i)}
      onpointerdown={(e) => startTabDrag(e, i)}
    >
      <span class="tabbar-tab-name">{label}</span>
      <button
        class="tabbar-close"
        onclick={(e) => { e.stopPropagation(); closeTab(i); }}
        tabindex="-1"
      >×</button>
    </div>
  {/each}
  <button class="tabbar-new" onclick={() => newTab()} tabindex="-1">+</button>
</div>

<!-- ── 툴바 ── -->
{#if toolbarVisible}
<div class="toolbar">
  {#if mdMode}
  <!-- 마크다운 문서는 페이지 주석(하이라이트/메모/도형) 개념이 없어 gc/gp/gs 뷰 전환 버튼만 보여준다 -->
  <button
    class="toolbar-btn"
    class:active={mdViewMode === 'code'}
    onclick={() => setMdViewMode('code')}
    title={t('toolbar.md_code')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="8 6 2 12 8 18" />
      <polyline points="16 6 22 12 16 18" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={mdViewMode === 'preview'}
    onclick={() => setMdViewMode('preview')}
    title={t('toolbar.md_preview')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={mdViewMode === 'split'}
    onclick={() => setMdViewMode('split')}
    title={t('toolbar.md_split')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="3" y="4" width="18" height="16" rx="2" />
      <line x1="12" y1="4" x2="12" y2="20" />
    </svg>
  </button>
  {:else}
  <button
    class="toolbar-btn"
    class:active={toolMode === 'pointer'}
    onclick={() => setToolMode('pointer')}
    title={t('toolbar.pointer')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M18 11V6a2 2 0 0 0-4 0" />
      <path d="M14 10V4a2 2 0 0 0-4 0v2" />
      <path d="M10 10.5V6a2 2 0 0 0-4 0v8" />
      <path d="M18 8a2 2 0 1 1 4 0v6a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={toolMode === 'text'}
    onclick={() => setToolMode('text')}
    title={t('toolbar.text_select')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M9 4h6M12 4v16M9 20h6" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={toolMode === 'highlight'}
    onclick={() => setToolMode('highlight')}
    title={t('toolbar.highlight')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="13" y="2" width="4" height="13" rx="1" transform="rotate(45 15 8.5)" />
      <path d="M4 21h6" stroke-width="3" />
    </svg>
  </button>
  {#if toolMode === 'highlight'}
    <input
      class="toolbar-color"
      type="color"
      value={highlightColor}
      oninput={(e) => saveHighlightColor(e.currentTarget.value)}
      title={t('settings.highlight_color')}
      tabindex="-1"
    />
    <input
      class="toolbar-opacity"
      type="range"
      min="0.1"
      max="1"
      step="0.05"
      value={highlightOpacity}
      oninput={(e) => saveHighlightOpacity(Number(e.currentTarget.value))}
      title={t('toolbar.highlight_opacity')}
      tabindex="-1"
    />
  {/if}
  <button
    class="toolbar-btn"
    class:active={toolMode === 'note'}
    onclick={() => setToolMode('note')}
    title={t('toolbar.note')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4 4h16v12H10l-4 4v-4H4z" />
      <path d="M7 9h10M7 12.5h6" />
    </svg>
  </button>
  {#if toolMode === 'note'}
    <label class="toolbar-swatch" title={t('toolbar.note_color')}>
      <svg width="16" height="16" viewBox="0 0 24 24">
        <rect x="3" y="3" width="18" height="18" rx="4" fill={noteColor} stroke="var(--sh-17)" stroke-width="1" />
      </svg>
      <input type="color" value={noteColor} oninput={(e) => saveNoteColor(e.currentTarget.value)} tabindex="-1" />
    </label>
  {/if}
  <span class="toolbar-sep"></span>
  <button
    class="toolbar-btn"
    class:active={toolMode === 'line'}
    onclick={() => setToolMode('line')}
    title={t('toolbar.line')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4 20L20 4" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={toolMode === 'rect'}
    onclick={() => setToolMode('rect')}
    title={t('toolbar.rect')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="4" y="6" width="16" height="12" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={toolMode === 'rounded_rect'}
    onclick={() => setToolMode('rounded_rect')}
    title={t('toolbar.rounded_rect')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="4" y="6" width="16" height="12" rx="4" />
    </svg>
  </button>
  <button
    class="toolbar-btn"
    class:active={toolMode === 'ellipse'}
    onclick={() => setToolMode('ellipse')}
    title={t('toolbar.ellipse')}
    tabindex="-1"
  >
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <ellipse cx="12" cy="12" rx="9" ry="6" />
    </svg>
  </button>
  {#if shapeMode}
    <!-- 선 색상은 속이 빈 테두리 사각형, 내부 색상은 꽉 찬 사각형 아이콘으로 그려서 한눈에
         구분되게 한다 — 그냥 색칠된 네모 두 개만 나란히 있으면 뭐가 선이고 뭐가 배경인지
         hover해서 title을 봐야만 알 수 있었다. -->
    <label class="toolbar-swatch" title={t('toolbar.shape_stroke_color')}>
      <svg width="16" height="16" viewBox="0 0 24 24">
        <rect x="3" y="3" width="18" height="18" rx="4" fill="none" stroke={shapeStrokeColor} stroke-width="5" />
      </svg>
      <input type="color" value={shapeStrokeColor} oninput={(e) => saveShapeStrokeColor(e.currentTarget.value)} tabindex="-1" />
    </label>
    {#if toolMode !== 'line'}
      <label class="toolbar-swatch" title={t('toolbar.shape_fill_color')}>
        <svg width="16" height="16" viewBox="0 0 24 24">
          <rect x="3" y="3" width="18" height="18" rx="4" fill={shapeFillColor} stroke="var(--sh-17)" stroke-width="1" />
        </svg>
        <input type="color" value={shapeFillColor} oninput={(e) => saveShapeFillColor(e.currentTarget.value)} tabindex="-1" />
      </label>
    {/if}
    <input
      class="toolbar-stroke-width"
      type="number"
      min="0.5"
      max="20"
      step="0.5"
      value={shapeStrokeWidth}
      oninput={(e) => saveShapeStrokeWidth(Number(e.currentTarget.value))}
      title={t('toolbar.shape_stroke_width')}
      tabindex="-1"
    />
    <input
      class="toolbar-opacity"
      type="range"
      min="0.1"
      max="1"
      step="0.05"
      value={shapeOpacity}
      oninput={(e) => saveShapeOpacity(Number(e.currentTarget.value))}
      title={t('toolbar.shape_opacity')}
      tabindex="-1"
    />
  {/if}
  {/if}
</div>
{/if}

<div class="app">
  <!-- ── Thumbnail sidebar ── -->
  {#if pageCount > 0 && sidebarVisible}
    <div class="sidebar" bind:this={sidebarEl} tabindex="-1" style="width:{sidebarWidth}px">
      <div class="sidebar-header">
        {#if !mdMode}
        <button
          class="sidebar-tab-btn"
          class:active={sidebarView === 'thumbs'}
          onclick={() => switchSidebarView('thumbs')}
          title={t('sidebar.thumbnails')}
          tabindex="-1"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
            <rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" /><rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" />
          </svg>
        </button>
        {/if}
        <button
          class="sidebar-tab-btn"
          class:active={sidebarView === 'toc'}
          disabled={toc.length === 0}
          onclick={() => switchSidebarView('toc')}
          title={toc.length === 0 ? t('sidebar.toc_none') : t('sidebar.toc')}
          tabindex="-1"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
            <path d="M4 6h16M4 12h16M4 18h10" />
          </svg>
        </button>
        <button
          class="sidebar-tab-btn"
          class:active={sidebarView === 'search'}
          onclick={focusSearchTab}
          title={t('sidebar.search')}
          tabindex="-1"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
            <circle cx="11" cy="11" r="7" />
            <path d="M21 21l-4.3-4.3" />
          </svg>
        </button>
        {#if !mdMode}
        <button
          class="sidebar-tab-btn"
          class:active={sidebarView === 'notes'}
          onclick={switchToNotesTab}
          title={t('sidebar.notes')}
          tabindex="-1"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
            <path d="M4 4h16v12H10l-4 4v-4H4z" />
          </svg>
        </button>
        {/if}
        {#if !mdMode && layers.length > 0}
        <button
          class="sidebar-tab-btn"
          class:active={sidebarView === 'layers'}
          onclick={() => switchSidebarView('layers')}
          title={t('sidebar.layers')}
          tabindex="-1"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 3l9 5-9 5-9-5 9-5z" /><path d="M3 13l9 5 9-5" />
          </svg>
        </button>
        {/if}
        {#if !mdMode && attachments.length > 0}
        <button
          class="sidebar-tab-btn"
          class:active={sidebarView === 'attachments'}
          onclick={() => switchSidebarView('attachments')}
          title={t('sidebar.attachments')}
          tabindex="-1"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.3" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 12.5l-8.5 8.5a5 5 0 01-7-7L14 5.5a3.5 3.5 0 015 5L10.5 19" />
          </svg>
        </button>
        {/if}
      </div>

      <div class="sidebar-content">
        {#if sidebarView === 'search'}
          <div class="search-panel-header">
            <input
              use:focusOnSearchMount
              bind:this={searchInputEl}
              bind:value={searchQuery}
              class="search-input sidebar-search-input"
              onkeydown={handleSearchInputKey}
              onfocus={() => mode = 'search'}
              onblur={() => { if (mode === 'search') mode = 'normal'; }}
              autocomplete="off"
              spellcheck="false"
              placeholder={t('search.placeholder')}
            />
            <button
              class="case-toggle"
              class:active={caseSensitive}
              onclick={() => { caseSensitive = !caseSensitive; if (lastSearch) runSearch(searchQuery || lastSearch); }}
              title={t('search.case_title')}
              tabindex="-1"
            >Aa</button>
            <button
              class="case-toggle"
              class:active={regexMode}
              onclick={() => { regexMode = !regexMode; if (lastSearch) runSearch(searchQuery || lastSearch); }}
              title={t('search.regex_title')}
              tabindex="-1"
            >.*</button>
            {#if lastSearch}
              <button onclick={() => { allSearchResults = []; mdSearchResults = []; pageHighlights = []; lastSearch = ''; searchQuery = ''; searchError = ''; searchInputEl?.focus(); }} title={t('search.clear')}>✕</button>
            {/if}
          </div>
          {#if searchError}
            <div class="search-empty-hint">{t('search.error_prefix', { msg: searchError })}</div>
          {:else if lastSearch && (mdMode ? mdSearchResults.length === 0 : allSearchResults.length === 0)}
            <div class="search-empty-hint">{t('search.no_results', { q: lastSearch })}</div>
          {/if}
          {#if mdMode}
            {#each mdSearchResults as hit, i}
              {@const ctx = mdHitContext(hit)}
              <div
                class="search-result-item md-search-result"
                class:active={i === searchResultIdx}
                bind:this={searchResultEls[i]}
                role="button"
                tabindex="-1"
                onclick={() => { searchResultIdx = i; jumpToMdSearchHit(i); }}
                onkeydown={(e) => e.key === 'Enter' && (searchResultIdx = i, jumpToMdSearchHit(i))}
              >
                <span class="md-result-snippet">{ctx.before}<mark>{ctx.match}</mark>{ctx.after}</span>
              </div>
            {/each}
          {:else}
            {#each allSearchResults as result, i}
              <div
                class="search-result-item"
                class:active={result.page === currentPage}
                bind:this={searchResultEls[i]}
                role="button"
                tabindex="-1"
                onclick={() => { searchResultIdx = flatSearchMatches().findIndex(m => m.page === result.page); gotoPage(result.page); }}
                onkeydown={(e) => e.key === 'Enter' && (searchResultIdx = flatSearchMatches().findIndex(m => m.page === result.page), gotoPage(result.page))}
              >
                <span class="result-page">p. {result.page + 1}</span>
                <span class="result-count">{t('search.hits', { n: result.hit_count })}</span>
              </div>
            {/each}
          {/if}
        {:else if sidebarView === 'notes'}
          {#if allNotes.length === 0}
            <div class="search-empty-hint">{t('sidebar.notes_none')}</div>
          {/if}
          {#each allNotes as note, i}
            <div
              class="search-result-item"
              class:active={note.page === currentPage}
              bind:this={notesResultEls[i]}
              role="button"
              tabindex="-1"
              onclick={() => gotoPage(note.page)}
              onkeydown={(e) => e.key === 'Enter' && gotoPage(note.page)}
            >
              <span class="result-page">p.<span class="note-page-num">{note.page + 1}</span></span>
              <span class="note-snippet">{note.contents.trim() || t('note.empty_snippet')}</span>
            </div>
          {/each}
        {:else if sidebarView === 'layers'}
          {#each flatLayers as layer, i (layer.id)}
            <div
              class="search-result-item layer-item"
              style="padding-left:{6 + layer.depth * 14}px"
              role="button"
              tabindex="-1"
              bind:this={layersResultEls[i]}
              onclick={() => toggleOptionalContentLayer(layer.xref)}
              onkeydown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleOptionalContentLayer(layer.xref); } }}
            >
              {#if layer.hasChildren}
                <span
                  class="toc-caret"
                  role="button"
                  tabindex="-1"
                  onclick={(e) => { e.stopPropagation(); toggleLayerNode(layer.id); }}
                  onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); toggleLayerNode(layer.id); } }}
                >{collapsedLayers.has(layer.id) ? '▸' : '▾'}</span>
              {:else}
                <span class="toc-caret-spacer"></span>
              {/if}
              <input type="checkbox" checked={layer.enabled} tabindex="-1" style="pointer-events:none" />
              <span class="note-snippet">{layer.name || `#${layer.xref}`}</span>
            </div>
          {/each}
        {:else if sidebarView === 'attachments'}
          {#each attachments as name, i (i)}
            <div class="search-result-item" tabindex="-1" bind:this={attachmentsResultEls[i]}>
              <span class="note-snippet">{name}</span>
            </div>
          {/each}
        {:else if sidebarView === 'toc'}
          {#each flatToc as item, i (item.id)}
            <div
              class="toc-item"
              class:toc-current={item.id === currentTocItem?.id}
              class:toc-selected={item.id === currentTocItem?.id && item.id === selectedTocId}
              style="padding-left:{6 + item.depth * 14}px"
              role="button"
              tabindex="-1"
              bind:this={tocItemEls[i]}
              onclick={() => gotoTocItem(item)}
              onkeydown={(e) => { if (e.key === 'Enter') gotoTocItem(item); }}
              oncontextmenu={(e) => onTocContextMenu(e, item)}
            >
              {#if item.hasChildren}
                <span
                  class="toc-caret"
                  role="button"
                  tabindex="-1"
                  onclick={(e) => { e.stopPropagation(); toggleTocNode(item.id); }}
                  onkeydown={(e) => { if (e.key === 'Enter') { e.stopPropagation(); toggleTocNode(item.id); } }}
                >{collapsed.has(item.id) ? '▸' : '▾'}</span>
              {:else}
                <span class="toc-caret-spacer"></span>
              {/if}
              <span class="toc-title">{item.title}</span>
              {#if item.page !== null}<span class="toc-page">{item.page + 1}</span>{/if}
            </div>
          {/each}
        {:else}
          {#each thumbnails as thumb, i}
            <div
              class="thumb-item"
              class:active={i === currentPage}
              class:selected={selectedThumbs.has(i)}
              bind:this={thumbEls[i]}
              use:lazyThumb={i}
              role="button"
              tabindex="-1"
              onclick={(e) => onThumbClick(e, i)}
              onkeydown={(e) => e.key === 'Enter' && gotoPage(i)}
              oncontextmenu={(e) => onThumbContextMenu(e, i)}
            >
              {#if thumb.loaded}
                <img src={thumb.src} alt="page {i + 1}" />
              {:else}
                <div class="thumb-placeholder" style="height:{thumbPlaceholderHeight}px"></div>
              {/if}
              <span class="thumb-num">{i + 1}</span>
            </div>
          {/each}
        {/if}
      </div>
    </div>
    <div
      class="sidebar-resizer"
      role="separator"
      aria-orientation="vertical"
      tabindex="-1"
      onmousedown={startSidebarResize}
    ></div>
  {/if}

  {#snippet mainPage(i)}
    {@const [pw, ph] = pageDims(i)}
    <div class="page-wrap" bind:this={pageWrapEls[i]} use:lazyMainPage={i}
         style="width:{pw * scale}px;height:{ph * scale}px;cursor:{pendingMouseDown?.page === i ? 'wait' : toolMode === 'note' ? (hoverNote ? 'pointer' : 'crosshair') : shapeMode ? 'crosshair' : textSelectMode ? 'text' : (isInteractiveField(hoverField) ? 'pointer' : hoverLink ? 'pointer' : 'default')}"
         onmousedown={(e) => onPageMouseDown(e, i)} ondblclick={(e) => onPageDoubleClick(e, i)}
         oncontextmenu={(e) => onPageContextMenu(e, i)}
         onmousemove={(e) => onPageMouseMove(e, i)} onmouseleave={onPageMouseLeave} onclick={(e) => onPageClick(e, i)}>
      <canvas bind:this={pageCanvasRefs[i]} aria-label="page {i + 1}"></canvas>
      {#if getPageRotation(i) === 0}
        <!-- 검색/링크/텍스트 선택 좌표는 회전 없는 페이지 기준이라, 돌아간 화면 위에 그대로
             올리면 안 맞는다 — 그래서 회전 중엔 이 오버레이들을 아예 안 그린다 -->
        {#if selection?.page === i}
          {#each selectionRects as r}
            <div class="highlight selecting" style="left:{r[0]*scale}px;top:{r[1]*scale}px;width:{(r[2]-r[0])*scale}px;height:{(r[3]-r[1])*scale}px"></div>
          {/each}
        {/if}
        {#if shapeDrag?.page === i}
          {#if toolMode === 'line'}
            <svg class="shape-drag-line" style="width:{pw*scale}px;height:{ph*scale}px">
              <line x1={shapeDrag.x0*scale} y1={shapeDrag.y0*scale} x2={shapeDrag.x1*scale} y2={shapeDrag.y1*scale}
                    stroke={shapeStrokeColor} stroke-width={shapeStrokeWidth*scale} opacity={shapeOpacity} />
            </svg>
          {:else if toolMode === 'ellipse'}
            <!-- CSS border-radius로 만든 타원은 가로/세로 반지름에서 각각 선 두께만큼을 그대로
                 빼는 방식이라, 원에서 많이 벗어난 타원일수록 곡률이 급한 위/아래 극점과 완만한
                 옆면에서 실제로 그려지는 선 두께가 서로 달라 보인다(경로에 수직인 두께가
                 일정하지 않음). SVG는 stroke-width를 경로에 수직으로 균일하게 적용해서
                 MuPDF가 실제로 그리는 방식과 일치한다. -->
            {@const [px0, py0, px1, py1] = shapeDrag.shift
              ? constrainSquareDrag(shapeDrag.x0, shapeDrag.y0, shapeDrag.x1, shapeDrag.y1)
              : [shapeDrag.x0, shapeDrag.y0, shapeDrag.x1, shapeDrag.y1]}
            {@const w = Math.abs(px1 - px0)}
            {@const h = Math.abs(py1 - py0)}
            {@const strokeInset = Math.min(shapeStrokeWidth / 2, w / 2, h / 2)}
            <svg class="shape-drag-line" style="width:{pw*scale}px;height:{ph*scale}px">
              <ellipse cx={(px0+px1)/2*scale} cy={(py0+py1)/2*scale}
                       rx={(w/2 - strokeInset)*scale} ry={(h/2 - strokeInset)*scale}
                       fill={shapeFillColor} stroke={shapeStrokeColor} stroke-width={shapeStrokeWidth*scale} opacity={shapeOpacity} />
            </svg>
          {:else}
            {@const [px0, py0, px1, py1] = [shapeDrag.x0, shapeDrag.y0, shapeDrag.x1, shapeDrag.y1]}
            {@const radiusCss = toolMode === 'rounded_rect'
              ? `${roundedRectPreviewRadius(Math.abs(px1 - px0), Math.abs(py1 - py0), shapeStrokeWidth) * scale}px`
              : '0'}
            <div class="shape-drag-preview"
                 style="left:{Math.min(px0,px1)*scale}px;top:{Math.min(py0,py1)*scale}px;width:{Math.abs(px1-px0)*scale}px;height:{Math.abs(py1-py0)*scale}px;border-radius:{radiusCss};border-color:{shapeStrokeColor};border-width:{shapeStrokeWidth*scale}px;background:{shapeFillColor};opacity:{shapeOpacity}"></div>
          {/if}
        {/if}
        {#if textSelectMode && caretRect && cursor?.page === i}
          <div class="text-caret" style="left:{caretRect[0]*scale}px;top:{caretRect[1]*scale}px;height:{(caretRect[3]-caretRect[1])*scale}px"></div>
        {/if}
        {#if i === currentPage && hlsearchOn}
          {#each pageHighlights as r}
            <div class="highlight" class:active={r === activeMatchRect} style="left:{r[0]*scale}px;top:{r[1]*scale}px;width:{(r[2]-r[0])*scale}px;height:{(r[3]-r[1])*scale}px"></div>
          {/each}
        {/if}
        {#if hintMode}
          {#each pageLinks.filter(l => l.srcPage === i) as l}
            <div class="link-hint" style="left:{l.rect[0]*scale}px;top:{l.rect[1]*scale}px">{l.label}</div>
          {/each}
        {/if}
        {#if toolMode === 'pointer'}
          <!-- 클릭 가능한 폼 필드 자리를 표시 — 많은 PDF가 필드 외곽선을 아예 안 그려서
               (인쇄용 디자인 등) 어디를 눌러야 할지 안 보이는 경우가 흔하다. pointer-events:
               none이라 클릭은 그대로 아래 page-wrap의 onclick으로 통과한다. 체크박스/라디오/
               초기화 버튼만 여기 해당 — 텍스트/콤보박스/리스트박스는 실제 폼 엘리먼트를
               얹으므로(아래) 박스가 따로 필요 없다. -->
          {#each (pageFieldCache.get(i) ?? []).filter(isInteractiveField) as f (f.xref)}
            <div class="field-box" class:hovered={f === hoverField}
                 style="left:{f.rect[0]*scale}px;top:{f.rect[1]*scale}px;width:{(f.rect[2]-f.rect[0])*scale}px;height:{(f.rect[3]-f.rect[1])*scale}px"></div>
          {/each}
          <!-- 텍스트 필드 — 브라우저 폼처럼 그 자리에서 바로 입력. 줄바꿈 가능한 필드만
               textarea, 나머지는 input. blur(포커스를 잃을 때)에 커밋한다 — 모달/확인 버튼
               없이 클릭해서 나가면 그대로 저장되는 게 폼다운 동작이라 판단. field-live-input
               클래스는 전역 vim 키 핸들러가 타이핑을 가로채지 않게 막는 표식(위 handleKeydown
               참고). mousedown/click을 막아 페이지의 커서 배치·SyncTeX 클릭으로 새지 않게 한다. -->
          {#each (pageFieldCache.get(i) ?? []).filter(f => !f.readonly && f.kind === 'text') as f (f.xref)}
            {#if f.multiline}
              <textarea class="field-input-inline field-live-input"
                        style="left:{f.rect[0]*scale}px;top:{f.rect[1]*scale}px;width:{(f.rect[2]-f.rect[0])*scale}px;height:{(f.rect[3]-f.rect[1])*scale}px"
                        value={f.value ?? ''}
                        onmousedown={(e) => e.stopPropagation()}
                        onclick={(e) => e.stopPropagation()}
                        onblur={(e) => commitFieldValue(i, f.xref, e.currentTarget.value, f.value)}></textarea>
            {:else if f.comb && f.max_len}
              <!-- comb(글자 하나당 칸 하나, 예: ID) 필드 — 처음엔 letter-spacing 기반 CSS
                   트릭으로 칸을 그렸는데, 한글처럼 라틴 문자보다 넓은 글자가 오면 "1ch" 가정이
                   깨져 칸을 넘쳐버렸다(monospace라도 CJK 글리프는 보통 2배 폭). 그래서 실제
                   칸 div를 max_len개 늘어놓고 각 칸에 글자를 text-align:center로 넣는 방식으로
                   바꿨다 — 글자 폭에 의존하지 않아 어떤 문자든 칸 안에 정확히 들어간다. 입력은
                   투명한 <input>이 이 칸들 위에 그대로 겹쳐서 받는다(caret만 보이고 글자 자신은
                   안 보이게 해서 칸 글자와 겹쳐 보이는 문제를 피함).
                   maxlength만으로는 부족하다 — WebKit 등 대부분의 브라우저가 한글/일본어 같은
                   IME 조합(composition) 중엔 maxlength를 검사하지 않아서, 눈엔 10칸인데 계속
                   입력되는 문제가 있었다(실제로 겪음). 그래서 input/compositionend마다 직접
                   코드포인트 단위로 잘라(String.length는 UTF-16 단위라 서로게이트 페어에서
                   틀릴 수 있어 [...str]로 코드포인트 배열을 씀) value를 강제로 덮어쓴다 —
                   단, 조합이 진행 중(e.isComposing)일 땐 안 자른다(조합 도중 value를 강제로
                   바꾸면 IME 후보 상태가 깨질 수 있어서, 조합이 끝난 직후에만 자른다). -->
              {@const boxW = (f.rect[2]-f.rect[0])*scale/f.max_len}
              <div class="field-comb-wrap"
                   style="left:{f.rect[0]*scale}px;top:{f.rect[1]*scale}px;width:{(f.rect[2]-f.rect[0])*scale}px;height:{(f.rect[3]-f.rect[1])*scale}px">
                {#each Array(f.max_len) as _, idx}
                  <div class="field-comb-box">{combChars(f)[idx] ?? ''}</div>
                {/each}
                {#if focusedCombXref === f.xref}
                  <div class="field-comb-caret" style="left:{combCaretIndex(f) * boxW}px"></div>
                {/if}
                <input type="text" class="field-comb-input field-live-input"
                       maxlength={f.max_len}
                       value={f.value ?? ''}
                       onmousedown={(e) => e.stopPropagation()}
                       onclick={(e) => onCombClick(e, f)}
                       onfocus={(e) => onCombFocus(e, f)}
                       oninput={(e) => onCombInput(e, f)}
                       oncompositionend={(e) => onCombCompositionEnd(e, f)}
                       onkeyup={(e) => syncCombCaret(e, f)}
                       onkeydown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); }}
                       onblur={(e) => onCombBlur(i, f, e)} />
              </div>
            {:else}
              <input type="text" class="field-input-inline field-live-input"
                     maxlength={f.max_len ?? undefined}
                     style="left:{f.rect[0]*scale}px;top:{f.rect[1]*scale}px;width:{(f.rect[2]-f.rect[0])*scale}px;height:{(f.rect[3]-f.rect[1])*scale}px"
                     value={f.value ?? ''}
                     onmousedown={(e) => e.stopPropagation()}
                     onclick={(e) => e.stopPropagation()}
                     onkeydown={(e) => { if (e.key === 'Enter') e.currentTarget.blur(); }}
                     onblur={(e) => commitFieldValue(i, f.xref, e.currentTarget.value, f.value)} />
            {/if}
          {/each}
          <!-- 콤보박스/리스트박스는 박스 표시 대신 실제 <select>를 필드 자리에 그대로 얹는다
               — 클릭하면 그 자리에서 바로 OS 드롭다운이 펼쳐지는, 웹 폼과 같은 경험을 준다.
               리스트박스는 size를 선택지 개수만큼 줘서 드롭다운이 아니라 목록이 쭉 펼쳐진
               형태로 보이게 한다(콤보박스는 size 없이 기본 드롭다운). 빈 값("—") 옵션은
               접힌 드롭다운(콤보박스)에서 "아직 안 고름"을 보여주려고 넣은 거라, 이미 전체가
               펼쳐져 보이는 리스트박스에는 불필요해서 뺀다. mousedown/click을 막아 아래
               page-wrap의 커서 배치·SyncTeX 클릭 처리로 새지 않게 한다. -->
          {#each (pageFieldCache.get(i) ?? []).filter(f => !f.readonly && (f.kind === 'combobox' || f.kind === 'listbox')) as f (f.xref)}
            <select class="field-select-inline field-live-input"
                    class:field-listbox-inline={f.kind === 'listbox'}
                    style="left:{f.rect[0]*scale}px;top:{f.rect[1]*scale}px;width:{(f.rect[2]-f.rect[0])*scale}px;height:{(f.rect[3]-f.rect[1])*scale}px"
                    size={f.kind === 'listbox' ? f.options.length : undefined}
                    value={f.value ?? ''}
                    onmousedown={(e) => e.stopPropagation()}
                    onclick={(e) => e.stopPropagation()}
                    onchange={(e) => commitFieldValue(i, f.xref, e.currentTarget.value, f.value)}>
              {#if f.kind === 'combobox'}
                <option value="">—</option>
              {/if}
              {#each f.options as opt}
                <option value={opt.value}>{opt.label}</option>
              {/each}
            </select>
          {/each}
        {/if}
      {/if}
    </div>
  {/snippet}

  <!-- ── Main viewer ── -->
  <div class="main" bind:this={mainEl} tabindex="-1" onwheel={onMainWheel} onscrollend={onMainScrollEnd}>
    {#if filePath}
      <!-- pageVersion이 바뀔 때마다(탭 전환/새 문서 로드) 페이지 canvas를 통째로 새로
           만든다 — 재사용된 canvas가 이전 문서의 그림을 그대로 들고 있는 채 "이미 이
           배율로 그렸다"며 재렌더를 건너뛰는 버그를 막는다. -->
      {#key pageVersion}
        {#if mdMode}
          <!-- 마크다운: 페이지/캔버스 개념 없이 원문을 통째로 렌더링. gc/gp/gs 또는 :md 명령으로 전환 -->
          <Markdown
            bind:this={mdComponentRef}
            source={mdSource}
            mode={mdViewMode}
            searchHits={hlsearchOn ? mdSearchResults : []}
            activeSearchIndex={hlsearchOn ? searchResultIdx : -1}
            theme={mdTheme}
            codeFont={mdCodeFont}
            zoom={mdScale}
          />
        {:else if hwpMode}
          <!-- hwp 최소 뷰어: 전체 페이지를 열 때 미리 SVG로 렌더링해뒀으므로 캔버스/지연 로딩 없이 그대로 나열 -->
          <div class="viewer viewer-continuous">
            {#each hwpPages as svg, i (i)}
              {@const [pw, ph] = hwpPageSize(i)}
              {@const pageRot = getPageRotation(i)}
              {@const swapped = pw > 0 && (pageRot === 90 || pageRot === 270)}
              <div class="page-wrap" bind:this={pageWrapEls[i]}
                   style="zoom:{scale}{swapped ? `;width:${ph}px;height:${pw}px` : ''}">
                <div style="{pw > 0 ? `width:${pw}px;height:${ph}px;` : ''}transform-origin:top left;transform:{pw > 0 ? hwpRotateTransform(pageRot) : 'none'}">
                  {@html i === currentPage && hlsearchOn && pageHighlights.length > 0 ? injectHwpHighlights(svg, pageHighlights, activeMatchRect) : svg}
                </div>
              </div>
            {/each}
          </div>
        {:else if viewMode === 'single'}
          <div class="viewer">
            {#each Array.from({ length: windowHi - windowLo + 1 }, (_, k) => windowLo + k) as i (i)}
              {@render mainPage(i)}
            {/each}
          </div>
        {:else if viewMode === 'two'}
          <div class="viewer viewer-two">
            {#each twoPageIndices as i (i)}
              {@render mainPage(i)}
            {/each}
          </div>
        {:else if viewMode === 'two-continuous'}
          <div class="viewer viewer-continuous">
            {#each pageRows as row (row[0])}
              <div class="page-row">
                {#each row as i}
                  {#if i !== null}{@render mainPage(i)}{/if}
                {/each}
              </div>
            {/each}
          </div>
        {:else}
          <div class="viewer" class:viewer-continuous={viewMode === 'single-continuous'} class:viewer-horizontal={viewMode === 'horizontal-continuous'}>
            {#each Array.from({ length: pageCount }) as _, i (i)}
              {@render mainPage(i)}
            {/each}
          </div>
        {/if}
      {/key}
    {:else if !bootLoading}
      <div class="viewer">
        <div class="welcome">
          <div class="welcome-title">Vimong <span class="welcome-version">v{appVersion}</span></div>
          <div class="welcome-sub">{t('welcome.subtitle')}</div>
          <table class="welcome-help"><tbody>
            <tr><td>:e</td><td>{t('welcome.help_open')}</td></tr>
            <tr><td>/</td><td>{t('welcome.help_search')}</td></tr>
            <tr><td>gg / G</td><td>{t('welcome.help_firstlast')}</td></tr>
            <tr><td>:?</td><td>{t('welcome.help_allshortcuts')}</td></tr>
            <tr><td>:q</td><td>{t('welcome.help_closetab')}</td></tr>
          </tbody></table>
          {#if recentFiles.length > 0}
            <div class="welcome-recent">
              <div class="welcome-recent-title">{t('welcome.recent_title')}</div>
              {#each recentFiles as path, i (path)}
                <div
                  class="welcome-recent-row"
                  class:active={i === recentIndex}
                  onmouseenter={() => recentIndex = i}
                >
                  <button
                    class="welcome-recent-item"
                    onclick={() => openFile(path)}
                    title={path}
                  >{baseName(path)}</button>
                  <button
                    class="welcome-recent-remove"
                    onclick={async (e) => { e.stopPropagation(); await removeRecentFile(path); }}
                    title={t('welcome.recent_remove')}
                  >×</button>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>

</div>

{#if presentationMode && pageCount > 0}
  <div class="presentation-pagenum">{currentPage + 1} / {pageCount}</div>
{/if}

{#if toastMsg}
  <div class="toast" role="status">{toastMsg}</div>
{/if}

<!-- ── Status bar ── -->
<div class="statusbar">
  {#if mode === 'command'}
    {#if cmdCompletion && cmdCompletion.candidates.length > 0}
      <div class="cmd-suggest">
        {#each cmdCompletion.candidates as c, i (c)}
          <div
            class="cmd-suggest-item"
            class:active={i === cmdCompletion.idx}
            bind:this={cmdSuggestEls[i]}
            onmousedown={(e) => e.preventDefault()}
            onclick={() => pickCmdCompletion(i)}
          >{c}</div>
        {/each}
      </div>
    {/if}
    <span class="cmd-slash">:</span>
    <input
      use:focusOnMount
      bind:value={cmdInput}
      class="search-input"
      onkeydown={handleCmdInputKey}
      oninput={() => { cmdCompletion = null; }}
      autocomplete="off"
      spellcheck="false"
    />
  {:else}
    <span>{statusMsg}</span>
    {#if mdMode}
      <span class="pagenum">{Math.round(mdScale * 100)}%</span>
    {:else if pageCount > 0}
      <!-- EPUB/HTML의 확대/축소(zi/zo)는 scale이 아니라 폰트 크기(em)를 바꾼다. EPUB의 CSS 단위는
           대부분 상대(em/%)라 "파일에 정해진 기본 크기"라는 게 없고, 그 기준값(1em이 몇 pt인지)은
           항상 리딩 앱이 정한다(브라우저의 "기본 글자 크기 16px = 확대 100%"와 같은 개념) — 이
           앱의 기준은 DEFAULT_EPUB_EM(12pt)이므로 그 대비 비율로 보여준다. zh/zv/zz(맞춤)는 이
           폰트 크기와 무관하게 scale만 바꾸는 별개 기능이라 이 퍼센트와는 안 연동된다.
           디바운스 대기 중이거나 relayout이 실행 중이면 "…"을 붙여 진행 중임을 알 수 있게 한다. -->
      <span class="pagenum">{currentPage + 1} / {pageCount}  {epubReflowable
        ? Math.round((pendingEpubEm ?? epubFontSize) / DEFAULT_EPUB_EM * 100)
        : Math.round(scale * 100)}%{epubReflowable && (pendingEpubEm !== null || reflowRunning) ? '…' : ''}</span>
    {/if}
  {/if}
</div>

<!-- ── 문서 정보 modal ── -->
{#if showFileInfo}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showFileInfo = false}
       onkeydown={(e) => e.key === 'Escape' && (showFileInfo = false)}>
    <div class="modal fileinfo-modal" onclick={(e) => e.stopPropagation()}>
      <div class="tab-toolbar">
        <h2>{t('shortcuts.file.doc_info')}</h2>
        <div class="modal-tabs">
          <button tabindex="-1" class:tab-active={fileInfoTab === 'general'} onclick={() => fileInfoTab = 'general'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><line x1="12" y1="11" x2="12" y2="16"/><circle cx="12" cy="7.5" r="0.9" fill="currentColor" stroke="none"/></svg>
            <span>{t('fileinfo.tab_general')}</span>
          </button>
          {#if fileInfo?.is_pdf || fileInfo?.is_hwp}
            <button tabindex="-1" class:tab-active={fileInfoTab === 'fonts'} onclick={() => fileInfoTab = 'fonts'}>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 18 11 6h2l5 12M8 13h8"/></svg>
              <span>{t('fileinfo.tab_fonts')}</span>
            </button>
          {/if}
        </div>
      </div>

      <div class="modal-body">
        {#if fileInfoTab === 'general'}
          <div class="tab-content" tabindex="0" bind:this={fileInfoScrollEl}>
            {#if fileInfo}
              <table>
                <tbody>
                  <tr><td>{t('fileinfo.name')}</td><td>{fileInfo.file_name}</td></tr>
                  <tr><td>{t('fileinfo.path')}</td><td>{fileInfo.path}</td></tr>
                  <tr><td>{t('fileinfo.pages')}</td><td>{fileInfo.page_count}</td></tr>
                  <tr><td>{t('fileinfo.size')}</td><td>{fileInfo.file_size_kb.toFixed(1)} KB</td></tr>
                  {#if fileInfo.is_pdf}
                    <tr><td>{t('fileinfo.pdf_version')}</td><td>{fileInfo.pdf_version}</td></tr>
                    {#if fileInfo.title}<tr><td>{t('fileinfo.title')}</td><td>{fileInfo.title}</td></tr>{/if}
                    {#if fileInfo.author}<tr><td>{t('fileinfo.author')}</td><td>{fileInfo.author}</td></tr>{/if}
                    {#if fileInfo.subject}<tr><td>{t('fileinfo.subject')}</td><td>{fileInfo.subject}</td></tr>{/if}
                    {#if fileInfo.producer}<tr><td>{t('fileinfo.producer')}</td><td>{fileInfo.producer}</td></tr>{/if}
                    {#if fileInfo.creation_date}<tr><td>{t('fileinfo.created')}</td><td>{formatPdfDate(fileInfo.creation_date)}</td></tr>{/if}
                    {#if fileInfo.mod_date}<tr><td>{t('fileinfo.modified')}</td><td>{formatPdfDate(fileInfo.mod_date)}</td></tr>{/if}
                  {:else if fileInfo.is_hwp}
                    <tr><td>{t('fileinfo.hwp_version')}</td><td>{fileInfo.hwp_version}</td></tr>
                    <tr><td>{t('fileinfo.encrypted')}</td><td>{fileInfo.encrypted ? t('fileinfo.encrypted_yes') : t('fileinfo.encrypted_no')}</td></tr>
                    <tr><td>{t('fileinfo.section_count')}</td><td>{fileInfo.section_count}</td></tr>
                  {/if}
                </tbody>
              </table>
            {:else}
              <div class="font-loading">{t('fileinfo.loading')}</div>
            {/if}
          </div>
        {:else}
          {#if !fileInfo?.is_hwp}
          <div class="font-tab-header">
            {#if fontView === 'list'}
              <button class="analyze-btn" class:analyzing={analyzingFonts}
                disabled={fileFonts.length === 0}
                onclick={async () => {
                  if (analyzingFonts) {
                    await invoke('cancel_font_analysis');
                    return;
                  }
                  if (fontStats.length > 0) {
                    // 같은 문서에 대해 이미 분석한 결과가 있으면 재분석하지 않고 재사용
                    fontView = 'stats';
                    return;
                  }
                  analyzingFonts = true;
                  // 분석 시작 전 화면이 실제로 그려질 시간을 확보 (안 그러면 무거운 작업 때문에 로딩 표시가 그려지기도 전에 묻힘)
                  await new Promise(requestAnimationFrame);
                  const start = performance.now();
                  try {
                    fontStats = await invoke('get_font_stats');
                    // 분석이 너무 빨리 끝나면 로딩 표시가 순간적으로만 보이므로 최소 노출 시간 보장
                    const elapsed = performance.now() - start;
                    if (elapsed < 2000) await new Promise(r => setTimeout(r, 2000 - elapsed));
                    fontView = 'stats';
                  } catch (e) {
                    statusMsg = String(e).includes('취소됨') ? t('status.analysis_cancelled') : `Error: ${e}`;
                  } finally {
                    analyzingFonts = false;
                  }
                }}>
                {analyzingFonts ? t('fonts.analyzing_stop') : t('fonts.analyze')}
              </button>
            {:else}
              <button class="analyze-btn" onclick={() => fontView = 'list'}>{t('fonts.back')}</button>
            {/if}
          </div>
          {/if}

          {#if fontView === 'list'}
            <div class="font-list" tabindex="0" bind:this={fileInfoScrollEl}>
              {#if analyzingFonts}
                <div class="analyzing-banner">
                  <span class="spinner spinner-lg"></span>
                  <span>{t('fonts.analyzing_banner')}</span>
                </div>
              {:else if !fontsLoaded}
                <div class="font-loading">{t('fileinfo.loading')}</div>
              {:else if fileFonts.length === 0}
                <div class="font-loading">{t('fonts.none')}</div>
              {:else}
                {#each fileFonts as f}
                  <div class="font-item">
                    <span class="font-name">{f.name}</span>
                    <span class="font-tags">
                      {#if f.bold}<span class="font-tag">{t('font.bold')}</span>{/if}
                      {#if f.italic}<span class="font-tag">{t('font.italic')}</span>{/if}
                      {#if f.monospaced}<span class="font-tag">{t('font.mono')}</span>{/if}
                    </span>
                  </div>
                {/each}
              {/if}
            </div>
          {:else}
            <div class="font-list" tabindex="0" bind:this={fileInfoScrollEl}>
              {#each fontStats as s}
                <div class="stat-item">
                  <div class="stat-header">
                    <span class="font-name">{s.name}</span>
                    <span class="stat-pct">{s.percent.toFixed(1)}%</span>
                  </div>
                  <div class="stat-bar-bg">
                    <div class="stat-bar-fill" style="width:{s.percent}%"></div>
                  </div>
                  <div class="stat-count">{t('fonts.count_suffix', { n: s.count.toLocaleString() })}</div>
                </div>
              {/each}
            </div>
          {/if}
        {/if}
      </div>

      <button onclick={() => showFileInfo = false}>{t('modal.close')}</button>
    </div>
  </div>
{/if}

<!-- ── 단축키 modal ── -->
{#if showShortcuts}
  <div class="overlay" role="dialog" aria-modal="true"
       onkeydown={(e) => e.key === 'Escape' && (showShortcuts = false)}>
    <div class="modal shortcuts-modal" onclick={(e) => e.stopPropagation()}>
      <div class="tab-toolbar">
        <h2>{t('shortcuts.title')}</h2>
        <div class="modal-tabs">
          <button tabindex="-1" class:tab-active={shortcutsTab === 'nav'}  onclick={() => shortcutsTab = 'nav'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="9"/><path d="M15 9l-2 5-5 2 2-5z"/></svg>
            <span>{t('shortcuts.tab_nav')}</span>
          </button>
          <button tabindex="-1" class:tab-active={shortcutsTab === 'zoom'} onclick={() => shortcutsTab = 'zoom'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="6"/><line x1="10" y1="7" x2="10" y2="13"/><line x1="7" y1="10" x2="13" y2="10"/><line x1="15" y1="15" x2="20" y2="20"/></svg>
            <span>{t('shortcuts.tab_zoom')}</span>
          </button>
          <button tabindex="-1" class:tab-active={shortcutsTab === 'file'}   onclick={() => shortcutsTab = 'file'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5"/></svg>
            <span>{t('shortcuts.tab_file')}</span>
          </button>
          <button tabindex="-1" class:tab-active={shortcutsTab === 'search'} onclick={() => shortcutsTab = 'search'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="6"/><line x1="15" y1="15" x2="20" y2="20"/></svg>
            <span>{t('shortcuts.tab_search')}</span>
          </button>
          <button tabindex="-1" class:tab-active={shortcutsTab === 'markdown'} onclick={() => shortcutsTab = 'markdown'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="M6 15V9l3 3.5L12 9v6"/><path d="M16 9v4m0 0-2-2m2 2 2-2"/></svg>
            <span>{t('settings.tab_markdown')}</span>
          </button>
          <button tabindex="-1" class:tab-active={shortcutsTab === 'tab'}    onclick={() => shortcutsTab = 'tab'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="8" y1="5" x2="8" y2="9"/><line x1="13" y1="5" x2="13" y2="9"/></svg>
            <span>{t('shortcuts.tab_tab')}</span>
          </button>
        </div>
      </div>
      <div class="modal-body">
        <div class="tab-content" bind:this={shortcutsScrollEl}>
          {#if shortcutsTab === 'nav'}
            <table><tbody>
              <tr><td>j / k</td><td>{t('shortcuts.nav.scroll_updown')}</td></tr>
              <tr><td>h / l</td><td>{t('shortcuts.nav.scroll_leftright')}</td></tr>
              <tr><td>⌃D / ⌃U</td><td>{t('shortcuts.nav.half_page')}</td></tr>
              <tr><td>J / K</td><td>{t('shortcuts.nav.next_prev_page')}</td></tr>
              <tr><td>gg</td><td>{t('shortcuts.nav.first_page')}</td></tr>
              <tr><td>ga</td><td>{t('shortcuts.nav.char_value')}</td></tr>
              <tr><td>G</td><td>{t('shortcuts.nav.last_page')}</td></tr>
              <tr><td>nG</td><td>{t('shortcuts.nav.goto_page')}</td></tr>
              <tr><td>m{'{a-z}'}</td><td>{t('shortcuts.nav.mark_set')}</td></tr>
              <tr><td>'{'{a-z}'} / `{'{a-z}'}</td><td>{t('shortcuts.nav.mark_goto')}</td></tr>
              <tr><td>f</td><td>{t('shortcuts.nav.link_hint')}</td></tr>
              <tr><td>H</td><td>{t('shortcuts.nav.highlight')}</td></tr>
              <tr><td>u / ⌘Z</td><td>{t('shortcuts.nav.undo_highlight')}</td></tr>
              <tr><td>{t('shortcuts.nav.synctex_key')}</td><td>{t('shortcuts.nav.synctex')}</td></tr>
              <tr><td>[ / ]</td><td>{t('shortcuts.nav.sidebar_tab')}</td></tr>
              <tr><td>Tab</td><td>{t('shortcuts.nav.sidebar_focus')}</td></tr>
              <tr><td>{t('shortcuts.nav.sidebar_jk_key')}</td><td>{t('shortcuts.nav.sidebar_jk_desc')}</td></tr>
              <tr><td>{t('shortcuts.nav.toc_hl_key')}</td><td>{t('shortcuts.nav.toc_hl_desc')}</td></tr>
              <tr><td>⌘⇧T</td><td>{t('shortcuts.nav.toggle_sidebar')}</td></tr>
              <tr><td>p</td><td>{t('shortcuts.nav.presentation')}</td></tr>
              <tr><td>⌃⌘F</td><td>{t('shortcuts.nav.fullscreen')}</td></tr>
            </tbody></table>
          {:else if shortcutsTab === 'zoom'}
            <table><tbody>
              <tr><td>zi</td><td>{t('shortcuts.zoom.in')}</td></tr>
              <tr><td>zo</td><td>{t('shortcuts.zoom.out')}</td></tr>
              <tr><td>z0</td><td>{t('shortcuts.zoom.reset')}</td></tr>
              <tr><td>zh</td><td>{t('shortcuts.zoom.fit_width')}</td></tr>
              <tr><td>zv / zz</td><td>{t('shortcuts.zoom.fit_height')}</td></tr>
              <tr><td>⌘+</td><td>{t('shortcuts.zoom.in')}</td></tr>
              <tr><td>⌘−</td><td>{t('shortcuts.zoom.out')}</td></tr>
            </tbody></table>
          {:else if shortcutsTab === 'file'}
            <table><tbody>
              <tr><td>⌘W</td><td>{t('shortcuts.file.close_tab')}</td></tr>
              <tr><td>⌘I</td><td>{t('shortcuts.file.doc_info')}</td></tr>
              <tr><td>⌘P</td><td>{t('shortcuts.file.print')}</td></tr>
              <tr><td>⌘⇧E</td><td>{t('shortcuts.file.export')}</td></tr>
              <tr><td>⌘,</td><td>{t('shortcuts.file.settings')}</td></tr>
              <tr><td>{t('shortcuts.file.dd_key')}</td><td>{t('shortcuts.file.dd_desc')}</td></tr>
            </tbody></table>
          {:else if shortcutsTab === 'search'}
            <table><tbody>
              <tr><td>/ / ⌘F</td><td>{t('shortcuts.search.search_key_desc')}</td></tr>
              <tr><td>n / N</td><td>{t('shortcuts.search.next_prev')}</td></tr>
              <tr><td>...\c / ...\C</td><td>{t('shortcuts.search.case_suffix')}</td></tr>
              <tr><td>...\r / ...\R</td><td>{t('shortcuts.search.regex_suffix')}</td></tr>
              <tr><td>.*</td><td>{t('shortcuts.search.regex_toggle')}</td></tr>
            </tbody></table>
            <div class="regex-syntax-title">{t('shortcuts.regex_syntax_title')}</div>
            <table><tbody>
              <tr><td>. * ^ $</td><td>{t('shortcuts.regex.any_char')}</td></tr>
              <tr><td>[...] [^...]</td><td>{t('shortcuts.regex.char_class')}</td></tr>
              <tr><td>\+ \? \=</td><td>{t('shortcuts.regex.quantifiers')}</td></tr>
              <tr><td>\{'{'}n,m{'}'} \{'{'}-...{'}'}</td><td>{t('shortcuts.regex.count')}</td></tr>
              <tr><td>\( \) \|</td><td>{t('shortcuts.regex.group_or')}</td></tr>
              <tr><td>\&lt; \&gt;</td><td>{t('shortcuts.regex.word_boundary')}</td></tr>
              <tr><td>\zs \ze</td><td>{t('shortcuts.regex.match_bounds')}</td></tr>
              <tr><td>\d \D \w \W \s \S</td><td>{t('shortcuts.regex.char_types')}</td></tr>
              <tr><td>\a \l \u \x \o \h</td><td>{t('shortcuts.regex.char_types2')}</td></tr>
              <tr><td>\r / \R</td><td>{t('shortcuts.regex.force_toggle')}</td></tr>
            </tbody></table>
            <div class="search-empty-hint">{t('shortcuts.regex.unsupported')}</div>
          {:else if shortcutsTab === 'markdown'}
            <table><tbody>
              <tr><td>gc / gp / gs</td><td>{t('shortcuts.nav.md_view')}</td></tr>
              <tr><td>:md code / preview / split</td><td>{t('commands.file.md')}</td></tr>
              <tr><td>:md theme {'{name}'}</td><td>{t('commands.file.md_theme')}</td></tr>
            </tbody></table>
          {:else if shortcutsTab === 'tab'}
            <table><tbody>
              <tr><td>⌘T</td><td>{t('shortcuts.tab.new')}</td></tr>
              <tr><td>gt / gT</td><td>{t('shortcuts.tab.next_prev')}</td></tr>
              <tr><td>⌘⇧] / ⌘⇧[</td><td>{t('shortcuts.tab.next_prev')}</td></tr>
              <tr><td>ngt</td><td>{t('shortcuts.tab.goto_n')}</td></tr>
              <tr><td>⌘⇧R</td><td>{t('shortcuts.tab.reopen')}</td></tr>
            </tbody></table>
          {/if}
        </div>
      </div>
      <button onclick={() => showShortcuts = false}>{t('modal.close')}</button>
    </div>
  </div>
{/if}

<!-- ── 명령어 modal ── -->
{#if showCommands}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showCommands = false}
       onkeydown={(e) => e.key === 'Escape' && (showCommands = false)}>
    <div class="modal shortcuts-modal commands-modal" onclick={(e) => e.stopPropagation()}>
      <div class="tab-toolbar">
        <h2>{t('commands.title')}</h2>
        <div class="modal-tabs">
          <button tabindex="-1" class:tab-active={commandsTab === 'file'}   onclick={() => commandsTab = 'file'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M6 3h8l5 5v13a1 1 0 0 1-1 1H6a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1z"/><path d="M14 3v5h5"/></svg>
            <span>{t('shortcuts.tab_file')}</span>
          </button>
          <button tabindex="-1" class:tab-active={commandsTab === 'tab'}    onclick={() => commandsTab = 'tab'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2"/><line x1="3" y1="9" x2="21" y2="9"/><line x1="8" y1="5" x2="8" y2="9"/><line x1="13" y1="5" x2="13" y2="9"/></svg>
            <span>{t('shortcuts.tab_tab')}</span>
          </button>
          <button tabindex="-1" class:tab-active={commandsTab === 'search'} onclick={() => commandsTab = 'search'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><circle cx="10" cy="10" r="6"/><line x1="15" y1="15" x2="20" y2="20"/></svg>
            <span>{t('shortcuts.tab_search')}</span>
          </button>
        </div>
      </div>
      <div class="modal-body">
        <div class="tab-content" bind:this={commandsScrollEl}>
          {#if commandsTab === 'file'}
            <table><tbody>
              <tr><td>:e</td><td>{t('shortcuts.file.open_dialog')}</td></tr>
              <tr><td>:e {'{path}'}</td><td>{t('shortcuts.file.open_path_desc')}</td></tr>
              <tr><td>:w</td><td>{t('commands.file.write_inplace')}</td></tr>
              <tr><td>:w [-p] [pagespec] &lt;path&gt;</td><td>{t('commands.file.write')}</td></tr>
              <tr><td>:docinfo</td><td>{t('shortcuts.file.doc_info')}</td></tr>
              <tr><td>:attachments / :att</td><td>{t('commands.file.attachments')}</td></tr>
              <tr><td>:print</td><td>{t('shortcuts.file.print')}</td></tr>
              <tr><td>:export [-m] [-p] [pagespec] &lt;path&gt;</td><td>{t('shortcuts.file.export')}</td></tr>
              <tr><td>:images</td><td>{t('commands.file.images')}</td></tr>
              <tr><td>:md code / preview / split</td><td>{t('commands.file.md')}</td></tr>
              <tr><td>:md theme [이름]</td><td>{t('commands.file.md_theme')}</td></tr>
              <tr><td>:view {'{mode}'}</td><td>{t('commands.file.view')}</td></tr>
              <tr><td>:font</td><td>{t('commands.file.font')}</td></tr>
              <tr><td>:google [text] / :g [text]</td><td>{t('commands.file.google')}</td></tr>
              <tr><td>:set / :settings</td><td>{t('shortcuts.file.settings')}</td></tr>
              <tr><td>:set lang={'{code}'}</td><td>{t('commands.file.set_lang')}</td></tr>
              <tr><td>:?</td><td>{t('shortcuts.file.open_help')}</td></tr>
              <tr><td>:cmd</td><td>{t('commands.file.open_help')}</td></tr>
              <tr><td>:oss</td><td>{t('commands.file.oss')}</td></tr>
            </tbody></table>
            <div class="regex-syntax-title" bind:this={pagespecSectionEl}>{t('commands.pagespec_title')}</div>
            <table><tbody>
              <tr><td>1</td><td>{t('commands.pagespec.single')}</td></tr>
              <tr><td>1-3</td><td>{t('commands.pagespec.range')}</td></tr>
              <tr><td>1,3</td><td>{t('commands.pagespec.multi')}</td></tr>
              <tr><td>1-3,7-8,20-22</td><td>{t('commands.pagespec.combo')}</td></tr>
            </tbody></table>
          {:else if commandsTab === 'tab'}
            <table><tbody>
              <tr><td>:tabnew / :tabe</td><td>{t('shortcuts.tab.new')}</td></tr>
              <tr><td>:tabnew {'{path}'} / :tabe {'{path}'}</td><td>{t('shortcuts.tab.open_path')}</td></tr>
              <tr><td>:q / :tabclose / :tabc</td><td>{t('shortcuts.file.close_tab')}</td></tr>
              <tr><td>:tabonly / :tabo</td><td>{t('shortcuts.tab.only')}</td></tr>
              <tr><td>:tabmove [N]</td><td>{t('shortcuts.tab.move')}</td></tr>
              <tr><td>:tabreopen / :tabre</td><td>{t('shortcuts.tab.reopen')}</td></tr>
            </tbody></table>
          {:else if commandsTab === 'search'}
            <table><tbody>
              <tr><td>:nohlsearch / :noh</td><td>{t('shortcuts.search.nohlsearch')}</td></tr>
            </tbody></table>
          {/if}
        </div>
      </div>
      <button onclick={() => showCommands = false}>{t('modal.close')}</button>
    </div>
  </div>
{/if}

<!-- ── 오픈소스 modal ── -->
{#if showOpenSource}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showOpenSource = false}
       onkeydown={(e) => e.key === 'Escape' && (showOpenSource = false)}>
    <div class="modal oss-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('oss.title')}</h2>
      <div class="oss-list">
        {#each [
          { name: 'Tauri',                   ver: '2.x',   license: 'MIT / Apache 2.0', url: 'https://tauri.app' },
          { name: 'mupdf (Rust bindings)',   ver: '0.8',   license: 'AGPL 3.0',         url: 'https://github.com/messense/mupdf-rs' },
          { name: 'MuPDF',                   ver: '1.x',   license: 'AGPL 3.0',         url: 'https://mupdf.com' },
          { name: 'base64',                  ver: '0.22',  license: 'MIT / Apache 2.0', url: 'https://github.com/marshallpierce/rust-base64' },
          { name: 'sys-locale',              ver: '0.3',   license: 'MIT / Apache 2.0', url: 'https://github.com/1Password/sys-locale' },
          { name: 'font-kit',                ver: '0.14',  license: 'MIT / Apache 2.0', url: 'https://github.com/servo/font-kit' },
          { name: 'tauri-plugin-dialog',     ver: '2.x',   license: 'MIT / Apache 2.0', url: 'https://github.com/tauri-apps/plugins-workspace' },
          { name: 'tauri-plugin-process',    ver: '2.x',   license: 'MIT / Apache 2.0', url: 'https://github.com/tauri-apps/plugins-workspace' },
          { name: 'tauri-plugin-single-instance', ver: '2.x', license: 'MIT / Apache 2.0', url: 'https://github.com/tauri-apps/plugins-workspace' },
          { name: 'Svelte',                  ver: '5.x',   license: 'MIT',              url: 'https://svelte.dev' },
          { name: 'SvelteKit',               ver: '2.x',   license: 'MIT',              url: 'https://kit.svelte.dev' },
          { name: 'Vite',                    ver: '6.x',   license: 'MIT',              url: 'https://vitejs.dev' },
          { name: '@tauri-apps/api',         ver: '2.x',   license: 'MIT / Apache 2.0', url: 'https://github.com/tauri-apps/tauri' },
          { name: '@tauri-apps/plugin-dialog',  ver: '2.x', license: 'MIT / Apache 2.0', url: 'https://github.com/tauri-apps/plugins-workspace' },
          { name: '@tauri-apps/plugin-process', ver: '2.x', license: 'MIT / Apache 2.0', url: 'https://github.com/tauri-apps/plugins-workspace' },
          { name: 'marked',                  ver: '18.x',  license: 'MIT',              url: 'https://github.com/markedjs/marked' },
          { name: 'highlight.js',            ver: '11.x',  license: 'BSD-3-Clause',     url: 'https://github.com/highlightjs/highlight.js' },
          { name: 'DOMPurify',               ver: '3.x',   license: 'Apache 2.0 / MIT', url: 'https://github.com/cure53/DOMPurify' },
        ] as lib}
          <div class="oss-item">
            <span class="oss-name">{lib.name}</span>
            <span class="oss-ver">{lib.ver}</span>
            <span class="oss-license">{lib.license}</span>
          </div>
        {/each}
      </div>
      <button onclick={() => showOpenSource = false}>{t('modal.close')}</button>
    </div>
  </div>
{/if}

<!-- ── Vimong 정보 modal ── -->
<!-- 네이티브 About 패널 대신 웹뷰 모달로 띄운다 — 버전 텍스트가 보통 DOM 텍스트라
     Cmd+C(copyActiveSelection의 window.getSelection() 폴백)로 그냥 복사된다. -->
{#if showAbout}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showAbout = false}
       onkeydown={(e) => e.key === 'Escape' && (showAbout = false)}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>Vimong</h2>
      <p class="quit-desc">{t('about.version', { version: appVersion })}</p>
      <p class="quit-desc">AGPL-3.0-or-later · © 2026 Textment</p>
      <div class="quit-actions">
        <button use:focusOnMount onclick={() => showAbout = false}>{t('modal.close')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 설정 modal ── -->
{#if showSettings}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showSettings = false}
       onkeydown={(e) => e.key === 'Escape' && (showSettings = false)}>
    <div class="modal settings-modal" use:persistModalSize onclick={(e) => e.stopPropagation()}>
      <div class="tab-toolbar">
        <h2>{t('settings.title')}</h2>
        <div class="modal-tabs">
          <button tabindex="-1" use:focusOnMount class:tab-active={settingsTab === 'ui'} onclick={() => settingsTab = 'ui'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"><line x1="4" y1="7" x2="20" y2="7"/><circle cx="9" cy="7" r="2.1" fill="currentColor" stroke="none"/><line x1="4" y1="12" x2="20" y2="12"/><circle cx="15" cy="12" r="2.1" fill="currentColor" stroke="none"/><line x1="4" y1="17" x2="20" y2="17"/><circle cx="11" cy="17" r="2.1" fill="currentColor" stroke="none"/></svg>
            <span>{t('settings.tab_ui')}</span>
          </button>
          <button tabindex="-1" class:tab-active={settingsTab === 'markdown'} onclick={() => settingsTab = 'markdown'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="M6 15V9l3 3.5L12 9v6"/><path d="M16 9v4m0 0-2-2m2 2 2-2"/></svg>
            <span>{t('settings.tab_markdown')}</span>
          </button>
          <button tabindex="-1" class:tab-active={settingsTab === 'synctex'} onclick={() => settingsTab = 'synctex'}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><path d="M4 12a8 8 0 0 1 14-5.3M20 4v4h-4"/><path d="M20 12a8 8 0 0 1-14 5.3M4 20v-4h4"/></svg>
            <span>{t('settings.tab_synctex')}</span>
          </button>
        </div>
      </div>
      {#if settingsTab === 'ui'}
        <div class="settings-row">
          <label for="settings-lang">{t('settings.language')}</label>
          <select id="settings-lang" bind:value={settingsLangDraft}>
            <option value="auto">{t('settings.language_auto')}</option>
            {#each LANGS as code}
              <option value={code}>{LANG_NAMES[code]}</option>
            {/each}
          </select>
        </div>
        <div class="settings-row">
          <label for="settings-theme">{t('settings.theme')}</label>
          <select id="settings-theme" bind:value={settingsThemeDraft}>
            <option value="auto">{t('settings.theme_auto')}</option>
            <option value="light">{t('settings.theme_light')}</option>
            <option value="dark">{t('settings.theme_dark')}</option>
          </select>
        </div>
        <label class="export-merge-row">
          <input type="checkbox" bind:checked={settingsConfirmCloseTabsDraft} />
          {t('settings.confirm_close_tabs')}
        </label>
        <label class="export-merge-row">
          <input type="checkbox" bind:checked={settingsResumeLastPageDraft} />
          {t('settings.resume_last_page')}
        </label>
        <label class="export-merge-row">
          <input type="checkbox" bind:checked={settingsRestoreSessionDraft} />
          {t('settings.restore_session')}
        </label>
      {:else if settingsTab === 'markdown'}
        <div class="settings-row">
          <label for="settings-md-theme">{t('settings.md_theme')}</label>
          <select id="settings-md-theme" bind:value={settingsMdThemeDraft}>
            {#each MD_THEMES as name}
              <option value={name}>{name === 'auto' ? t('toolbar.md_theme_auto') : MD_THEME_LABELS[name]}</option>
            {/each}
          </select>
        </div>
        <div class="settings-row">
          <label for="settings-md-font">{t('settings.md_code_font')}</label>
          <select id="settings-md-font" bind:value={settingsMdCodeFontDraft} disabled={systemFontsLoading}>
            {#if systemFontsLoading}
              <option value="default">{t('settings.md_code_font_loading')}</option>
            {:else}
              <option value="default">{t('settings.md_code_font_default')}</option>
              {#each systemFonts as name}
                <option value={name}>{name}</option>
              {/each}
            {/if}
          </select>
        </div>
      {:else if settingsTab === 'synctex'}
        <div class="settings-row settings-row-col">
          <label for="settings-synctex-preset">{t('settings.synctex_editor')}</label>
          <div class="synctex-preset-row">
            <select id="settings-synctex-preset" value={settingsSynctexPreset}
              onchange={(e) => applySynctexPreset(e.target.value)}>
              <option value="vscode">Visual Studio Code</option>
              <option value="texshop">TeXShop</option>
              <option value="custom">{t('settings.synctex_editor_custom')}</option>
            </select>
            {#if settingsSynctexPreset === 'texshop'}
              <button type="button" class="synctex-help-btn" onclick={() => showTexshopHelp = true}>{t('settings.texshop_help_open')}</button>
            {/if}
          </div>
          <input id="settings-synctex" type="text" class="password-input"
            bind:value={settingsSynctexDraft} placeholder={t('settings.synctex_editor_placeholder')} />
          <span class="quit-desc">{t('settings.synctex_editor_hint')}</span>
        </div>
      {/if}
      <div class="quit-actions">
        <button onclick={() => showSettings = false}>{t('modal.cancel')}</button>
        <button class="password-confirm-btn" onclick={applySettings}>{t('settings.apply')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── TeXShop 연동 도움말 modal (설정 모달 위에 겹쳐 뜬다) ── -->
{#if showTexshopHelp}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={closeTexshopHelp}
       onkeydown={(e) => e.key === 'Escape' && closeTexshopHelp()}>
    <div class="modal texshop-help-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('settings.texshop_help_title')}</h2>
      <p class="quit-desc">{t('settings.texshop_help_intro')}</p>
      <pre class="texshop-help-cmd">{TEXSHOP_INSTALL_CMD}</pre>
      <p class="quit-desc">{t('settings.texshop_help_outro', { field: SYNCTEX_PRESETS.texshop })}</p>
      <div class="quit-actions">
        <button onclick={closeTexshopHelp}>{t('modal.close')}</button>
        <button class="password-confirm-btn" onclick={copyTexshopInstallCmd}>{t('settings.texshop_help_copy')}</button>
      </div>
      {#if texshopCopyFeedback}
        <div class="texshop-copied-toast">{t('settings.texshop_help_copied')}</div>
      {/if}
    </div>
  </div>
{/if}

<!-- ── 종료 확인 modal ── -->
{#if showQuitConfirm}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showQuitConfirm = false}
       onkeydown={(e) => e.key === 'Escape' && (showQuitConfirm = false)}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('quit.title', { n: quitTabCount })}</h2>
      <p class="quit-desc">{t('quit.desc', { n: quitTabCount })}</p>
      <label class="export-merge-row">
        <input type="checkbox" checked={restoreSession} onchange={(e) => setRestoreSession(e.currentTarget.checked)} />
        {t('quit.restore_tabs')}
      </label>
      <div class="quit-actions">
        <button use:focusOnMount onclick={() => showQuitConfirm = false}>{t('modal.cancel')}</button>
        <button class="quit-confirm-btn" onclick={() => invoke('quit_app')}>{t('quit.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── :tabonly 확인 modal ── -->
{#if showTabOnlyConfirm}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showTabOnlyConfirm = false}
       onkeydown={(e) => e.key === 'Escape' && (showTabOnlyConfirm = false)}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('tabonly.title', { n: tabOnlyCount })}</h2>
      <p class="quit-desc">{t('tabonly.desc', { n: tabOnlyCount })}</p>
      <div class="quit-actions">
        <button use:focusOnMount onclick={() => showTabOnlyConfirm = false}>{t('modal.cancel')}</button>
        <button class="quit-confirm-btn" onclick={async () => { showTabOnlyConfirm = false; await closeOtherTabs(); }}>{t('tabonly.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 저장 안 된 하이라이트 등이 있는 탭을 닫으려 할 때 확인 modal ── -->
{#if showDiscardConfirm}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => { showDiscardConfirm = false; pendingCloseTabIdx = null; }}
       onkeydown={(e) => e.key === 'Escape' && (showDiscardConfirm = false, pendingCloseTabIdx = null)}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('discard.title', { name: pendingCloseTabName })}</h2>
      <p class="quit-desc">{t('discard.desc')}</p>
      <div class="quit-actions">
        <button onclick={() => { showDiscardConfirm = false; pendingCloseTabIdx = null; }}>{t('modal.cancel')}</button>
        <button class="quit-confirm-btn" onclick={async () => {
          showDiscardConfirm = false;
          const idx = pendingCloseTabIdx;
          pendingCloseTabIdx = null;
          await closeTabConfirmed(idx);
        }}>{t('discard.dont_save')}</button>
        <button class="password-confirm-btn" use:focusOnMount onclick={async () => {
          showDiscardConfirm = false;
          const idx = pendingCloseTabIdx;
          pendingCloseTabIdx = null;
          await saveDocument(idx);
          await closeTabConfirmed(idx);
        }}>{t('discard.save')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 앱 종료 직전, 저장 안 된 탭이 있으면 하나씩 확인 modal ── -->
{#if showQuitDiscardConfirm}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => { showQuitDiscardConfirm = false; quitDirtyQueue = []; pendingQuitTabIdx = null; }}
       onkeydown={(e) => e.key === 'Escape' && (showQuitDiscardConfirm = false, quitDirtyQueue = [], pendingQuitTabIdx = null)}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('discard.title', { name: pendingQuitTabName })}</h2>
      <p class="quit-desc">{t('discard.desc')}</p>
      <div class="quit-actions">
        <button onclick={() => { showQuitDiscardConfirm = false; quitDirtyQueue = []; pendingQuitTabIdx = null; }}>{t('modal.cancel')}</button>
        <button class="quit-confirm-btn" onclick={async () => {
          showQuitDiscardConfirm = false;
          quitDirtyQueue.shift();
          pendingQuitTabIdx = null;
          await processNextQuitDirtyTab();
        }}>{t('discard.dont_save')}</button>
        <button class="password-confirm-btn" use:focusOnMount onclick={async () => {
          showQuitDiscardConfirm = false;
          await saveDocument(pendingQuitTabIdx);
          quitDirtyQueue.shift();
          pendingQuitTabIdx = null;
          await processNextQuitDirtyTab();
        }}>{t('discard.save')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 메모 입력/편집 modal ── 예전엔 페이지 클릭 좌표에 바로 뜨는 팝업이었는데, 포커스가
     사이드바 등 다른 곳으로 옮겨간 뒤에는 전역 키 핸들러가 notePending일 때 통째로
     무시해버려서(vim 단축키와 타이핑 충돌 방지용) Esc가 안 먹히는 문제가 있었다. 다른
     확인창들과 똑같이 오버레이+모달로 만들어서 Esc/바깥 클릭 처리를 그 확인창들과
     같은 경로(ESC 통합 처리)로 통일했다. -->
{#if notePending}
  <!-- 다른 확인창들과 달리 바깥(오버레이) 클릭으로는 안 닫는다 — 메모는 입력 중 실수로
       사이드바 등을 클릭했다고 내용이 날아가면 안 되니, Esc나 취소/확인 버튼으로만 닫는다. -->
  <div class="overlay" role="dialog" aria-modal="true" onclick={(e) => e.stopPropagation()}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('toolbar.note')}</h2>
      <textarea
        class="note-textarea"
        bind:value={noteText}
        use:focusOnMount
        placeholder={t('note.placeholder')}
        onkeydown={(e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) confirmNote(); }}
      ></textarea>
      <div class="quit-actions">
        {#if notePending.xref != null}
          <button class="note-delete-btn" onclick={deleteNote}>{t('note.delete')}</button>
        {/if}
        <button onclick={cancelNote}>{t('modal.cancel')}</button>
        <button class="password-confirm-btn" onclick={confirmNote}>{t('note.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 암호 입력 modal ── -->
{#if showPasswordPrompt}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => cancelPassword()}
       onkeydown={(e) => e.key === 'Escape' && cancelPassword()}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('password.protected_title')}</h2>
      <input
        type="password"
        class="password-input"
        bind:value={passwordInput}
        use:focusOnMount
        onkeydown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') submitPassword();
          else if (e.key === 'Escape') cancelPassword();
        }}
        placeholder={t('password.placeholder')}
      />
      {#if passwordError}<p class="password-error">{passwordError}</p>{/if}
      <div class="quit-actions">
        <button onclick={() => cancelPassword()}>{t('modal.cancel')}</button>
        <button class="password-confirm-btn" disabled={!passwordInput} onclick={() => submitPassword()}>{t('password.open')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 암호 설정/제거 modal ── -->
{#if showPasswordManage}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => showPasswordManage = false}
       onkeydown={(e) => e.key === 'Escape' && (showPasswordManage = false)}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <div class="tab-toolbar">
        <h2>{t('export.password')}</h2>
        <div class="modal-tabs">
          <button tabindex="-1" class:tab-active={passwordManageTab === 'set'}
            onclick={() => { passwordManageTab = 'set'; passwordManageError = ''; }}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/></svg>
            <span>{t('password.tab_set')}</span>
          </button>
          <button tabindex="-1" class:tab-active={passwordManageTab === 'remove'}
            onclick={() => { passwordManageTab = 'remove'; passwordManageError = ''; }}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="5" y="11" width="14" height="9" rx="2"/><path d="M8 11V7a4 4 0 0 1 7-2.5"/></svg>
            <span>{t('password.tab_remove')}</span>
          </button>
        </div>
      </div>
      {#if passwordManageTab === 'set'}
        <input
          type="password"
          class="password-input"
          bind:value={newPassword1}
          use:focusOnMount
          onkeydown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter') submitSetPassword();
            else if (e.key === 'Escape') showPasswordManage = false;
          }}
          placeholder={t('password.new')}
        />
        <input
          type="password"
          class="password-input"
          bind:value={newPassword2}
          onkeydown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter') submitSetPassword();
            else if (e.key === 'Escape') showPasswordManage = false;
          }}
          placeholder={t('password.new_confirm')}
        />
        {#if passwordManageError}<p class="password-error">{passwordManageError}</p>{/if}
        <div class="quit-actions">
          <button onclick={() => showPasswordManage = false}>{t('modal.cancel')}</button>
          <button class="password-confirm-btn"
            disabled={!newPassword1 || !newPassword2 || passwordManageBusy}
            onclick={submitSetPassword}>{t('password.tab_set')}</button>
        </div>
      {:else}
        <p class="quit-desc">{t('password.remove_desc')}</p>
        {#if passwordManageError}<p class="password-error">{passwordManageError}</p>{/if}
        <div class="quit-actions">
          <button onclick={() => showPasswordManage = false}>{t('modal.cancel')}</button>
          <button class="password-confirm-btn" disabled={passwordManageBusy} onclick={submitRemovePassword}>{t('password.tab_remove')}</button>
        </div>
      {/if}
    </div>
  </div>
{/if}

<!-- ── :w -p 저장용 암호 입력 modal ── -->
{#if showWritePasswordPrompt}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={cancelWritePassword}
       onkeydown={(e) => e.key === 'Escape' && cancelWritePassword()}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('password.write_title')}</h2>
      <input
        type="password"
        class="password-input"
        bind:value={writePassword1}
        use:focusOnMount
        onkeydown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') submitWritePassword();
          else if (e.key === 'Escape') cancelWritePassword();
        }}
        placeholder={t('password.new')}
      />
      <input
        type="password"
        class="password-input"
        bind:value={writePassword2}
        onkeydown={(e) => {
          e.stopPropagation();
          if (e.key === 'Enter') submitWritePassword();
          else if (e.key === 'Escape') cancelWritePassword();
        }}
        placeholder={t('password.new_confirm')}
      />
      {#if writePasswordError}<p class="password-error">{writePasswordError}</p>{/if}
      <div class="quit-actions">
        <button onclick={cancelWritePassword}>{t('modal.cancel')}</button>
        <button class="password-confirm-btn"
          disabled={!writePassword1 || writePassword1 !== writePassword2 || writePasswordBusy}
          onclick={submitWritePassword}>{t('password.write_confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 파일 메뉴 "내보내기" modal ── -->
{#if showExportModal}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={cancelExport}
       onkeydown={(e) => e.key === 'Escape' && (e.preventDefault(), cancelExport())}>
    <div class="modal quit-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t('export.title')}</h2>
      <div class="settings-row settings-row-col">
        <label for="export-page-spec">{t('export.page_spec')}</label>
        <input
          id="export-page-spec"
          type="text"
          class="password-input"
          bind:value={exportPageSpec}
          use:focusOnMount
          onkeydown={(e) => {
            e.stopPropagation();
            if (e.key === 'Enter' && !e.isComposing) doExport();
            else if (e.key === 'Escape') { e.preventDefault(); cancelExport(); }
          }}
          placeholder={t('export.page_spec_placeholder')}
        />
      </div>
      <div class="settings-row settings-row-col">
        <label for="export-filename">{t('export.filename')}</label>
        <div class="export-filename-row">
          <input
            id="export-filename"
            type="text"
            class="password-input"
            bind:value={exportFilename}
            onkeydown={(e) => {
              e.stopPropagation();
              if (e.key === 'Enter' && !e.isComposing) doExport();
              else if (e.key === 'Escape') { e.preventDefault(); cancelExport(); }
            }}
            placeholder={t('export.filename_placeholder')}
          />
          <select id="export-format" aria-label={t('export.format')} bind:value={exportFormat}>
            <option value="png">PNG</option>
            <option value="jpg">JPG</option>
            <option value="pdf">PDF</option>
          </select>
        </div>
      </div>
      <label class="export-merge-row">
        <input type="checkbox" bind:checked={exportMerge} />
        {t('export.merge')}
      </label>
      {#if exportFormat === 'pdf'}
        <div class="settings-row settings-row-col">
          <label for="export-password">{t('export.password')}</label>
          <input
            id="export-password"
            type="password"
            class="password-input"
            bind:value={exportPassword}
            onkeydown={(e) => {
              e.stopPropagation();
              if (e.key === 'Enter' && !e.isComposing) doExport();
              else if (e.key === 'Escape') { e.preventDefault(); cancelExport(); }
            }}
            placeholder={t('export.password_placeholder')}
          />
        </div>
      {/if}
      <p class="quit-desc">
        {t('pagespec.help_short')}
        <button type="button" class="pagespec-help-btn" onclick={openPageSpecHelp} title={t('pagespec.help_open')}>?</button>
      </p>
      {#if exportError}
        <p class="password-error">{exportError}</p>
      {/if}
      <div class="quit-actions">
        <button onclick={cancelExport}>{t('modal.cancel')}</button>
        <button class="password-confirm-btn" disabled={exportBusy || !exportFilename.trim()}
          onclick={doExport}>{t('export.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<!-- ── 내보내기 파일 개수 확인 modal (취소에 기본 포커스) ── -->
{#if showExportManyConfirm}
  <div class="overlay" role="dialog" aria-modal="true"
       onclick={() => resolveExportMany(false)}
       onkeydown={(e) => e.key === 'Escape' && resolveExportMany(false)}>
    <div class="modal quit-modal export-many-modal" onclick={(e) => e.stopPropagation()}>
      <h2>{t(exportManyMerge ? 'export.confirm_many_merge' : 'export.confirm_many', { n: exportManyCount })}</h2>
      <div class="quit-actions">
        <button use:focusOnMount onclick={() => resolveExportMany(false)}>{t('modal.cancel')}</button>
        <button class="password-confirm-btn" onclick={() => resolveExportMany(true)}>{t('export.confirm')}</button>
      </div>
    </div>
  </div>
{/if}

<style>
  :global(*, *::before, *::after) { box-sizing: border-box; margin: 0; padding: 0; }

  /* ── 테마 색상 스케일 ── 앱 UI 크롬(사이드바/탭/모달 등) 전용 무채색 램프 — 어두운 쪽부터
     밝은 쪽까지 24단계. 라이트 테마는 이 램프를 그대로 뒤집은 값(228 = 255 - 값)이라 명암비가
     그대로 보존된다. 문서 페이지 자체(캔버스/썸네일/하이라이트 등)는 실제 종이처럼 항상 흰
     배경이라 이 스케일과 무관 — 의도적으로 토큰화하지 않았다. */
  :global(body) {
    --sh-0: #181818; --sh-1: #1e1e1e; --sh-2: #202020; --sh-3: #222222; --sh-4: #252525;
    --sh-5: #2a2a2a; --sh-6: #2e2e2e; --sh-7: #333333; --sh-8: #363636; --sh-9: #383838;
    --sh-10: #3a3a3a; --sh-11: #424242; --sh-12: #444444; --sh-13: #505050;
    --sh-14: #555555; --sh-15: #666666; --sh-16: #777777; --sh-17: #888888; --sh-18: #999999;
    --sh-19: #aaaaaa; --sh-20: #bbbbbb; --sh-21: #cccccc; --sh-22: #dddddd; --sh-23: #eeeeee;
    --accent: #d1af4b; /* 다크: 채도 낮춘 노란색(머스터드) — 활성 사이드바 탭 등 눈에 띄어야 하는 강조색 */
  }
  :global(body.theme-light) {
    --sh-0: #ffffff; --sh-1: #f7f7f7; --sh-2: #f0f0f0; --sh-3: #eaeaea; --sh-4: #e4e4e4;
    --sh-5: #dcdcdc; --sh-6: #d4d4d4; --sh-7: #cccccc; --sh-8: #c4c4c4; --sh-9: #c0c0c0;
    --sh-10: #bcbcbc; --sh-11: #b0b0b0; --sh-12: #acacac; --sh-13: #999999;
    --sh-14: #8f8f8f; --sh-15: #767676; --sh-16: #626262; --sh-17: #4d4d4d; --sh-18: #3d3d3d;
    --sh-19: #333333; --sh-20: #262626; --sh-21: #1a1a1a; --sh-22: #111111; --sh-23: #0a0a0a;
    --accent: #ff8800; /* 라이트: 밝은 주황색 */
  }
  :global(body) { background: var(--sh-1); color: var(--sh-21); font-family: monospace; overflow: hidden; }

  /* ── 탭바 ── */
  .tabbar {
    position: fixed;
    top: 0; left: 0; right: 0;
    height: 32px;
    background: var(--sh-0);
    border-bottom: 1px solid var(--sh-6);
    display: flex;
    align-items: stretch;
    z-index: 100;
    overflow-x: auto;
    overflow-y: hidden;
  }
  .tabbar::-webkit-scrollbar { height: 3px; }
  .tabbar::-webkit-scrollbar-thumb { background: var(--sh-10); }

  .tabbar-tab {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 8px 0 12px;
    border-right: 1px solid var(--sh-5);
    cursor: pointer;
    font-size: 0.75rem;
    color: var(--sh-14);
    white-space: nowrap;
    max-width: 180px;
    min-width: 60px;
    user-select: none;
  }
  .tabbar-tab:hover { background: var(--sh-3); color: var(--sh-17); }
  .tabbar-tab.active { background: var(--sh-5); color: var(--sh-23); border-bottom: 2px solid #4e8fc7; }
  .tabbar-tab.dragging {
    position: relative;
    z-index: 10;
    background: var(--sh-5);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.3);
    cursor: grabbing;
  }

  .tabbar-tab-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .tabbar-close {
    background: none;
    border: none;
    color: var(--sh-12);
    cursor: pointer;
    font-size: 0.85rem;
    padding: 0 2px;
    flex-shrink: 0;
    line-height: 1;
  }
  .tabbar-close:hover { color: var(--sh-19); }

  .tabbar-new {
    background: none;
    border: none;
    color: var(--sh-12);
    cursor: pointer;
    font-size: 1rem;
    padding: 0 12px;
    flex-shrink: 0;
  }
  .tabbar-new:hover { color: var(--sh-19); }

  .toolbar {
    position: fixed;
    top: 32px; left: 0; right: 0;
    height: 34px;
    background: var(--sh-2);
    border-bottom: 1px solid var(--sh-6);
    display: flex;
    align-items: center;
    padding: 0 6px;
    gap: 4px;
    z-index: 99;
  }
  .toolbar-btn {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--sh-17);
    padding: 6px;
    border-radius: 3px;
    cursor: pointer;
  }
  .toolbar-btn:hover { background: var(--sh-6); color: var(--sh-21); }
  .toolbar-btn.active { color: #eef5ff; background: #2e5b82; }
  .toolbar-color {
    width: 22px;
    height: 22px;
    padding: 0;
    border: none;
    border-radius: 3px;
    background: none;
    cursor: pointer;
  }
  .toolbar-opacity {
    width: 70px;
    cursor: pointer;
  }
  .toolbar-swatch {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 22px;
    height: 22px;
    cursor: pointer;
  }
  .toolbar-swatch input[type="color"] {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    padding: 0;
    border: none;
    opacity: 0;
    cursor: pointer;
  }
  .toolbar-swatch svg { pointer-events: none; }
  .toolbar-sep {
    width: 1px;
    height: 20px;
    background: var(--sh-6);
    margin: 0 2px;
  }
  .toolbar-stroke-width {
    width: 44px;
    background: var(--sh-2);
    color: var(--sh-21);
    border: 1px solid var(--sh-6);
    border-radius: 3px;
    padding: 2px 4px;
  }
  .app {
    display: flex;
    height: calc(100vh - 28px - 66px);
    margin-top: 66px;
    overflow: hidden;
  }
  :global(body.toolbar-hidden) .app { height: calc(100vh - 28px - 32px); margin-top: 32px; }

  /* ── 전체화면(F)/프레젠테이션(p) ── 탭바/툴바/사이드바/상태표시줄을 감추고 문서만 채운다 */
  :global(body.focus-mode) .tabbar,
  :global(body.focus-mode) .toolbar,
  :global(body.focus-mode) .sidebar,
  :global(body.focus-mode) .sidebar-resizer,
  :global(body.focus-mode) .statusbar { display: none; }
  :global(body.focus-mode) .app { height: 100vh; margin-top: 0; }

  /* 프레젠테이션은 한 페이지가 이미 화면 안에 꽉 차게 맞춰져 있으니(fitPresentationPage)
     스크롤 앵커링용 flex-start 대신 가운데 정렬 + 검은 배경으로 슬라이드처럼 보이게 한다 */
  :global(body.presentation-mode) .main { background: #000; }
  :global(body.presentation-mode) .viewer {
    justify-content: center;
    align-items: center;
    min-width: 100%;
  }

  .presentation-pagenum {
    position: fixed;
    left: 50%;
    bottom: 22px;
    transform: translateX(-50%);
    padding: 3px 10px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.08);
    color: rgba(255, 255, 255, 0.55);
    font-size: 0.7rem;
    z-index: 150;
    pointer-events: none;
  }

  /* ── Sidebar ── */
  .sidebar {
    flex-shrink: 0;
    background: var(--sh-4);
    display: flex;
    flex-direction: column;
  }
  /* 사이드바 안의 항목(썸네일/목차/검색결과) 중 하나라도 포커스면 사이드바 전체에
     링을 줘서 본문(.main:focus)과 대칭으로 "지금 포커스가 어디 있는지" 바로 보이게.
     파란색은 "현재 페이지" 전용 색이라(썸네일 배경 등과 겹쳐 헷갈려서) 포커스는
     완전히 다른 색(호박색)을 쓴다. 처음엔 1px/옅은 톤으로 뒀는데 두 패널 다 어두운
     배경이라 거의 안 보여서, 평소 상태부터 3px 굵기·0.9 불투명도로 확실히 눈에 띄게 했다 */
  .sidebar:focus-within { box-shadow: inset 0 0 0 3px rgba(224, 166, 64, 0.9); }
  /* Tab으로 포커스를 옮긴 순간에만 .focus-flash가 붙어서 굵고 진하게 반짝였다가 위의
     평소 상태로 서서히 가라앉는다 (아래 flashFocusRing() 참고) */
  .sidebar.focus-flash:focus-within { animation: focus-flash-inset 0.6s ease-out; }

  .sidebar-resizer {
    flex-shrink: 0;
    width: 4px;
    cursor: col-resize;
    background: var(--sh-9);
  }
  .sidebar-resizer:hover, .sidebar-resizer:active { background: #4e8fc7; }

  .sidebar-header {
    display: flex;
    flex-shrink: 0;
    border-bottom: 1px solid var(--sh-9);
  }

  .sidebar-tab-btn {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    background: none;
    border: none;
    color: var(--sh-15);
    padding: 6px 0;
    cursor: pointer;
    font-size: 0.85rem;
  }
  .sidebar-tab-btn:hover:not(:disabled) { background: var(--sh-6); color: var(--sh-19); }
  .sidebar-tab-btn.active { color: var(--accent); background: var(--sh-6); box-shadow: inset 0 -2px 0 var(--accent); }
  .sidebar-tab-btn:disabled { opacity: 0.3; cursor: default; }

  .sidebar-content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  /* ── TOC ── */
  .sidebar-content:has(.toc-item) { gap: 1px; }
  .sidebar-content:has(.thumb-item) { gap: 2px; }
  .toc-item {
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 2px 6px 2px 0;
    border-radius: 3px;
    cursor: pointer;
    font-size: 0.78rem;
  }
  .toc-item:hover { background: var(--sh-6); }
  .toc-item.toc-current { background: rgba(58, 110, 168, 0.55); }
  .toc-item.toc-selected { background: rgba(58, 110, 168, 0.8); }
  /* thumb-item.active(썸네일 쪽 "현재 페이지" 표시 — 진한 파랑 배경 + 흰 글자)와 같은 강도로
     맞춘다. 처음엔 옅은 틴트(16%) + 살짝만 밝은 글자(#eef5ff)였는데 둘 다 배경색과 거의 안
     구별돼서 사실상 안 보였다 — 배경을 훨씬 진하게, 글자는 순백으로 확실한 대비를 준다 */
  .toc-item.toc-current .toc-title,
  .toc-item.toc-selected .toc-title { color: #fff; font-weight: 600; }
  .toc-item.toc-current .toc-page,
  .toc-item.toc-selected .toc-page { color: rgba(255, 255, 255, 0.75); }
  /* 배경색은 "현재 보고 있는 페이지"(.toc-current) 전용 — 키보드 포커스는 테두리로만
     표시해서(.toc-item:focus) 서로 다른 시각 언어로 구분한다 */
  .toc-item:focus { box-shadow: 0 0 0 1px rgba(224, 166, 64, 0.5); }
  .toc-item.focus-flash:focus { animation: focus-flash-item 0.6s ease-out; }
  /* current/selected는 이미 배경색으로 표시되니 포커스 링을 더 얹지 않는다 (썸네일과 동일) */
  .toc-item.toc-current:focus,
  .toc-item.toc-selected:focus { box-shadow: none; }
  .toc-item.toc-current.focus-flash:focus,
  .toc-item.toc-selected.focus-flash:focus { animation: none; }

  .toc-caret {
    flex-shrink: 0;
    width: 12px;
    color: var(--sh-16);
    font-size: 0.65rem;
    text-align: center;
  }
  .toc-caret-spacer { flex-shrink: 0; width: 12px; }

  .toc-title {
    flex: 1;
    min-width: 0;
    color: var(--sh-21);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .toc-page {
    flex-shrink: 0;
    color: var(--sh-15);
    font-size: 0.72rem;
  }

  /* Skim처럼 이미지-번호를 한 줄에 좌우로 배치하는 가로 레이아웃 — 사이드바를 아주
     좁게 줄여도(예: 최소폭) 세로로 큰 여백을 쌓던 이전 방식보다 훨씬 공간을 잘 쓴다 */
  .thumb-item {
    cursor: pointer;
    border: 2px solid transparent;
    border-radius: 4px;
    padding: 4px 8px;
    display: grid;
    grid-template-columns: 1fr auto; /* 이미지는 이 1fr 칸 안에서 가운데 정렬, 번호는 고정폭 */
    align-items: center;
    gap: 8px;
  }
  .thumb-item:hover { border-color: var(--sh-14); }
  /* Skim처럼 셀 전체를 진한 단색으로 채운다 — "현재 페이지"는 채우기, "포커스"는
     아래 :focus의 테두리로 나눠서 둘을 헷갈리지 않게 한다 */
  .thumb-item.active { background: #3a6ea8; }
  .thumb-item.active .thumb-num { color: #fff; }
  /* 다중 선택(Cmd/Shift+클릭) — 현재 보고 있는 페이지(.active, 진한 파랑)와는 옅은 톤으로
     구분해서, 선택된 페이지들 중 어느 게 지금 화면에 떠 있는 페이지인지 계속 알 수 있게 한다 */
  .thumb-item.selected { background: rgba(58, 110, 168, 0.35); border-color: #3a6ea8; }
  .thumb-item.selected.active { background: #3a6ea8; border-color: #7ab4e0; }
  .thumb-item:focus { box-shadow: 0 0 0 1px rgba(224, 166, 64, 0.5); }
  .thumb-item.focus-flash:focus { animation: focus-flash-item 0.6s ease-out; }
  /* active(현재 페이지)는 이미 배경색으로 꽉 채워져 있어서 포커스 링을 더 얹으면
     중복이다 — 그 경우엔 링/반짝임 없이 배경색만으로 표시한다 */
  .thumb-item.active:focus { box-shadow: none; }
  .thumb-item.active.focus-flash:focus { animation: none; }

  .thumb-item img {
    display: block;
    /* max-width였을 땐 사이드바를 연 뒤에 넓혀도 이미 낮은 해상도로 그려둔 이미지가
       원본 크기보다 커지질 못했다 — width로 강제해서 실제 렌더 해상도와 무관하게
       항상 이 크기로 표시되게 한다 (JS의 THUMB_MAX_WIDTH와 반드시 맞출 것) */
    width: min(100%, 85px);
    min-width: 0;
    height: auto;
    background: #fff;
    justify-self: center;
  }

  .thumb-placeholder {
    width: min(100%, 85px);
    justify-self: center;
    background: var(--sh-6);
    border: 1px solid var(--sh-10);
  }

  .thumb-num {
    font-size: 0.68rem;
    color: var(--sh-16);
    /* 자릿수(1~3자리)에 따라 폭이 늘었다 줄었다 하면 그만큼 이미지가 밀려서 작아지므로
       고정 폭 칼럼으로 만들어 페이지 번호 자릿수와 무관하게 이미지 크기가 항상 같게 한다 */
    min-width: 3ch;
    text-align: right;
  }

  /* ── Main viewer ── */
  .main {
    flex: 1;
    overflow-y: auto;
    overflow-x: auto;
    /* 브라우저 자체의 스크롤 앵커링(콘텐츠가 뷰포트 위에 삽입되면 자동으로 scrollTop을
       보정하는 기능)을 끈다 — single 모드에서 위로 스크롤할 때 인접 페이지를 위에 끼워
       넣으면서 우리가 이미 scrollTop을 직접 보정하는데, 브라우저의 자동 보정과 겹치면서
       (WKWebView가 둘을 다른 타이밍에 적용) 위로 올릴 때만 유독 깜빡임이 남아있었다. */
    overflow-anchor: none;
  }
  /* 사이드바(.sidebar:focus-within)와 정확히 같은 처리 — 링이 페이지 사각형 하나에만 있으면
     어두운 배경 위 얇은 선이라 눈에 잘 안 띈다. 패널 전체 테두리에도 같은 링을 둬서 "지금
     포커스가 사이드바냐 본문이냐"를 페이지를 안 보고도 즉시 구분할 수 있게 한다. */
  .main:focus { outline: none; box-shadow: inset 0 0 0 3px rgba(224, 166, 64, 0.9); }
  .main.focus-flash:focus { animation: focus-flash-inset 0.6s ease-out; }

  .viewer {
    display: flex;
    flex-direction: column;
    align-items: center;
    /* justify-content:center은 페이지가 컨테이너보다 클 때(고배율 줌) WebKit에서
       스크롤 가능 영역의 시작점이 실제 콘텐츠 맨 위와 어긋나는 문제가 있다 —
       scrollTo(0,0)을 해도 위쪽 일부가 화면 밖으로 밀려 배경(검은색)이 그만큼 드러남.
       flex-start로 위쪽 기준 정렬하면 스크롤 0이 항상 콘텐츠 맨 위와 일치한다. */
    justify-content: flex-start;
    padding: 20px;
    min-height: 100%;
    min-width: max-content;
  }

  .page-wrap {
    position: relative;
    display: inline-block;
    box-shadow: 0 2px 16px rgba(0,0,0,0.6);
    /* 캔버스가 아직 안 그려진 찰나에도 뒤(.main)의 어두운 배경이 비치지 않도록 래퍼 자체를
       흰 배경으로 — 캔버스는 이미 #fff 배경이지만, 크기가 먼저 잡히는 건 이 wrap이라
       이중으로 막아 둔다. */
    background: #fff;
  }

  /* w로 본문에 포커스가 왔을 때 시각적으로 보이게 — .main 전체에 테두리를 두르면
     너무 무거워서 실제 페이지 쪽에만 은은하게 링을 준다 */
  .main:focus .page-wrap {
    box-shadow: 0 2px 16px rgba(0,0,0,0.6), 0 0 0 1px rgba(224, 166, 64, 0.5);
  }
  .main.focus-flash:focus .page-wrap { animation: focus-flash-page 0.6s ease-out; }

  @keyframes focus-flash-item {
    from { box-shadow: 0 0 0 3px rgba(224, 166, 64, 0.95); }
    to   { box-shadow: 0 0 0 1px rgba(224, 166, 64, 0.5); }
  }
  @keyframes focus-flash-inset {
    from { box-shadow: inset 0 0 0 5px rgba(224, 166, 64, 1); }
    to   { box-shadow: inset 0 0 0 3px rgba(224, 166, 64, 0.9); }
  }
  @keyframes focus-flash-page {
    from { box-shadow: 0 2px 16px rgba(0,0,0,0.6), 0 0 0 3px rgba(224, 166, 64, 0.95); }
    to   { box-shadow: 0 2px 16px rgba(0,0,0,0.6), 0 0 0 1px rgba(224, 166, 64, 0.5); }
  }

  .page-wrap canvas { display: block; background: #fff; width: 100%; height: 100%; }

  /* 두 페이지: 가로로 나란히. 연속 모드들: 세로/가로로 죽 이어 붙임 */
  .viewer-two { flex-direction: row; gap: 4px; }
  .viewer-continuous { gap: 16px; }
  .viewer-horizontal { flex-direction: row; gap: 16px; }
  .page-row { display: flex; flex-direction: row; gap: 4px; align-items: flex-start; }

  .highlight {
    position: absolute;
    background: rgba(255, 200, 0, 0.25);
    pointer-events: none;
  }

  .highlight.active {
    background: rgba(255, 230, 0, 0.65);
    outline: 2px solid #ffd400;
  }

  .highlight.selecting {
    background: rgba(78, 143, 199, 0.35);
  }

  .shape-drag-line {
    position: absolute;
    left: 0;
    top: 0;
    pointer-events: none;
  }
  .shape-drag-preview {
    position: absolute;
    box-sizing: border-box;
    border-style: solid;
    pointer-events: none;
  }

  .note-textarea {
    width: 100%;
    height: 100px;
    resize: none;
    background: var(--sh-2);
    color: var(--sh-21);
    border: 1px solid var(--sh-6);
    border-radius: 4px;
    padding: 6px;
    font: inherit;
    box-sizing: border-box;
    margin: 10px 0 16px;
  }
  .field-select-inline {
    /* 배경을 불투명하게 둔다 — mupdf가 고른 값을 필드 외관(appearance stream)에도 이미
       그려 넣어서(저장/인쇄/다른 뷰어에서도 값이 보이도록), 반투명이면 그 글자 위에 이
       <select> 자신의 글자가 겹쳐 보인다(예: "New York" 위에 또 "New York"). 완전히
       가려서 화면엔 이 <select>가 그리는 글자만 보이게 한다. */
    position: absolute;
    box-sizing: border-box;
    background: var(--sh-2);
    color: var(--sh-21);
    border: 1px solid rgba(78, 143, 199, 0.55);
    border-radius: 2px;
    font: inherit;
    padding: 0 2px;
  }
  .field-select-inline:hover { border-color: #4e8fc7; }
  .field-listbox-inline { overflow-y: auto; }
  .field-input-inline {
    /* field-select-inline과 같은 이유로 불투명 배경 — mupdf가 그려 넣은 값 글자 위에
       이 엘리먼트 자신의 글자가 겹쳐 보이는 걸 막는다. */
    position: absolute;
    box-sizing: border-box;
    background: var(--sh-2);
    color: var(--sh-21);
    border: 1px solid rgba(78, 143, 199, 0.55);
    border-radius: 2px;
    font: inherit;
    padding: 2px;
    resize: none;
  }
  .field-input-inline:hover, .field-input-inline:focus { border-color: #4e8fc7; outline: none; }
  /* comb 필드(예: ID) — 칸 div를 max_len개 늘어놓고 글자를 가운데 정렬해서 넣는다. 글자 폭에
     기대는 CSS 트릭(letter-spacing 등)은 한글처럼 라틴 문자보다 넓은 글자에서 칸을 넘쳐서
     버렸던 실제 버그가 있었음 — flex로 칸 너비를 균등 분배하고 text-align:center로 넣으면
     글자가 뭐든 칸 안에 정확히 들어간다. */
  .field-comb-wrap {
    position: absolute;
    display: flex;
    box-sizing: border-box;
    border: 1px solid rgba(78, 143, 199, 0.55);
    border-radius: 2px;
    background: var(--sh-2);
  }
  .field-comb-wrap:focus-within { border-color: #4e8fc7; }
  .field-comb-box {
    flex: 1 1 0;
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--sh-21);
    overflow: hidden;
    border-right: 1px solid rgba(78, 143, 199, 0.35);
  }
  .field-comb-box:last-child { border-right: none; }
  .field-comb-input {
    /* 칸 글자와 겹쳐 보이지 않게 이 입력창 자체를 통째로 안 보이게 한다 — 실제 값 표시는
       옆의 .field-comb-box들이, 커서는 .field-comb-caret이 맡는다(위 syncCombCaret 참고).
       color: transparent만으로는 부족했다 — 한글 IME로 입력하는 동안(조합 중) 브라우저가
       그리는 밑줄(마킹된 텍스트 표시)은 글자 색과 별개의 레이어라 안 지워지고, 이 입력창
       내부의 기본 폰트 레이아웃 기준으로 그려져서(칸 grid와 안 맞음) 엉뚱한 자리에 파란
       밑줄이 나타났다(라틴 문자는 IME 조합을 안 거치니 이 문제가 없었음 — 실제로 겪음).
       opacity: 0은 글자색뿐 아니라 그 조합 중 밑줄까지 포함해 이 입력창의 렌더링 자체를
       통째로 안 보이게 해서 완전히 없앤다. */
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    box-sizing: border-box;
    opacity: 0;
    border: none;
    outline: none;
    font: inherit;
    padding: 0;
  }
  .field-comb-caret {
    position: absolute;
    top: 15%;
    bottom: 15%;
    width: 2px;
    margin-left: -1px;
    background: #4e8fc7;
    pointer-events: none;
    animation: blink 1s step-end infinite;
  }
  .note-delete-btn { background: #6b3232; border-color: #8a4444; color: var(--sh-23); margin-right: auto; }
  .note-delete-btn:hover { background: #7c3a3a; }

  .text-caret {
    position: absolute;
    width: 2px;
    background: #4e8fc7;
    pointer-events: none;
    animation: blink 1s step-end infinite;
  }

  .field-box {
    position: absolute;
    background: rgba(78, 143, 199, 0.12);
    border: 1px solid rgba(78, 143, 199, 0.55);
    border-radius: 2px;
    pointer-events: none;
    box-sizing: border-box;
  }
  .field-box.hovered { background: rgba(78, 143, 199, 0.24); border-color: #4e8fc7; }

  .link-hint {
    position: absolute;
    background: #ffcc00;
    color: #000;
    font-family: monospace;
    font-size: 0.72rem;
    font-weight: bold;
    line-height: 1;
    padding: 2px 4px;
    border-radius: 2px;
    border: 1px solid #b38f00;
    pointer-events: none;
    transform: translate(-2px, -2px);
    z-index: 10;
  }

  .welcome {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    gap: 4px;
    color: var(--sh-14);
  }
  .welcome-title { font-size: 1.6rem; color: var(--sh-16); }
  .welcome-version { font-size: 0.85rem; color: var(--sh-14); font-weight: normal; vertical-align: middle; }
  .welcome-sub { font-size: 0.85rem; color: var(--sh-14); margin-bottom: 18px; }

  .welcome-help { border-collapse: collapse; }
  .welcome-help td { padding: 3px 10px; font-size: 0.82rem; color: var(--sh-15); }
  .welcome-help td:first-child { color: #4e8fc7; text-align: right; }

  .welcome-recent {
    margin-top: 24px;
    width: min(80vw, 640px);
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .welcome-recent-title { font-size: 0.75rem; color: var(--sh-14); margin-bottom: 4px; text-align: center; }
  .welcome-recent-row { display: flex; align-items: center; border-radius: 3px; }
  .welcome-recent-row:hover,
  .welcome-recent-row.active { background: var(--sh-5); }
  .welcome-recent-item {
    flex: 1;
    background: none;
    border: none;
    color: var(--sh-17);
    font-family: inherit;
    font-size: 0.82rem;
    text-align: center;
    padding: 4px 8px;
    cursor: pointer;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .welcome-recent-row:hover .welcome-recent-item,
  .welcome-recent-row.active .welcome-recent-item { color: var(--sh-21); }
  .welcome-recent-remove {
    background: none;
    border: none;
    color: var(--sh-15);
    font-size: 0.9rem;
    line-height: 1;
    padding: 4px 8px;
    cursor: pointer;
    opacity: 0;
  }
  .welcome-recent-row:hover .welcome-recent-remove { opacity: 1; }
  .welcome-recent-remove:hover { color: #e06c75; }

  /* ── Search results (sidebar tab) ── */
  .search-panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: -8px -8px 4px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--sh-9);
  }

  .sidebar-search-input {
    font-size: 0.78rem;
    font-family: inherit;
  }

  .search-panel-header button {
    background: none;
    border: none;
    color: var(--sh-15);
    cursor: pointer;
    font-size: 0.85rem;
    padding: 0 2px;
    flex-shrink: 0;
  }
  .search-panel-header button:hover { color: var(--sh-21); }

  .case-toggle {
    font-size: 0.68rem !important;
    font-weight: 700;
    border-radius: 3px;
    padding: 1px 4px !important;
  }
  .case-toggle.active {
    color: #eef5ff;
    background: rgba(147, 197, 253, 0.28);
    box-shadow: inset 0 0 0 1px #93c5fd;
  }

  .search-empty-hint {
    padding: 10px 4px;
    color: var(--sh-15);
    font-size: 0.75rem;
    text-align: center;
    word-break: break-all;
  }

  .search-result-item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 6px;
    border-radius: 3px;
    cursor: pointer;
    font-size: 0.78rem;
  }
  .search-result-item:hover { background: var(--sh-6); }
  .search-result-item.active { background: rgba(147, 197, 253, 0.16); }
  .search-result-item:focus { box-shadow: 0 0 0 1px rgba(224, 166, 64, 0.5); }
  .search-result-item.focus-flash:focus { animation: focus-flash-item 0.6s ease-out; }
  .search-result-item.active:focus { box-shadow: none; }
  .search-result-item.active.focus-flash:focus { animation: none; }
  .layer-item { justify-content: flex-start; gap: 7px; }
  .layer-item input[type="checkbox"] { margin: 0; flex-shrink: 0; }

  .result-page { color: var(--sh-21); }
  .result-count { color: var(--sh-15); font-size: 0.72rem; }

  .md-search-result { display: block; }
  /* 사이드바가 좁으면 한 줄 말줄임으로는 매치 위치(줄 중간~끝)가 화면 밖으로 잘려 안 보이는
     경우가 많아서(예: 헤딩 끝쪽 괄호 안에 매치가 있는 문서), 2줄까지 감싸서 항상 보이게 한다 */
  .md-result-snippet {
    display: -webkit-box;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    overflow: hidden;
    white-space: pre-wrap;
    word-break: break-word;
    line-height: 1.35;
    color: var(--sh-19);
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  .md-result-snippet mark { background: #ffd400; color: #24292e; border-radius: 2px; }
  .note-page-num { margin-left: 2px; } /* "p." 뒤 기본 공백(스페이스 문자)의 절반쯤 */
  .note-snippet {
    color: var(--sh-23);
    font-size: 0.72rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin-left: 10px;
    flex: 1;
    min-width: 0;
  }

  /* ── Status bar ── */
  .statusbar {
    position: fixed;
    bottom: 0; left: 0; right: 0;
    height: 28px;
    background: var(--sh-4);
    border-top: 1px solid var(--sh-9);
    display: flex;
    align-items: center;
    padding: 0 10px;
    font-size: 0.82rem;
    justify-content: space-between;
    z-index: 100;
  }

  .cmd-suggest {
    position: absolute;
    left: 10px;
    bottom: 100%;
    min-width: 160px;
    max-width: 420px;
    max-height: 176px;
    overflow-y: auto;
    background: var(--sh-3);
    border: 1px solid var(--sh-8);
    border-bottom: none;
    border-radius: 4px 4px 0 0;
    box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.35);
  }
  .cmd-suggest-item {
    padding: 4px 10px;
    font-size: 0.75rem;
    color: var(--sh-19);
    white-space: nowrap;
    cursor: pointer;
  }
  .cmd-suggest-item.active { background: var(--sh-6); color: var(--accent); }
  .cmd-suggest-item:hover { background: var(--sh-6); }

  .toast {
    position: fixed;
    left: 50%; top: 50%;
    transform: translate(-50%, -50%);
    background: var(--sh-9);
    color: var(--sh-23);
    border: 1px solid var(--sh-16);
    border-radius: 6px;
    padding: 8px 16px;
    font-size: 0.85rem;
    z-index: 200;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.25);
    animation: toast-in 0.15s ease-out, toast-out 0.4s ease-in 2.1s forwards;
    pointer-events: none;
  }
  @keyframes toast-in { from { opacity: 0; transform: translate(-50%, calc(-50% + 8px)); } to { opacity: 1; transform: translate(-50%, -50%); } }
  @keyframes toast-out { from { opacity: 1; } to { opacity: 0; } }

  .cmd       { color: var(--sh-23); }
  .cmd-slash { color: var(--sh-23); margin-right: 1px; }
  .cursor    { animation: blink 1s step-end infinite; }
  .search-input {
    flex: 1;
    background: none;
    border: none;
    outline: none;
    color: var(--sh-23);
    font-family: monospace;
    font-size: 0.82rem;
    caret-color: var(--sh-23);
  }
  @keyframes blink { 50% { opacity: 0; } }
  .pagenum { color: var(--sh-16); }

  /* ── Modal ── */
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0,0,0,0.55);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }

  .modal {
    background: var(--sh-5);
    border: 1px solid var(--sh-12);
    border-radius: 6px;
    padding: 24px;
    min-width: 380px;
    max-width: 580px;
  }

  .modal h2 {
    font-size: 0.95rem;
    color: var(--sh-22);
    margin-bottom: 16px;
  }

  .modal table {
    width: 100%;
    border-collapse: collapse;
    margin-bottom: 18px;
  }

  .modal td {
    padding: 7px 8px;
    border-bottom: 1px solid var(--sh-8);
    font-size: 0.82rem;
    vertical-align: top;
  }

  /* ── Modal tabs ── */
  .fileinfo-modal {
    width: 460px;
    height: 400px;
    min-width: 380px;
    min-height: 300px;
    max-width: 90vw;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    resize: both;
    overflow: auto;
  }

  .modal-tabs {
    display: flex;
    gap: 2px;
    margin-bottom: 16px;
    border-bottom: 1px solid var(--sh-10);
    padding-bottom: 0;
  }

  .modal-tabs button {
    background: none;
    border: none;
    border-bottom: 2px solid transparent;
    color: var(--sh-16);
    padding: 5px 14px 7px;
    cursor: pointer;
    font-family: monospace;
    font-size: 0.82rem;
    margin-bottom: -1px;
  }

  .modal-tabs button:hover { color: var(--sh-21); }
  .modal-tabs button.tab-active { color: var(--sh-21); border-bottom-color: #4e8fc7; }

  .modal-body {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    margin-bottom: 18px;
  }

  .tab-content {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .tab-content:focus,
  .font-list:focus {
    outline: 2px solid #7ab4e0;
    outline-offset: -2px;
  }

  .font-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .font-item {
    padding: 6px 8px;
    border-bottom: 1px solid var(--sh-6);
    font-size: 0.82rem;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .font-name { color: var(--sh-21); font-family: monospace; }

  .font-tags { display: flex; gap: 4px; flex-shrink: 0; }

  .font-tag {
    font-size: 0.7rem;
    color: #7ab4e0;
    border: 1px solid #3a5a7a;
    border-radius: 3px;
    padding: 1px 5px;
  }

  .font-loading {
    padding: 20px 8px;
    color: var(--sh-14);
    font-size: 0.82rem;
    text-align: center;
  }

  .analyzing-banner {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 12px;
    padding: 48px 8px;
    color: var(--sh-23);
    font-size: 0.9rem;
  }

  .spinner.spinner-lg {
    width: 28px;
    height: 28px;
    margin-right: 0;
    border-width: 3px;
  }

  .font-tab-header {
    display: flex;
    justify-content: flex-end;
    margin-bottom: 8px;
  }

  .analyze-btn {
    background: var(--sh-6);
    border: 1px solid var(--sh-13);
    color: var(--sh-20);
    padding: 4px 12px;
    border-radius: 4px;
    cursor: pointer;
    font-family: monospace;
    font-size: 0.78rem;
  }
  .analyze-btn:hover:not(:disabled) { background: var(--sh-10); color: var(--sh-23); }
  .analyze-btn:disabled { opacity: 0.4; cursor: default; }
  .analyze-btn.analyzing { opacity: 0.85; color: var(--sh-23); }

  .spinner {
    display: inline-block;
    width: 9px;
    height: 9px;
    margin-right: 5px;
    border: 1.5px solid var(--sh-15);
    border-top-color: var(--sh-23);
    border-radius: 50%;
    animation: spin 0.6s linear infinite;
    vertical-align: middle;
  }
  @keyframes spin { to { transform: rotate(360deg); } }

  .stat-item {
    padding: 8px 8px 6px;
    border-bottom: 1px solid var(--sh-6);
  }

  .stat-header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    margin-bottom: 5px;
  }

  .stat-pct { color: #7ab4e0; font-size: 0.8rem; flex-shrink: 0; }

  .stat-bar-bg {
    background: var(--sh-7);
    border-radius: 2px;
    height: 4px;
    margin-bottom: 3px;
  }

  .stat-bar-fill {
    background: #4e8fc7;
    border-radius: 2px;
    height: 100%;
    min-width: 2px;
  }

  .stat-count { color: var(--sh-14); font-size: 0.72rem; }

  .modal td:first-child { color: var(--sh-17); width: 110px; white-space: nowrap; }
  .modal td:last-child  { color: var(--sh-21); word-break: break-all; }

  .modal button {
    background: var(--sh-8);
    border: 1px solid var(--sh-13);
    color: var(--sh-21);
    padding: 5px 16px;
    border-radius: 4px;
    cursor: pointer;
    font-family: monospace;
    font-size: 0.82rem;
  }
  .modal button:hover { background: var(--sh-11); }
  .modal button:focus { outline: 2px solid #7ab4e0; outline-offset: 3px; }
  /* 네이티브 체크박스는 버튼과 달리 기본 포커스 링이 잘 안 보여서(특히 macOS), Tab으로
     넘어가도 지금 어디에 포커스가 있는지 구분이 안 됐다 — 버튼과 같은 색으로 맞춰준다. */
  .modal input[type="checkbox"]:focus-visible { outline: 2px solid #7ab4e0; outline-offset: 2px; }

  .shortcuts-modal {
    /* overlay의 flex 중앙정렬에 맡기면 resize로 커질 때마다 다시 가운데로 맞추면서 좌상단
       좌표까지 같이 움직인다(양쪽으로 늘어나는 것처럼 보임) — position:fixed로 뷰포트에
       직접 고정해서 리사이즈 핸들(우하단)이 우하단 방향으로만 늘어나게 한다. */
    position: fixed;
    top: calc(50vh - 230px);
    left: calc(50vw - 240px);
    width: 480px;
    height: 460px;
    min-width: 380px;
    min-height: 300px;
    max-width: 90vw;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    resize: both;
    overflow: auto;
  }

  .shortcuts-modal .modal-body td:first-child {
    color: var(--sh-23);
    font-weight: 600;
    width: 120px;
    font-family: monospace;
    white-space: nowrap;
  }

  .shortcuts-modal .modal-body td:last-child { color: var(--sh-20); }

  .regex-syntax-title {
    margin: 14px 0 4px;
    padding-top: 8px;
    border-top: 1px solid var(--sh-6);
    color: var(--sh-17);
    font-size: 0.75rem;
  }

  /* ── 오픈소스 ── */
  .oss-modal { min-width: 480px; max-width: 600px; }

  .oss-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-bottom: 18px;
    max-height: 400px;
    overflow-y: auto;
  }

  .oss-item {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 12px;
    padding: 6px 4px;
    border-bottom: 1px solid var(--sh-6);
    font-size: 0.8rem;
    align-items: center;
  }

  .oss-name    { color: var(--sh-21); }
  .oss-ver     { color: var(--sh-15); font-family: monospace; }
  .oss-license { color: #7ab4e0; white-space: nowrap; }

  /* ── 설정 ── */
  .settings-modal {
    width: 460px;
    height: 380px;
    min-width: 340px;
    min-height: 300px;
    max-width: none;
    resize: both;
    overflow: auto;
  }
  /* macOS 환경설정/Skim 설정 창 스타일 — 제목(현재 탭 이름)과 탭 아이콘 줄을 같은 툴바
     행에 나란히(제목 좌측, 탭 우측) 놓고, 그 행 전체 밑에 가로선 하나로 아래 세부 항목과
     나눈다. 탭은 아이콘+라벨을 쌓은 flat 버튼이고 선택된 탭만 옅은 파란 pill로 떠 보이게
     한다 — 박스 테두리로 감싸던 기존 방식과 달리 pill 배경 자체가 "선택됨"을 나타내므로
     .modal button의 박스 스타일을 완전히 걷어낸다. 설정 창뿐 아니라 탭이 있는 모든 모달
     (파일 정보/단축키/명령어)이 .modal-tabs를 같이 쓰므로 .settings-modal이 아니라 .modal에
     걸어서 한 군데 정의로 전부 적용되게 한다. */
  .modal .tab-toolbar {
    display: flex;
    align-items: flex-end;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 10px 16px;
    border-bottom: 1px solid var(--sh-11);
    padding-bottom: 10px;
    margin-bottom: 22px;
  }
  .modal .tab-toolbar h2 {
    margin-bottom: 6px;
  }
  .modal .modal-tabs {
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 10px 18px;
    background: none;
    border: none;
    padding: 0;
    margin-bottom: 0;
  }
  .modal .modal-tabs button {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 3px;
    background: none;
    border: none;
    border-radius: 8px;
    padding: 6px 12px 5px;
    margin-bottom: 0;
    color: var(--sh-16);
    font-family: monospace;
    font-size: 0.68rem;
    white-space: nowrap;
  }
  .modal .modal-tabs button svg { width: 20px; height: 20px; }
  .modal .modal-tabs button:hover { background: var(--sh-8); color: var(--sh-19); }
  .modal .modal-tabs button.tab-active { background: rgba(78, 143, 199, 0.18); color: #4e8fc7; }
  .modal .modal-tabs button.tab-active span { color: var(--sh-23); }
  /* .modal button:focus의 outline(3px 바깥 간격)이 둥근 pill 바깥에 각진 테두리를 하나 더
     그려서 "박스 두 겹"처럼 보이던 문제 — pill 모양을 따라가는 box-shadow 링으로 바꾼다.
     :focus-visible이 아니라 그냥 :focus를 쓰는 이유: 이 모달들의 Tab 이동은 네이티브 탭
     순회가 아니라 커스텀 keydown 핸들러가 next.focus()를 직접 호출하는 방식이라, WebKit이
     스크립트로 건 포커스를 "키보드에 의한 포커스"로 안 쳐서 :focus-visible이 안 붙는 경우가
     있다 — 그래서 Tab으로 옮겨간 탭에 아무 표시도 안 남는 문제가 있었다. */
  .modal .modal-tabs button:focus {
    outline: none;
    box-shadow: 0 0 0 2px rgba(78, 143, 199, 0.5);
  }
  .settings-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 20px;
    font-size: 0.85rem;
    color: var(--sh-21);
  }
  .settings-row-col { flex-direction: column; align-items: stretch; gap: 8px; }
  .synctex-preset-row { display: flex; align-items: center; gap: 10px; }
  .synctex-preset-row select { flex: 1; }
  .synctex-help-btn {
    background: none; border: none; padding: 0; margin: 0;
    color: #7ab4e0; font-size: 0.82rem; text-decoration: underline; cursor: pointer; white-space: nowrap;
  }
  .synctex-help-btn:hover { color: #93c5ea; }
  .pagespec-help-btn {
    display: inline-flex; align-items: center; justify-content: center;
    width: 16px; height: 16px; margin-left: 4px;
    background: none; border: 1px solid var(--sh-12); border-radius: 50%;
    color: var(--sh-14); font-size: 0.7rem; line-height: 1; cursor: pointer; padding: 0;
  }
  .pagespec-help-btn:hover { color: #93c5ea; border-color: #7ab4e0; }
  .settings-modal select:focus,
  .settings-modal .password-input:focus {
    outline: none;
    border-color: #4e8fc7;
    box-shadow: 0 0 0 2px rgba(78, 143, 199, 0.25);
  }
  .texshop-help-modal { max-width: 520px; position: relative; }
  .texshop-help-cmd {
    background: var(--sh-1);
    border: 1px solid var(--sh-12);
    border-radius: 4px;
    padding: 10px 12px;
    font-family: monospace;
    font-size: 0.78rem;
    line-height: 1.5;
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 260px;
    overflow-y: auto;
    margin: 12px 0;
  }
  .texshop-copied-toast {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: #3e9cf6;
    color: #ffffff;
    padding: 16px 32px;
    margin: 24px;
    border-radius: 8px;
    font-size: 0.9rem;
    box-shadow: 0 4px 20px rgba(0,0,0,0.45);
    pointer-events: none;
  }
  .export-filename-row { display: flex; align-items: center; gap: 8px; }
  .export-filename-row input { flex: 1; min-width: 0; }
  .export-merge-row {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
    font-size: 0.85rem;
    color: var(--sh-21);
    cursor: pointer;
  }
  .settings-row select {
    background: var(--sh-5);
    color: var(--sh-21);
    border: 1px solid var(--sh-12);
    border-radius: 4px;
    padding: 5px 8px;
    font-family: inherit;
    font-size: 0.82rem;
    transition: border-color 0.15s;
  }

  /* ── 종료 확인 ── */
  .quit-modal { min-width: 340px; }
  .quit-desc { font-size: 0.82rem; color: var(--sh-18); margin-bottom: 20px; }
  .quit-actions { display: flex; justify-content: flex-end; gap: 8px; }
  .quit-confirm-btn { background: #6b3232; border-color: #8a4444; color: var(--sh-23); }
  .quit-confirm-btn:hover { background: #7c3a3a; }

  /* ── 암호 입력 ── */
  .password-input {
    width: 100%;
    background: var(--sh-1);
    border: 1px solid var(--sh-12);
    border-radius: 4px;
    padding: 7px 10px;
    color: var(--sh-23);
    font-family: monospace;
    font-size: 0.85rem;
    margin-bottom: 10px;
  }
  .password-input:focus { outline: none; border-color: #7ab4e0; }
  .password-error { font-size: 0.78rem; color: #e08080; margin-bottom: 14px; }
  .password-confirm-btn { background: #2e5a7c; border-color: #3a6f96; color: var(--sh-23); }
  .password-confirm-btn:hover:not(:disabled) { background: #366d94; }
  .password-confirm-btn:disabled { opacity: 0.4; cursor: default; }

  /* ── 인쇄 ── printDocument()가 인쇄 직전에 single-continuous 모드로 강제 전환해두므로
     여기서는 앱 크롬만 지우고 페이지들을 세로로 자연스럽게 흘려보내면 된다 */
  @media print {
    .tabbar, .toolbar, .sidebar, .sidebar-resizer, .statusbar, .overlay { display: none !important; }
    .app { height: auto; margin-top: 0; overflow: visible; }
    .main { overflow: visible; }
    .viewer { padding: 0; min-height: 0; align-items: flex-start; }
    .viewer-continuous { gap: 0; }
    .page-wrap { box-shadow: none; break-inside: avoid; break-after: page; }
    .page-wrap:last-child { break-after: auto; }
  }
</style>
