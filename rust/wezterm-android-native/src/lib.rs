#![deny(unsafe_op_in_unsafe_fn)]

use anyhow::{anyhow, bail, Context, Result};
use jni::objects::{JClass, JObject, JString};
use jni::sys::{
    jboolean, jfloat, jint, jlong, jobject, jstring, JNIEnv as RawJniEnv, JNI_FALSE, JNI_TRUE,
};
use jni::JNIEnv;
use raw_window_handle::{
    AndroidDisplayHandle, AndroidNdkWindowHandle, DisplayHandle, HandleError, HasDisplayHandle,
    HasWindowHandle, RawDisplayHandle, RawWindowHandle, WindowHandle,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, Once, OnceLock};
use wezterm_android_core::{
    android_key_bytes, idle_cat_ansi, TerminalModel, TerminalSnapshot, UPSTREAM_WEZTERM_REVISION,
};
use wezterm_android_font::{
    AlphaAtlas, FontSet, UPSTREAM_WEZTERM_REVISION as FONT_UPSTREAM_WEZTERM_REVISION,
};
use wezterm_android_mux::{
    AndroidMuxSession, MuxEndpoint, MuxEvent, WEZTERM_REVISION as MUX_UPSTREAM_WEZTERM_REVISION,
};
use wezterm_android_ssh::{
    AndroidSshConfig, AndroidSshSession, ClientEvent, PtySize, SshEndpoint,
    WEZTERM_REVISION as SSH_UPSTREAM_WEZTERM_REVISION,
};
use wezterm_term::TerminalSize as MuxTerminalSize;
use wgpu::util::DeviceExt;

#[link(name = "android")]
unsafe extern "C" {
    fn ANativeWindow_fromSurface(env: *mut RawJniEnv, surface: jobject) -> *mut c_void;
    fn ANativeWindow_release(window: *mut c_void);
}

thread_local! {
    // SurfaceHolder callbacks are delivered on Android's UI thread. A render
    // thread will replace this in a later gate; thread-local ownership keeps
    // the non-Send ANativeWindow handle honest for P0.
    static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) };
    // The terminal state deliberately outlives each Android Surface. A later
    // networking gate will move this behind the client/session thread.
    static TERMINAL: RefCell<Option<TerminalModel>> = const { RefCell::new(None) };
    static LAST_MUX_SNAPSHOT: RefCell<Option<TerminalSnapshot>> = const { RefCell::new(None) };
    static LAST_VIEW_SNAPSHOT: RefCell<Option<TerminalSnapshot>> = const { RefCell::new(None) };
    static VIEW_INTERACTION: RefCell<ViewInteraction> = const { RefCell::new(ViewInteraction::new()) };
    // SSHMUX scrollback viewport anchors are remembered per remote tab so a
    // switch (toolbar, restore, or another client) returns to the position
    // the user left in that tab instead of sharing one global offset.
    static MUX_TAB_VIEWPORTS: RefCell<HashMap<usize, Option<isize>>> =
        RefCell::new(HashMap::new());
    static MUX_ACTIVE_TAB: Cell<Option<usize>> = const { Cell::new(None) };
}

static LOGGING: Once = Once::new();
static SSH_SESSION: OnceLock<Mutex<Option<AndroidSshSession>>> = OnceLock::new();
static MUX_SESSION: OnceLock<Mutex<Option<AndroidMuxSession>>> = OnceLock::new();
static MUX_READY: AtomicBool = AtomicBool::new(false);
static TERMINAL_ZOOM_PERCENT: AtomicU32 = AtomicU32::new(100);
const TERMINAL_ZOOM_MIN_PERCENT: u32 = 50;
const TERMINAL_ZOOM_MAX_PERCENT: u32 = 200;
const MAX_TERMINAL_CELLS: usize = 65_536;
const GLYPH_ATLAS_WIDTH: u32 = 1024;
const GLYPH_ATLAS_HEIGHT: u32 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CellPoint {
    row: usize,
    column: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellSelection {
    anchor_start: CellPoint,
    anchor_end: CellPoint,
    focus: CellPoint,
}

impl CellSelection {
    fn range(self) -> (CellPoint, CellPoint) {
        if self.focus < self.anchor_start {
            (self.focus, self.anchor_end)
        } else if self.focus > self.anchor_end {
            (self.anchor_start, self.focus)
        } else {
            (self.anchor_start, self.anchor_end)
        }
    }

    fn contains(self, point: CellPoint) -> bool {
        let (start, end) = self.range();
        point >= start && point <= end
    }
}

#[derive(Debug)]
struct ViewInteraction {
    viewport_offset: usize,
    max_viewport_offset: usize,
    /// Absolute physical row anchoring the viewport while browsing history.
    /// `None` follows the live bottom.
    pinned_top: Option<isize>,
    selection: Option<CellSelection>,
}

impl ViewInteraction {
    const fn new() -> Self {
        Self {
            viewport_offset: 0,
            max_viewport_offset: 0,
            pinned_top: None,
            selection: None,
        }
    }

    fn observe_snapshot(&mut self, snapshot: &TerminalSnapshot) {
        self.viewport_offset = snapshot.viewport_offset;
        self.max_viewport_offset = snapshot.max_viewport_offset;
        self.pinned_top = if snapshot.viewport_offset > 0 {
            Some(snapshot.viewport_top)
        } else {
            None
        };
    }

    fn reset(&mut self) {
        self.viewport_offset = 0;
        self.max_viewport_offset = 0;
        self.pinned_top = None;
        self.selection = None;
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ViewUniform {
    resolution: [f32; 2],
    cell_size: [f32; 2],
    cursor_cell: [f32; 2],
    padding: [f32; 2],
    terminal_size: [u32; 2],
    _reserved: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
struct CellRenderData {
    atlas_min: [f32; 2],
    atlas_max: [f32; 2],
    glyph_origin: [f32; 2],
    glyph_size: [f32; 2],
    foreground: [f32; 4],
    background: [f32; 4],
    flags: [u32; 4],
}

impl Default for CellRenderData {
    fn default() -> Self {
        Self {
            atlas_min: [0.0; 2],
            atlas_max: [0.0; 2],
            glyph_origin: [0.0; 2],
            glyph_size: [0.0; 2],
            foreground: [0.84, 0.88, 0.94, 1.0],
            background: [0.0, 0.0, 0.0, 1.0],
            flags: [0; 4],
        }
    }
}

struct PreparedTerminal {
    cells: Vec<CellRenderData>,
    atlas: AlphaAtlas,
    shaped_cells: usize,
    fallback_cells: usize,
    multi_glyph_cells: usize,
}

fn terminal_zoom_scale() -> f32 {
    let percent = TERMINAL_ZOOM_PERCENT
        .load(Ordering::SeqCst)
        .clamp(TERMINAL_ZOOM_MIN_PERCENT, TERMINAL_ZOOM_MAX_PERCENT);
    percent as f32 / 100.0
}

fn cell_size(density_dpi: u32) -> [f32; 2] {
    let density_scale = density_dpi.max(120) as f32 / 160.0;
    let zoom = terminal_zoom_scale();
    [11.0 * density_scale * zoom, 21.0 * density_scale * zoom]
}

fn font_pixel_height(density_dpi: u32) -> u32 {
    (cell_size(density_dpi)[1] * 0.72).round().max(8.0) as u32
}

fn terminal_dimensions(width: u32, height: u32, density_dpi: u32) -> (usize, usize) {
    let [cell_width, cell_height] = cell_size(density_dpi);
    let columns = ((width.max(1) as f32 / cell_width).floor() as usize)
        .max(1)
        .min(MAX_TERMINAL_CELLS);
    let rows = ((height.max(1) as f32 / cell_height).floor() as usize)
        .max(1)
        .min((MAX_TERMINAL_CELLS / columns).max(1));
    (columns, rows)
}

fn idle_terminal(
    columns: usize,
    rows: usize,
    pixel_width: usize,
    pixel_height: usize,
    density_dpi: u32,
) -> (TerminalModel, TerminalSnapshot) {
    let mut model = TerminalModel::new(columns, rows, pixel_width, pixel_height, density_dpi);
    model.feed(idle_cat_ansi(columns, rows));
    let mut snapshot = model.snapshot();
    // The disconnected artwork has no input cursor. One cell beyond the grid
    // uses the renderer's existing out-of-viewport cursor suppression path.
    snapshot.cursor_column = snapshot.columns;
    snapshot.cursor_row = snapshot.rows;
    (model, snapshot)
}

fn remote_session_active() -> bool {
    let ssh_active = ssh_session_slot()
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or_else(|_| {
            log::error!("unable to inspect poisoned SSH session lock");
            false
        });
    if ssh_active {
        return true;
    }
    mux_session_slot()
        .lock()
        .map(|slot| slot.is_some())
        .unwrap_or_else(|_| {
            log::error!("unable to inspect poisoned SSHMUX session lock");
            false
        })
}

fn terminal_snapshot_for_surface(width: u32, height: u32, density_dpi: u32) -> TerminalSnapshot {
    let (columns, rows) = terminal_dimensions(width, height, density_dpi);
    if !remote_session_active() {
        let (model, snapshot) =
            idle_terminal(columns, rows, width as usize, height as usize, density_dpi);
        TERMINAL.with(|slot| slot.borrow_mut().replace(model));
        return snapshot;
    }
    let pinned_top = VIEW_INTERACTION.with(|interaction| interaction.borrow().pinned_top);
    TERMINAL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let model = slot.get_or_insert_with(|| {
            TerminalModel::new(columns, rows, width as usize, height as usize, density_dpi)
        });
        model.resize(columns, rows, width as usize, height as usize, density_dpi);
        model.snapshot_with_viewport_top(pinned_top)
    })
}

fn build_font_set(density_dpi: u32) -> Result<FontSet> {
    if UPSTREAM_WEZTERM_REVISION != FONT_UPSTREAM_WEZTERM_REVISION {
        bail!(
            "terminal/font WezTerm revisions differ: {} != {}",
            UPSTREAM_WEZTERM_REVISION,
            FONT_UPSTREAM_WEZTERM_REVISION,
        );
    }

    let pixel_height = font_pixel_height(density_dpi);
    let mut fonts = FontSet::bundled(pixel_height)?;
    for (path, face_index) in [
        ("/system/fonts/NotoSansCJK-Regular.ttc", 2),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
    ] {
        if !Path::new(path).exists() {
            continue;
        }
        match fonts.add_path("Noto Sans CJK SC", path, face_index) {
            Ok(index) => {
                log::info!(
                    "loaded Android CJK fallback index={} path={} face={}",
                    index,
                    path,
                    face_index,
                );
                break;
            }
            Err(error) => log::warn!("failed to load CJK fallback {}: {:#}", path, error),
        }
    }
    for (index, face) in fonts.faces().iter().enumerate() {
        log::info!(
            "font face index={} family={} source={} pixel_height={}",
            index,
            face.family_name(),
            face.label(),
            face.pixel_height(),
        );
    }
    Ok(fonts)
}

fn rgba_to_float(color: [u8; 4]) -> [f32; 4] {
    color.map(|channel| f32::from(channel) / 255.0)
}

fn prepare_terminal(
    fonts: &mut FontSet,
    terminal: &TerminalSnapshot,
    density_dpi: u32,
    selection: Option<CellSelection>,
) -> Result<PreparedTerminal> {
    let cell_count = terminal
        .columns
        .checked_mul(terminal.rows)
        .context("terminal cell count overflow")?;
    if cell_count > MAX_TERMINAL_CELLS {
        bail!(
            "terminal snapshot contains {} cells; capacity is {}",
            cell_count,
            MAX_TERMINAL_CELLS,
        );
    }

    let mut cells = vec![CellRenderData::default(); cell_count];
    let mut atlas = AlphaAtlas::new(GLYPH_ATLAS_WIDTH, GLYPH_ATLAS_HEIGHT)?;
    let [cell_width, cell_height] = cell_size(density_dpi);
    let mut shaped_cells = 0;
    let mut fallback_cells = 0;
    let mut multi_glyph_cells = 0;

    for cell in &terminal.cells {
        if cell.row >= terminal.rows || cell.column >= terminal.columns {
            continue;
        }
        let width_in_cells = cell
            .width
            .max(1)
            .min(terminal.columns.saturating_sub(cell.column));
        let run = fonts
            .shape(&cell.text)
            .with_context(|| format!("shape cell {:?}", cell.text))?;
        shaped_cells += 1;
        fallback_cells += usize::from(run.font_index != 0);
        multi_glyph_cells += usize::from(run.glyphs.len() > 1);

        let base = CellRenderData {
            foreground: rgba_to_float(cell.style.foreground_rgba),
            background: rgba_to_float(cell.style.background_rgba),
            flags: [
                0,
                u32::from(cell.style.underline != 0),
                u32::from(cell.style.strikethrough),
                u32::from(cell.style.intensity),
            ],
            ..CellRenderData::default()
        };
        for covered_column in 0..width_in_cells {
            cells[cell.row * terminal.columns + cell.column + covered_column] = base;
        }

        if cell.style.invisible {
            continue;
        }
        let Some(shaped) = run.glyphs.first() else {
            continue;
        };
        let placement = atlas.get_or_insert(fonts, run.font_index, shaped.glyph_id)?;
        if placement.width == 0 || placement.height == 0 {
            continue;
        }
        let metrics = fonts.faces()[run.font_index].line_metrics()?;
        let run_advance = run.glyphs.iter().map(|glyph| glyph.x_advance).sum::<f32>();
        let span_width = width_in_cells as f32 * cell_width;
        let pen_x = (span_width - run_advance) * 0.5;
        let baseline = (cell_height - metrics.height) * 0.5 + metrics.ascender;
        let glyph_origin = [
            pen_x + shaped.x_offset + placement.bearing_x as f32,
            baseline - shaped.y_offset - placement.bearing_y as f32,
        ];
        let atlas_min = [
            placement.x as f32 / GLYPH_ATLAS_WIDTH as f32,
            placement.y as f32 / GLYPH_ATLAS_HEIGHT as f32,
        ];
        let atlas_max = [
            (placement.x + placement.width) as f32 / GLYPH_ATLAS_WIDTH as f32,
            (placement.y + placement.height) as f32 / GLYPH_ATLAS_HEIGHT as f32,
        ];

        for covered_column in 0..width_in_cells {
            let index = cell.row * terminal.columns + cell.column + covered_column;
            cells[index] = CellRenderData {
                atlas_min,
                atlas_max,
                glyph_origin: [
                    glyph_origin[0] - covered_column as f32 * cell_width,
                    glyph_origin[1],
                ],
                glyph_size: [placement.width as f32, placement.height as f32],
                flags: [1, base.flags[1], base.flags[2], base.flags[3]],
                ..base
            };
        }
    }

    if let Some(selection) = selection {
        for row in 0..terminal.rows {
            for column in 0..terminal.columns {
                if !selection.contains(CellPoint { row, column }) {
                    continue;
                }
                let render_cell = &mut cells[row * terminal.columns + column];
                render_cell.background = [0.18, 0.42, 0.72, 1.0];
                render_cell.foreground = [0.98, 0.99, 1.0, 1.0];
            }
        }
    }

    Ok(PreparedTerminal {
        cells,
        atlas,
        shaped_cells,
        fallback_cells,
        multi_glyph_cells,
    })
}

fn log_prepared_terminal(prepared: &PreparedTerminal) {
    log::info!(
        "prepared glyph atlas glyphs={} shaped_cells={} fallback_cells={} multi_glyph_cells={}",
        prepared.atlas.glyph_count(),
        prepared.shaped_cells,
        prepared.fallback_cells,
        prepared.multi_glyph_cells,
    );
}

struct NativeWindowOwner {
    ptr: NonNull<c_void>,
}

impl Drop for NativeWindowOwner {
    fn drop(&mut self) {
        // SAFETY: ANativeWindow_fromSurface acquired one reference and this
        // owner releases exactly that reference after the wgpu Surface drops.
        unsafe { ANativeWindow_release(self.ptr.as_ptr()) };
        log::debug!("released ANativeWindow");
    }
}

#[derive(Clone, Copy)]
struct AndroidWindowHandle {
    ptr: NonNull<c_void>,
}

impl HasWindowHandle for AndroidWindowHandle {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        let handle = AndroidNdkWindowHandle::new(self.ptr);
        // SAFETY: NativeWindowOwner keeps this pointer alive for the complete
        // lifetime of the wgpu Surface created from this handle.
        unsafe {
            Ok(WindowHandle::borrow_raw(RawWindowHandle::AndroidNdk(
                handle,
            )))
        }
    }
}

impl HasDisplayHandle for AndroidWindowHandle {
    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        // Android's raw display handle carries no borrowed data.
        unsafe {
            Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Android(
                AndroidDisplayHandle::new(),
            )))
        }
    }
}

struct Renderer {
    // Field order matters: wgpu resources and Surface drop before the native
    // window owner releases the ANativeWindow reference.
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    cell_buffer: wgpu::Buffer,
    atlas_texture: wgpu::Texture,
    _atlas_view: wgpu::TextureView,
    _atlas_sampler: wgpu::Sampler,
    view_bind_group: wgpu::BindGroup,
    surface_config: wgpu::SurfaceConfiguration,
    density_dpi: u32,
    zoom_percent: u32,
    font_set: FontSet,
    _instance: wgpu::Instance,
    _native_window: NativeWindowOwner,
}

impl Renderer {
    fn new(
        native_window: NonNull<c_void>,
        width: u32,
        height: u32,
        density_dpi: u32,
        terminal: &TerminalSnapshot,
    ) -> Result<Self> {
        let native_window_owner = NativeWindowOwner { ptr: native_window };
        let mut font_set = build_font_set(density_dpi)?;
        let prepared = prepare_terminal(&mut font_set, terminal, density_dpi, None)?;
        let raw_handle = AndroidWindowHandle { ptr: native_window };
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..Default::default()
        });
        let surface: wgpu::Surface<'static> = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::from_window(&raw_handle)?)
        }
        .context("create wgpu Surface from ANativeWindow")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("request Vulkan adapter compatible with Android Surface")?;

        let adapter_info = adapter.get_info();
        log::info!(
            "adapter name={} backend={:?} type={:?} driver={} info={}",
            adapter_info.name,
            adapter_info.backend,
            adapter_info.device_type,
            adapter_info.driver,
            adapter_info.driver_info,
        );

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("wezterm-android-p1-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        }))
        .context("request wgpu device")?;

        let capabilities = surface.get_capabilities(&adapter);
        let format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| anyhow!("Android Surface reported no texture formats"))?;
        let alpha_mode = capabilities
            .alpha_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::CompositeAlphaMode::Opaque)
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let present_mode = if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Fifo)
        {
            wgpu::PresentMode::Fifo
        } else {
            *capabilities
                .present_modes
                .first()
                .ok_or_else(|| anyhow!("Android Surface reported no present modes"))?
        };

        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            alpha_mode,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        let uniform = Self::uniform(
            surface_config.width,
            surface_config.height,
            density_dpi,
            terminal,
        );
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("wezterm-android-p1-view-uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let cell_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wezterm-android-p1-cell-render-data"),
            size: (MAX_TERMINAL_CELLS * std::mem::size_of::<CellRenderData>()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let atlas_texture = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("wezterm-android-p1-glyph-atlas"),
                size: wgpu::Extent3d {
                    width: GLYPH_ATLAS_WIDTH,
                    height: GLYPH_ATLAS_HEIGHT,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            Default::default(),
            prepared.atlas.pixels(),
        );
        let atlas_view = atlas_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let atlas_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wezterm-android-p1-glyph-atlas-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let view_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wezterm-android-p1-view-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let view_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wezterm-android-p1-view-bind-group"),
            layout: &view_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: cell_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&atlas_sampler),
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wezterm-android-p1-terminal-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wezterm-android-p1-pipeline-layout"),
            bind_group_layouts: &[&view_layout],
            push_constant_ranges: &[],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wezterm-android-p1-terminal-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        log::info!(
            "configured native surface {}x{} density={} format={:?} present={:?}",
            surface_config.width,
            surface_config.height,
            density_dpi,
            surface_config.format,
            surface_config.present_mode,
        );

        queue.write_buffer(&cell_buffer, 0, bytemuck::cast_slice(&prepared.cells));
        log_prepared_terminal(&prepared);

        Ok(Self {
            surface,
            device,
            queue,
            pipeline,
            uniform_buffer,
            cell_buffer,
            atlas_texture,
            _atlas_view: atlas_view,
            _atlas_sampler: atlas_sampler,
            view_bind_group,
            surface_config,
            density_dpi,
            zoom_percent: TERMINAL_ZOOM_PERCENT.load(Ordering::SeqCst),
            font_set,
            _instance: instance,
            _native_window: native_window_owner,
        })
    }

    fn uniform(
        width: u32,
        height: u32,
        density_dpi: u32,
        terminal: &TerminalSnapshot,
    ) -> ViewUniform {
        ViewUniform {
            resolution: [width.max(1) as f32, height.max(1) as f32],
            cell_size: cell_size(density_dpi),
            cursor_cell: [terminal.cursor_column as f32, terminal.cursor_row as f32],
            padding: [0.0, 0.0],
            terminal_size: [terminal.columns as u32, terminal.rows as u32],
            _reserved: [0, 0],
        }
    }

    fn resize(
        &mut self,
        width: u32,
        height: u32,
        terminal: &TerminalSnapshot,
        selection: Option<CellSelection>,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.surface_config.width = width;
        self.surface_config.height = height;
        self.surface.configure(&self.device, &self.surface_config);
        self.upload_terminal(terminal, selection)?;
        log::debug!(
            "resized native surface to {}x{} terminal={}x{}",
            width,
            height,
            terminal.columns,
            terminal.rows,
        );
        Ok(())
    }

    fn upload_terminal(
        &mut self,
        terminal: &TerminalSnapshot,
        selection: Option<CellSelection>,
    ) -> Result<()> {
        let zoom_percent = TERMINAL_ZOOM_PERCENT.load(Ordering::SeqCst);
        if zoom_percent != self.zoom_percent {
            self.zoom_percent = zoom_percent;
            self.font_set = build_font_set(self.density_dpi)?;
            log::info!("rebuilt glyph fonts for terminal zoom {zoom_percent}%");
        }
        let uniform = Self::uniform(
            self.surface_config.width,
            self.surface_config.height,
            self.density_dpi,
            terminal,
        );
        self.queue
            .write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
        let prepared = prepare_terminal(&mut self.font_set, terminal, self.density_dpi, selection)?;
        self.queue
            .write_buffer(&self.cell_buffer, 0, bytemuck::cast_slice(&prepared.cells));
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            prepared.atlas.pixels(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(GLYPH_ATLAS_WIDTH),
                rows_per_image: Some(GLYPH_ATLAS_HEIGHT),
            },
            wgpu::Extent3d {
                width: GLYPH_ATLAS_WIDTH,
                height: GLYPH_ATLAS_HEIGHT,
                depth_or_array_layers: 1,
            },
        );
        log_prepared_terminal(&prepared);
        Ok(())
    }

    fn render(&mut self) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                self.surface
                    .get_current_texture()
                    .context("acquire frame after Surface reconfigure")?
            }
            Err(wgpu::SurfaceError::Timeout) => {
                log::warn!("timed out while acquiring Android Surface frame");
                return Ok(());
            }
            Err(wgpu::SurfaceError::OutOfMemory) => bail!("wgpu Surface is out of memory"),
            Err(error) => return Err(error).context("acquire Android Surface frame"),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wezterm-android-p1-command-encoder"),
            });
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wezterm-android-p1-terminal-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            render_pass.set_pipeline(&self.pipeline);
            render_pass.set_bind_group(0, &self.view_bind_group, &[]);
            render_pass.draw(0..3, 0..1);
        }
        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

fn init_logging() {
    LOGGING.call_once(|| {
        android_logger::init_once(
            android_logger::Config::default()
                .with_tag("WezTermAndroid")
                .with_max_level(log::LevelFilter::Debug)
                .with_filter(
                    android_logger::FilterBuilder::new()
                        .parse("warn,wezterm_android=debug")
                        .build(),
                ),
        );
    });
}

fn ssh_session_slot() -> &'static Mutex<Option<AndroidSshSession>> {
    SSH_SESSION.get_or_init(|| Mutex::new(None))
}

fn mux_session_slot() -> &'static Mutex<Option<AndroidMuxSession>> {
    MUX_SESSION.get_or_init(|| Mutex::new(None))
}

fn read_java_string(env: &mut JNIEnv<'_>, value: &JString<'_>) -> Result<String> {
    Ok(env.get_string(value).context("read Java string")?.into())
}

fn return_java_string(env: &mut JNIEnv<'_>, value: Option<String>) -> jstring {
    let Some(value) = value else {
        return std::ptr::null_mut();
    };
    match env.new_string(value) {
        Ok(value) => value.into_raw(),
        Err(error) => {
            log::error!("unable to allocate JNI result string: {}", error);
            std::ptr::null_mut()
        }
    }
}

fn ffi_error_string(
    env: &mut JNIEnv<'_>,
    operation: &str,
    callback: impl FnOnce() -> Result<()>,
) -> jstring {
    init_logging();
    let error = match catch_unwind(AssertUnwindSafe(callback)) {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            log::error!("{} failed: {:#}", operation, error);
            Some(format!("{:#}", error))
        }
        Err(_) => {
            log::error!("{} panicked", operation);
            Some(format!("{} panicked", operation))
        }
    };
    return_java_string(env, error)
}

fn serialize_client_event(event: &ClientEvent) -> Result<String> {
    serde_json::to_string(event).context("serialize SSH event for Android UI")
}

fn serialize_mux_event(event: &MuxEvent) -> Result<String> {
    serde_json::to_string(event).context("serialize SSHMUX event for Android UI")
}

fn current_pty_size() -> Result<PtySize> {
    TERMINAL.with(|slot| {
        let slot = slot.borrow();
        let terminal = slot
            .as_ref()
            .ok_or_else(|| anyhow!("terminal model is not initialized"))?
            .snapshot();
        Ok(PtySize {
            rows: terminal
                .rows
                .try_into()
                .context("terminal row count exceeds u16")?,
            cols: terminal
                .columns
                .try_into()
                .context("terminal column count exceeds u16")?,
            // The selected libssh backend transmits rows and columns. Keep
            // pixel dimensions zero instead of pretending these are cells.
            pixel_width: 0,
            pixel_height: 0,
        })
    })
}

fn current_mux_size() -> Result<MuxTerminalSize> {
    let size = current_pty_size()?;
    let (pixel_width, pixel_height, dpi) = RENDERER.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|renderer| {
                (
                    renderer.surface_config.width as usize,
                    renderer.surface_config.height as usize,
                    renderer.density_dpi,
                )
            })
            .unwrap_or((0, 0, 0))
    });
    Ok(MuxTerminalSize {
        rows: usize::from(size.rows),
        cols: usize::from(size.cols),
        pixel_width,
        pixel_height,
        dpi,
    })
}

fn render_terminal_snapshot(terminal: &TerminalSnapshot) -> Result<()> {
    let selection = VIEW_INTERACTION.with(|interaction| {
        let mut interaction = interaction.borrow_mut();
        interaction.observe_snapshot(terminal);
        interaction.selection
    });
    LAST_VIEW_SNAPSHOT.with(|last| {
        last.borrow_mut().replace(terminal.clone());
    });
    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(renderer) = slot.as_mut() {
            renderer.upload_terminal(terminal, selection)?;
            renderer.render()?;
        }
        Ok::<_, anyhow::Error>(())
    })
}

fn replace_with_idle_terminal() -> Result<()> {
    let renderer_geometry = RENDERER.with(|slot| {
        slot.borrow().as_ref().map(|renderer| {
            (
                renderer.surface_config.width,
                renderer.surface_config.height,
                renderer.density_dpi,
            )
        })
    });
    let (columns, rows, pixel_width, pixel_height, density_dpi) =
        if let Some((width, height, density_dpi)) = renderer_geometry {
            let (columns, rows) = terminal_dimensions(width, height, density_dpi);
            (columns, rows, width as usize, height as usize, density_dpi)
        } else {
            let dimensions = TERMINAL.with(|slot| {
                slot.borrow()
                    .as_ref()
                    .map(TerminalModel::snapshot)
                    .map(|snapshot| (snapshot.columns, snapshot.rows))
            });
            let (columns, rows) = dimensions.unwrap_or((80, 24));
            (columns, rows, 0, 0, 160)
        };

    let (model, snapshot) = idle_terminal(columns, rows, pixel_width, pixel_height, density_dpi);
    TERMINAL.with(|slot| slot.borrow_mut().replace(model));
    reset_view_interaction();
    render_terminal_snapshot(&snapshot)
}

fn feed_terminal_and_render(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let pinned_top = VIEW_INTERACTION.with(|interaction| interaction.borrow().pinned_top);
    let terminal = TERMINAL.with(|slot| {
        let mut slot = slot.borrow_mut();
        let model = slot
            .as_mut()
            .ok_or_else(|| anyhow!("terminal model is not initialized"))?;
        model.feed(bytes);
        Ok::<_, anyhow::Error>(model.snapshot_with_viewport_top(pinned_top))
    })?;
    render_terminal_snapshot(&terminal)
}

fn pump_mux_terminal() -> Result<bool> {
    // SurfaceView can disappear before Activity.onStop removes the Kotlin poll
    // callback. Do not let a background poll mark a snapshot as presented when
    // there is no renderer; otherwise the replacement Surface will suppress
    // the identical snapshot and remain on the disconnected placeholder.
    if !RENDERER.with(|slot| slot.borrow().is_some()) {
        return Ok(false);
    }
    let pinned_top = VIEW_INTERACTION.with(|interaction| interaction.borrow().pinned_top);
    let fetched = {
        let slot = mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
        let Some(session) = slot.as_ref() else {
            return Ok(false);
        };
        session.snapshot_with_viewport_top(pinned_top)?
    };
    let Some(mut snapshot) = fetched else {
        return Ok(false);
    };

    let active_tab = snapshot
        .tabs
        .iter()
        .find(|tab| tab.active)
        .map(|tab| tab.remote_tab_id);
    let previous_tab = MUX_ACTIVE_TAB.with(|cell| cell.replace(active_tab));
    if let (Some(previous), Some(active_id)) = (previous_tab, active_tab) {
        if previous != active_id {
            // The anchor just used belongs to the tab we left; save it and
            // restore the incoming tab's own anchor. This covers every switch
            // path: toolbar buttons, auto-reattach restore, and another
            // WezTerm client changing the active tab remotely.
            let outgoing_anchor =
                VIEW_INTERACTION.with(|interaction| interaction.borrow().pinned_top);
            MUX_TAB_VIEWPORTS.with(|slots| {
                slots.borrow_mut().insert(previous, outgoing_anchor);
            });
            let incoming_anchor = MUX_TAB_VIEWPORTS
                .with(|slots| slots.borrow_mut().get(&active_id).copied().flatten());
            VIEW_INTERACTION.with(|interaction| {
                let mut interaction = interaction.borrow_mut();
                interaction.pinned_top = incoming_anchor;
                interaction.selection = None;
            });
            LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
            let refetched = {
                let slot = mux_session_slot()
                    .lock()
                    .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
                match slot.as_ref() {
                    Some(session) => session.snapshot_with_viewport_top(incoming_anchor)?,
                    None => None,
                }
            };
            let Some(refetched) = refetched else {
                return Ok(false);
            };
            snapshot = refetched;
        }
    }
    if let Some(active_id) = active_tab {
        // Normalize the stored anchor against the effective (clamped) top.
        let anchor = if snapshot.terminal.viewport_offset > 0 {
            Some(snapshot.terminal.viewport_top)
        } else {
            None
        };
        MUX_TAB_VIEWPORTS.with(|slots| {
            slots.borrow_mut().insert(active_id, anchor);
        });
    }
    MUX_TAB_VIEWPORTS.with(|slots| {
        slots
            .borrow_mut()
            .retain(|tab_id, _| snapshot.tabs.iter().any(|tab| tab.remote_tab_id == *tab_id));
    });
    let changed = LAST_MUX_SNAPSHOT.with(|last| {
        let mut last = last.borrow_mut();
        if last.as_ref() == Some(&snapshot.terminal) {
            false
        } else {
            last.replace(snapshot.terminal.clone());
            true
        }
    });
    if changed {
        render_terminal_snapshot(&snapshot.terminal)?;
    }
    Ok(changed)
}

fn report_mux_transport_failure(message: String) {
    // Only the first failed operation for an attached session should enqueue a
    // reconnect event. Clearing readiness immediately also stops the 100 ms UI
    // poll from producing an unbounded stream of identical errors.
    if !MUX_READY.swap(false, Ordering::SeqCst) {
        return;
    }
    let reported = mux_session_slot()
        .lock()
        .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))
        .and_then(|slot| {
            let session = slot
                .as_ref()
                .ok_or_else(|| anyhow!("SSHMUX session disappeared after transport failure"))?;
            session.report_transport_failure(message);
            Ok(())
        });
    if let Err(error) = reported {
        log::error!("unable to report SSHMUX transport failure: {error:#}");
    }
}

fn refresh_terminal_view() -> Result<usize> {
    if MUX_READY.load(Ordering::SeqCst) {
        // A scrolled viewport may need to fetch rows that are not in the
        // ClientPane cache yet. Subsequent poll ticks will render them as the
        // upstream adaptive fetch completes.
        let _ = pump_mux_terminal()?;
    } else {
        let pinned_top = VIEW_INTERACTION.with(|interaction| interaction.borrow().pinned_top);
        let terminal = TERMINAL.with(|slot| {
            let slot = slot.borrow();
            let model = slot
                .as_ref()
                .ok_or_else(|| anyhow!("terminal model is not initialized"))?;
            Ok::<_, anyhow::Error>(model.snapshot_with_viewport_top(pinned_top))
        })?;
        render_terminal_snapshot(&terminal)?;
    }
    Ok(VIEW_INTERACTION.with(|interaction| interaction.borrow().viewport_offset))
}

fn reset_view_interaction() {
    VIEW_INTERACTION.with(|interaction| interaction.borrow_mut().reset());
    LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
    // The next pump observes the active tab afresh; per-tab anchors persist.
    MUX_ACTIVE_TAB.with(|cell| cell.take());
}

fn point_from_surface_pixels(x: f32, y: f32) -> Option<CellPoint> {
    if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
        return None;
    }
    let density_dpi =
        RENDERER.with(|slot| slot.borrow().as_ref().map(|renderer| renderer.density_dpi))?;
    let [cell_width, cell_height] = cell_size(density_dpi);
    let (columns, rows) = LAST_VIEW_SNAPSHOT.with(|last| {
        last.borrow()
            .as_ref()
            .map(|snapshot| (snapshot.columns, snapshot.rows))
    })?;
    if columns == 0
        || rows == 0
        || x >= columns as f32 * cell_width
        || y >= rows as f32 * cell_height
    {
        return None;
    }
    Some(CellPoint {
        row: (y / cell_height).floor() as usize,
        column: (x / cell_width).floor() as usize,
    })
}

fn clamped_point_from_surface_pixels(x: f32, y: f32) -> Option<CellPoint> {
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    let density_dpi =
        RENDERER.with(|slot| slot.borrow().as_ref().map(|renderer| renderer.density_dpi))?;
    let [cell_width, cell_height] = cell_size(density_dpi);
    let (columns, rows) = LAST_VIEW_SNAPSHOT.with(|last| {
        last.borrow()
            .as_ref()
            .map(|snapshot| (snapshot.columns, snapshot.rows))
    })?;
    if columns == 0 || rows == 0 {
        return None;
    }
    Some(CellPoint {
        row: ((y.max(0.0) / cell_height).floor() as usize).min(rows - 1),
        column: ((x.max(0.0) / cell_width).floor() as usize).min(columns - 1),
    })
}

fn redraw_last_view() -> Result<()> {
    let snapshot = LAST_VIEW_SNAPSHOT.with(|last| last.borrow().clone());
    if let Some(snapshot) = snapshot {
        render_terminal_snapshot(&snapshot)?;
    }
    Ok(())
}

fn scroll_viewport(delta_rows: isize) -> Result<usize> {
    let changed = VIEW_INTERACTION.with(|interaction| {
        let mut interaction = interaction.borrow_mut();
        if delta_rows == 0 {
            return false;
        }
        let Some(last) = LAST_VIEW_SNAPSHOT.with(|last| last.borrow().clone()) else {
            return false;
        };
        let live_top = last
            .viewport_top
            .saturating_add(isize::try_from(last.viewport_offset).unwrap_or(isize::MAX));
        let oldest_top = live_top
            .saturating_sub(isize::try_from(last.max_viewport_offset).unwrap_or(isize::MAX));
        // Positive deltas scroll up into history (smaller absolute rows).
        let anchor_top = interaction.pinned_top.unwrap_or(last.viewport_top);
        let next_top = anchor_top
            .saturating_sub(delta_rows)
            .clamp(oldest_top, live_top);
        let next_offset = usize::try_from(live_top - next_top).unwrap_or(0);
        let next_pinned = if next_offset > 0 {
            Some(next_top)
        } else {
            None
        };
        let changed =
            next_offset != interaction.viewport_offset || next_pinned != interaction.pinned_top;
        interaction.viewport_offset = next_offset;
        interaction.pinned_top = next_pinned;
        interaction.selection = None;
        changed
    });
    if changed {
        LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
        refresh_terminal_view()?;
    }
    Ok(VIEW_INTERACTION.with(|interaction| interaction.borrow().viewport_offset))
}

fn scroll_viewport_to_bottom() -> Result<usize> {
    let offset = VIEW_INTERACTION.with(|interaction| interaction.borrow().viewport_offset);
    scroll_viewport(-(isize::try_from(offset).unwrap_or(isize::MAX)))
}

/// Applies a terminal zoom percentage (cell scale). Returns null on success.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSetTerminalZoom(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    zoom_percent: jint,
) -> jstring {
    ffi_error_string(&mut env, "nativeSetTerminalZoom", || {
        let zoom_percent =
            u32::try_from(zoom_percent).context("terminal zoom percent must be non-negative")?;
        if !(TERMINAL_ZOOM_MIN_PERCENT..=TERMINAL_ZOOM_MAX_PERCENT).contains(&zoom_percent) {
            bail!(
                "terminal zoom percent is outside {TERMINAL_ZOOM_MIN_PERCENT}..={TERMINAL_ZOOM_MAX_PERCENT}"
            );
        }
        TERMINAL_ZOOM_PERCENT.store(zoom_percent, Ordering::SeqCst);
        let geometry = RENDERER.with(|slot| {
            slot.borrow().as_ref().map(|renderer| {
                (
                    renderer.surface_config.width,
                    renderer.surface_config.height,
                    renderer.density_dpi,
                )
            })
        });
        let Some((width, height, density_dpi)) = geometry else {
            // No live Surface yet: the stored zoom applies at Surface creation.
            return Ok(());
        };
        let snapshot = terminal_snapshot_for_surface(width, height, density_dpi);
        resize_remote_pty_if_ready()?;
        if MUX_READY.load(Ordering::SeqCst) {
            // The local TERMINAL model is not the SSHMUX mirror, and the
            // forced resize above usually keeps the pane size unchanged, so
            // the next poll would diff-equal and keep whatever frame this
            // call presented. Invalidate the last mux snapshot and re-pump
            // the real pane instead of rendering the local frame.
            LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
            pump_mux_terminal()?;
        } else {
            render_terminal_snapshot(&snapshot)?;
        }
        Ok(())
    })
}

fn begin_cell_selection(point: CellPoint) -> Result<()> {
    let (word_start, word_end) = LAST_VIEW_SNAPSHOT.with(|last| {
        last.borrow()
            .as_ref()
            .map(|snapshot| snapshot.word_bounds(point.row, point.column))
            .ok_or_else(|| anyhow!("there is no rendered terminal snapshot"))
    })?;
    VIEW_INTERACTION.with(|interaction| {
        interaction.borrow_mut().selection = Some(CellSelection {
            anchor_start: CellPoint {
                row: point.row,
                column: word_start,
            },
            anchor_end: CellPoint {
                row: point.row,
                column: word_end,
            },
            focus: CellPoint {
                row: point.row,
                column: word_end,
            },
        });
    });
    redraw_last_view()
}

fn update_cell_selection(point: CellPoint) -> Result<()> {
    let updated = VIEW_INTERACTION.with(|interaction| {
        let mut interaction = interaction.borrow_mut();
        let Some(selection) = interaction.selection.as_mut() else {
            return false;
        };
        selection.focus = point;
        true
    });
    if updated {
        redraw_last_view()?;
    }
    Ok(())
}

fn select_all_visible_cells() -> Result<()> {
    let (columns, rows) = LAST_VIEW_SNAPSHOT.with(|last| {
        last.borrow()
            .as_ref()
            .map(|snapshot| (snapshot.columns, snapshot.rows))
            .ok_or_else(|| anyhow!("there is no rendered terminal snapshot"))
    })?;
    if columns == 0 || rows == 0 {
        bail!("terminal viewport is empty");
    }
    let start = CellPoint { row: 0, column: 0 };
    let end = CellPoint {
        row: rows - 1,
        column: columns - 1,
    };
    VIEW_INTERACTION.with(|interaction| {
        interaction.borrow_mut().selection = Some(CellSelection {
            anchor_start: start,
            anchor_end: end,
            focus: end,
        });
    });
    redraw_last_view()
}

fn selected_terminal_text() -> Option<String> {
    let selection = VIEW_INTERACTION.with(|interaction| interaction.borrow().selection)?;
    let (start, end) = selection.range();
    LAST_VIEW_SNAPSHOT.with(|last| {
        last.borrow()
            .as_ref()
            .map(|snapshot| snapshot.selected_text(start.row, start.column, end.row, end.column))
    })
}

fn clear_cell_selection() -> Result<()> {
    let changed =
        VIEW_INTERACTION.with(|interaction| interaction.borrow_mut().selection.take().is_some());
    if changed {
        redraw_last_view()?;
    }
    Ok(())
}

fn resize_remote_pty_if_ready() -> Result<()> {
    let size = current_pty_size()?;
    {
        let mut slot = ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
        if let Some(session) = slot.as_mut().filter(|session| session.pty_is_ready()) {
            session.resize_pty(size)?;
            log::debug!("resized remote PTY to {}x{}", size.cols, size.rows);
        }
    }
    if MUX_READY.load(Ordering::SeqCst) {
        let mux_size = current_mux_size()?;
        let slot = mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
        if let Some(session) = slot.as_ref() {
            session.resize(mux_size)?;
            log::debug!(
                "resized remote mux pane to {}x{}",
                mux_size.cols,
                mux_size.rows
            );
        }
    }
    Ok(())
}

fn write_remote_pty(bytes: &[u8]) -> Result<bool> {
    if bytes.is_empty() {
        return Ok(false);
    }
    {
        let mut slot = ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
        if let Some(session) = slot.as_mut().filter(|session| session.pty_is_ready()) {
            session.write_pty(bytes)?;
            return Ok(true);
        }
    }
    if MUX_READY.load(Ordering::SeqCst) {
        let slot = mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
        if let Some(session) = slot.as_ref() {
            session.write(bytes)?;
            return Ok(true);
        }
    }
    Ok(false)
}

fn send_remote_mouse_wheel(x: f32, y: f32, delta: isize) -> Result<bool> {
    if delta == 0 {
        return Ok(false);
    }
    let point = clamped_point_from_surface_pixels(x, y)
        .ok_or_else(|| anyhow!("terminal surface has no addressable cells"))?;

    if MUX_READY.load(Ordering::SeqCst) {
        let slot = mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
        let session = slot
            .as_ref()
            .ok_or_else(|| anyhow!("there is no active SSHMUX session"))?;
        session.mouse_wheel(point.column, point.row, delta)?;
        drop(slot);
        reset_view_interaction();
        log::debug!(
            "sent SSHMUX mouse wheel delta={} cell={},{}",
            delta,
            point.column,
            point.row,
        );
        return Ok(true);
    }

    // The standalone SSH transport does not expose a remote Pane object yet.
    // Use cursor keys so the remote application still owns the gesture;
    // SSHMUX uses the real Pane::mouse_event path above and therefore
    // preserves every negotiated mouse-reporting mode.
    let key_code = if delta > 0 { 20 } else { 19 };
    let key_bytes = android_key_bytes(key_code, 0, 0);
    let mut bytes = Vec::with_capacity(key_bytes.len() * delta.unsigned_abs().min(32));
    for _ in 0..delta.unsigned_abs().min(32) {
        bytes.extend_from_slice(&key_bytes);
    }
    let sent = write_remote_pty(&bytes)?;
    if sent {
        reset_view_interaction();
        log::debug!("sent plain-SSH scroll-key delta={delta}");
    }
    Ok(sent)
}

fn ffi_guard(operation: &str, callback: impl FnOnce() -> Result<()>) -> jboolean {
    init_logging();
    match catch_unwind(AssertUnwindSafe(callback)) {
        Ok(Ok(())) => JNI_TRUE,
        Ok(Err(error)) => {
            log::error!("{} failed: {:#}", operation, error);
            JNI_FALSE
        }
        Err(_) => {
            log::error!("{} panicked", operation);
            JNI_FALSE
        }
    }
}

fn with_renderer(operation: &str, callback: impl FnOnce(&mut Renderer) -> Result<()>) {
    let _ = ffi_guard(operation, || {
        RENDERER.with(|slot| {
            let mut slot = slot.borrow_mut();
            let renderer = slot
                .as_mut()
                .ok_or_else(|| anyhow!("renderer has no attached Surface"))?;
            callback(renderer)
        })
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSurfaceCreated(
    env: JNIEnv<'_>,
    _class: JClass<'_>,
    surface: JObject<'_>,
    width: jint,
    height: jint,
    density_dpi: jint,
) -> jboolean {
    ffi_guard("nativeSurfaceCreated", || {
        // Android vendors may deliver a new surfaceCreated callback after a
        // failed resize without a matching callback that Kotlin considered
        // attached. Disconnect any native producer before Vulkan connects to
        // the replacement ANativeWindow; keeping the old renderer until after
        // Renderer::new causes native_window_api_connect(-22).
        let old_renderer = RENDERER.with(|slot| slot.borrow_mut().take());
        if old_renderer.is_some() {
            log::warn!("replacing a still-attached renderer during Surface hand-off");
        }
        drop(old_renderer);

        let width = width.max(1) as u32;
        let height = height.max(1) as u32;
        let density_dpi = density_dpi.max(1) as u32;
        let terminal = terminal_snapshot_for_surface(width, height, density_dpi);
        let ptr = unsafe { ANativeWindow_fromSurface(env.get_raw(), surface.as_raw()) };
        let ptr =
            NonNull::new(ptr).ok_or_else(|| anyhow!("ANativeWindow_fromSurface returned null"))?;
        let mut renderer = Renderer::new(ptr, width, height, density_dpi, &terminal)?;
        renderer.render()?;
        RENDERER.with(|slot| {
            slot.borrow_mut().replace(renderer);
        });
        VIEW_INTERACTION.with(|interaction| interaction.borrow_mut().observe_snapshot(&terminal));
        LAST_VIEW_SNAPSHOT.with(|last| {
            last.borrow_mut().replace(terminal.clone());
        });
        // LAST_MUX_SNAPSHOT describes what was presented to the old Surface,
        // not to this new Vulkan swapchain. Force one fresh remote snapshot so
        // a still-live SSHMUX session immediately replaces the idle model.
        LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
        if MUX_READY.load(Ordering::SeqCst) {
            if let Err(error) = pump_mux_terminal() {
                let message = format!("{error:#}");
                log::warn!("unable to restore SSHMUX terminal after Surface creation: {message}");
                report_mux_transport_failure(message);
            }
        }
        // Surface ownership is already valid at this point. A transient
        // network/SSHMUX resize error must not report the renderer as absent;
        // doing so desynchronizes Kotlin from the native producer and makes a
        // later create attempt connect Vulkan twice to one ANativeWindow.
        if let Err(error) = resize_remote_pty_if_ready() {
            log::warn!(
                "unable to resize remote PTY after Surface creation: {:#}",
                error
            );
        }
        log::info!(
            "P1-B WezTerm/HarfBuzz/FreeType atlas is ready revision={} grid={}x{} occupied={} cursor={},{}",
            UPSTREAM_WEZTERM_REVISION,
            terminal.columns,
            terminal.rows,
            terminal.cells.len(),
            terminal.cursor_column,
            terminal.cursor_row,
        );
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSurfaceChanged(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    width: jint,
    height: jint,
) -> jboolean {
    let result = ffi_guard("nativeSurfaceChanged", || {
        RENDERER.with(|slot| {
            let mut slot = slot.borrow_mut();
            let renderer = slot
                .as_mut()
                .ok_or_else(|| anyhow!("renderer has no attached Surface"))?;
            let width = width.max(1) as u32;
            let height = height.max(1) as u32;
            let terminal = terminal_snapshot_for_surface(width, height, renderer.density_dpi);
            let selection = VIEW_INTERACTION.with(|interaction| interaction.borrow().selection);
            renderer.resize(width, height, &terminal, selection)?;
            VIEW_INTERACTION
                .with(|interaction| interaction.borrow_mut().observe_snapshot(&terminal));
            LAST_VIEW_SNAPSHOT.with(|last| {
                last.borrow_mut().replace(terminal);
            });
            renderer.render()
        })
    });

    if result == JNI_FALSE {
        // A configure/acquire panic or error leaves the Vulkan producer
        // unusable. Drop it before Kotlin retries nativeSurfaceCreated.
        let failed_renderer = RENDERER.with(|slot| slot.borrow_mut().take());
        drop(failed_renderer);
        return JNI_FALSE;
    }

    if let Err(error) = resize_remote_pty_if_ready() {
        log::warn!(
            "unable to resize remote PTY after Surface change: {:#}",
            error
        );
    }
    if MUX_READY.load(Ordering::SeqCst) {
        // The block above uploaded the local TERMINAL model, which during
        // SSHMUX is a stale placeholder; the forced/normal remote resize
        // usually keeps the pane size unchanged, so the next poll would
        // diff-equal and keep that placeholder on screen. Drop the last mux
        // snapshot and re-pump the real pane. The RENDERER borrow is already
        // released here, so pump may take it again to render.
        LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
        if let Err(error) = pump_mux_terminal() {
            log::warn!("unable to restore SSHMUX terminal after Surface change: {error:#}");
        }
    }
    JNI_TRUE
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSurfaceRedrawNeeded(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    with_renderer("nativeSurfaceRedrawNeeded", Renderer::render);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSurfaceDestroyed(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    let _ = ffi_guard("nativeSurfaceDestroyed", || {
        let renderer = RENDERER.with(|slot| slot.borrow_mut().take());
        drop(renderer);
        // Snapshot equality is meaningful only for the Surface on which the
        // snapshot was rendered. The next Surface must receive a full frame.
        LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
        VIEW_INTERACTION.with(|interaction| interaction.borrow_mut().selection = None);
        log::info!("destroyed wgpu Surface while retaining the WezTerm terminal model");
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeScrollByRows(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    delta_rows: jint,
) -> jint {
    init_logging();
    match catch_unwind(AssertUnwindSafe(|| scroll_viewport(delta_rows as isize))) {
        Ok(Ok(offset)) => jint::try_from(offset).unwrap_or(jint::MAX),
        Ok(Err(error)) => {
            log::warn!("nativeScrollByRows failed: {error:#}");
            -1
        }
        Err(_) => {
            log::error!("nativeScrollByRows panicked");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeScrollToBottom(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jint {
    init_logging();
    match catch_unwind(AssertUnwindSafe(scroll_viewport_to_bottom)) {
        Ok(Ok(offset)) => jint::try_from(offset).unwrap_or(jint::MAX),
        Ok(Err(error)) => {
            log::warn!("nativeScrollToBottom failed: {error:#}");
            -1
        }
        Err(_) => {
            log::error!("nativeScrollToBottom panicked");
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSelectionStart(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    x: jfloat,
    y: jfloat,
) -> jboolean {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<bool> {
        let Some(point) = point_from_surface_pixels(x, y) else {
            return Ok(false);
        };
        begin_cell_selection(point)?;
        Ok(true)
    }));
    match outcome {
        Ok(Ok(true)) => JNI_TRUE,
        Ok(Ok(false)) => JNI_FALSE,
        Ok(Err(error)) => {
            log::warn!("nativeSelectionStart failed: {error:#}");
            JNI_FALSE
        }
        Err(_) => {
            log::error!("nativeSelectionStart panicked");
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSelectionUpdate(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    x: jfloat,
    y: jfloat,
) -> jboolean {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<bool> {
        let Some(point) = point_from_surface_pixels(x, y) else {
            return Ok(false);
        };
        update_cell_selection(point)?;
        Ok(true)
    }));
    if matches!(outcome, Ok(Ok(true))) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSelectionSelectAll(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    ffi_guard("nativeSelectionSelectAll", select_all_visible_cells)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSelectionClear(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    let _ = ffi_guard("nativeSelectionClear", clear_cell_selection);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSelectionText(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    init_logging();
    let value = catch_unwind(AssertUnwindSafe(selected_terminal_text))
        .ok()
        .flatten();
    return_java_string(&mut env, value)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshWriteText(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    text: JString<'_>,
) -> jstring {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        let text = read_java_string(&mut env, &text)?;
        if text.is_empty() {
            return Ok(());
        }
        reset_view_interaction();
        if !write_remote_pty(text.as_bytes())? {
            bail!("remote PTY is not ready");
        }
        // Do not log the input itself. Lengths are sufficient for transport
        // diagnostics and avoid exposing commands or secrets in logcat.
        log::debug!(
            "committed input bytes={} chars={}",
            text.len(),
            text.chars().count()
        );
        Ok(())
    }));
    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            log::error!("nativeSshWriteText failed: {:#}", error);
            Some(format!("{:#}", error))
        }
        Err(_) => Some("nativeSshWriteText panicked".to_string()),
    };
    return_java_string(&mut env, error)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeKeyEvent(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    key_code: jint,
    unicode_code_point: jint,
    meta_state: jint,
    is_down: jboolean,
) {
    init_logging();
    log::debug!(
        "key code={} unicode_present={} meta={:#x} down={}",
        key_code,
        unicode_code_point != 0,
        meta_state,
        is_down != JNI_FALSE,
    );
    if is_down != JNI_FALSE {
        let bytes = android_key_bytes(key_code, unicode_code_point, meta_state);
        if !bytes.is_empty() {
            reset_view_interaction();
        }
        if let Err(error) = write_remote_pty(&bytes) {
            log::warn!("unable to send key to remote PTY: {:#}", error);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeRemoteMouseWheel(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
    x: jfloat,
    y: jfloat,
    delta: jint,
) -> jboolean {
    init_logging();
    let mux_was_ready = MUX_READY.load(Ordering::SeqCst);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        send_remote_mouse_wheel(x, y, delta as isize)
    }));
    match outcome {
        Ok(Ok(true)) => JNI_TRUE,
        Ok(Ok(false)) => JNI_FALSE,
        Ok(Err(error)) => {
            let message = format!("{error:#}");
            log::warn!("nativeRemoteMouseWheel failed: {message}");
            if mux_was_ready {
                report_mux_transport_failure(message);
            }
            JNI_FALSE
        }
        Err(_) => {
            log::error!("nativeRemoteMouseWheel panicked");
            if mux_was_ready {
                report_mux_transport_failure("nativeRemoteMouseWheel panicked".to_string());
            }
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    host: JString<'_>,
    user: JString<'_>,
    port: jint,
    app_files_dir: JString<'_>,
    identity_file: JString<'_>,
) -> jstring {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        if UPSTREAM_WEZTERM_REVISION != SSH_UPSTREAM_WEZTERM_REVISION {
            bail!(
                "terminal/SSH WezTerm revisions differ: {} != {}",
                UPSTREAM_WEZTERM_REVISION,
                SSH_UPSTREAM_WEZTERM_REVISION,
            );
        }
        let host = read_java_string(&mut env, &host)?;
        let user = read_java_string(&mut env, &user)?;
        let app_files_dir = read_java_string(&mut env, &app_files_dir)?;
        let identity_file = read_java_string(&mut env, &identity_file)?;
        let port = u16::try_from(port).context("SSH port is outside 1..=65535")?;

        let endpoint = SshEndpoint::new(host, user, port)?;
        let mut config = AndroidSshConfig::new(endpoint, app_files_dir)?;
        std::fs::create_dir_all(config.ssh_dir().join("identities"))
            .context("create app-private SSH directory")?;
        if !identity_file.is_empty() {
            config = config.with_identity_file(identity_file)?;
        }

        let mut slot = ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
        if slot.is_some() {
            bail!("an SSH session is already active; disconnect it before reconnecting");
        }
        if mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?
            .is_some()
        {
            bail!("an SSHMUX session is active; safely detach it before using plain SSH");
        }
        reset_view_interaction();
        log::info!(
            "starting isolated Android SSH session user={} host={} port={} revision={}",
            config.endpoint().user(),
            config.endpoint().host(),
            config.endpoint().port(),
            SSH_UPSTREAM_WEZTERM_REVISION,
        );
        slot.replace(config.connect()?);
        Ok(())
    }));

    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            log::error!("nativeSshStart failed: {:#}", error);
            Some(format!("{:#}", error))
        }
        Err(_) => {
            log::error!("nativeSshStart panicked");
            Some("nativeSshStart panicked".to_string())
        }
    };
    return_java_string(&mut env, error)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshHasSession(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    init_logging();
    match ssh_session_slot().lock() {
        Ok(slot) if slot.is_some() => JNI_TRUE,
        Ok(_) => JNI_FALSE,
        Err(_) => {
            log::error!("nativeSshHasSession: SSH session lock is poisoned");
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshOpenPty(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    ffi_error_string(&mut env, "nativeSshOpenPty", || {
        let size = current_pty_size()?;
        {
            let mut slot = ssh_session_slot()
                .lock()
                .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
            let session = slot
                .as_mut()
                .ok_or_else(|| anyhow!("there is no active SSH session"))?;
            session.open_pty(size)?;
        }
        feed_terminal_and_render(b"\x1b[?25h\x1b[2J\x1b[H")?;
        log::info!("requested remote PTY size={}x{}", size.cols, size.rows);
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshPtyReady(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    init_logging();
    match ssh_session_slot().lock() {
        Ok(slot) if slot.as_ref().is_some_and(AndroidSshSession::pty_is_ready) => JNI_TRUE,
        _ => JNI_FALSE,
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshPumpTerminal(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<bool> {
        let bytes = {
            let mut slot = ssh_session_slot()
                .lock()
                .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
            let Some(session) = slot.as_mut() else {
                return Ok(false);
            };
            session.drain_pty_output(256 * 1024)
        };
        if bytes.is_empty() {
            return Ok(false);
        }
        log::debug!("feeding {} remote PTY bytes into wezterm-term", bytes.len());
        feed_terminal_and_render(&bytes)?;
        Ok(true)
    }));
    match outcome {
        Ok(Ok(true)) => JNI_TRUE,
        Ok(Ok(false)) => JNI_FALSE,
        Ok(Err(error)) => {
            log::error!("nativeSshPumpTerminal failed: {:#}", error);
            JNI_FALSE
        }
        Err(_) => {
            log::error!("nativeSshPumpTerminal panicked");
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshDisconnect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    ffi_error_string(&mut env, "nativeSshDisconnect", || {
        ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?
            .take();
        replace_with_idle_terminal()?;
        log::info!("dropped Android SSH session independently of the Surface");
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshPollEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    init_logging();
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Option<String>> {
        let mut slot = ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
        let Some(session) = slot.as_mut() else {
            return Ok(None);
        };
        session
            .try_next_event()?
            .as_ref()
            .map(serialize_client_event)
            .transpose()
    }));

    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            log::error!("nativeSshPollEvent failed: {:#}", error);
            Some(
                serde_json::json!({"type": "control_error", "message": format!("{:#}", error)})
                    .to_string(),
            )
        }
        Err(_) => Some(
            serde_json::json!({"type": "control_error", "message": "nativeSshPollEvent panicked"})
                .to_string(),
        ),
    };
    return_java_string(&mut env, value)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshPendingEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    init_logging();
    let value = ssh_session_slot()
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().and_then(AndroidSshSession::pending_event))
        .and_then(|event| match serialize_client_event(&event) {
            Ok(value) => Some(value),
            Err(error) => {
                log::error!("nativeSshPendingEvent failed: {:#}", error);
                None
            }
        });
    return_java_string(&mut env, value)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshAnswerHostVerification(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    trust: jboolean,
) -> jstring {
    ffi_error_string(&mut env, "nativeSshAnswerHostVerification", || {
        let mut slot = ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
        let session = slot
            .as_mut()
            .ok_or_else(|| anyhow!("there is no active SSH session"))?;
        session.answer_host_verification(trust != JNI_FALSE)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeSshAnswerAuthentication(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    answers_json: JString<'_>,
) -> jstring {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        let answers_json = read_java_string(&mut env, &answers_json)?;
        let answers: Vec<String> =
            serde_json::from_str(&answers_json).context("parse authentication answers")?;
        let mut slot = ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?;
        let session = slot
            .as_mut()
            .ok_or_else(|| anyhow!("there is no active SSH session"))?;
        session.answer_authentication(answers)?;
        Ok(())
    }));
    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            log::error!("nativeSshAnswerAuthentication failed: {:#}", error);
            Some(format!("{:#}", error))
        }
        Err(_) => Some("nativeSshAnswerAuthentication panicked".to_string()),
    };
    return_java_string(&mut env, error)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxStart(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    host: JString<'_>,
    user: JString<'_>,
    port: jint,
    app_files_dir: JString<'_>,
    identity_file: JString<'_>,
    remote_wezterm_path: JString<'_>,
    preferred_remote_tab_id: jlong,
) -> jstring {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        if UPSTREAM_WEZTERM_REVISION != MUX_UPSTREAM_WEZTERM_REVISION {
            bail!(
                "terminal/SSHMUX WezTerm revisions differ: {} != {}",
                UPSTREAM_WEZTERM_REVISION,
                MUX_UPSTREAM_WEZTERM_REVISION,
            );
        }

        let host = read_java_string(&mut env, &host)?;
        let user = read_java_string(&mut env, &user)?;
        let app_files_dir = read_java_string(&mut env, &app_files_dir)?;
        let identity_file = read_java_string(&mut env, &identity_file)?;
        let remote_wezterm_path = read_java_string(&mut env, &remote_wezterm_path)?;
        let port = u16::try_from(port).context("SSH port is outside 1..=65535")?;
        let preferred_remote_tab_id = (preferred_remote_tab_id >= 0)
            .then(|| usize::try_from(preferred_remote_tab_id))
            .transpose()
            .context("preferred remote tab id is outside the platform range")?;

        if ssh_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSH session lock is poisoned"))?
            .is_some()
        {
            bail!("a plain SSH session is active; disconnect it before attaching SSHMUX");
        }
        {
            let slot = mux_session_slot()
                .lock()
                .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
            if slot.is_some() {
                bail!("an SSHMUX session is already active");
            }
        }

        let private_home = PathBuf::from(&app_files_dir);
        for relative in ["ssh/identities", "cache", "config", "data", "runtime"] {
            std::fs::create_dir_all(private_home.join(relative))
                .with_context(|| format!("create app-private {relative} directory"))?;
        }
        #[cfg(target_os = "android")]
        if !dirs_next::set_android_home_dir(private_home) {
            bail!("Android private home was already initialized to a different directory");
        }
        let identity = (!identity_file.is_empty()).then(|| PathBuf::from(identity_file));
        let endpoint = MuxEndpoint::new(
            &host,
            &user,
            port,
            &app_files_dir,
            identity,
            &remote_wezterm_path,
        )?;
        let session =
            AndroidMuxSession::start(endpoint, current_mux_size()?, preferred_remote_tab_id)?;

        reset_view_interaction();
        MUX_READY.store(false, Ordering::SeqCst);
        mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?
            .replace(session);
        log::info!(
            "starting isolated Android SSHMUX attachment user={} host={} port={} revision={}",
            user,
            host,
            port,
            MUX_UPSTREAM_WEZTERM_REVISION,
        );
        Ok(())
    }));

    let error = match outcome {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            log::error!("nativeMuxStart failed: {:#}", error);
            Some(format!("{:#}", error))
        }
        Err(_) => Some("nativeMuxStart panicked".to_string()),
    };
    return_java_string(&mut env, error)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxHasSession(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    init_logging();
    match mux_session_slot().lock() {
        Ok(slot) if slot.is_some() => JNI_TRUE,
        Ok(_) => JNI_FALSE,
        Err(_) => {
            log::error!("nativeMuxHasSession: SSHMUX session lock is poisoned");
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxReady(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    if MUX_READY.load(Ordering::SeqCst) {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxPumpTerminal(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jboolean {
    init_logging();
    let outcome = catch_unwind(AssertUnwindSafe(pump_mux_terminal));
    match outcome {
        Ok(Ok(true)) => JNI_TRUE,
        Ok(Ok(false)) => JNI_FALSE,
        Ok(Err(error)) => {
            let message = format!("{error:#}");
            log::error!("nativeMuxPumpTerminal failed: {message}");
            report_mux_transport_failure(message);
            JNI_FALSE
        }
        Err(_) => {
            log::error!("nativeMuxPumpTerminal panicked");
            report_mux_transport_failure("nativeMuxPumpTerminal panicked".to_string());
            JNI_FALSE
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxPollEvent(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    init_logging();
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<Option<String>> {
        let event = {
            let slot = mux_session_slot()
                .lock()
                .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
            slot.as_ref().and_then(AndroidMuxSession::try_next_event)
        };
        if let Some(event) = event {
            match event {
                MuxEvent::Attached { .. } => MUX_READY.store(true, Ordering::SeqCst),
                MuxEvent::Detached | MuxEvent::Error { .. } => {
                    MUX_READY.store(false, Ordering::SeqCst)
                }
                MuxEvent::Connecting | MuxEvent::TabsChanged { .. } => {}
            }
            return serialize_mux_event(&event).map(Some);
        }
        Ok(None)
    }));

    let value = match result {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            log::error!("nativeMuxPollEvent failed: {:#}", error);
            Some(
                serde_json::json!({"type": "error", "message": format!("{:#}", error)}).to_string(),
            )
        }
        Err(_) => Some(
            serde_json::json!({"type": "error", "message": "nativeMuxPollEvent panicked"})
                .to_string(),
        ),
    };
    return_java_string(&mut env, value)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxCurrentTabs(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    init_logging();
    let value = catch_unwind(AssertUnwindSafe(|| -> Result<Option<String>> {
        if !MUX_READY.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let slot = mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
        let Some(session) = slot.as_ref() else {
            return Ok(None);
        };
        let tabs = session.tabs()?;
        Ok(Some(
            serde_json::to_string(&tabs).context("serialize current SSHMUX tabs")?,
        ))
    }))
    .ok()
    .and_then(|result| match result {
        Ok(value) => value,
        Err(error) => {
            log::error!("nativeMuxCurrentTabs failed: {:#}", error);
            None
        }
    });
    return_java_string(&mut env, value)
}

fn with_mux_session(
    operation: &str,
    callback: impl FnOnce(&AndroidMuxSession) -> Result<()>,
) -> Result<()> {
    let slot = mux_session_slot()
        .lock()
        .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?;
    let session = slot
        .as_ref()
        .ok_or_else(|| anyhow!("there is no active SSHMUX session"))?;
    callback(session).with_context(|| operation.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxActivateRelative(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    delta: jint,
) -> jstring {
    ffi_error_string(&mut env, "nativeMuxActivateRelative", || {
        if delta != -1 && delta != 1 {
            bail!("relative tab delta must be -1 or 1");
        }
        // No reset here: the next pump detects the active-tab change, saves
        // the outgoing tab's viewport anchor, and restores the incoming tab's
        // own anchor. Zeroing here would destroy the outgoing scroll position.
        with_mux_session("activate remote tab", |session| {
            session.activate_relative(delta as isize)
        })?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxActivateTab(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    remote_tab_id: jlong,
) -> jstring {
    ffi_error_string(&mut env, "nativeMuxActivateTab", || {
        let remote_tab_id =
            usize::try_from(remote_tab_id).context("remote tab id must be non-negative")?;
        // Same as above: the pump owns per-tab anchor save/restore on switch.
        with_mux_session("restore remote tab", |session| {
            session.activate(remote_tab_id)
        })?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxSpawnTab(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    ffi_error_string(&mut env, "nativeMuxSpawnTab", || {
        with_mux_session("spawn remote tab", AndroidMuxSession::spawn_tab)?;
        reset_view_interaction();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxCloseActiveTab(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    ffi_error_string(&mut env, "nativeMuxCloseActiveTab", || {
        with_mux_session("close active remote tab", |session| {
            session.close_active_tab()
        })?;
        reset_view_interaction();
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_example_wezterm_1android_NativeBridge_nativeMuxDetach(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    ffi_error_string(&mut env, "nativeMuxDetach", || {
        // Release the global lock before the synchronous domain detach and
        // runtime-thread join. This also prevents lock-order inversions with
        // renderer resize or input callbacks.
        let session = mux_session_slot()
            .lock()
            .map_err(|_| anyhow!("SSHMUX session lock is poisoned"))?
            .take();
        MUX_READY.store(false, Ordering::SeqCst);
        LAST_MUX_SNAPSHOT.with(|last| last.borrow_mut().take());
        let detach_result = if let Some(session) = session {
            let result = session.detach();
            drop(session);
            result
        } else {
            Ok(())
        };
        replace_with_idle_terminal()?;
        detach_result?;
        log::info!("safely detached Android SSHMUX client; remote panes were not killed");
        Ok(())
    })
}
