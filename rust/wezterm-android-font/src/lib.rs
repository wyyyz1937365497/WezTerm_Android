#![deny(unsafe_op_in_unsafe_fn)]

use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::path::Path;
use std::ptr;
use std::slice;

pub const UPSTREAM_WEZTERM_REVISION: &str = "d2f3f05b38f26a872f4b0bfbb3d2eaa7bdfc1b0b";
pub const JETBRAINS_MONO_REGULAR: &[u8] = include_bytes!("../assets/JetBrainsMono-Regular.ttf");
pub const MESLO_LGS_NERD_FONT_MONO_REGULAR: &[u8] =
    include_bytes!("../assets/MesloLGSNerdFontMono-Regular.ttf");
pub const NOTO_SANS_MATH_REGULAR: &[u8] = include_bytes!("../assets/NotoSansMath-Regular.ttf");

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    pub ascender: f32,
    pub descender: f32,
    pub height: f32,
    pub max_advance: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapedGlyph {
    pub glyph_id: u32,
    pub cluster: u32,
    pub x_advance: f32,
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RasterizedGlyph {
    /// One byte of linear coverage for each pixel, row-major from top to bottom.
    pub alpha: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance_x: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontRun {
    pub font_index: usize,
    pub glyphs: Vec<ShapedGlyph>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasPlacement {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub advance_x: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct GlyphKey {
    font_index: usize,
    glyph_id: u32,
}

/// Deterministic single-channel shelf atlas used by the Android renderer.
/// Packing is kept independent of wgpu so it can be tested on the host.
pub struct AlphaAtlas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    placements: HashMap<GlyphKey, AtlasPlacement>,
    cursor_x: u32,
    cursor_y: u32,
    shelf_height: u32,
}

impl std::fmt::Debug for AlphaAtlas {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AlphaAtlas")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("glyph_count", &self.placements.len())
            .field("cursor_x", &self.cursor_x)
            .field("cursor_y", &self.cursor_y)
            .field("shelf_height", &self.shelf_height)
            .finish()
    }
}

impl AlphaAtlas {
    const PADDING: u32 = 1;

    pub fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("atlas dimensions must be non-zero");
        }
        let len = usize::try_from(width)
            .ok()
            .and_then(|width| {
                usize::try_from(height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .context("atlas dimensions overflow address space")?;
        Ok(Self {
            width,
            height,
            pixels: vec![0; len],
            placements: HashMap::new(),
            cursor_x: Self::PADDING,
            cursor_y: Self::PADDING,
            shelf_height: 0,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    pub fn glyph_count(&self) -> usize {
        self.placements.len()
    }

    pub fn get_or_insert(
        &mut self,
        fonts: &mut FontSet,
        font_index: usize,
        glyph_id: u32,
    ) -> Result<AtlasPlacement> {
        let key = GlyphKey {
            font_index,
            glyph_id,
        };
        if let Some(placement) = self.placements.get(&key) {
            return Ok(*placement);
        }

        let raster = fonts.rasterize(font_index, glyph_id)?;
        let placement = if raster.width == 0 || raster.height == 0 {
            AtlasPlacement {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                bearing_x: raster.bearing_x,
                bearing_y: raster.bearing_y,
                advance_x: raster.advance_x,
            }
        } else {
            self.pack(&raster)?
        };
        self.placements.insert(key, placement);
        Ok(placement)
    }

    fn pack(&mut self, raster: &RasterizedGlyph) -> Result<AtlasPlacement> {
        let padded_width = raster
            .width
            .checked_add(Self::PADDING * 2)
            .context("glyph width overflow")?;
        let padded_height = raster
            .height
            .checked_add(Self::PADDING * 2)
            .context("glyph height overflow")?;
        if padded_width > self.width || padded_height > self.height {
            bail!(
                "glyph {}x{} does not fit in {}x{} atlas",
                raster.width,
                raster.height,
                self.width,
                self.height
            );
        }

        if self.cursor_x + raster.width + Self::PADDING > self.width {
            self.cursor_x = Self::PADDING;
            self.cursor_y = self
                .cursor_y
                .checked_add(self.shelf_height + Self::PADDING)
                .context("atlas row coordinate overflow")?;
            self.shelf_height = 0;
        }
        if self.cursor_y + raster.height + Self::PADDING > self.height {
            bail!(
                "{}x{} glyph atlas is full after {} glyphs",
                self.width,
                self.height,
                self.placements.len()
            );
        }

        let x = self.cursor_x;
        let y = self.cursor_y;
        let atlas_width = self.width as usize;
        let glyph_width = raster.width as usize;
        for glyph_y in 0..raster.height as usize {
            let source_start = glyph_y * glyph_width;
            let destination_start = (y as usize + glyph_y) * atlas_width + x as usize;
            self.pixels[destination_start..destination_start + glyph_width]
                .copy_from_slice(&raster.alpha[source_start..source_start + glyph_width]);
        }

        self.cursor_x += raster.width + Self::PADDING;
        self.shelf_height = self.shelf_height.max(raster.height + Self::PADDING);
        Ok(AtlasPlacement {
            x,
            y,
            width: raster.width,
            height: raster.height,
            bearing_x: raster.bearing_x,
            bearing_y: raster.bearing_y,
            advance_x: raster.advance_x,
        })
    }
}

/// Minimal Android ownership wrapper around the exact FreeType and HarfBuzz
/// packages vendored by the pinned WezTerm revision.
///
/// It deliberately does not implement `Send`: FreeType face mutation and the
/// corresponding HarfBuzz font remain on the renderer thread.
pub struct FontFace {
    library: freetype::FT_Library,
    face: freetype::FT_Face,
    harfbuzz_font: *mut harfbuzz::hb_font_t,
    label: String,
    pixel_height: u32,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FontFace")
            .field("label", &self.label)
            .field("family_name", &self.family_name())
            .field("pixel_height", &self.pixel_height)
            .finish_non_exhaustive()
    }
}

impl Drop for FontFace {
    fn drop(&mut self) {
        // Destruction order is significant: the HarfBuzz font holds a
        // reference to the FreeType face.
        unsafe {
            if !self.harfbuzz_font.is_null() {
                harfbuzz::hb_font_destroy(self.harfbuzz_font);
            }
            if !self.face.is_null() {
                freetype::FT_Done_Face(self.face);
            }
            if !self.library.is_null() {
                freetype::FT_Done_FreeType(self.library);
            }
        }
    }
}

impl FontFace {
    pub fn bundled_meslo_lgs_nerd_font_mono(pixel_height: u32) -> Result<Self> {
        Self::from_static_memory(
            "bundled MesloLGS Nerd Font Mono Regular",
            MESLO_LGS_NERD_FONT_MONO_REGULAR,
            0,
            pixel_height,
        )
    }

    /// Kept for provenance tests and an explicit fallback choice. The Android
    /// client's default face is MesloLGS Nerd Font Mono.
    pub fn bundled_jetbrains_mono(pixel_height: u32) -> Result<Self> {
        Self::from_static_memory(
            "bundled JetBrains Mono Regular",
            JETBRAINS_MONO_REGULAR,
            0,
            pixel_height,
        )
    }

    pub fn bundled_noto_sans_math(pixel_height: u32) -> Result<Self> {
        Self::from_static_memory(
            "bundled Noto Sans Math Regular",
            NOTO_SANS_MATH_REGULAR,
            0,
            pixel_height,
        )
    }

    pub fn from_static_memory(
        label: impl Into<String>,
        bytes: &'static [u8],
        face_index: i64,
        pixel_height: u32,
    ) -> Result<Self> {
        if bytes.is_empty() {
            bail!("font memory is empty");
        }
        let label = label.into();
        Self::create(label, face_index, pixel_height, |library, face| {
            let byte_len = freetype::FT_Long::try_from(bytes.len())
                .context("font is too large for FreeType")?;
            ft_result(
                unsafe {
                    freetype::FT_New_Memory_Face(
                        library,
                        bytes.as_ptr(),
                        byte_len,
                        face_index as freetype::FT_Long,
                        face,
                    )
                },
                "FT_New_Memory_Face",
            )
        })
    }

    pub fn from_path(
        label: impl Into<String>,
        path: impl AsRef<Path>,
        face_index: i64,
        pixel_height: u32,
    ) -> Result<Self> {
        let label = label.into();
        let display_path = path.as_ref().display().to_string();
        let path = CString::new(path.as_ref().to_string_lossy().as_bytes())
            .with_context(|| format!("font path contains NUL: {display_path}"))?;
        Self::create(label, face_index, pixel_height, |library, face| {
            ft_result(
                unsafe {
                    freetype::FT_New_Face(
                        library,
                        path.as_ptr(),
                        face_index as freetype::FT_Long,
                        face,
                    )
                },
                "FT_New_Face",
            )
            .with_context(|| format!("open {display_path}"))
        })
    }

    fn create(
        label: String,
        face_index: i64,
        pixel_height: u32,
        load_face: impl FnOnce(freetype::FT_Library, *mut freetype::FT_Face) -> Result<()>,
    ) -> Result<Self> {
        if pixel_height == 0 {
            bail!("pixel height must be non-zero");
        }
        if face_index < 0 {
            bail!("face index must be non-negative");
        }

        let mut library = ptr::null_mut();
        ft_result(
            unsafe { freetype::FT_Init_FreeType(&mut library) },
            "FT_Init_FreeType",
        )?;
        if library.is_null() {
            bail!("FT_Init_FreeType returned a null library");
        }

        let mut face = ptr::null_mut();
        if let Err(error) = load_face(library, &mut face) {
            unsafe {
                freetype::FT_Done_FreeType(library);
            }
            return Err(error);
        }
        if face.is_null() {
            unsafe {
                freetype::FT_Done_FreeType(library);
            }
            bail!("FreeType returned a null face for {label}");
        }

        if let Err(error) = ft_result(
            unsafe { freetype::FT_Set_Pixel_Sizes(face, 0, pixel_height) },
            "FT_Set_Pixel_Sizes",
        ) {
            unsafe {
                freetype::FT_Done_Face(face);
                freetype::FT_Done_FreeType(library);
            }
            return Err(error).with_context(|| format!("select {pixel_height}px for {label}"));
        }

        let harfbuzz_font = unsafe { harfbuzz::hb_ft_font_create_referenced(face.cast()) };
        if harfbuzz_font.is_null() {
            unsafe {
                freetype::FT_Done_Face(face);
                freetype::FT_Done_FreeType(library);
            }
            bail!("hb_ft_font_create_referenced returned null for {label}");
        }
        unsafe {
            harfbuzz::hb_ft_font_set_load_flags(harfbuzz_font, freetype::FT_LOAD_DEFAULT as i32);
            harfbuzz::hb_ft_font_changed(harfbuzz_font);
        }

        Ok(Self {
            library,
            face,
            harfbuzz_font,
            label,
            pixel_height,
        })
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn family_name(&self) -> String {
        let name = unsafe { (*self.face).family_name };
        if name.is_null() {
            return self.label.clone();
        }
        unsafe { CStr::from_ptr(name) }
            .to_string_lossy()
            .into_owned()
    }

    pub fn pixel_height(&self) -> u32 {
        self.pixel_height
    }

    pub fn glyph_index(&self, character: char) -> u32 {
        unsafe {
            freetype::FT_Get_Char_Index(self.face, character as u32 as freetype::FT_ULong) as u32
        }
    }

    pub fn supports_text(&self, text: &str) -> bool {
        text.chars()
            .filter(|character| !is_ignorable_for_font_selection(*character))
            .all(|character| self.glyph_index(character) != 0)
    }

    pub fn line_metrics(&self) -> Result<LineMetrics> {
        let size = unsafe { (*self.face).size };
        if size.is_null() {
            bail!("{} has no active FreeType size", self.label);
        }
        let metrics = unsafe { (*size).metrics };
        Ok(LineMetrics {
            ascender: ft_fixed_26_6(metrics.ascender),
            descender: ft_fixed_26_6(metrics.descender),
            height: ft_fixed_26_6(metrics.height),
            max_advance: ft_fixed_26_6(metrics.max_advance),
        })
    }

    pub fn shape(&mut self, text: &str) -> Result<Vec<ShapedGlyph>> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        let buffer = HbBuffer::new()?;
        let bytes = text.as_bytes();
        let text_len = i32::try_from(bytes.len()).context("text is too long for HarfBuzz")?;
        unsafe {
            harfbuzz::hb_buffer_add_utf8(buffer.raw, bytes.as_ptr().cast(), text_len, 0, text_len);
            harfbuzz::hb_buffer_guess_segment_properties(buffer.raw);
            harfbuzz::hb_shape(self.harfbuzz_font, buffer.raw, ptr::null(), 0);
        }

        let mut info_len = 0;
        let infos = unsafe { harfbuzz::hb_buffer_get_glyph_infos(buffer.raw, &mut info_len) };
        let mut position_len = 0;
        let positions =
            unsafe { harfbuzz::hb_buffer_get_glyph_positions(buffer.raw, &mut position_len) };
        if info_len != position_len {
            bail!(
                "HarfBuzz returned {} infos but {} positions",
                info_len,
                position_len
            );
        }
        if info_len == 0 {
            return Ok(Vec::new());
        }
        if infos.is_null() || positions.is_null() {
            bail!("HarfBuzz returned null glyph arrays");
        }

        let infos = unsafe { slice::from_raw_parts(infos, info_len as usize) };
        let positions = unsafe { slice::from_raw_parts(positions, position_len as usize) };
        Ok(infos
            .iter()
            .zip(positions)
            .map(|(info, position)| ShapedGlyph {
                glyph_id: info.codepoint,
                cluster: info.cluster,
                x_advance: hb_fixed_26_6(position.x_advance),
                y_advance: hb_fixed_26_6(position.y_advance),
                x_offset: hb_fixed_26_6(position.x_offset),
                y_offset: hb_fixed_26_6(position.y_offset),
            })
            .collect())
    }

    pub fn rasterize(&mut self, glyph_id: u32) -> Result<RasterizedGlyph> {
        ft_result(
            unsafe {
                freetype::FT_Load_Glyph(
                    self.face,
                    glyph_id as freetype::FT_UInt,
                    freetype::FT_LOAD_DEFAULT as i32,
                )
            },
            "FT_Load_Glyph",
        )
        .with_context(|| format!("load glyph {glyph_id} from {}", self.label))?;

        let slot = unsafe { (*self.face).glyph };
        if slot.is_null() {
            bail!("{} returned a null glyph slot", self.label);
        }
        ft_result(
            unsafe {
                freetype::FT_Render_Glyph(slot, freetype::FT_Render_Mode::FT_RENDER_MODE_NORMAL)
            },
            "FT_Render_Glyph",
        )
        .with_context(|| format!("render glyph {glyph_id} from {}", self.label))?;

        let slot = unsafe { &*slot };
        let bitmap = &slot.bitmap;
        let width = bitmap.width;
        let height = bitmap.rows;
        let alpha = copy_bitmap_alpha(bitmap)?;

        Ok(RasterizedGlyph {
            alpha,
            width,
            height,
            bearing_x: slot.bitmap_left,
            bearing_y: slot.bitmap_top,
            advance_x: ft_fixed_26_6(slot.advance.x),
        })
    }
}

pub struct FontSet {
    faces: Vec<FontFace>,
}

impl std::fmt::Debug for FontSet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FontSet")
            .field("faces", &self.faces)
            .finish()
    }
}

impl FontSet {
    pub fn bundled(pixel_height: u32) -> Result<Self> {
        Ok(Self {
            faces: vec![
                FontFace::bundled_meslo_lgs_nerd_font_mono(pixel_height)?,
                FontFace::bundled_noto_sans_math(pixel_height)?,
            ],
        })
    }

    pub fn add_path(
        &mut self,
        label: impl Into<String>,
        path: impl AsRef<Path>,
        face_index: i64,
    ) -> Result<usize> {
        let pixel_height = self
            .faces
            .first()
            .ok_or_else(|| anyhow!("font set has no primary face"))?
            .pixel_height();
        let face = FontFace::from_path(label, path, face_index, pixel_height)?;
        self.faces.push(face);
        Ok(self.faces.len() - 1)
    }

    pub fn faces(&self) -> &[FontFace] {
        &self.faces
    }

    pub fn select_face(&self, text: &str) -> usize {
        self.faces
            .iter()
            .position(|face| face.supports_text(text))
            .unwrap_or(0)
    }

    pub fn shape(&mut self, text: &str) -> Result<FontRun> {
        let font_index = self.select_face(text);
        let glyphs = self
            .faces
            .get_mut(font_index)
            .ok_or_else(|| anyhow!("font set has no face at index {font_index}"))?
            .shape(text)?;
        Ok(FontRun { font_index, glyphs })
    }

    pub fn rasterize(&mut self, font_index: usize, glyph_id: u32) -> Result<RasterizedGlyph> {
        self.faces
            .get_mut(font_index)
            .ok_or_else(|| anyhow!("font set has no face at index {font_index}"))?
            .rasterize(glyph_id)
    }
}

struct HbBuffer {
    raw: *mut harfbuzz::hb_buffer_t,
}

impl HbBuffer {
    fn new() -> Result<Self> {
        let raw = unsafe { harfbuzz::hb_buffer_create() };
        if raw.is_null() || unsafe { harfbuzz::hb_buffer_allocation_successful(raw) } == 0 {
            if !raw.is_null() {
                unsafe { harfbuzz::hb_buffer_destroy(raw) };
            }
            bail!("hb_buffer_create failed");
        }
        Ok(Self { raw })
    }
}

impl Drop for HbBuffer {
    fn drop(&mut self) {
        unsafe { harfbuzz::hb_buffer_destroy(self.raw) };
    }
}

fn ft_fixed_26_6(value: freetype::FT_Pos) -> f32 {
    value.f26d6().to_num::<f32>()
}

fn hb_fixed_26_6(value: harfbuzz::hb_position_t) -> f32 {
    value as f32 / 64.0
}

fn is_ignorable_for_font_selection(character: char) -> bool {
    matches!(character, '\u{200d}' | '\u{fe0e}' | '\u{fe0f}') || character.is_control()
}

fn ft_result(error: freetype::FT_Error, operation: &str) -> Result<()> {
    if error == freetype::FT_Err_Ok as freetype::FT_Error {
        return Ok(());
    }
    let reason = unsafe { freetype::FT_Error_String(error) };
    if reason.is_null() {
        bail!("{operation} failed with FreeType error {error:#x}");
    }
    let reason = unsafe { CStr::from_ptr(reason) }.to_string_lossy();
    bail!("{operation} failed with FreeType error {error:#x}: {reason}")
}

fn copy_bitmap_alpha(bitmap: &freetype::FT_Bitmap) -> Result<Vec<u8>> {
    let width = bitmap.width as usize;
    let height = bitmap.rows as usize;
    if width == 0 || height == 0 {
        return Ok(Vec::new());
    }
    if bitmap.buffer.is_null() {
        bail!("FreeType bitmap has dimensions but a null buffer");
    }

    let pitch = bitmap.pitch.unsigned_abs() as usize;
    let source_len = pitch
        .checked_mul(height)
        .context("FreeType bitmap byte count overflow")?;
    let source = unsafe { slice::from_raw_parts(bitmap.buffer, source_len) };
    let mut alpha = vec![0; width * height];

    for output_y in 0..height {
        let source_y = if bitmap.pitch >= 0 {
            output_y
        } else {
            height - output_y - 1
        };
        let row = &source[source_y * pitch..(source_y + 1) * pitch];
        match bitmap.pixel_mode as u32 {
            mode if mode == freetype::FT_Pixel_Mode::FT_PIXEL_MODE_GRAY as u32 => {
                if row.len() < width {
                    bail!("FreeType grayscale bitmap pitch is smaller than its width");
                }
                alpha[output_y * width..(output_y + 1) * width].copy_from_slice(&row[..width]);
            }
            mode if mode == freetype::FT_Pixel_Mode::FT_PIXEL_MODE_MONO as u32 => {
                for x in 0..width {
                    alpha[output_y * width + x] = if row[x / 8] & (0x80 >> (x % 8)) != 0 {
                        255
                    } else {
                        0
                    };
                }
            }
            mode if mode == freetype::FT_Pixel_Mode::FT_PIXEL_MODE_BGRA as u32 => {
                if row.len() < width * 4 {
                    bail!("FreeType BGRA bitmap pitch is smaller than its width");
                }
                for x in 0..width {
                    alpha[output_y * width + x] = row[x * 4 + 3];
                }
            }
            mode => bail!("unsupported FreeType pixel mode {mode}"),
        }
    }
    Ok(alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_font_shapes_and_rasterizes() {
        let mut fonts = FontSet::bundled(36).unwrap();
        assert_eq!(fonts.faces()[0].family_name(), "MesloLGS Nerd Font Mono");
        assert_eq!(fonts.faces()[0].glyph_index('A') > 0, true);

        let run = fonts.shape("A!=e\u{301}").unwrap();
        assert_eq!(run.font_index, 0);
        assert!(!run.glyphs.is_empty());
        assert!(run.glyphs.iter().all(|glyph| glyph.glyph_id != 0));
        assert!(run.glyphs.iter().map(|glyph| glyph.x_advance).sum::<f32>() > 0.0);

        let glyph_id = fonts.faces()[0].glyph_index('A');
        let glyph = fonts.rasterize(0, glyph_id).unwrap();
        assert!(glyph.width > 0);
        assert!(glyph.height > 0);
        assert_eq!(glyph.alpha.len(), (glyph.width * glyph.height) as usize);
        assert!(glyph.alpha.iter().any(|coverage| *coverage != 0));
    }

    #[test]
    fn bundled_fonts_cover_nerd_powerline_and_mathematical_alphanumerics() {
        let fonts = FontSet::bundled(36).unwrap();

        // U+E0B0 is Powerline's right-facing separator. U+F120 is the
        // Font Awesome terminal icon. Both would be absent from plain Meslo.
        assert_ne!(fonts.faces()[0].glyph_index('\u{e0b0}'), 0);
        assert_ne!(fonts.faces()[0].glyph_index('\u{f120}'), 0);

        for character in ['\u{1d41f}', '\u{1d42b}', '\u{1d405}'] {
            assert_eq!(fonts.select_face(&character.to_string()), 1);
            assert_ne!(fonts.faces()[1].glyph_index(character), 0);
        }
    }

    #[test]
    fn reports_pixel_line_metrics() {
        let fonts = FontSet::bundled(42).unwrap();
        let metrics = fonts.faces()[0].line_metrics().unwrap();
        assert!(metrics.ascender > 0.0);
        assert!(metrics.descender < 0.0);
        assert!(metrics.height >= 42.0);
        assert!(metrics.max_advance > 0.0);
    }

    #[test]
    fn harfbuzz_shapes_combining_sequence_to_one_glyph() {
        let mut fonts = FontSet::bundled(36).unwrap();
        let run = fonts.shape("e\u{301}").unwrap();

        assert_eq!(run.font_index, 0);
        assert_eq!(run.glyphs.len(), 1);
        assert_ne!(run.glyphs[0].glyph_id, 0);
        assert!(run.glyphs[0].x_advance > 0.0);
    }

    #[test]
    fn can_add_system_cjk_fallback_when_available() {
        let candidates = [
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/system/fonts/NotoSansCJK-Regular.ttc",
        ];
        let Some(path) = candidates.iter().find(|path| Path::new(path).exists()) else {
            return;
        };

        let mut fonts = FontSet::bundled(36).unwrap();
        fonts.add_path("Noto Sans CJK SC", path, 2).unwrap();
        assert_eq!(fonts.select_face("中"), 2);
        let run = fonts.shape("中文").unwrap();
        assert_eq!(run.font_index, 2);
        assert!(run.glyphs.iter().all(|glyph| glyph.glyph_id != 0));
        let glyph = fonts.rasterize(2, run.glyphs[0].glyph_id).unwrap();
        assert!(glyph.alpha.iter().any(|coverage| *coverage != 0));
    }

    #[test]
    fn atlas_caches_and_packs_rasterized_glyphs() {
        let mut fonts = FontSet::bundled(36).unwrap();
        let run = fonts.shape("AB").unwrap();
        let mut atlas = AlphaAtlas::new(128, 128).unwrap();
        let a = atlas
            .get_or_insert(&mut fonts, run.font_index, run.glyphs[0].glyph_id)
            .unwrap();
        let a_again = atlas
            .get_or_insert(&mut fonts, run.font_index, run.glyphs[0].glyph_id)
            .unwrap();
        let b = atlas
            .get_or_insert(&mut fonts, run.font_index, run.glyphs[1].glyph_id)
            .unwrap();

        assert_eq!(a, a_again);
        assert_eq!(atlas.glyph_count(), 2);
        assert!(a.width > 0 && b.width > 0);
        assert_ne!((a.x, a.y), (b.x, b.y));
        assert!(atlas.pixels().iter().any(|coverage| *coverage != 0));
    }
}
