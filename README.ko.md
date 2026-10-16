<h1 align="center">Vimong</h1>

<p align="center">Vim 키바인딩으로 조작하는 문서 뷰어. <code>j</code>, <code>k</code>, <code>gg</code>, <code>G</code>.</p>

<p align="center">
  <a href="https://github.com/textment/vimong/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg"></a>
  <a href="https://vimong.app"><img alt="Website" src="https://img.shields.io/badge/website-vimong.app-2ea043"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS-lightgrey?logo=apple&logoColor=white">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white">
  <img alt="Svelte" src="https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white">
</p>

<p align="center">
  <a href="README.md">English</a> · <b>한국어</b>
</p>

<p align="center">
  <a href="https://vimong.app/ko/">웹사이트</a> ·
  <a href="https://vimong.app/ko/key-mapping.html">전체 키바인딩 레퍼런스</a>
</p>

## 소개

Vimong은 화면을 마우스로 스크롤하는 대신 `j`/`k`/`gg`/`G` 같은 Vim 동작으로 읽는 문서 뷰어입니다.
탐색뿐 아니라 검색, 마크, 링크 힌트, 탭까지 Vim 사용자에게 익숙한 조작 방식을 그대로 따릅니다.

## 기능

- **Vim 키바인딩** — `j`/`k`/`h`/`l` 스크롤, `gg`/`G` 페이지 이동, `m{a-z}`로 마크 저장 후 `` '{a-z}``/`` `{a-z}``로 점프
- **정규식 검색** — `/`로 검색, vim magic 모드 정규식 문법(`\+`, `\{n,m}`, `\zs`/`\ze`, 문자 클래스 등) 지원, 한국어/일본어/중국어 검색 지원
- **링크 힌트** — `f`를 누르면 페이지의 모든 링크에 라벨이 붙고, 문자를 입력해 바로 이동
- **페이지 내보내기** — 페이지를 PNG/JPG/PDF로 내보내기, 여러 페이지를 한 파일로 병합, PDF는 암호 설정 가능
- **폰트 사용 분석** — 문서에 어떤 폰트가 얼마나 쓰였는지 비율로 확인
- **빠른 줌** — 배율/실제 크기/폭 맞춤/높이 맞춤을 즉시 전환
- **페이지 회전** — `r`/`R`로 90°씩 회전 (PDF, HWP 모두 지원)
- **SyncTeX 연동** — LaTeX 에디터와 PDF 사이를 정방향/역방향으로 동기화 이동
- **HWP 지원** — 한글(HWP) 문서를 PDF와 나란히 열람
- **탭** — `Cmd+T`로 새 탭, `gt`/`gT`로 전환, `:tabonly`/`:tabmove` 등 vim 탭 명령 지원
- **프레젠테이션 모드** — `p`로 전체화면 슬라이드쇼 전환
- **AI 연동 (MCP)** — Claude 등 AI 어시스턴트가 지금 보는 페이지를 읽고, 하이라이트를 정리하고, 페이지를 이동

각 기능의 상세 설명은 [웹사이트](https://vimong.app/ko/)에서 확인할 수 있습니다.

## 설치

macOS (Apple Silicon) — [Homebrew](https://brew.sh)로 설치:

```bash
brew install --cask vimong-app/tap/vimong
```

또는 [Releases](https://github.com/vimong-app/vimong/releases/latest)에서 `.dmg`를 내려받으세요.

## AI 연동 (MCP)

Vimong에는 [MCP](https://modelcontextprotocol.io) 서버가 내장되어 있어서, Claude 같은 AI 어시스턴트가 지금 열어둔 문서를 다룰 수 있습니다. "이 페이지 요약해줘", "선택한 문장 번역해줘", "이 그림 설명해줘", "3장으로 이동해줘" 같은 요청이 가능합니다.

**Claude Code**에 등록:

```bash
claude mcp add -s user vimong -- /Applications/Vimong.app/Contents/MacOS/vimong --mcp
```

**Claude Desktop**은 `~/Library/Application Support/Claude/claude_desktop_config.json`에 추가한 뒤 Claude Desktop을 완전히 종료했다가 다시 실행하세요:

```json
{
  "mcpServers": {
    "vimong": {
      "command": "/Applications/Vimong.app/Contents/MacOS/vimong",
      "args": ["--mcp"]
    }
  }
}
```

| 도구 | 기능 |
|------|------|
| `get_current_view` | 문서 경로, 현재 페이지, 전체 페이지 수 |
| `get_selection` | Vimong에서 선택한 텍스트 |
| `get_page_text` | 페이지 텍스트를 마크다운으로 (글자 크기로 제목 인식) |
| `get_page_image` | 페이지를 이미지로 — 그림, 수식, 스캔 페이지 |
| `get_notes` | 하이라이트(하이라이트한 문장 포함)와 메모 |
| `get_outline` | 목차와 페이지 번호 |
| `goto_page` | Vimong 화면을 해당 페이지로 이동 |
| `save_pages` | 페이지를 새 PDF로 저장 |
| `save_markdown` | 페이지를 마크다운 파일로 저장 |

페이지 인자를 생략하면 지금 보는 페이지가 대상이고, `"3"`, `"1-3,7"`, `"all"` 형식도 받습니다. Vimong이 실행 중이고 문서가 열려 있어야 합니다. 디스크의 파일 기준으로 읽기 때문에 `:w`로 저장하지 않은 주석은 포함되지 않습니다.

## 지원 포맷

PDF 외에도 MuPDF가 네이티브로 여는 포맷을 함께 지원합니다: **PDF · HWP · XPS · EPUB · CBZ · SVG · DOCX · XLSX · PPTX** 및 PNG/JPG/GIF/BMP/TIFF 이미지.

## 플랫폼

현재 **macOS**에서 사용할 수 있습니다. Windows · Linux는 개발 중입니다.

## Stack

| Layer | Choice |
|-------|--------|
| Desktop shell | Tauri 2 |
| PDF rendering | MuPDF (Rust) |
| Frontend | Svelte 5 |

## 키바인딩

전체 목록은 [키바인딩 레퍼런스](https://vimong.app/ko/key-mapping.html)에서 볼 수 있습니다. 자주 쓰는 것만 추리면:

### 탐색

| 키 | 동작 |
|----|------|
| `j` / `k` | 아래 / 위 스크롤 |
| `h` / `l` | 좌 / 우 스크롤 |
| `Ctrl+D` / `Ctrl+U` | 반 페이지 스크롤 |
| `J` / `K` | 다음 / 이전 페이지 |
| `gg` / `G` | 첫 / 마지막 페이지 |
| `nG` / `:n` | n번째 페이지로 이동 (예: `5G`, `:5`) |
| `m{a-z}` | 현재 위치를 마크로 저장 (파일별로 저장돼 다시 열어도 유지) |
| `'{a-z}` / `` `{a-z}`` | 저장한 마크로 이동 |
| `m{A-Z}` / `'{A-Z}` | 파일 마크 저장 / 이동 — 다른 파일이면 그 파일을 열어서 이동 |
| `Ctrl+O` / `Ctrl+I` | 이전 / 다음 점프 위치로 이동 |
| `f` | 링크 힌트 표시 후 이동 |
| `r` / `R` | 페이지 90° 시계 / 반시계 방향 회전 |
| `p` | 프레젠테이션 모드 (전체화면 슬라이드쇼) |
| `Ctrl+R` / `:set invert` | 페이지 다크 모드 켜기/끄기 (페이지 색 반전) |

### 확대/축소

| 키 | 동작 |
|----|------|
| `zi` / `zo` | 확대 / 축소 |
| `z0` | 100% |
| `zh` | 폭 맞춤 |
| `zv` / `zz` | 높이 맞춤 |
| `Cmd++` / `Cmd+-` | 확대 / 축소 |

### 파일 / 검색

| 키 | 동작 |
|----|------|
| `:e` | 파일 열기 (다이얼로그) |
| `:e {경로}` | 경로로 파일 열기 (Cmd+V 붙여넣기 가능) |
| `:q` / `Cmd+W` | 현재 탭 닫기 |
| `:print` / `Cmd+P` | 인쇄 |
| `:export [-m] [-p] [pagespec] <경로>` / `Cmd+Shift+E` | 이미지(PNG/JPG) 또는 PDF로 내보내기 |
| `Cmd+I` | 문서 정보 |
| `/` / `Cmd+F` | 검색 (한국어/일본어/중국어 지원) |
| `n` / `N` | 다음 / 이전 검색 결과 |
| `:nohlsearch` / `:noh` | 검색 하이라이트 끄기 |

### 탭

| 키 | 동작 |
|----|------|
| `Cmd+T` / `:tabnew` | 새 탭 |
| `:tabe {경로}` | 새 탭으로 파일 열기 |
| `:tabclose` | 현재 탭 닫기 |
| `:tabonly` / `:tabo` | 현재 탭만 남기고 나머지 닫기 |
| `gt` / `gT` | 다음 / 이전 탭 |
| `ngt` | n번째 탭 (예: `2gt`) |
| `:tabmove [N]` | 탭 위치 이동 |

## 개발

```bash
npm install
npm run tauri dev
```

Claude Code의 도움을 받아 개발했습니다.

## 빌드

```bash
npm run tauri build
```

## 라이센스

AGPL-3.0

## 의존성

| 라이브러리 | 라이센스 |
|---|---|
| [Tauri](https://tauri.app) | MIT / Apache 2.0 |
| [MuPDF](https://mupdf.com) | AGPL 3.0 |
| [mupdf (Rust bindings)](https://github.com/messense/mupdf-rs) | AGPL 3.0 |
| [@rhwp/core](https://github.com/edwardkim/rhwp) (HWP 파서/렌더러) | MIT |
| [Svelte](https://svelte.dev) / [SvelteKit](https://kit.svelte.dev) | MIT |
| [Vite](https://vitejs.dev) | MIT |
| [base64](https://github.com/marshallpierce/rust-base64) | MIT / Apache 2.0 |
| [sys-locale](https://github.com/1Password/sys-locale) | MIT / Apache 2.0 |
| tauri-plugin-{dialog,process,clipboard-manager,window-state,single-instance,opener} | MIT / Apache 2.0 |

앱 메뉴바의 **도움말 → 오픈소스**에서 전체 목록과 버전을 확인할 수 있습니다.
