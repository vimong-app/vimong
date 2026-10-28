<script>
  // :split / :vsplit로 여는 두 번째 창 — 같은 문서의 다른 위치를 나란히 보는 참조용 창이다.
  // 메인 뷰의 상태(검색·커서·주석·링크·보기 모드)는 전부 단일 뷰를 전제로 얽혀 있어서 공유하지
  // 않고, 이 창은 자기 스크롤/배율만 갖고 연속 스크롤로 페이지를 직접 render_page해서 그린다.
  // 키는 부모의 단일 keydown 리스너가 이 창이 활성일 때 handleKey로 넘긴다(처리 못 한 키는
  // false를 돌려줘서 부모가 이어서 처리 — :, / 등).
  // ponytail: 검색 하이라이트·주석 오버레이·링크·텍스트 커서 없음, 필요해지면 메인 뷰 로직을 컴포넌트로 빼서 공유
  import { onMount, onDestroy, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';

  let { rotationOf, initialScale = 1, active = false, onactivate, page = $bindable(0) } = $props();

  const PAD = 20; // .pages의 padding — 페이지 맨 위를 이만큼 띄워 맞춘다(메인 .viewer와 같게)
  const STEP = 60; // j/k 한 번에 움직일 거리(메인 scrollStep과 같게)

  let el = $state();
  let sizes = $state([]); // [w, h] (scale 1, 회전 전)
  let scale = $state(initialScale);
  let wraps = [];
  let canvases = [];
  let observer;
  const visible = new Set();
  let count = '';
  let pending = ''; // 'g' | 'z'

  function dims(i) {
    const s = sizes[i];
    if (!s) return [0, 0];
    const r = rotationOf(i);
    return r === 90 || r === 270 ? [s[1], s[0]] : s;
  }

  async function paint(i) {
    const canvas = canvases[i];
    const key = `${scale}:${rotationOf(i)}`;
    if (!canvas || canvas.dataset.scale === key) return;
    canvas.dataset.scale = key; // 응답 오기 전에 표시해서 같은 페이지 중복 요청을 막는다
    try {
      const dpr = window.devicePixelRatio || 1;
      const buf = await invoke('render_page', { pageNum: i, scale: scale * dpr, rotation: rotationOf(i) });
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

  function gotoPage(n) {
    const i = Math.max(0, Math.min(n, sizes.length - 1));
    if (wraps[i]) el.scrollTop = wraps[i].offsetTop - PAD;
    page = i;
  }

  // 배율을 바꿔도 보던 페이지 안의 같은 지점에 머문다
  async function setScale(s) {
    const p = currentPageFromScroll();
    const frac = wraps[p] ? (el.scrollTop - wraps[p].offsetTop) / wraps[p].offsetHeight : 0;
    scale = Math.min(Math.max(Math.round(s * 100) / 100, 0.25), 12);
    await tick();
    if (wraps[p]) el.scrollTop = wraps[p].offsetTop + frac * wraps[p].offsetHeight;
    for (const i of visible) paint(i);
  }

  export function focus() { el?.focus(); }

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
    sizes = await invoke('get_all_page_sizes');
    await tick();
    gotoPage(page);
  });

  onDestroy(() => observer?.disconnect());
</script>

<div class="split-pane" class:active bind:this={el} tabindex="-1"
     onmousedown={() => onactivate?.()}
     onscroll={() => { page = currentPageFromScroll(); }}>
  <div class="pages">
    {#each sizes as _, i (i)}
      {@const [pw, ph] = dims(i)}
      <div class="wrap" bind:this={wraps[i]} use:lazy={i} style="width:{pw * scale}px;height:{ph * scale}px">
        <canvas bind:this={canvases[i]}></canvas>
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
  .wrap { background: #fff; box-shadow: 0 2px 16px rgba(0,0,0,0.6); }
  canvas { display: block; width: 100%; height: 100%; }
  :global(body.invert-pages) .split-pane canvas { filter: invert(0.88) hue-rotate(180deg); }
</style>
