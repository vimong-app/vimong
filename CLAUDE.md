# Vimong — Vim-keybinding PDF Viewer

## Stack

| Layer | Choice |
|-------|--------|
| Desktop shell | Tauri |
| PDF rendering | MuPDF (all platforms) |
| Frontend | Svelte |
| Backend | Rust |

## Platforms

- Windows
- Linux
- macOS

## Core Feature: Vim Keybindings

### Navigation (MVP)
| Key | Action |
|-----|--------|
| `j` | Scroll down |
| `k` | Scroll up |
| `l` | Scroll right |
| `h` | Scroll left |
| `gg` | First page |
| `G` | Last page |
| `{n}G` | Go to page n (e.g. `10G` → page 10) |
| `Ctrl-d` / `Ctrl-u` | Half-page scroll down/up |
| `m{a-z}` | Set mark at current position |
| `'{a-z}` / `` `{a-z} `` | Jump to mark |
| `p` | Toggle presentation mode (full screen + fit page to screen, like PowerPoint slide show) |

### File
| Key | Action |
|-----|--------|
| `:e {path}` | Open file by path |
| `:e` (no arg) | Open native file picker dialog |
| `:q` | Quit |
| `:print` / `Ctrl-p` | Print document |

### Zoom
| Key | Action |
|-----|--------|
| `zi` | Zoom in |
| `zo` | Zoom out |
| `z0` | Reset zoom |
| `zh` | Fit to window width |
| `zv` / `zz` | Fit to window height |

### Search
| Key | Action |
|-----|--------|
| `/` | Open search (forward) |
| `n` | Next match |
| `N` | Previous match |

## Architecture

```
Svelte UI
  ↕ Tauri invoke
Rust core
  └── MuPDF bindings → rendered page bitmap
```

- PDF rendering runs in Rust (MuPDF)
- Rendered pages sent to Svelte as base64 image or raw bytes
- Vim key handling in Svelte (keydown events)
- Only page render commands cross the JS↔Rust boundary

## Implementation Phases

1. Tauri + Svelte scaffold
2. MuPDF Rust binding + render single page
3. Basic page navigation (j/k/gg/G)
4. Vim command mode (`:e`, `:q`, `{n}G`)
5. Search (`/`, `n`, `N`)
6. Prefetch adjacent pages for smooth navigation

## Conventions

- Rust: standard `cargo fmt` / `clippy`
- Svelte: no UI framework, plain Svelte components
- No CSS framework — minimal custom CSS only
- Key handling: single `keydown` listener on `document`, mode-based state machine (normal / command / search)
