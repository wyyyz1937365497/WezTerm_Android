# Upstream provenance

This crate is the Android-specific seam for WezTerm's font stack.

- Upstream repository: `https://github.com/wezterm/wezterm.git`
- Pinned revision: `d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b`
- Reused packages: upstream `deps/freetype` and `deps/harfbuzz`
- Historical P1-B font source: upstream `assets/fonts/JetBrainsMono-Regular.ttf`
- Bundled font SHA-256:
  `a0bf60ef0f83c5ed4d7a75d45838548b1f6873372dfac88f71804491898d138f`
- Font license: `assets/LICENSE_OFL.txt` (SIL Open Font License 1.1)

The current Android primary face is `MesloLGS Nerd Font Mono Regular`:

- Project: `https://github.com/ryanoasis/nerd-fonts`
- Family reported by FreeType/fontconfig: `MesloLGS Nerd Font Mono`
- Style: `Regular`; fixed-width spacing
- Source used for this checkout:
  `/usr/local/share/fonts/m/MesloLGSNerdFontMono_Regular.ttf`
- Embedded asset: `assets/MesloLGSNerdFontMono-Regular.ttf`
- Meslo license notice: `assets/LICENSE_MESLO_APACHE_2.0.txt`
- Nerd Fonts Meslo attribution and icon-license inventory:
  `https://github.com/ryanoasis/nerd-fonts/blob/master/patched-fonts/Meslo/README.md`

The deterministic mathematical fallback is `Noto Sans Math Regular`:

- Project: `https://github.com/notofonts/math`
- Source used for this checkout: `/usr/share/fonts/truetype/noto/NotoSansMath-Regular.ttf`
- Embedded asset: `assets/NotoSansMath-Regular.ttf`
- SHA-256:
  `8242bd1e55368b27e32455260754cf9aa58f3ad7ea80664b66c21f1b09910d6c`
- License: `assets/LICENSE_OFL.txt` (SIL Open Font License 1.1)

The `Nerd Font Mono` variant is intentional: every patched icon is constrained
to one terminal cell. Noto Sans Math is the next face, and Android's Noto Sans
CJK SC remains the final fallback for text that the bundled faces do not cover.

The thin Rust ownership wrappers in `src/lib.rs` intentionally avoid the
desktop-only dependency path currently pulled in by the complete
`wezterm-font` crate (`config -> wezterm-ssh -> OpenSSL`, fontconfig, and
desktop toast notifications). This is an integration seam, not a claim that
the complete upstream `wezterm-font` crate already supports Android.
