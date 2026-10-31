# References and attribution

Visual layout, provider response formats and endpoint discovery follow CodeNotch. Provider glyph paths in gpui-app/assets/glyphs.json are adapted from Sources/Providers/GlyphOutline.swift (normalized polygon outlines), https://github.com/vinzdg/codenotch. The application is independently implemented in Rust. Claude Code empty-input credential renewal follows the approach documented in windows/codenotch/src/usage.rs in the same project; the CLI owns credential rotation.

MIT License

Copyright (c) 2026 Vinz

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

## Coucou

Companion UX and Windows architecture reference: Coucou by Louis Raillé,
https://github.com/Louis-CFM/coucou. No Coucou source code, character, brand,
icon, media or sounds are bundled. No companion character is included.

## Dependencies

GPUI (Apache-2.0), Lucide icons (ISC) and the other Rust crates carry their respective licenses in their distributions; dependency versions are pinned in `gpui-app/Cargo.lock`. The 0.1.x Tauri builds also used React (MIT), Tauri (MIT/Apache-2.0) and Radix UI (MIT), pinned in the repository history (bun.lock and src-tauri/Cargo.lock up to 0.2.1).
