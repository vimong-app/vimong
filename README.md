<h1 align="center">Vimong</h1>

<p align="center">A document viewer you drive entirely with Vim keybindings. <code>j</code>, <code>k</code>, <code>gg</code>, <code>G</code>.</p>

<p align="center">
  <a href="https://github.com/textment/vimong/blob/main/LICENSE"><img alt="License" src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg"></a>
  <a href="https://vimong.app"><img alt="Website" src="https://img.shields.io/badge/website-vimong.app-2ea043"></a>
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS-lightgrey?logo=apple&logoColor=white">
  <img alt="Tauri" src="https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri&logoColor=white">
  <img alt="Rust" src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white">
  <img alt="Svelte" src="https://img.shields.io/badge/Svelte-5-FF3E00?logo=svelte&logoColor=white">
</p>

<p align="center">
  <b>English</b> · <a href="README.ko.md">한국어</a>
</p>

<p align="center">
  <a href="https://vimong.app/en/">Website</a> ·
  <a href="https://vimong.app/en/key-mapping.html">Full key mapping reference</a>
</p>

## Introduction

Vimong is a document viewer you read with Vim keybindings instead of scrolling with a mouse —
`j`/`k`/`gg`/`G` navigation, plus search, marks, link hints, and tabs, all following the
conventions Vim users already know.

## Features

- **Vim keybindings** — `j`/`k`/`h`/`l` scrolling, `gg`/`G` page jumps, `m{a-z}` to drop a mark and `` '{a-z}``/`` `{a-z}`` to jump back
- **Regex search** — search with vim magic-mode regex syntax (`\+`, `\{n,m}`, `\zs`/`\ze`, character classes, and more), with Korean/Japanese/Chinese search support
- **Link hints** — press `f` to label every link on the page, then type a letter to jump
- **Export pages** — export pages as PNG, JPG, or PDF; merge multiple pages into one file; password-protect PDF exports
- **Font usage breakdown** — see which fonts are used in the document and in what proportion
- **Fast zoom** — switch instantly between zoom levels, actual size, and fit-to-width/height
- **Page rotation** — rotate pages 90° at a time with `r`/`R` (works for both PDF and HWP)
- **SyncTeX support** — jump forward and backward between your LaTeX source and the PDF
- **HWP support** — open Korean HWP documents right alongside your PDFs
- **Tabs** — `Cmd+T` for a new tab, `gt`/`gT` to switch, `:tabonly`/`:tabmove` and other vim tab commands
- **Presentation mode** — `p` to enter a fullscreen slideshow
- **AI integration (MCP)** — let Claude and other AI assistants see the page you're on, read your highlights, and jump pages for you

See full descriptions on the [website](https://vimong.app/en/).

## Install

macOS (Apple Silicon) via [Homebrew](https://brew.sh):

```bash
brew install --cask vimong-app/tap/vimong
```

Or download the `.dmg` from [Releases](https://github.com/vimong-app/vimong/releases/latest).

## AI integration (MCP)

Vimong ships a built-in [MCP](https://modelcontextprotocol.io) server, so AI assistants such as Claude can work with the document you have open — "summarize this page", "translate the selected sentence", "explain this figure", "go to chapter 3".

Register it with **Claude Code**:

```bash
claude mcp add -s user vimong -- /Applications/Vimong.app/Contents/MacOS/vimong --mcp
```

Or with **Claude Desktop**, in `~/Library/Application Support/Claude/claude_desktop_config.json` (then quit and relaunch Claude Desktop):

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

| Tool | What it does |
|------|--------------|
| `get_current_view` | Document path, current page and page count |
| `get_selection` | Text selected in Vimong |
| `get_page_text` | Page text as Markdown (headings detected from font size) |
| `get_page_image` | Page rendered as an image — figures, equations, scanned pages |
| `get_notes` | Your highlights (with the highlighted text) and notes |
| `get_outline` | Table of contents with page numbers |
| `goto_page` | Move Vimong to a page |
| `save_pages` | Save pages as a new PDF |
| `save_markdown` | Save pages as a Markdown file |

Page arguments default to the page you're viewing; they also accept `"3"`, `"1-3,7"` or `"all"`. Vimong must be running with a document open. Tools read the file on disk, so annotations not yet saved with `:w` are not included.

## Supported formats

Beyond PDF, Vimong supports every format MuPDF opens natively: **PDF · HWP · XPS · EPUB · CBZ · SVG · DOCX · XLSX · PPTX**, plus PNG/JPG/GIF/BMP/TIFF images.

## Platforms

Currently available on **macOS**. Windows and Linux support is in development.

## Stack

| Layer | Choice |
|-------|--------|
| Desktop shell | Tauri 2 |
| PDF rendering | MuPDF (Rust) |
| Frontend | Svelte 5 |

## Key bindings

The full list is available in the [key mapping reference](https://vimong.app/en/key-mapping.html). The most commonly used ones:

### Navigation

| Key | Action |
|-----|--------|
| `j` / `k` | Scroll down / up |
| `h` / `l` | Scroll left / right |
| `Ctrl+D` / `Ctrl+U` | Scroll half page down/up |
| `J` / `K` | Next / previous page |
| `gg` / `G` | First / last page |
| `nG` | Go to page n (e.g. `5G`) |
| `m{a-z}` | Save current position as a mark |
| `'{a-z}` / `` `{a-z}`` | Jump to a saved mark |
| `Ctrl+O` / `Ctrl+I` | Jump to older / newer position |
| `f` | Show link hints, then jump |
| `r` / `R` | Rotate the page 90° clockwise / counterclockwise |
| `p` | Presentation mode (fullscreen slideshow) |

### Zoom

| Key | Action |
|-----|--------|
| `zi` / `zo` | Zoom in / out |
| `z0` | 100% size |
| `zh` | Fit width |
| `zv` / `zz` | Fit height |
| `Cmd++` / `Cmd+-` | Zoom in / out |

### File / Search

| Key | Action |
|-----|--------|
| `:e` | Open file (dialog) |
| `:e {path}` | Open file by path (Cmd+V to paste) |
| `:q` / `Cmd+W` | Close current tab |
| `:print` / `Cmd+P` | Print |
| `:export [-m] [-p] [pagespec] <path>` / `Cmd+Shift+E` | Export as image (PNG/JPG) or PDF |
| `Cmd+I` | Document info |
| `/` / `Cmd+F` | Search (Korean/Japanese/Chinese supported) |
| `n` / `N` | Next / previous match |
| `:nohlsearch` / `:noh` | Clear search highlighting |

### Tabs

| Key | Action |
|-----|--------|
| `Cmd+T` / `:tabnew` | New tab |
| `:tabe {path}` | Open file in a new tab |
| `:tabclose` | Close current tab |
| `:tabonly` / `:tabo` | Close all other tabs |
| `gt` / `gT` | Next / previous tab |
| `ngt` | Go to tab n (e.g. `2gt`) |
| `:tabmove [N]` | Move tab position |

## Development

```bash
npm install
npm run tauri dev
```

Built with the help of Claude Code.

## Build

```bash
npm run tauri build
```

## License

AGPL-3.0

## Dependencies

| Library | License |
|---|---|
| [Tauri](https://tauri.app) | MIT / Apache 2.0 |
| [MuPDF](https://mupdf.com) | AGPL 3.0 |
| [mupdf (Rust bindings)](https://github.com/messense/mupdf-rs) | AGPL 3.0 |
| [@rhwp/core](https://github.com/edwardkim/rhwp) (HWP parser/renderer) | MIT |
| [Svelte](https://svelte.dev) / [SvelteKit](https://kit.svelte.dev) | MIT |
| [Vite](https://vitejs.dev) | MIT |
| [base64](https://github.com/marshallpierce/rust-base64) | MIT / Apache 2.0 |
| [sys-locale](https://github.com/1Password/sys-locale) | MIT / Apache 2.0 |
| tauri-plugin-{dialog,process,clipboard-manager,window-state,single-instance,opener} | MIT / Apache 2.0 |

The full list with versions is available in the app menu under **Help → Open Source**.
