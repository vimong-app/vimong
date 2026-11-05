// : 명령 / 검색어 기록 — vim처럼 localStorage에 저장해서 앱을 재시작해도 남고,
// ↑/↓로 훑어본다. 입력창(명령/검색/AI 질문)마다 같은 동작이라 하나로 묶어서 공유한다.
export function createHistory(key, max) {
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
