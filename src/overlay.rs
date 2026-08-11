use crate::config::Config;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use anyhow::{Context, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use smithay_client_toolkit::reexports::client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Col {
    pub name: String,
    pub value: String,
    pub sub: String,
    pub best: bool,
}

pub enum Msg {
    Show(Vec<Col>),
}

pub type Handle = smithay_client_toolkit::reexports::calloop::channel::Sender<Msg>;

const SLOT_W: f32 = 968.0 / 1920.0 / 4.0;

pub fn spawn(cfg: &Config) -> Handle {
    let (tx, rx) = smithay_client_toolkit::reexports::calloop::channel::channel();
    let cfg = cfg.clone();
    std::thread::spawn(move || {
        if let Err(e) = run(&cfg, rx) {
            eprintln!("overlay disabled: {e:#}");
        }
    });
    tx
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: SlotPool,
    font: FontVec,
    cfg: Config,
    target_output: String,
    layer: Option<LayerSurface>,
    size: (u32, u32),
    content: Vec<Col>,
    hide_at: Option<Instant>,
}

fn run(
    cfg: &Config,
    rx: smithay_client_toolkit::reexports::calloop::channel::Channel<Msg>,
) -> Result<()> {
    let font_data = load_font(cfg)?;
    let font = FontVec::try_from_vec(font_data).context("parsing overlay font")?;

    let conn = Connection::connect_to_env().context("connecting to Wayland")?;
    let (globals, event_queue) = registry_queue_init::<App>(&conn).context("wayland registry")?;
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor")?;
    let layer_shell = LayerShell::bind(&globals, &qh).context(
        "layer shell (compositor has no zwlr_layer_shell_v1; overlay needs KDE/wlroots)",
    )?;
    let shm = Shm::bind(&globals, &qh).context("wl_shm")?;
    let pool = SlotPool::new(256 * 256 * 4, &shm).context("shm pool")?;

    let target_output = xcap::Monitor::all()
        .ok()
        .and_then(|ms| ms.into_iter().nth(cfg.monitor))
        .and_then(|m| m.name().ok())
        .unwrap_or_default();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool,
        font,
        cfg: cfg.clone(),
        target_output,
        layer: None,
        size: (0, 0),
        content: Vec::new(),
        hide_at: None,
    };

    let mut event_loop =
        smithay_client_toolkit::reexports::calloop::EventLoop::<App>::try_new()
            .context("event loop")?;
    smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource::new(
        conn,
        event_queue,
    )
    .insert(event_loop.handle())
    .map_err(|e| anyhow::anyhow!("inserting wayland source: {e}"))?;

    let qh2 = qh.clone();
    event_loop
        .handle()
        .insert_source(rx, move |ev, _, app| {
            if let smithay_client_toolkit::reexports::calloop::channel::Event::Msg(Msg::Show(
                cols,
            )) = ev
            {
                app.show(cols, &qh2);
            }
        })
        .map_err(|e| anyhow::anyhow!("inserting channel source: {e}"))?;

    loop {
        event_loop
            .dispatch(Some(Duration::from_millis(250)), &mut app)
            .context("dispatching events")?;
        if let Some(at) = app.hide_at
            && Instant::now() >= at
        {
            app.layer = None;
            app.hide_at = None;
        }
    }
}

fn load_font(cfg: &Config) -> Result<Vec<u8>> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(p) = &cfg.overlay.font_path {
        candidates.push(p.clone());
    }
    for p in [
        "/usr/share/fonts/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/liberation-fonts/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/noto/NotoSans-Bold.ttf",
    ] {
        candidates.push(p.into());
    }
    for p in &candidates {
        if let Ok(data) = std::fs::read(p) {
            return Ok(data);
        }
    }
    anyhow::bail!(
        "no usable font found (set font_path under [overlay]); tried {} paths",
        candidates.len()
    )
}

impl App {
    fn show(&mut self, cols: Vec<Col>, qh: &QueueHandle<App>) {
        if cols.is_empty() {
            return;
        }
        let (output, out_size) = self.pick_output();
        let (ow, oh) = out_size;
        let panel_w = ((SLOT_W * ow as f32) as u32 * cols.len() as u32).max(200);
        let panel_h = ((self.cfg.overlay.height * oh as f32) as u32).max(60);
        let margin_top = (self
            .cfg
            .overlay
            .y
            .unwrap_or(self.cfg.band.y + self.cfg.band.h + 0.008)
            * oh as f32) as i32;

        self.content = cols;
        self.size = (panel_w, panel_h);
        self.hide_at =
            Some(Instant::now() + Duration::from_millis(self.cfg.overlay.timeout_ms));

        self.layer = None;
        let surface = self.compositor.create_surface(qh);

        if let Ok(region) = Region::new(&self.compositor) {
            surface.set_input_region(Some(region.wl_region()));
        }
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("relic-check"),
            output.as_ref(),
        );
        layer.set_anchor(Anchor::TOP);
        layer.set_size(panel_w, panel_h);
        layer.set_margin(margin_top, 0, 0, 0);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(-1);
        layer.commit();
        self.layer = Some(layer);
    }

    fn pick_output(&mut self) -> (Option<wl_output::WlOutput>, (i32, i32)) {
        let mut fallback = None;
        for output in self.output_state.outputs() {
            let Some(info) = self.output_state.info(&output) else { continue };
            let size = info.logical_size.unwrap_or((1920, 1080));
            if info.name.as_deref() == Some(self.target_output.as_str()) {
                return (Some(output), size);
            }
            fallback.get_or_insert((output, size));
        }
        match fallback {
            Some((o, s)) => (Some(o), s),
            None => (None, (1920, 1080)),
        }
    }

    fn draw(&mut self, qh: &QueueHandle<App>) {
        let Some(layer) = &self.layer else { return };
        let (w, h) = self.size;
        let stride = w as i32 * 4;
        let Ok((buffer, canvas)) =
            self.pool
                .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)
        else {
            return;
        };

        const BG_A: u32 = 222;
        let bg = [(20 * BG_A / 255) as u8, (16 * BG_A / 255) as u8, (15 * BG_A / 255) as u8, BG_A as u8];
        for px in canvas.chunks_exact_mut(4) {
            px.copy_from_slice(&bg);
        }

        let n = self.content.len() as u32;
        let col_w = w / n.max(1);
        const WHITE: (u8, u8, u8) = (235, 235, 235);
        const GOLD: (u8, u8, u8) = (255, 190, 60);
        const DIM: (u8, u8, u8) = (150, 150, 150);
        let name_px = PxScale::from(h as f32 * 0.17);
        let value_px = PxScale::from(h as f32 * 0.24);
        let sub_px = PxScale::from(h as f32 * 0.14);

        let cols = std::mem::take(&mut self.content);
        for (i, col) in cols.iter().enumerate() {
            let x0 = i as u32 * col_w;

            if i > 0 {
                for y in (h / 8)..(h * 7 / 8) {
                    blend_px(canvas, w, x0, y, (90, 90, 90), 0.8);
                }
            }
            if col.best {
                for y in 0..(h / 24).max(3) {
                    for x in x0..(x0 + col_w).min(w) {
                        blend_px(canvas, w, x, y, GOLD, 1.0);
                    }
                }
            }
            let cx = x0 as f32 + col_w as f32 / 2.0;
            let pad = col_w as f32 * 0.06;
            let name_color = if col.best { GOLD } else { WHITE };
            let mut y = h as f32 * 0.30;
            for line in wrap(&self.font, name_px, &col.name, col_w as f32 - 2.0 * pad, 2) {
                draw_text(&self.font, canvas, w, h, &line, name_px, cx, y, name_color);
                y += h as f32 * 0.19;
            }
            draw_text(
                &self.font,
                canvas,
                w,
                h,
                &col.value,
                value_px,
                cx,
                h as f32 * 0.68,
                if col.best { GOLD } else { WHITE },
            );
            draw_text(&self.font, canvas, w, h, &col.sub, sub_px, cx, h as f32 * 0.87, DIM);
        }
        self.content = cols;

        let surface = layer.wl_surface();
        buffer.attach_to(surface).ok();
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(qh, surface.clone());
        surface.commit();
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_text(
    font: &FontVec,
    canvas: &mut [u8],
    w: u32,
    h: u32,
    text: &str,
    scale: PxScale,
    cx: f32,
    y: f32,
    color: (u8, u8, u8),
) {
    let sf = font.as_scaled(scale);
    let width: f32 = text.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum();
    let mut x = cx - width / 2.0;
    for ch in text.chars() {
        let gid = sf.glyph_id(ch);
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(x, y));
        x += sf.h_advance(gid);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                    blend_px(canvas, w, px as u32, py as u32, color, cov);
                }
            });
        }
    }
}

fn blend_px(canvas: &mut [u8], w: u32, x: u32, y: u32, color: (u8, u8, u8), cov: f32) {
    let idx = ((y * w + x) * 4) as usize;
    let Some(px) = canvas.get_mut(idx..idx + 4) else { return };
    let inv = 1.0 - cov;
    px[0] = (color.2 as f32 * cov + px[0] as f32 * inv) as u8;
    px[1] = (color.1 as f32 * cov + px[1] as f32 * inv) as u8;
    px[2] = (color.0 as f32 * cov + px[2] as f32 * inv) as u8;
    px[3] = (255.0 * cov + px[3] as f32 * inv) as u8;
}

fn wrap(font: &FontVec, scale: PxScale, text: &str, max_w: f32, max_lines: usize) -> Vec<String> {
    let sf = font.as_scaled(scale);
    let width = |s: &str| -> f32 { s.chars().map(|c| sf.h_advance(sf.glyph_id(c))).sum() };
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate =
            if current.is_empty() { word.to_string() } else { format!("{current} {word}") };
        if width(&candidate) <= max_w || current.is_empty() {
            current = candidate;
        } else {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
            if lines.len() == max_lines {
                break;
            }
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    } else if lines.len() == max_lines
        && let Some(last) = lines.last_mut()
    {
        last.push('…');
    }
    lines
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.layer = None;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if configure.new_size.0 > 0 && configure.new_size.1 > 0 {
            self.size = configure.new_size;
        }
        self.draw(qh);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_layer!(App);
delegate_shm!(App);
delegate_registry!(App);
