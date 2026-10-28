<script>
  // :split / :vsplit로 여는 두 번째 창 — 같은 문서의 다른 위치나 다른 문서를 나란히 보는 참조용 창.
  // 문서는 Rust 쪽 분할 창 전용 칸(open_split_doc, 부모가 열어 둠)을 split: true로 그린다.
  // 메인 뷰의 상태(검색·커서·주석·링크·보기 모드)는 전부 단일 뷰를 전제로 얽혀 있어서 공유하지
  // 않고, 이 창은 자기 스크롤/배율/회전만 갖고 연속 스크롤로 페이지를 직접 render_page해서 그린다.
  // 키는 부모의 단일 keydown 리스너가 이 창이 활성일 때 handleKey로 넘긴다(처리 못 한 키는
  // false를 돌려줘서 부모가 이어서 처리 — :, / 등).
  // ponytail: 검색 하이라이트·주석 오버레이·링크·텍스트 커서 없음, 필요해지면 메인 뷰 로직을 컴포넌트로 빼서 공유
  import { onMount, onDestroy, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  let { path, showName = false, initialRotations = {}, initialScale = 1, active = false, onactivate, page = $bindable(0),
        searchHits = [], activeMatch = null } = $props();

  const PAD = 20; // .pages의 padding — 페이지 맨 위를 이만큼 띄워 맞춘다(메인 .viewer와 같게)
  const STEP = 60; // j/k 한 번에 움직일 거리(메인 scrollStep과 같게)

  let el = $state();
  let sizes = $state([]); // [w, h] (scale 1, 회전 전)
  let scale = $state(initialScale);
  let rotations = $state({ ...initialRotations }); // { [page]: 0|90|180|270 } — r/R, 메인 창과 별개
  const rot = (i) => rotations[i] ?? 0;
  // 검색 하이라이트 — 부모가 이 창 문서에서 찾은 결과(searchHits)와 지금 매치(activeMatch)를 넘긴다
  let hitsByPage = $derived(new Map(searchHits.map(r => [r.page, r.rects])));
  let wraps = [];
  let canvases = [];
  let observer;
  const visible = new Set();
  let count = '';
  let pending = ''; // 'g' | 'z'

  function dims(i) {
    const s = sizes[i];
    if (!s) return [0, 0];
    const r = rot(i);
    return r === 90 || r === 270 ? [s[1], s[0]] : s;
  }

  async function paint(i) {
    const canvas = canvases[i];
    const key = `${scale}:${rot(i)}`;
    if (!canvas || canvas.dataset.scale === key) return;
    canvas.dataset.scale = key; // 응답 오기 전에 표시해서 같은 페이지 중복 요청을 막는다
    try {
      const dpr = window.devicePixelRatio || 1;
      const buf = await invoke('render_page', { pageNum: i, scale: scale * dpr, rotation: rot(i), split: true });
      if (canvas.dataset.scale !== key) return; // 그 사이 배율이 바뀌었거나 화면 밖으로 나가 비워짐
      const v = new DataView(buf);
      const w = v.getUint32(0, true), h = v.getUint32(4, true);
      canvas.width = w; canvas.height = h;
      canvas.getContext('2d').putImageData(new ImageData(new Uint8ClampedArray(buf, 8), w, h), 0, 0);
    } catch (_) {
      delete canvas.dataset.scale;
    }
  }

  // 화면 근처에 온 페이지만 그리고, 멀어지면 캔버스를 비워 큰 문서에서 메모리가 쌓이지 않게 한다
  function lazy(node, i) {
    node.dataset.page = i;
    observer.observe(node);
    return { destroy: () => { observer.unobserve(node); visible.delete(i); } };
  }

  function currentPageFromScroll() {
    const y = el.scrollTop + PAD + 1;
    let p = 0;
    for (let i = 0; i < wraps.length; i++) {
      if (wraps[i] && wraps[i].offsetTop <= y) p = i;
      else break;
    }
    return p;
  }

  // y: 페이지 안 세로 위치(배율 적용 전 좌표) — 목차 항목 점프용. 없으면 페이지 맨 위.
  export function gotoPage(n, y = null) {
    const i = Math.max(0, Math.min(n, sizes.length - 1));
    if (wraps[i]) el.scrollTop = wraps[i].offsetTop + (y != null ? y * scale : -PAD);
    // page는 여기서 바꾸지 않는다 — 스크롤 이벤트(onScroll)가 바꿔야 zc 상태의 가로 맞춤이 같이 돈다
  }

  // 배율을 바꿔도 보던 페이지 안의 같은 지점에 머문다
  async function setScale(s) {
    await keepPlace(() => { scale = Math.min(Math.max(Math.round(s * 100) / 100, 0.25), 12); });
  }

  // 보던 페이지 안의 같은 지점을 유지한 채 레이아웃을 바꾼다(배율/회전/리로드 공용)
  async function keepPlace(change) {
    const p = currentPageFromScroll();
    const frac = wraps[p] ? (el.scrollTop - wraps[p].offsetTop) / wraps[p].offsetHeight : 0;
    await change();
    await tick();
    if (wraps[p]) el.scrollTop = wraps[p].offsetTop + frac * wraps[p].offsetHeight;
    for (const i of visible) paint(i);
  }

  function rotate(delta) {
    const p = currentPageFromScroll();
    keepPlace(() => { rotations[p] = (rot(p) + delta + 360) % 360; });
  }

  // 파일이 디스크에서 바뀌면(LaTeX 재컴파일 등) 다시 열어 그린다 — 메인 창의 자동 리로드와 같은
  // 1초 폴링. 메인 창과 문서를 따로 들고 있어서 메인 쪽 리로드를 따라가지 않으므로 직접 한다.
  // 암호 걸린 문서는 암호 없이 다시 열 수 없어 그대로 둔다.
  let mtime = 0;
  async function checkChanged() {
    const m = await invoke('get_file_mtime', { path }).catch(() => 0);
    if (!m || m === mtime) return;
    const first = !mtime;
    mtime = m;
    if (first) return;
    const r = await invoke('open_split_doc', { path }).catch(() => null);
    if (!r || r.needs_password) return;
    await keepPlace(async () => {
      sizes = await invoke('get_all_page_sizes', { split: true });
      for (const c of canvases) if (c) delete c.dataset.scale;
    });
  }

  // zc — 메인 창의 fitToContentWidth와 같다: 여백 뺀 내용 폭에 맞추고 가로 스크롤을 내용 왼쪽 끝으로.
  // 그때의 배율을 기억해 두고, 배율이 그대로인 동안 페이지가 바뀌면 그 페이지 기준으로 다시 맞춘다.
  let contentFitScale = null;
  async function contentBbox(p) {
    return invoke('get_page_content_bbox', { pageNum: p, rotation: rot(p), split: true }).catch(() => null);
  }
  function alignContentLeft(p, x0) {
    if (wraps[p]) el.scrollLeft += wraps[p].getBoundingClientRect().left + x0 * scale - el.getBoundingClientRect().left - PAD;
  }
  async function fitContentWidth() {
    const p = currentPageFromScroll();
    const b = await contentBbox(p);
    const pw = dims(p)[0];
    await setScale((el.clientWidth - 2 * PAD) / (b ? b[2] - b[0] : pw)); // 빈 페이지면 zh처럼
    contentFitScale = b ? scale : null;
    if (b) alignContentLeft(p, b[0]);
  }
  async function onScroll() {
    const p = currentPageFromScroll();
    if (p === page) return;
    page = p;
    if (scale !== contentFitScale) return;
    const b = await contentBbox(p);
    if (b && p === page && scale === contentFitScale) alignContentLeft(p, b[0]);
  }

  export function focus() { el?.focus(); }
  export function getScale() { return scale; }

  // 지금 보는 위치 — 페이지와 그 안의 세로 위치(배율 적용 전 좌표, gotoPageAndY와 같은 단위)
  export function view() {
    const p = currentPageFromScroll();
    return { page: p, y: wraps[p] ? Math.max(0, el.scrollTop - wraps[p].offsetTop) / scale : 0 };
  }

  // 처리했으면 true. 숫자 접두어는 j/k/J/K/G/gg/Ctrl-d/u에 count로 쓴다.
  export async function handleKey(e) {
    const key = e.key;
    const n = count ? parseInt(count, 10) : 1;
    if (pending) {
      const p = pending;
      pending = '';
      if (p === 'g' && key === 'g') { gotoPage(count ? n - 1 : 0); count = ''; return true; }
      if (p === 'z') {
        const [pw, ph] = dims(currentPageFromScroll());
        if (key === 'i') await setScale(scale + 0.25 * n);
        else if (key === 'o') await setScale(scale - 0.25 * n);
        else if (key === '0') await setScale(count ? n / 100 : 1);
        else if (key === 'h' && pw) await setScale((el.clientWidth - 2 * PAD) / pw);
        else if ((key === 'v' || key === 'z') && ph) await setScale((el.clientHeight - 2 * PAD) / ph);
        else if (key === 'c') await fitContentWidth();
        count = '';
        return true;
      }
      count = '';
      return false;
    }
    if (/^[0-9]$/.test(key) && !(key === '0' && !count) && !e.ctrlKey && !e.metaKey) { count += key; return true; }
    const ctrl = e.ctrlKey || e.metaKey;
    let handled = true;
    if (key === 'Escape') {}
    else if (!ctrl && (key === 'j' || key === 'ArrowDown')) el.scrollBy(0, STEP * n);
    else if (!ctrl && (key === 'k' || key === 'ArrowUp')) el.scrollBy(0, -STEP * n);
    else if (!ctrl && (key === 'l' || key === 'ArrowRight')) el.scrollBy(STEP * n, 0);
    else if (!ctrl && (key === 'h' || key === 'ArrowLeft')) el.scrollBy(-STEP * n, 0);
    else if (e.ctrlKey && key === 'e') el.scrollBy(0, STEP * n);
    else if (e.ctrlKey && key === 'y') el.scrollBy(0, -STEP * n);
    else if (ctrl && (key === 'd' || key === 'u')) {
      const ph = dims(currentPageFromScroll())[1] * scale || el.clientHeight;
      el.scrollBy(0, (key === 'd' ? 1 : -1) * (ph / 2) * n);
    }
    else if (!ctrl && key === 'J') gotoPage(currentPageFromScroll() + n);
    else if (!ctrl && key === 'K') gotoPage(currentPageFromScroll() - n);
    else if (!ctrl && key === 'G') gotoPage(count ? n - 1 : sizes.length - 1);
    else if (!ctrl && (key === 'g' || key === 'z')) { pending = key; return true; }
    else if (!ctrl && key === 'r') rotate(90);
    else if (!ctrl && key === 'R') rotate(-90);
    else handled = false;
    if (handled) e.preventDefault();
    count = '';
    return handled;
  }

  onMount(async () => {
    observer = new IntersectionObserver((entries) => {
      for (const en of entries) {
        const i = Number(en.target.dataset.page);
        if (en.isIntersecting) { visible.add(i); paint(i); }
        else {
          visible.delete(i);
          const c = canvases[i];
          if (c) { c.width = 0; c.height = 0; delete c.dataset.scale; }
        }
      }
    }, { root: el, rootMargin: '600px 0px' });
    sizes = await invoke('get_all_page_sizes', { split: true });
    await tick();
    gotoPage(page);
    checkChanged();
    reloadTimer = setInterval(checkChanged, 1000);
  });

  let reloadTimer;
  onDestroy(() => { observer?.disconnect(); clearInterval(reloadTimer); });
</script>

<div class="split-pane" class:active bind:this={el} tabindex="-1"
     onmousedown={() => onactivate?.()}
     onscroll={onScroll}>
  {#if showName}<div class="split-name" title={path}>{path.split('/').pop()}</div>{/if}
  <div class="pages">
    {#each sizes as _, i (i)}
      {@const [pw, ph] = dims(i)}
      <div class="wrap" bind:this={wraps[i]} use:lazy={i} style="width:{pw * scale}px;height:{ph * scale}px">
        <canvas bind:this={canvases[i]}></canvas>
        <!-- ponytail: 회전한 페이지엔 하이라이트를 안 그린다(좌표 변환 생략), 필요하면 메인 뷰의 회전 변환을 가져올 것 -->
        {#if rot(i) === 0}
          {#each hitsByPage.get(i) ?? [] as r}
            <div class="hit" class:active={activeMatch?.page === i && activeMatch.rect === r}
                 style="left:{r[0] * scale}px;top:{r[1] * scale}px;width:{(r[2] - r[0]) * scale}px;height:{(r[3] - r[1]) * scale}px"></div>
          {/each}
        {/if}
      </div>
    {/each}
  </div>
</div>

<style>
  .split-pane {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: auto;
    overflow-anchor: none;
    outline: none;
  }
  .split-pane.active { box-shadow: inset 0 0 0 3px rgba(224, 166, 64, 0.9); }
  .pages {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 16px;
    padding: 20px;
    min-width: max-content;
  }
  /* 메인 창과 다른 문서를 볼 때만 — 어느 창이 어느 문서인지 헷갈리지 않게 파일 이름을 띄운다 */
  .split-name {
    position: sticky;
    top: 0;
    left: 0;
    z-index: 1;
    width: max-content;
    max-width: 100%;
    padding: 2px 8px;
    font-size: 12px;
    color: var(--sh-21, #ddd);
    background: var(--sh-2, rgba(0,0,0,0.6));
    border-bottom-right-radius: 4px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* 메인 창 .highlight와 같은 색 */
  .hit { position: absolute; background: rgba(255, 200, 0, 0.25); pointer-events: none; }
  .hit.active { background: rgba(255, 230, 0, 0.65); outline: 2px solid #ffd400; }
  .wrap { position: relative; background: #fff; box-shadow: 0 2px 16px rgba(0,0,0,0.6); }
  canvas { display: block; width: 100%; height: 100%; }
  :global(body.invert-pages) .split-pane canvas { filter: invert(0.88) hue-rotate(180deg); }
</style>
