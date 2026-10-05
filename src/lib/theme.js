// ── 앱 UI 크롬(사이드바/탭/모달 등) 라이트/다크 테마 ────────────────
// 문서 페이지 자체(캔버스)는 실제 종이처럼 항상 흰 배경이라 이 테마와 무관.

const THEME_KEY = 'vimong-theme';

export function loadThemePref() {
  try { return localStorage.getItem(THEME_KEY) || 'auto'; } catch (_) { return 'auto'; }
}

export function saveThemePref(pref) {
  try { localStorage.setItem(THEME_KEY, pref); } catch (_) { /* skip */ }
}

export function detectSystemTheme() {
  if (typeof matchMedia !== 'function') return 'dark';
  return matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}
