# Bundled font provenance

Every font file under `crates/ebbtop-gui/assets/fonts/` is committed to this
repository, compiled in with `include_bytes!` (no build-time or runtime
fetch), and pinned to an exact upstream commit. This slice bundles the
upstream files **unmodified** — no subsetting — so the committed bytes are the
upstream bytes.

| File | Upstream path | Bytes | SHA-256 |
| --- | --- | ---: | --- |
| `NotoSansSC-Regular.otf` | `Sans/SubsetOTF/SC/NotoSansSC-Regular.otf` | 8,331,336 | `faa6c9df652116dde789d351359f3d7e5d2285a2b2a1f04a2d7244df706d5ea9` |
| `NotoSansTC-Regular.otf` | `Sans/SubsetOTF/TC/NotoSansTC-Regular.otf` | 5,683,368 | `5bab0cb3c1cf89dde07c4a95a4054b195afbcfe784d69d75c340780712237537` |

- **Total bundled font bytes:** 14,014,704 (13.4 MiB), under the 16 MiB cap.
- **Upstream project:** Noto CJK (`notofonts/noto-cjk`), release Sans 2.004.
- **Exact commit:** `523d033d6cb47f4a80c58a35753646f5c3608a78`
  (release `Sans2.004`).
- **Source URLs (raw at that commit):**
  - https://raw.githubusercontent.com/notofonts/noto-cjk/523d033d6cb47f4a80c58a35753646f5c3608a78/Sans/SubsetOTF/SC/NotoSansSC-Regular.otf
  - https://raw.githubusercontent.com/notofonts/noto-cjk/523d033d6cb47f4a80c58a35753646f5c3608a78/Sans/SubsetOTF/TC/NotoSansTC-Regular.otf
  - https://raw.githubusercontent.com/notofonts/noto-cjk/523d033d6cb47f4a80c58a35753646f5c3608a78/LICENSE
- **Retrieval:** `curl -sSL -o <file> <url>` on 2026-11-22 against the commit
  (not a moving branch), then `sha256sum` verified against the table above.
- **Format:** OpenType/CFF (`CFF ` table present, no `glyf`), font face index 0.
  This renderer (epaint 0.37: `skrifa` + `vello_cpu`/harfrust) rasterizes CFF
  outlines; the coverage and rendered-ink tests in
  `crates/ebbtop-gui/src/app/tests/l10n.rs` prove it for the exact bytes, in
  both the `Proportional` and `Monospace` families.

## License

Both files are licensed under the **SIL Open Font License, Version 1.1**. The
upstream license file for these bytes is committed unmodified beside the fonts
as `LICENSE-OFL.txt` (first line: `This Font Software is licensed under the SIL
Open Font License,`; the full text follows, including `SIL OPEN FONT LICENSE
Version 1.1 - 26 February 2007`).

Copyright and license lines read from the `name` table of the exact committed
bytes (nameID 0 / 13 / 14, both files identical):

- nameID 0 (copyright): `© 2014-2021 Adobe (http://www.adobe.com/).`
- nameID 13 (license): `This Font Software is licensed under the SIL Open Font
  License, Version 1.1. This Font Software is distributed on an "AS IS" BASIS,
  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.`
- nameID 14 (license URL): `http://scripts.sil.org/OFL`
- nameID 5 (version): `Version 2.004;hotconv 1.0.118;makeotfexe 2.5.65603`

The OFL permits bundling and redistributing the fonts with software provided
the license accompanies them and no Reserved Font Name is used by a modified
version. These files are unmodified, keep their names, and ship with
`LICENSE-OFL.txt`.

## Subsetting

None. If a future size decision needs a subset, regenerate it deterministically
from the pinned upstream bytes above and record the tool name and version, the
exact command and the resulting SHA-256 here, with coverage of every codepoint
in the string tables **plus** CJK Unified Ideographs (process and host names
are arbitrary). A UI-strings-only subset is not acceptable as the only bundled
face.
