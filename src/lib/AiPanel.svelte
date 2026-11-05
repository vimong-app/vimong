<script module>
  import { invoke, Channel } from '@tauri-apps/api/core';
  import { openUrl } from '@tauri-apps/plugin-opener';
  import { save as saveDialog } from '@tauri-apps/plugin-dialog';
  import { writeText as clipboardWrite } from '@tauri-apps/plugin-clipboard-manager';
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import { createHistory } from '$lib/history.js';

  // 기본 모델/맥락/Ollama 주소 — 설정 > AI(applyAiSettings)에서 정한다. 패널에서 고른 모델·맥락은 이번 실행 동안만
  const MODEL_KEY = 'vimong-ai-model';
  const CONTEXT_KEY = 'vimong-ai-context';
  const HOST_KEY = 'vimong-ai-host';
  const PROVIDER_KEY = 'vimong-ai-provider'; // 'ollama' | 'openai'(OpenAI 호환: LM Studio, llama.cpp 등) — Rust ai.rs Server
  const API_KEY_KEY = 'vimong-ai-key';
  function load(key) { try { return localStorage.getItem(key); } catch (_) { return null; } }
  function save(key, v) { try { localStorage.setItem(key, v); } catch (_) { /* skip */ } }

  // 사이드바 탭을 바꾸거나 숨기면 패널이 언마운트되니 대화는 모듈 수준에 둔다 — :ai <질문>이
  // 패널이 그려지기 전에 askAi를 불러도 같은 상태에 쌓인다.
  // chats: 파일 경로 → { messages, error } — 맥락은 늘 지금 보고 있는 문서라, 다른 문서와 주고받은
  // 기록이 한 대화에 섞이면 모델이 두 문서를 섞어 답한다. error(최상위)는 모델 목록/연결 오류.
  // context: 'page' | 'doc' | 'image' (Rust ai_chat 참고) — 예전에 있던 'none' 등 모르는 값이 저장돼 있으면 'page'로
  const savedContext = load(CONTEXT_KEY);
  export const aiState = $state({ chats: {}, busy: false, error: '', models: null, model: load(MODEL_KEY) ?? '', context: ['page', 'doc', 'image'].includes(savedContext) ? savedContext : 'page' });
  const ai = aiState;
  // 첫 서버 호출보다 서버 설정이 먼저 Rust에 들어가 있어야 한다 — 목록/질문은 이걸 기다린 뒤 부른다
  const savedServer = () => ({ provider: load(PROVIDER_KEY) || 'ollama', host: load(HOST_KEY) ?? '', key: load(API_KEY_KEY) ?? '' });
  const hostReady = invoke('ai_set_server', { server: savedServer() }).catch(() => {});

  // 서버에 연결 자체가 안 될 때 Rust(ai.rs Server::http_err)가 넘기는 "AI_UNREACHABLE|provider|state|url|detail"을
  // 상황별 안내로 — state: missing(설치 안 됨) | stopped(설치됐지만 꺼짐) | remote(다른 컴퓨터 주소). 그 밖의 오류는 그대로
  export function aiErrorInfo(err, t) {
    const [code, provider, state, url, ...rest] = String(err).split('|');
    if (code !== 'AI_UNREACHABLE') return { text: String(err), detail: '', link: '' };
    const app = provider === 'openai' ? 'lmstudio' : 'ollama';
    return {
      text: t(state === 'remote' ? 'ai.err_remote' : `ai.err_${app}_${state}`, { url }),
      detail: rest.join('|'),
      link: state === 'missing' ? (app === 'ollama' ? 'https://ollama.com/download' : 'https://lmstudio.ai') : '',
    };
  }

  export function aiSettings() {
    return { ...savedServer(), model: load(MODEL_KEY) ?? ai.model, context: load(CONTEXT_KEY) ?? 'page' };
  }

  // 설정 창의 "적용" — 지금 패널도 기본값으로 되돌린다
  export async function applyAiSettings({ provider, host, key, model, context }) {
    const server = { provider, host: host.trim(), key: key.trim() };
    save(PROVIDER_KEY, server.provider);
    save(HOST_KEY, server.host);
    save(API_KEY_KEY, server.key);
    await invoke('ai_set_server', { server });
    if (model) { save(MODEL_KEY, model); ai.model = model; }
    save(CONTEXT_KEY, context);
    ai.context = context;
    await loadModels();
  }

  // 모델 받기/업데이트(Ollama만) — onProgress({ status, completed, total }). 끝나면 목록을 새로 받는다
  export async function pullModel(name, server, onProgress) {
    const onEvent = new Channel();
    onEvent.onmessage = onProgress;
    await invoke('ai_pull', { model: name, server, onEvent });
    await loadModels(server);
  }
  const NO_CHAT = { messages: [], error: '' };
  // 보낸 질문 기록 — 문서와 상관없이 하나(같은 질문을 다른 문서에 다시 묻는 일이 많다)
  const promptHistory = createHistory('vimong-ai-prompt-history', 50);

  // server: 설정 창에서 적용 전 서버로 목록을 볼 때 { provider, host, key } (없으면 적용된 서버)
  export async function loadModels(server = null) {
    await hostReady;
    try {
      ai.models = await invoke('ai_models', { server });
      ai.error = '';
      if (!ai.models.includes(ai.model)) ai.model = ai.models[0] ?? '';
    } catch (e) {
      ai.models = [];
      ai.error = String(e);
    }
  }

  // 목차 "AI에게 묻기"로 정한 범위 — { spec: '96-110', title } | null. 대화(문서)마다 따로
  export function setAiScope(path, scope) {
    ai.chats[path] ??= { messages: [], error: '' };
    ai.chats[path].scope = scope;
  }

  // 답을 .md로 — 저장 위치는 사용자가 저장 대화상자에서 고른다(모델에게 경로를 맡기면 덮어쓰기 같은 판단을 자주
  // 틀린다). 답을 만들 때 보낸 페이지 그림(imagePages)은 Rust ai_save_markdown이 옆에 JPEG로 저장해 링크한다
  async function saveReply(path, m) {
    const dir = path.replace(/[^/\\]*$/, '');
    const base = path.slice(dir.length).replace(/\.[^.]*$/, '');
    const out = await saveDialog({ defaultPath: `${dir}${base}-AI.md`, filters: [{ name: 'Markdown', extensions: ['md'] }] });
    if (!out) return;
    try {
      await invoke('ai_save_markdown', { path: out, markdown: m.content, docPath: path, imagePages: m.imagePages ?? [] });
      m.saved = out.replace(/^.*[/\\]/, '');
      m.saveError = '';
    } catch (e) {
      m.saveError = String(e);
    }
  }

  // :ai save — 지금 문서 대화의 마지막 답. 저장할 답이 없으면 false(호출부가 상태줄에 알린다)
  export function saveLastReply(path) {
    const m = ai.chats[path]?.messages.findLast((m) => m.role === 'assistant' && m.content);
    if (!m || ai.busy) return false;
    saveReply(path, m);
    return true;
  }

  export async function askAi(path, text) {
    text = text.trim();
    if (!text || ai.busy) return;
    if (!ai.model) await loadModels();
    if (!ai.model) return; // 패널이 "모델 없음"/연결 오류 안내를 보여준다
    promptHistory.push(text);
    ai.chats[path] ??= { messages: [], error: '' };
    const chat = ai.chats[path]; // ??=의 결과값은 프록시가 아닌 원본 객체라 다시 읽는다
    chat.messages.push({ role: 'user', content: text });
    const history = chat.messages.map(({ role, content }) => ({ role, content })); // notice는 화면용이라 모델엔 안 보낸다
    chat.messages.push({ role: 'assistant', content: '' });
    const reply = chat.messages[chat.messages.length - 1];
    const onEvent = new Channel();
    onEvent.onmessage = (m) => {
      if (m.notice) reply.notice = m.notice;
      else if (m.image_pages) reply.imagePages = m.image_pages;
      else if (m.tool) { if (!reply.tools) reply.tools = []; reply.tools.push({ name: m.tool, args: m.args }); }
      else reply.content += m.delta;
    };
    ai.busy = true;
    chat.error = '';
    try {
      await invoke('ai_chat', { model: ai.model, messages: history, context: ai.context, pages: chat.scope?.spec ?? null, title: chat.scope?.title ?? null, onEvent });
    } catch (e) {
      chat.error = String(e);
      if (!reply.content) chat.messages.pop();
    } finally {
      ai.busy = false;
    }
  }
</script>

<script>
  import { onMount, tick } from 'svelte';

  // path: 지금 문서 — 이 문서의 대화만 보여준다. onExit: Esc로 입력창을 떠날 때 본문으로 포커스를 돌려주는 콜백
  let { t, path, onExit } = $props();
  const chat = $derived(ai.chats[path] ?? NO_CHAT);
  let input = $state('');
  // 긴 코드 블록 전체 보기 — { html, code, lines } | null. 사이드바가 좁아 앱 전체 위에 띄운다
  let codeView = $state(null);
  let codeViewEl = $state(null);
  let copied = $state(false);
  const CODE_PREVIEW_LINES = 12; // 이보다 몇 줄 넘게 길면 접고 "… 전체 보기"를 단다
  let inputEl, listEl;
  onMount(() => {
    loadModels(); // 열 때마다 — 그 사이 ollama pull로 받은 모델도 바로 보이게
    inputEl.focus();
  });

  $effect(() => {
    chat.messages.length; chat.messages.at(-1)?.content; chat.error; // 답이 자랄 때마다 맨 아래로
    tick().then(() => { if (listEl) listEl.scrollTop = listEl.scrollHeight; });
  });

  // 문서 전체 keydown 리스너(+page.svelte)는 .ai-input에 포커스가 있으면 손대지 않는다 — Esc만
  // 여기서 처리하고 전파를 끊는다(안 끊으면 포커스가 빠진 뒤 그 리스너가 같은 Esc로 검색 결과 등을 지운다).
  function onPanelKeydown(e) {
    if (codeView) {
      // 코드 창: j/k 스크롤, Esc 닫기 — 창 자체가 .ai-input이라 문서 쪽 vim 키로 새지 않는다
      if (e.key === 'j' || e.key === 'k') { e.preventDefault(); codeViewEl?.scrollBy(0, e.key === 'j' ? 40 : -40); return; }
      if (e.key !== 'Escape') return;
      e.preventDefault();
      e.stopPropagation();
      codeView = null;
      inputEl?.focus();
      return;
    }
    if (e.key !== 'Escape') return;
    e.preventDefault();
    e.stopPropagation();
    if (ai.busy) invoke('ai_stop');
    else { document.activeElement?.blur(); onExit?.(); }
  }

  function onInputKeydown(e) {
    if (e.isComposing) return; // 한글 IME 조합 중 Enter/↑/↓는 조합용
    // ↑/↓: 이전/다음 질문 — 여러 줄 입력 안에서 줄 이동은 그대로 두려고 첫 줄(↑)/마지막 줄(↓)에 커서가 있을 때만
    const el = e.currentTarget;
    if (e.key === 'ArrowUp' && !el.value.slice(0, el.selectionStart).includes('\n')) {
      e.preventDefault();
      input = promptHistory.up(input);
      return;
    }
    if (e.key === 'ArrowDown' && !el.value.slice(el.selectionEnd).includes('\n')) {
      e.preventDefault();
      input = promptHistory.down(input);
      return;
    }
    if (e.key !== 'Enter' || e.shiftKey) return;
    e.preventDefault();
    const q = input;
    input = '';
    askAi(path, q);
  }

  // 문서를 찾아 읽은 기록 — 모델의 도구 호출, 그리고 "파일 전체"에서 앱이 직접 범위를 읽은 단계(scope/read/merge,
  // Rust read_scope). 답이 어디서 나왔는지 보이게
  function toolLabel({ name, args }) {
    if (name === 'search_text') return t('ai.tool_search_text', { q: args.query ?? '' });
    if (name === 'get_page_text') return args.pages ? t('ai.tool_get_page_text', { p: args.pages }) : t('ai.tool_get_page_text_current');
    return t('ai.tool_' + name, args).replace(/\s+/g, ' ').trim();
  }

  // 답 HTML은 {@html} 대신 직접 넣는다 — 실제 앱(WKWebView)에서 새 말풍선이 이전 답의 HTML을 단 채로 생기고
  // {@html}이 첫 렌더링을 건너뛰어, 첫 토큰이 올 때까지 이전 답이 그대로 보였다(MutationObserver로 확인. 따로 띄운
  // WebKit/happy-dom에서는 재현이 안 돼 Svelte 쪽 원인은 못 찾았다). 만들 때와 바뀔 때마다 덮어쓰면 처음 상태와 상관없다.
  function markdown(node, src) {
    const render = (s) => {
      node.innerHTML = DOMPurify.sanitize(marked.parse(s || '…'));
      // 긴 코드 블록은 앞부분만 보이게 접고(CSS로 높이만 줄여 하이라이트는 그대로) 전체 보기 버튼을 단다 — 답이 자랄
      // 때마다 다시 그려지니 매번 다시 단다
      for (const pre of node.querySelectorAll('pre')) {
        const code = pre.textContent.replace(/\n$/, '');
        const lines = code.split('\n').length;
        if (lines <= CODE_PREVIEW_LINES + 3) continue; // 몇 줄 더 긴 정도면 접는 게 더 번거롭다
        pre.classList.add('ai-code-collapsed');
        const more = document.createElement('button');
        more.className = 'ai-code-more';
        more.tabIndex = -1;
        more.textContent = t('ai.code_more', { n: lines });
        more.onclick = () => { codeView = { html: pre.innerHTML, code, lines }; copied = false; };
        pre.after(more);
      }
    };
    render(src);
    return { update: render };
  }

  function setHtml(node, html) {
    node.innerHTML = html; // markdown()에서 이미 DOMPurify를 거친 HTML
    return { update: (h) => { node.innerHTML = h; } };
  }

  function focusOnMount(node) {
    node.focus();
  }

  async function copyCode() {
    await clipboardWrite(codeView.code);
    copied = true;
  }

  // 답 안의 링크를 그대로 따라가면 앱 webview 자체가 그 페이지로 넘어가 버린다 — 브라우저로 연다
  function onListClick(e) {
    const a = e.target.closest('a[href]');
    if (!a) return;
    e.preventDefault();
    if (/^https?:/.test(a.href)) openUrl(a.href);
  }
</script>

<div class="ai-panel" role="presentation" tabindex="-1" onkeydown={onPanelKeydown}>
  <div class="search-panel-header ai-header">
    <select class="ai-input" bind:value={ai.model} title={t('ai.model')} disabled={!ai.models?.length}>
      {#each ai.models ?? [] as m (m)}<option value={m}>{m}</option>{/each}
    </select>
    <select class="ai-input" bind:value={ai.context} title={chat.scope ? t('ai.context_scoped') : t('ai.context')} disabled={!!chat.scope}>
      <option value="page">{t('ai.context_page')}</option>
      <option value="doc">{t('ai.context_doc')}</option>
      <option value="image">{t('ai.context_image')}</option>
    </select>
    <button onclick={() => { delete ai.chats[path]; ai.error = ''; }} disabled={ai.busy} title={t('ai.clear')} tabindex="-1">✕</button>
  </div>
  <div class="ai-messages" bind:this={listEl} role="presentation" onclick={onListClick}>
    {#if chat.messages.length === 0 && !ai.error && !chat.error}
      <div class="search-empty-hint">{ai.models?.length === 0 ? t('ai.no_models') : t('ai.hint')}</div>
    {/if}
    {#each chat.messages as m, i (i)}
      {#if m.role === 'user'}
        <div class="ai-msg ai-user">{m.content}</div>
      {:else}
        {#if m.notice}<div class="ai-msg ai-notice">{t('ai.notice_' + m.notice)}</div>{/if}
        {#each m.tools ?? [] as tool, j (j)}<div class="ai-msg ai-tool">{toolLabel(tool)}</div>{/each}
        <div class="ai-msg ai-assistant" use:markdown={m.content}></div>
        {#if m.content && !(ai.busy && i === chat.messages.length - 1)}
          <div class="ai-actions">
            <button onclick={() => saveReply(path, m)} title={t('ai.save_title')} tabindex="-1">{t('ai.save')}</button>
            {#if m.saveError}<span class="ai-error">{m.saveError}</span>{:else if m.saved}<span>{t('ai.saved', { name: m.saved })}</span>{/if}
          </div>
        {/if}
      {/if}
    {/each}
    {#if ai.error || chat.error}
      {@const err = aiErrorInfo(ai.error || chat.error, t)}
      <!-- 링크는 onListClick이 브라우저로 연다 -->
      <!-- 기술적인 원인(Connection refused 등)은 마우스를 올렸을 때만 -->
      <div class="ai-msg ai-error" title={err.detail}>
        {err.text}
        {#if err.link}<a href={err.link}>{t('ai.err_download')}</a>{/if}
      </div>
    {/if}
  </div>
  {#if codeView}
    <!-- 패널 안에 두어야 Esc/j/k가 onPanelKeydown으로 온다. position: fixed라 화면은 앱 전체를 덮는다 -->
    <div class="ai-code-overlay" role="presentation" onclick={(e) => { if (e.target === e.currentTarget) { codeView = null; inputEl?.focus(); } }}>
      <div class="ai-code-dialog ai-input" role="dialog" aria-modal="true" aria-label={t('ai.code_title', { n: codeView.lines })} tabindex="-1" use:focusOnMount>
        <div class="ai-code-head">
          <span>{t('ai.code_title', { n: codeView.lines })}</span>
          <button onclick={copyCode} tabindex="-1">{copied ? t('ai.copied') : t('ai.copy')}</button>
          <button onclick={() => { codeView = null; inputEl?.focus(); }} title={t('ai.close')} tabindex="-1">✕</button>
        </div>
        <pre class="hljs" bind:this={codeViewEl} use:setHtml={codeView.html}></pre>
      </div>
    </div>
  {/if}
  {#if chat.scope}
    <div class="ai-scope">
      <span>{chat.scope.title ? t('ai.scope_title', { title: chat.scope.title, spec: chat.scope.spec }) : t('ai.scope', { spec: chat.scope.spec })}</span>
      <button onclick={() => setAiScope(path, null)} title={t('ai.scope_clear')} tabindex="-1">✕</button>
    </div>
  {/if}
  <textarea
    class="ai-input ai-textarea"
    bind:this={inputEl}
    bind:value={input}
    onkeydown={onInputKeydown}
    rows="3"
    placeholder={ai.busy ? t('ai.busy') : t('ai.placeholder')}
  ></textarea>
</div>

<style>
  .ai-panel { outline: none; flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 6px; }
  .ai-header { gap: 4px; }
  .ai-header select { flex: 1; min-width: 0; font-size: 0.75rem; }
  .ai-messages { flex: 1; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; gap: 8px; }
  .ai-msg { font-size: 0.8rem; line-height: 1.45; overflow-wrap: anywhere; }
  .ai-user { white-space: pre-wrap; align-self: flex-end; max-width: 90%; padding: 4px 8px; border-radius: 6px; background: var(--sh-9); }
  .ai-assistant :global(:is(p, ul, ol, pre)) { margin: 0 0 6px; }
  .ai-assistant :global(pre) { overflow-x: auto; padding: 6px; border-radius: 4px; background: var(--sh-9); }
  .ai-error { color: #e5484d; }
  .ai-error a { color: inherit; text-decoration: underline; }
  /* +page.svelte의 .search-scope(검색 범위 표시)와 같은 모양 */
  .ai-scope { display: flex; align-items: center; justify-content: space-between; gap: 6px; padding: 3px 6px; border-radius: 3px; background: rgba(58, 110, 168, 0.25); color: var(--sh-21); font-size: 0.75rem; }
  .ai-scope button { background: none; border: none; color: var(--sh-18); cursor: pointer; font-size: 0.75rem; }
  .ai-assistant :global(pre.ai-code-collapsed) {
    max-height: calc(12 * 1.45em);
    overflow: hidden;
    -webkit-mask-image: linear-gradient(to bottom, #000 70%, transparent);
    mask-image: linear-gradient(to bottom, #000 70%, transparent);
  }
  .ai-assistant :global(.ai-code-more) { display: block; margin: -2px 0 6px; background: none; border: none; padding: 0; color: var(--sh-18); cursor: pointer; font-size: 0.72rem; }
  .ai-assistant :global(.ai-code-more:hover) { color: var(--sh-21); text-decoration: underline; }
  .ai-code-overlay { position: fixed; inset: 0; z-index: 1000; display: flex; align-items: center; justify-content: center; background: rgba(0, 0, 0, 0.45); }
  .ai-code-dialog { outline: none; display: flex; flex-direction: column; width: min(900px, 90vw); height: min(80vh, 900px); border-radius: 8px; background: var(--sh-4); box-shadow: 0 10px 40px rgba(0, 0, 0, 0.4); }
  .ai-code-head { display: flex; align-items: center; gap: 8px; padding: 8px 12px; border-bottom: 1px solid var(--sh-9); font-size: 0.8rem; }
  .ai-code-head span { flex: 1; }
  .ai-code-head button { background: none; border: 1px solid var(--sh-9); border-radius: 3px; color: var(--sh-18); cursor: pointer; font-size: 0.75rem; padding: 2px 8px; }
  .ai-code-head button:hover { color: var(--sh-21); }
  .ai-code-dialog pre { flex: 1; min-height: 0; margin: 0; padding: 12px; overflow: auto; font-size: 0.8rem; line-height: 1.45; }
  .ai-actions { display: flex; align-items: center; gap: 6px; margin-top: -4px; font-size: 0.72rem; color: var(--sh-15); }
  .ai-actions button { background: none; border: 1px solid var(--sh-9); border-radius: 3px; color: var(--sh-18); cursor: pointer; font-size: 0.72rem; padding: 1px 6px; }
  .ai-actions button:hover { color: var(--sh-21); }
  .ai-tool { font-size: 0.72rem; color: var(--sh-15); margin-bottom: -4px; }
  .ai-tool::before { content: '↳ '; }
  .ai-notice { font-size: 0.72rem; padding: 4px 6px; border-radius: 4px; border-left: 3px solid #f5a524; background: var(--sh-9); }
  .ai-textarea { resize: vertical; font: inherit; font-size: 0.8rem; padding: 6px; }
</style>
