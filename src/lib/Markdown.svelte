<script module>
  // 마크다운 뷰어 — 코드 보기/미리보기/분할 3가지 모드 (마크다운 지원 요청 참고).
  // marked(파싱) + highlight.js(하이라이팅) + DOMPurify(신뢰 못하는 문서를 열 수 있으니 정화)
  // 모듈 스코프에서 한 번만 설정한다 — 컴포넌트가 여러 번 생겨도(탭 전환 등) 재등록하지 않음.
  import { marked } from 'marked';
  import DOMPurify from 'dompurify';
  import hljs from 'highlight.js/lib/core';
  import markdownLang from 'highlight.js/lib/languages/markdown';
  import javascript from 'highlight.js/lib/languages/javascript';
  import typescript from 'highlight.js/lib/languages/typescript';
  import xml from 'highlight.js/lib/languages/xml';
  import css from 'highlight.js/lib/languages/css';
  import json from 'highlight.js/lib/languages/json';
  import python from 'highlight.js/lib/languages/python';
  import bash from 'highlight.js/lib/languages/bash';
  import yaml from 'highlight.js/lib/languages/yaml';
  import rust from 'highlight.js/lib/languages/rust';
  import go from 'highlight.js/lib/languages/go';
  import java from 'highlight.js/lib/languages/java';
  import c from 'highlight.js/lib/languages/c';
  import cpp from 'highlight.js/lib/languages/cpp';
  import sql from 'highlight.js/lib/languages/sql';

  hljs.registerLanguage('markdown', markdownLang);
  hljs.registerLanguage('javascript', javascript);
  hljs.registerLanguage('typescript', typescript);
  hljs.registerLanguage('xml', xml);
  hljs.registerLanguage('css', css);
  hljs.registerLanguage('json', json);
  hljs.registerLanguage('python', python);
  hljs.registerLanguage('bash', bash);
  hljs.registerLanguage('yaml', yaml);
  hljs.registerLanguage('rust', rust);
  hljs.registerLanguage('go', go);
  hljs.registerLanguage('java', java);
  hljs.registerLanguage('c', c);
  hljs.registerLanguage('cpp', cpp);
  hljs.registerLanguage('sql', sql);
  hljs.registerAliases(['js', 'jsx', 'mjs'], { languageName: 'javascript' });
  hljs.registerAliases(['ts', 'tsx'], { languageName: 'typescript' });
  hljs.registerAliases(['html', 'svg'], { languageName: 'xml' });
  hljs.registerAliases(['yml'], { languageName: 'yaml' });
  hljs.registerAliases(['sh', 'shell', 'zsh'], { languageName: 'bash' });
  hljs.registerAliases(['c++'], { languageName: 'cpp' });
  hljs.registerAliases(['md'], { languageName: 'markdown' });

  function highlightCode(text, lang) {
    const language = lang && hljs.getLanguage(lang) ? lang : null;
    const result = language ? hljs.highlight(text, { language }) : hljs.highlightAuto(text);
    return result.value;
  }

  const FENCE_OPEN_RE = /^\s*(```+|~~~+)\s*(\S*)\s*$/;

  // highlight.js의 markdown 문법은 펜스 코드블록을 언어별로 하이라이팅하지 못한다(자체
  // TODO로 남아있는 제약 — markdown.js의 CODE 모드 참고). 그래서 원문을 "마크다운 산문"과
  // "펜스 코드" 구간으로 직접 나눠 각각 맞는 언어로 하이라이팅한 뒤 다시 이어붙인다.
  // split('\n')/join('\n')은 항상 원문과 1:1 왕복이 보장되므로, 구간 경계만 정확히 맞추면
  // 결과 textContent가 원문(mdSource)과 완전히 같다 — 검색/TOC의 문자 오프셋 계산이 이 위에서 그대로 성립.
  function highlightMarkdownSource(source) {
    const lines = source.split('\n');
    const htmlRuns = [];
    let i = 0;
    while (i < lines.length) {
      const open = FENCE_OPEN_RE.exec(lines[i]);
      if (!open) {
        const start = i;
        do { i++; } while (i < lines.length && !FENCE_OPEN_RE.test(lines[i]));
        htmlRuns.push(highlightCode(lines.slice(start, i).join('\n'), 'markdown'));
        continue;
      }
      const fenceChar = open[1][0];
      const lang = open[2] || null;
      const closeRe = new RegExp(`^\\s*${fenceChar}{${open[1].length},}\\s*$`);
      let j = i + 1;
      while (j < lines.length && !closeRe.test(lines[j])) j++;
      const hasClose = j < lines.length;
      const body = lines.slice(i + 1, j);
      const parts = [highlightCode(lines[i], 'markdown')];
      if (body.length > 0) parts.push(highlightCode(body.join('\n'), lang));
      if (hasClose) { parts.push(highlightCode(lines[j], 'markdown')); j++; }
      htmlRuns.push(parts.join('\n'));
      i = j;
    }
    return htmlRuns.join('\n');
  }

  // 헤딩 텍스트 → 앵커 id. GitHub 슬러그와 완전히 같진 않지만, TOC(buildMdToc)와 미리보기
  // 렌더러(아래 heading())가 같은 함수+같은 순서로 호출되는 한 서로 id가 항상 일치한다.
  function slugifyHeading(text, counts) {
    let slug = text.trim().toLowerCase()
      .replace(/[^\p{L}\p{N}\s-]/gu, '')
      .trim().replace(/\s+/g, '-');
    if (!slug) slug = 'section';
    const n = counts.get(slug) ?? 0;
    counts.set(slug, n + 1);
    return n === 0 ? slug : `${slug}-${n}`;
  }

  let previewHeadingSlugs = new Map(); // renderPreview() 호출마다 리셋 — buildMdToc의 카운터와 각자 독립
  marked.use({
    renderer: {
      code({ text, lang }) {
        const language = (lang || '').trim().split(/\s+/)[0];
        return `<pre class="hljs"><code>${highlightCode(text, language)}</code></pre>`;
      },
      heading({ tokens, depth }) {
        const plain = this.parser.parseInline(tokens, this.parser.textRenderer);
        const id = slugifyHeading(plain, previewHeadingSlugs);
        return `<h${depth} id="${id}">${this.parser.parseInline(tokens)}</h${depth}>\n`;
      },
    },
  });

  function renderPreview(source) {
    previewHeadingSlugs = new Map();
    return DOMPurify.sanitize(marked.parse(source, { async: false }));
  }

  // 인라인 마크업(굵게/기울임/코드/링크)을 대충 걷어낸 평문 — buildMdToc가 heading() 렌더러와
  // 똑같은 슬러그를 만들려고 쓰는 근사치다. 표/각주처럼 드문 인라인 문법은 다루지 않는다.
  function stripInlineMarkdown(text) {
    return text
      .replace(/`([^`]*)`/g, '$1')
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/[*_~]{1,3}([^*_~]+)[*_~]{1,3}/g, '$1');
  }

  // 원문에서 ATX 헤딩(# ~ ######)만 훑어 계층형 TOC를 만든다 — 코드 펜스 안의 '#'은 헤딩이
  // 아니므로 건너뛴다. mdOffset은 그 헤딩 줄이 시작하는 원문 문자 오프셋(코드 뷰 스크롤/검색/
  // 스크롤 싱크와 같은 좌표계), mdAnchor는 미리보기의 <h*> id와 짝을 이루는 앵커.
  export function buildMdToc(source) {
    const lines = source.split('\n');
    const counts = new Map();
    const flat = [];
    let inFence = false;
    let offset = 0;
    for (const line of lines) {
      const trimmed = line.trim();
      if (/^(```|~~~)/.test(trimmed)) { inFence = !inFence; offset += line.length + 1; continue; }
      if (!inFence) {
        const m = /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
        if (m) {
          const plain = stripInlineMarkdown(m[2].trim());
          flat.push({ depth: m[1].length, title: plain, mdOffset: offset, mdAnchor: slugifyHeading(plain, counts) });
        }
      }
      offset += line.length + 1;
    }
    const root = [];
    const stack = []; // [{depth, node}]
    for (const h of flat) {
      const node = { title: h.title, page: 0, y: null, mdOffset: h.mdOffset, mdAnchor: h.mdAnchor, children: [] };
      while (stack.length && stack[stack.length - 1].depth >= h.depth) stack.pop();
      (stack.length ? stack[stack.length - 1].node.children : root).push(node);
      stack.push({ depth: h.depth, node });
    }
    return root;
  }

  function flattenHeadings(nodes, out = []) {
    for (const n of nodes) { out.push(n); flattenHeadings(n.children, out); }
    return out;
  }

  // 'auto'는 앱 라이트/다크 설정을 따라가는 테마(색상은 스타일 블록의 .md-theme-auto 참고),
  // 나머지는 이름난 코드에디터 팔레트 — 앱 테마와 무관하게 항상 같은 색으로 고정된다.
  export const MD_THEMES = ['auto', 'github-light', 'github-dark', 'dracula', 'nord', 'solarized-light'];

  // 코드 뷰 + 미리보기의 인라인/펜스 코드에 쓰는 폰트 — 'default'면 이 기본 스택, 그 외엔
  // 시스템에서 실제로 스캔해온 폰트 이름(list_system_fonts, 설정 모달 쪽에서 호출)을 그대로
  // 앞에 붙이고 이 스택을 폴백으로 둔다.
  const DEFAULT_CODE_FONT_STACK = 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace';
</script>

<script>
  let { source = '', mode = 'preview', searchHits = [], activeSearchIndex = -1, theme = 'auto', codeFont = 'default', zoom = 1 } = $props();

  const codeFontStack = $derived(
    codeFont && codeFont !== 'default' ? `"${codeFont}", ${DEFAULT_CODE_FONT_STACK}` : DEFAULT_CODE_FONT_STACK
  );

  /** @type {HTMLElement|null} */
  let codeEl = $state(null);
  /** @type {HTMLElement|null} */
  let previewEl = $state(null);

  const codeHtml = $derived(highlightMarkdownSource(source));
  const previewHtml = $derived(renderPreview(source));
  const flatHeadings = $derived(flattenHeadings(buildMdToc(source)));

  // 검색 결과(원문 문자 오프셋 기준)를 코드 뷰 텍스트 노드 위에 <mark>로 덧씌운다. 미리보기는
  // 렌더링되며 원문 마크업이 사라져 오프셋이 안 맞으므로 하이라이트 대상에서 제외한다
  // (대신 검색 이동 시 가장 가까운 헤딩으로만 스크롤 — scrollToOffset 참고).
  function applyHighlights(container, hits, activeIndex) {
    if (!container || hits.length === 0) return;
    const nodes = [];
    let acc = 0, node;
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    while ((node = walker.nextNode())) {
      const len = node.nodeValue?.length ?? 0;
      nodes.push({ node, start: acc, end: acc + len });
      acc += len;
    }
    for (let i = hits.length - 1; i >= 0; i--) {
      const { start, end } = hits[i];
      // 하이라이트 문법(hljs 토큰 span) 경계를 넘나드는 매치는 건너뛴다 — 스크롤 이동 자체는
      // scrollToOffset이 오프셋으로 직접 하므로 여전히 그 위치로는 간다, 노란 표시만 없을 뿐.
      const entry = nodes.find(n => start >= n.start && end <= n.end);
      if (!entry) continue;
      const range = document.createRange();
      range.setStart(entry.node, start - entry.start);
      range.setEnd(entry.node, end - entry.start);
      const mark = document.createElement('mark');
      mark.className = i === activeIndex ? 'md-hit md-hit-active' : 'md-hit';
      range.surroundContents(mark);
    }
  }

  // 매치 표시만 여기서 반응형으로 처리하고, 실제 스크롤 이동은 scrollToOffset(외부에서 호출,
  // 미리보기 쪽 앵커 이동과 항상 같이 일어나야 해서)로 통일한다 — 여기서까지 스크롤하면 두 번 움직인다.
  $effect(() => {
    void codeHtml; // 소스/모드가 바뀌어 코드 뷰 DOM이 새로 그려질 때마다 다시 적용
    const el = codeEl;
    if (!el) return;
    applyHighlights(el, searchHits, activeSearchIndex);
  });

  // 컨테이너 안의 원문 문자 오프셋 위치로 정확히 "점프"한다(마커를 잠깐 심었다 지운다) —
  // 줄바꿈/폭 상관없이 항상 맞고, TOC/검색처럼 한 번에 딱 이동하는 액션에 쓴다. 스크롤 싱크처럼
  // 매 프레임 불리는 곳엔 DOM을 건드리지 않는 offsetToCodeTop을 대신 쓴다(아래).
  function scrollToTextOffset(container, offset) {
    if (!container) return;
    const walker = document.createTreeWalker(container, NodeFilter.SHOW_TEXT);
    let node, acc = 0;
    while ((node = walker.nextNode())) {
      const len = node.nodeValue?.length ?? 0;
      if (acc + len >= offset) {
        const range = document.createRange();
        range.setStart(node, Math.max(0, Math.min(offset - acc, len)));
        range.collapse(true);
        const marker = document.createElement('span');
        range.insertNode(marker);
        marker.scrollIntoView({ block: 'center' });
        marker.remove();
        container.normalize();
        return;
      }
      acc += len;
    }
    container.scrollTo({ top: container.scrollHeight });
  }

  // TOC 클릭/검색 이동 공용 진입점 — 코드 뷰는 원문 오프셋으로 정확히, 미리보기는 앵커 id로
  // 정확히(둘 다 준 경우) 또는 앵커만으로 스크롤한다.
  export function scrollToOffset(offset, anchor) {
    if (codeEl) scrollToTextOffset(codeEl, offset);
    if (previewEl && anchor) previewEl.querySelector('#' + CSS.escape(anchor))?.scrollIntoView({ block: 'start' });
  }

  export function scrollBy(dy) { codeEl?.scrollBy(0, dy); previewEl?.scrollBy(0, dy); }
  export function scrollByX(dx) { codeEl?.scrollBy(dx, 0); }
  export function halfPageScrollBy(sign) {
    if (codeEl) codeEl.scrollBy(0, sign * codeEl.clientHeight / 2);
    if (previewEl) previewEl.scrollBy(0, sign * previewEl.clientHeight / 2);
  }
  export function scrollToTop() { codeEl?.scrollTo(0, 0); previewEl?.scrollTo(0, 0); }
  export function scrollToBottom() {
    if (codeEl) codeEl.scrollTo(0, codeEl.scrollHeight);
    if (previewEl) previewEl.scrollTo(0, previewEl.scrollHeight);
  }

  // ── 분할 뷰 스크롤 싱크 ──────────────────────────────────────────────────
  // 헤딩을 "같은 지점"의 기준점으로 삼아, 두 창 사이를 구간별 선형보간으로 매핑한다. 헤딩이
  // 하나도 없으면 문서 전체를 한 구간으로 취급해 그냥 비례 스크롤이 된다(자연스러운 축소 경우).
  // 코드 쪽 좌표 변환은 Range.getBoundingClientRect만 읽고 DOM은 건드리지 않는다 — 스크롤
  // 이벤트마다(초당 수십 번) 노드를 삽입/삭제하면 버벅이므로 점프 전용 scrollToTextOffset과는
  // 별도로 가볍게 만들었다.
  function elTopWithin(container, el) {
    return el.getBoundingClientRect().top - container.getBoundingClientRect().top + container.scrollTop;
  }

  function offsetToCodeTop(offset) {
    if (!codeEl) return 0;
    const walker = document.createTreeWalker(codeEl, NodeFilter.SHOW_TEXT);
    let node, acc = 0;
    while ((node = walker.nextNode())) {
      const len = node.nodeValue?.length ?? 0;
      if (acc + len >= offset) {
        const range = document.createRange();
        const local = Math.max(0, Math.min(offset - acc, len));
        range.setStart(node, local);
        range.setEnd(node, local);
        return elTopWithin(codeEl, { getBoundingClientRect: () => range.getBoundingClientRect() });
      }
      acc += len;
    }
    return codeEl.scrollHeight;
  }

  // codeEl 뷰포트 맨 위(패딩 안쪽)에 지금 어떤 원문 오프셋이 보이는지 — WebKit/Chromium
  // 공용 API인 caretRangeFromPoint로 화면 좌표를 텍스트 위치로 되짚는다(Tauri의 세 플랫폼
  // 웹뷰가 전부 이걸 지원: WKWebView/WebView2/WebKitGTK).
  function codeTopOffset() {
    if (!codeEl || typeof document.caretRangeFromPoint !== 'function') return null;
    const rect = codeEl.getBoundingClientRect();
    const range = document.caretRangeFromPoint(rect.left + 12, rect.top + 22);
    if (!range) return null;
    const walker = document.createTreeWalker(codeEl, NodeFilter.SHOW_TEXT);
    let node, acc = 0;
    while ((node = walker.nextNode())) {
      if (node === range.startContainer) return acc + range.startOffset;
      acc += node.nodeValue?.length ?? 0;
    }
    return null;
  }

  // 헤딩들을 [원문 오프셋 ↔ 미리보기 px] 기준점으로 삼아, 양끝(문서 시작/끝)까지 포함한
  // 정렬된 좌표 쌍 목록을 만든다 — offsetToPreviewTop/previewTopToOffset이 이 위에서
  // 구간별 선형보간만 하면 되게 한다.
  function syncPoints() {
    if (!previewEl) return [];
    const pts = [{ offset: 0, top: 0 }];
    for (const h of flatHeadings) {
      const el = previewEl.querySelector('#' + CSS.escape(h.mdAnchor));
      if (el) pts.push({ offset: h.mdOffset, top: elTopWithin(previewEl, el) });
    }
    pts.push({ offset: source.length, top: previewEl.scrollHeight });
    return pts;
  }

  function offsetToPreviewTop(offset) {
    const pts = syncPoints();
    for (let i = 1; i < pts.length; i++) {
      if (offset <= pts[i].offset) {
        const a = pts[i - 1], b = pts[i];
        const frac = (offset - a.offset) / Math.max(1, b.offset - a.offset);
        return a.top + frac * (b.top - a.top);
      }
    }
    return previewEl ? previewEl.scrollHeight : 0;
  }

  function previewTopToOffset(top) {
    const pts = syncPoints();
    for (let i = 1; i < pts.length; i++) {
      if (top <= pts[i].top) {
        const a = pts[i - 1], b = pts[i];
        const frac = (top - a.top) / Math.max(1, b.top - a.top);
        return a.offset + frac * (b.offset - a.offset);
      }
    }
    return source.length;
  }

  let suppressSync = false;
  let syncPending = false;
  function withSyncGuard(fn) {
    if (suppressSync) return;
    if (syncPending) return;
    syncPending = true;
    requestAnimationFrame(() => {
      syncPending = false;
      suppressSync = true;
      fn();
      requestAnimationFrame(() => { suppressSync = false; });
    });
  }

  function onCodeScroll() {
    if (mode !== 'split' || !codeEl || !previewEl) return;
    const preview = previewEl;
    withSyncGuard(() => {
      const offset = codeTopOffset();
      if (offset !== null) preview.scrollTop = offsetToPreviewTop(offset);
    });
  }

  function onPreviewScroll() {
    if (mode !== 'split' || !codeEl || !previewEl) return;
    const code = codeEl, preview = previewEl;
    withSyncGuard(() => {
      code.scrollTop = offsetToCodeTop(previewTopToOffset(preview.scrollTop));
    });
  }

  // 분할 뷰로 막 들어왔을 때(또는 문서를 새로 열었을 때) 코드 뷰 기준으로 한 번 맞춰둔다 —
  // 안 그러면 이전에 프리뷰만 보다가 분할로 바꿨을 때 두 창이 서로 다른 위치를 보여준다.
  $effect(() => {
    if (mode === 'split' && codeEl && previewEl) {
      requestAnimationFrame(() => onCodeScroll());
    }
  });
</script>

<div class="md-root md-theme-{theme}" style="--md-code-font: {codeFontStack}; --md-zoom: {zoom}">
  {#if mode === 'split'}
    <div class="md-split">
      <pre class="md-code hljs" bind:this={codeEl} onscroll={onCodeScroll}><code>{@html codeHtml}</code></pre>
      <div class="md-preview" bind:this={previewEl} onscroll={onPreviewScroll}>{@html previewHtml}</div>
    </div>
  {:else if mode === 'code'}
    <pre class="md-code hljs md-code-full" bind:this={codeEl}><code>{@html codeHtml}</code></pre>
  {:else}
    <div class="md-preview md-preview-full" bind:this={previewEl}>{@html previewHtml}</div>
  {/if}
</div>

<style>
  /* ── 테마 ── 색상은 전부 --md-* 커스텀 프로퍼티로 뽑아뒀고, 실제 규칙(아래)은 이 변수만
     읽는다. md-theme-{name} 클래스가 이 변수들의 값을 정의 — 새 테마를 추가할 때 여기 블록
     하나만 더 쓰면 된다. auto는 앱 라이트/다크(--sh-*, --accent)를 따라가는 유일한 테마고,
     나머지는 이름난 에디터 팔레트라 앱 테마와 무관하게 고정된 색을 쓴다. */
  .md-theme-auto {
    --md-bg: var(--sh-1); --md-fg: var(--sh-23); --md-muted: var(--sh-16); --md-border: var(--sh-7);
    --md-code-bg: var(--sh-4); --md-block-bg: var(--sh-2); --md-accent: var(--accent);
    --md-hl-keyword: #ff7b72; --md-hl-string: #a5d6ff; --md-hl-number: #79c0ff; --md-hl-title: #d2a8ff;
    --md-hl-attribute: #ffa657; --md-hl-tag: #7ee787; --md-hl-comment: #8b949e;
    --md-hl-code-fg: #a5d6ff; --md-hl-code-bg: rgba(110,118,129,0.2);
    --md-hl-deletion-fg: #ffa198; --md-hl-deletion-bg: rgba(248,81,73,0.15); --md-hl-addition-bg: rgba(46,160,67,0.15);
  }
  :global(body.theme-light) .md-theme-auto {
    --md-hl-keyword: #d73a49; --md-hl-string: #032f62; --md-hl-number: #005cc5; --md-hl-title: #6f42c1;
    --md-hl-attribute: #e36209; --md-hl-tag: #22863a; --md-hl-comment: #6a737d;
    --md-hl-code-fg: #032f62; --md-hl-code-bg: rgba(175,184,193,0.25);
    --md-hl-deletion-fg: #b31d28; --md-hl-deletion-bg: #ffeef0; --md-hl-addition-bg: #f0fff4;
  }
  .md-theme-github-light {
    --md-bg: #ffffff; --md-fg: #24292e; --md-muted: #6a737d; --md-border: #d0d7de;
    --md-code-bg: rgba(175,184,193,.2); --md-block-bg: #f6f8fa; --md-accent: #0969da;
    --md-hl-keyword: #d73a49; --md-hl-string: #032f62; --md-hl-number: #005cc5; --md-hl-title: #6f42c1;
    --md-hl-attribute: #e36209; --md-hl-tag: #22863a; --md-hl-comment: #6a737d;
    --md-hl-code-fg: #032f62; --md-hl-code-bg: rgba(175,184,193,.25);
    --md-hl-deletion-fg: #b31d28; --md-hl-deletion-bg: #ffeef0; --md-hl-addition-bg: #f0fff4;
  }
  .md-theme-github-dark {
    --md-bg: #0d1117; --md-fg: #c9d1d9; --md-muted: #8b949e; --md-border: #30363d;
    --md-code-bg: rgba(110,118,129,.2); --md-block-bg: #161b22; --md-accent: #58a6ff;
    --md-hl-keyword: #ff7b72; --md-hl-string: #a5d6ff; --md-hl-number: #79c0ff; --md-hl-title: #d2a8ff;
    --md-hl-attribute: #ffa657; --md-hl-tag: #7ee787; --md-hl-comment: #8b949e;
    --md-hl-code-fg: #a5d6ff; --md-hl-code-bg: rgba(110,118,129,.2);
    --md-hl-deletion-fg: #ffa198; --md-hl-deletion-bg: rgba(248,81,73,.15); --md-hl-addition-bg: rgba(46,160,67,.15);
  }
  .md-theme-dracula {
    --md-bg: #282a36; --md-fg: #f8f8f2; --md-muted: #6272a4; --md-border: #44475a;
    --md-code-bg: rgba(98,114,164,.25); --md-block-bg: #21222c; --md-accent: #8be9fd;
    --md-hl-keyword: #ff79c6; --md-hl-string: #f1fa8c; --md-hl-number: #bd93f9; --md-hl-title: #50fa7b;
    --md-hl-attribute: #8be9fd; --md-hl-tag: #ff79c6; --md-hl-comment: #6272a4;
    --md-hl-code-fg: #f1fa8c; --md-hl-code-bg: rgba(98,114,164,.3);
    --md-hl-deletion-fg: #ff5555; --md-hl-deletion-bg: rgba(255,85,85,.15); --md-hl-addition-bg: rgba(80,250,123,.15);
  }
  .md-theme-nord {
    --md-bg: #2e3440; --md-fg: #d8dee9; --md-muted: #4c566a; --md-border: #3b4252;
    --md-code-bg: rgba(76,86,106,.4); --md-block-bg: #3b4252; --md-accent: #88c0d0;
    --md-hl-keyword: #81a1c1; --md-hl-string: #a3be8c; --md-hl-number: #b48ead; --md-hl-title: #88c0d0;
    --md-hl-attribute: #d08770; --md-hl-tag: #8fbcbb; --md-hl-comment: #4c566a;
    --md-hl-code-fg: #a3be8c; --md-hl-code-bg: rgba(76,86,106,.4);
    --md-hl-deletion-fg: #bf616a; --md-hl-deletion-bg: rgba(191,97,106,.15); --md-hl-addition-bg: rgba(163,190,140,.15);
  }
  .md-theme-solarized-light {
    --md-bg: #fdf6e3; --md-fg: #657b83; --md-muted: #93a1a1; --md-border: #eee8d5;
    --md-code-bg: rgba(147,161,161,.2); --md-block-bg: #eee8d5; --md-accent: #268bd2;
    --md-hl-keyword: #859900; --md-hl-string: #2aa198; --md-hl-number: #d33682; --md-hl-title: #268bd2;
    --md-hl-attribute: #cb4b16; --md-hl-tag: #b58900; --md-hl-comment: #93a1a1;
    --md-hl-code-fg: #cb4b16; --md-hl-code-bg: rgba(147,161,161,.25);
    --md-hl-deletion-fg: #dc322f; --md-hl-deletion-bg: rgba(220,50,47,.12); --md-hl-addition-bg: rgba(133,153,0,.12);
  }

  .md-root, .md-split, .md-code-full, .md-preview-full {
    width: 100%;
    height: 100%;
  }
  .md-split {
    display: flex;
    align-items: stretch;
  }
  .md-split > * {
    flex: 1 1 50%;
    min-width: 0;
    height: 100%;
    overflow: auto;
  }
  .md-split > .md-code {
    border-right: 1px solid var(--md-border);
  }

  .md-code {
    margin: 0;
    padding: 20px 24px;
    background: var(--md-bg);
    color: var(--md-fg);
    font-size: calc(13px * var(--md-zoom, 1));
    line-height: 1.6;
    font-family: var(--md-code-font);
    white-space: pre-wrap;
    word-break: break-word;
    box-sizing: border-box;
    overflow: auto;
  }

  .md-preview {
    background: var(--md-bg);
    color: var(--md-fg);
    padding: 32px 40px;
    box-sizing: border-box;
    overflow: auto;
    font: calc(15px * var(--md-zoom, 1))/1.7 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  }
  .md-preview :global(h1), .md-preview :global(h2) {
    border-bottom: 1px solid var(--md-border);
    padding-bottom: 0.3em;
  }
  .md-preview :global(h1) { font-size: 1.8em; margin: 0.8em 0 0.4em; }
  .md-preview :global(h2) { font-size: 1.4em; margin: 0.8em 0 0.4em; }
  .md-preview :global(h3) { font-size: 1.2em; margin: 0.8em 0 0.4em; }
  .md-preview :global(p) { margin: 0.6em 0; }
  .md-preview :global(a) { color: var(--md-accent); }
  .md-preview :global(code) {
    background: var(--md-code-bg);
    padding: 0.15em 0.35em;
    border-radius: 4px;
    font-size: 0.9em;
    font-family: var(--md-code-font);
  }
  .md-preview :global(pre) {
    background: var(--md-block-bg);
    padding: 14px 16px;
    border-radius: 6px;
    overflow: auto;
  }
  .md-preview :global(pre code) { background: none; padding: 0; }
  .md-preview :global(blockquote) {
    margin: 0.6em 0;
    padding: 0 1em;
    color: var(--md-muted);
    border-left: 0.25em solid var(--md-border);
  }
  .md-preview :global(table) { border-collapse: collapse; margin: 0.6em 0; }
  .md-preview :global(th), .md-preview :global(td) {
    border: 1px solid var(--md-border);
    padding: 6px 12px;
  }
  .md-preview :global(img) { max-width: 100%; }
  .md-preview :global(hr) { border: none; border-top: 1px solid var(--md-border); margin: 1.5em 0; }

  /* highlight.js 토큰 — 전부 위 테마 블록의 --md-hl-* 변수만 읽는다 */
  :global(.hljs-comment), :global(.hljs-quote) { color: var(--md-hl-comment); }
  :global(.hljs-keyword), :global(.hljs-selector-tag), :global(.hljs-literal),
  :global(.hljs-section), :global(.hljs-link) { color: var(--md-hl-keyword); }
  :global(.hljs-string), :global(.hljs-addition), :global(.hljs-attr),
  :global(.hljs-meta .hljs-string) { color: var(--md-hl-string); }
  :global(.hljs-number), :global(.hljs-symbol), :global(.hljs-bullet) { color: var(--md-hl-number); }
  :global(.hljs-title), :global(.hljs-name), :global(.hljs-built_in),
  :global(.hljs-type), :global(.hljs-class .hljs-title) { color: var(--md-hl-title); }
  :global(.hljs-attribute), :global(.hljs-variable), :global(.hljs-template-variable) { color: var(--md-hl-attribute); }
  :global(.hljs-tag) { color: var(--md-hl-tag); }
  :global(.hljs-emphasis) { font-style: italic; }
  :global(.hljs-strong) { font-weight: 600; }
  :global(.hljs-deletion) { color: var(--md-hl-deletion-fg); background: var(--md-hl-deletion-bg); }
  :global(.hljs-addition) { background: var(--md-hl-addition-bg); }
  /* 인라인/들여쓰기 코드 스팬(백틱) — highlight.js markdown 문법이 이걸 .hljs-code로만 묶고
     따로 색을 안 줘서, 지정 안 하면 backtick 텍스트가 본문과 구분 없이 그냥 검게 나온다 */
  :global(.hljs-code) { color: var(--md-hl-code-fg); background: var(--md-hl-code-bg); border-radius: 3px; }

  /* 검색 매치 하이라이트 — PDF 쪽 .highlight(노란색 계열)과 톤을 맞췄다. 텍스트는 테마와
     무관하게 항상 어둡게 고정(노란 배경 위엔 밝은 다크모드 글자색이 안 읽힌다) */
  :global(mark.md-hit) { background: rgba(255, 212, 0, 0.55); color: #1a1a1a; border-radius: 2px; }
  :global(mark.md-hit-active) { background: #ffd400; color: #1a1a1a; box-shadow: 0 0 0 2px #ffb300; }
</style>
