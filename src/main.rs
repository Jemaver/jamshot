//! JamShot — швидкий скріншотер для COSMIC у стилі Lightshot.

mod gfx;
mod scene;
mod theme;

use scene::{Action, Cursor, Icons, Scene, Screen, Tool};
use std::io::Write;
use std::time::Duration;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop, LoopHandle,
        },
        calloop_wayland_source::WaylandSource,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers},
        pointer::{cursor_shape::CursorShapeManager, PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Proxy, QueueHandle, WEnum,
};
use wayland_protocols::{
    ext::{
        image_capture_source::v1::client::{
            ext_image_capture_source_v1::ExtImageCaptureSourceV1,
            ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
        },
        image_copy_capture::v1::client::{
            ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
            ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
            ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
        },
    },
    wp::{
        cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape as CShape, WpCursorShapeDeviceV1},
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
};

// evdev-коди клавіш (не залежать від розкладки)
const KEY_C: u32 = 46;
const KEY_S: u32 = 31;
const KEY_Z: u32 = 44;
const KEY_V: u32 = 47;
const KEY_P: u32 = 25;
const KEY_L: u32 = 38;
const KEY_A: u32 = 30;
const KEY_R: u32 = 19;
const KEY_M: u32 = 50;
const KEY_T: u32 = 20;
const KEY_B: u32 = 48;

#[derive(Default)]
struct Cap {
    w: u32,
    h: u32,
    formats: Vec<wl_shm::Format>,
    format: Option<wl_shm::Format>,
    done: bool,
    ready: bool,
    failed: bool,
}

struct Out {
    screen: Screen,
    layer: LayerSurface,
    viewport: WpViewport,
    configured: bool,
    frame: resvg::tiny_skia::Pixmap,
    frame_pending: bool,
    dirty: bool,
}

struct App {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: Option<SlotPool>,
    caps: Vec<Cap>,
    outs: Vec<Out>,
    scene: Scene,
    theme: theme::Theme,
    icons: Option<Icons>,
    fonts: gfx::Fonts,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    cursor_mgr: Option<CursorShapeManager>,
    cursor_dev: Option<WpCursorShapeDeviceV1>,
    enter_serial: u32,
    cur_shape: Option<CShape>,
    mods: Modifiers,
    action: Action,
    exit: bool,
    hover_gen: u64,
    loop_handle: Option<LoopHandle<'static, App>>,
    qh: QueueHandle<App>,
}

struct Noop;
struct SessData(usize);
struct FrameData(usize);

impl<I: Proxy> Dispatch2<I, App> for Noop {
    fn event(&self, _: &mut App, _: &I, _: I::Event, _: &Connection, _: &QueueHandle<App>) {}
}

impl Dispatch2<ExtImageCopyCaptureSessionV1, App> for SessData {
    fn event(
        &self,
        app: &mut App,
        _: &ExtImageCopyCaptureSessionV1,
        ev: ext_image_copy_capture_session_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        use ext_image_copy_capture_session_v1::Event as E;
        let c = &mut app.caps[self.0];
        match ev {
            E::BufferSize { width, height } => {
                c.w = width;
                c.h = height;
            }
            E::ShmFormat { format: WEnum::Value(f) } => c.formats.push(f),
            E::Done => c.done = true,
            E::Stopped => c.failed = true,
            _ => {}
        }
    }
}

impl Dispatch2<ExtImageCopyCaptureFrameV1, App> for FrameData {
    fn event(
        &self,
        app: &mut App,
        _: &ExtImageCopyCaptureFrameV1,
        ev: ext_image_copy_capture_frame_v1::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        use ext_image_copy_capture_frame_v1::Event as E;
        let c = &mut app.caps[self.0];
        match ev {
            E::Ready => c.ready = true,
            E::Failed { .. } => c.failed = true,
            _ => {}
        }
    }
}

fn to_pixmap(bytes: &[u8], w: u32, h: u32, f: wl_shm::Format) -> resvg::tiny_skia::Pixmap {
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
    let bgr = matches!(f, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888);
    for (d, s) in pm.data_mut().chunks_exact_mut(4).zip(bytes.chunks_exact(4)) {
        if bgr {
            d[0] = s[2];
            d[1] = s[1];
            d[2] = s[0];
        } else {
            d[0] = s[0];
            d[1] = s[1];
            d[2] = s[2];
        }
        d[3] = 255;
    }
    pm
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "--test-render" {
        return test_render(&args[2], &args[3]);
    }
    let t0 = std::time::Instant::now();
    let timing = std::env::var("JAMSHOT_TIMING").is_ok();

    let conn = Connection::connect_to_env().expect("немає Wayland-з'єднання");
    let (globals, mut queue) = registry_queue_init::<App>(&conn).unwrap();
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor");
    let layer_shell = LayerShell::bind(&globals, &qh).expect("layer shell недоступний");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm");
    let viewporter: WpViewporter = globals.bind(&qh, 1..=1, Noop).expect("wp_viewporter");
    let src_mgr: ExtOutputImageCaptureSourceManagerV1 =
        globals.bind(&qh, 1..=1, Noop).expect("ext_output_image_capture_source_manager_v1");
    let copy_mgr: ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, Noop).expect("ext_image_copy_capture_manager_v1");
    let cursor_mgr = CursorShapeManager::bind(&globals, &qh).ok();

    let mut app = App {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool: None,
        caps: vec![],
        outs: vec![],
        scene: Scene::new(),
        theme: theme::Theme::load(),
        icons: None,
        fonts: gfx::Fonts::load(),
        keyboard: None,
        pointer: None,
        cursor_mgr,
        cursor_dev: None,
        enter_serial: 0,
        cur_shape: None,
        mods: Modifiers::default(),
        action: Action::None,
        exit: false,
        hover_gen: 0,
        loop_handle: None,
        qh: qh.clone(),
    };
    queue.roundtrip(&mut app).unwrap();

    // ---- 1. знімок кожного монітора ----
    let outputs: Vec<wl_output::WlOutput> = app.output_state.outputs().collect();
    let mut sessions = vec![];
    for (i, o) in outputs.iter().enumerate() {
        app.caps.push(Cap::default());
        let src: ExtImageCaptureSourceV1 = src_mgr.create_source(o, &qh, Noop);
        let sess = copy_mgr.create_session(&src, Options::empty(), &qh, SessData(i));
        sessions.push((src, sess));
    }
    while !app.caps.iter().all(|c| c.done || c.failed) {
        queue.blocking_dispatch(&mut app).unwrap();
    }
    let mut cap_pool = SlotPool::new(4096, &app.shm).unwrap();
    let mut bufs = vec![];
    let mut frames = vec![];
    for (i, (_, sess)) in sessions.iter().enumerate() {
        let c = &mut app.caps[i];
        let pref = [wl_shm::Format::Abgr8888, wl_shm::Format::Xbgr8888, wl_shm::Format::Xrgb8888, wl_shm::Format::Argb8888];
        let f = pref.iter().find(|f| c.formats.contains(f)).copied();
        if c.failed || f.is_none() {
            c.failed = true;
            bufs.push(None);
            continue;
        }
        c.format = f;
        let (buf, _) = cap_pool.create_buffer(c.w as i32, c.h as i32, c.w as i32 * 4, f.unwrap()).unwrap();
        let frame = sess.create_frame(&qh, FrameData(i));
        frame.attach_buffer(buf.wl_buffer());
        frame.damage_buffer(0, 0, c.w as i32, c.h as i32);
        frame.capture();
        frames.push(frame);
        bufs.push(Some(buf));
    }
    while !app.caps.iter().all(|c| c.ready || c.failed) {
        queue.blocking_dispatch(&mut app).unwrap();
    }
    if timing {
        eprintln!("capture: {:?}", t0.elapsed());
    }

    // ---- 2. оверлей на кожен монітор ----
    let mut max_k: f32 = 1.0;
    for (i, o) in outputs.iter().enumerate() {
        let c = &app.caps[i];
        let Some(buf) = bufs[i].as_mut() else { continue };
        if !c.ready {
            continue;
        }
        let pm = to_pixmap(buf.canvas(&mut cap_pool).expect("canvas"), c.w, c.h, c.format.unwrap());
        let logical_w = app
            .output_state
            .info(o)
            .and_then(|inf| inf.logical_size.map(|s| s.0))
            .filter(|w| *w > 0)
            .unwrap_or(c.w as i32);
        let k = c.w as f32 / logical_w as f32;
        max_k = max_k.max(k);
        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("jamshot"), Some(o));
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        layer.set_size(0, 0);
        layer.commit();
        let viewport = viewporter.get_viewport(layer.wl_surface(), &qh, Noop);
        let frame = resvg::tiny_skia::Pixmap::new(c.w, c.h).unwrap();
        app.outs.push(Out { screen: Screen::new(pm, k), layer, viewport, configured: false, frame, frame_pending: false, dirty: false });
    }
    for f in frames {
        f.destroy();
    }
    drop(bufs);
    drop(cap_pool);
    for (src, sess) in sessions {
        sess.destroy();
        src.destroy();
    }
    if app.outs.is_empty() {
        eprintln!("jamshot: не вдалося зняти жоден монітор");
        std::process::exit(1);
    }
    app.icons = Some(Icons::new(&app.theme, (18.0 * max_k).round() as u32));
    let total: usize = app.outs.iter().map(|o| o.frame.data().len()).sum();
    app.pool = Some(SlotPool::new(total * 2, &app.shm).unwrap());
    if timing {
        eprintln!("overlay ready: {:?}", t0.elapsed());
    }

    // ---- 3. цикл подій ----
    let mut event_loop: EventLoop<App> = EventLoop::try_new().unwrap();
    app.loop_handle = Some(event_loop.handle());
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).unwrap();
    while !app.exit {
        event_loop.dispatch(Duration::from_millis(500), &mut app).unwrap();
    }

    // ---- 4. прибрати оверлей і віддати результат ----
    let o = app.scene.sel.map(|_| app.scene.sel_out).unwrap_or(app.scene.cursor_out).min(app.outs.len() - 1);
    let img = match app.action {
        Action::Copy | Action::Save => Some(app.scene.export(&app.outs[o].screen, &app.fonts)),
        _ => None,
    };
    let action = app.action;
    for out in app.outs.drain(..) {
        out.viewport.destroy();
        drop(out.layer);
    }
    let _ = conn.flush();

    if let Some(img) = img {
        let png = gfx::encode_png(&img);
        match action {
            Action::Copy => copy_png(&png),
            Action::Save => save_png(&png),
            _ => {}
        }
    }
}

fn copy_png(png: &[u8]) {
    match std::process::Command::new("wl-copy").args(["--type", "image/png"]).stdin(std::process::Stdio::piped()).spawn() {
        Ok(mut ch) => {
            if let Some(mut si) = ch.stdin.take() {
                let _ = si.write_all(png);
            }
            let _ = ch.wait();
        }
        Err(e) => eprintln!("jamshot: wl-copy недоступний: {e}"),
    }
}

fn save_png(png: &[u8]) {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = std::path::PathBuf::from(format!("{home}/Pictures/Screenshots"));
    let _ = std::fs::create_dir_all(&dir);
    let name = format!("JamShot_{}.png", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    let path = dir.join(&name);
    if let Err(e) = std::fs::write(&path, png) {
        eprintln!("jamshot: не вдалося зберегти: {e}");
        return;
    }
    let _ = std::process::Command::new("notify-send")
        .args(["-a", "JamShot", "-i", &path.to_string_lossy(), if scene::ukrainian() { "Скріншот збережено" } else { "Screenshot saved" }, &name])
        .spawn();
}

impl App {
    fn out_of(&self, s: &wl_surface::WlSurface) -> Option<usize> {
        self.outs.iter().position(|o| o.layer.wl_surface() == s)
    }

    fn redraw_all(&mut self) {
        for i in 0..self.outs.len() {
            self.draw(i);
        }
    }

    fn draw(&mut self, i: usize) {
        let App { outs, scene, theme, icons, fonts, pool, qh, .. } = self;
        let o = &mut outs[i];
        if !o.configured {
            return;
        }
        if o.frame_pending {
            o.dirty = true;
            return;
        }
        scene.render(i, &o.screen, theme, icons.as_ref().unwrap(), fonts, &mut o.frame);
        let (w, h) = (o.frame.width() as i32, o.frame.height() as i32);
        let pool = pool.as_mut().unwrap();
        let (buffer, canvas) = pool.create_buffer(w, h, w * 4, wl_shm::Format::Abgr8888).expect("buffer");
        canvas.copy_from_slice(o.frame.data());
        let surf = o.layer.wl_surface();
        surf.damage_buffer(0, 0, w, h);
        surf.frame(qh, FrameCallbackData(surf.clone()));
        buffer.attach_to(surf).expect("attach");
        o.layer.commit();
        o.frame_pending = true;
        o.dirty = false;
    }

    fn apply(&mut self, a: Action) {
        match a {
            Action::None => {}
            Action::Redraw => self.redraw_all(),
            Action::Copy | Action::Save | Action::Exit => {
                let k = self.k(self.scene.sel_out);
                self.scene.commit_text(k);
                self.action = a;
                self.exit = true;
            }
        }
    }

    fn k(&self, i: usize) -> f32 {
        self.outs.get(i).map_or(1.0, |o| o.screen.k)
    }

    fn update_cursor(&mut self, i: usize, p: scene::P) {
        let o = &self.outs[i];
        let kind = self.scene.cursor_kind(i, p, o.screen.w(), o.screen.h(), o.screen.k);
        let shape = match kind {
            Cursor::Crosshair => CShape::Crosshair,
            Cursor::Default => CShape::Default,
            Cursor::Move => CShape::Move,
            Cursor::Text => CShape::Text,
            Cursor::Resize(h) => [
                CShape::NwResize,
                CShape::NResize,
                CShape::NeResize,
                CShape::EResize,
                CShape::SeResize,
                CShape::SResize,
                CShape::SwResize,
                CShape::WResize,
            ][h],
        };
        if self.cur_shape != Some(shape) {
            if let Some(d) = &self.cursor_dev {
                d.set_shape(self.enter_serial, shape);
            }
            self.cur_shape = Some(shape);
        }
    }

    fn schedule_tooltip(&mut self) {
        self.hover_gen += 1;
        let generation = self.hover_gen;
        if self.scene.hover.is_none() {
            return;
        }
        if let Some(lh) = &self.loop_handle {
            let _ = lh.insert_source(Timer::from_duration(Duration::from_millis(380)), move |_, _, app: &mut App| {
                if app.hover_gen == generation && app.scene.hover.is_some() && !app.exit {
                    app.scene.tooltip = true;
                    app.redraw_all();
                }
                TimeoutAction::Drop
            });
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, s: &wl_surface::WlSurface, _: u32) {
        if let Some(i) = self.out_of(s) {
            self.outs[i].frame_pending = false;
            if self.outs[i].dirty {
                self.draw(i);
            }
        }
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
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
        self.exit = true;
    }
    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface, cfg: LayerSurfaceConfigure, _: u32) {
        let Some(i) = self.out_of(layer.wl_surface()) else { return };
        let o = &mut self.outs[i];
        let (lw, lh) = cfg.new_size;
        if lw > 0 && lh > 0 {
            o.viewport.set_destination(lw as i32, lh as i32);
            o.screen.k = o.frame.width() as f32 / lw as f32;
        }
        o.configured = true;
        o.frame_pending = false;
        self.draw(i);
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, cap: Capability) {
        if cap == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
        if cap == Capability::Pointer && self.pointer.is_none() {
            if let Ok(p) = self.seat_state.get_pointer(qh, &seat) {
                self.cursor_dev = self.cursor_mgr.as_ref().map(|m| m.get_shape_device(&p, qh));
                self.pointer = Some(p);
            }
        }
    }
    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, _: Capability) {}
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl KeyboardHandler for App {
    fn enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32, _: &[u32], _: &[Keysym]) {}
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: &wl_surface::WlSurface, _: u32) {}

    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, ev: KeyEvent) {
        self.on_key(ev);
    }
    fn repeat_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, ev: KeyEvent) {
        if self.scene.text_edit.is_some() {
            self.on_key(ev);
        }
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, m: Modifiers, _: RawModifiers, _: u32) {
        self.mods = m;
    }
}

impl App {
    fn on_key(&mut self, ev: KeyEvent) {
        let ctrl = self.mods.ctrl;
        let k = self.k(self.scene.sel_out);
        let sym = ev.keysym;
        // набір тексту
        if self.scene.text_edit.is_some() && !ctrl {
            if sym == Keysym::Escape {
                self.scene.text_edit = None;
            } else if sym == Keysym::Return || sym == Keysym::KP_Enter {
                if self.mods.shift {
                    self.scene.text_edit.as_mut().unwrap().1.push('\n');
                } else {
                    self.scene.commit_text(k);
                }
            } else if sym == Keysym::BackSpace {
                self.scene.text_edit.as_mut().unwrap().1.pop();
            } else if let Some(s) = ev.utf8.as_deref() {
                self.scene.text_edit.as_mut().unwrap().1.extend(s.chars().filter(|c| !c.is_control()));
            }
            return self.redraw_all();
        }
        if ctrl {
            self.scene.commit_text(k);
        }
        let a = if sym == Keysym::Escape {
            if self.scene.popover {
                self.scene.popover = false;
                Action::Redraw
            } else {
                Action::Exit
            }
        } else if sym == Keysym::Return || sym == Keysym::KP_Enter {
            Action::Copy
        } else if ctrl {
            match ev.raw_code {
                KEY_C => Action::Copy,
                KEY_S => Action::Save,
                KEY_Z => {
                    self.scene.undo();
                    Action::Redraw
                }
                _ => Action::None,
            }
        } else if self.scene.sel.is_some() {
            let t = match ev.raw_code {
                KEY_V => Some(Tool::Move),
                KEY_P => Some(Tool::Pen),
                KEY_L => Some(Tool::Line),
                KEY_A => Some(Tool::Arrow),
                KEY_R => Some(Tool::Rect),
                KEY_M => Some(Tool::Marker),
                KEY_T => Some(Tool::Text),
                KEY_B => Some(Tool::Pixel),
                _ => None,
            };
            if let Some(t) = t {
                self.scene.set_tool(t, k);
                Action::Redraw
            } else if ev.raw_code == KEY_C {
                self.scene.popover = !self.scene.popover;
                Action::Redraw
            } else {
                Action::None
            }
        } else {
            Action::None
        };
        self.apply(a);
    }
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            let Some(i) = self.out_of(&ev.surface) else { continue };
            let o = &self.outs[i];
            let k = o.screen.k;
            let (sw, sh) = (o.screen.w(), o.screen.h());
            let p = (ev.position.0 as f32 * k, ev.position.1 as f32 * k);
            let prev_hover = self.scene.hover;
            let a = match ev.kind {
                PointerEventKind::Enter { serial } => {
                    self.enter_serial = serial;
                    self.cur_shape = None;
                    self.scene.motion(i, p, sw, sh, k)
                }
                PointerEventKind::Motion { .. } => self.scene.motion(i, p, sw, sh, k),
                PointerEventKind::Press { button: 0x110, .. } => self.scene.press(i, p, sw, sh, k),
                PointerEventKind::Press { button: 0x111, .. } => {
                    if self.scene.text_edit.take().is_some() || self.scene.popover {
                        self.scene.popover = false;
                        Action::Redraw
                    } else {
                        Action::Exit
                    }
                }
                PointerEventKind::Release { button: 0x110, .. } => self.scene.release(i, p, k),
                PointerEventKind::Axis { vertical, .. } => {
                    let d = if vertical.discrete != 0 { vertical.discrete as f64 } else { vertical.absolute };
                    if d.abs() > 0.0 { self.scene.scroll(d) } else { Action::None }
                }
                _ => Action::None,
            };
            if self.scene.hover != prev_hover {
                self.schedule_tooltip();
            }
            self.update_cursor(i, p);
            self.apply(a);
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);

// ---------- офлайн-перевірка рендеру ----------

fn test_render(input: &str, out_dir: &str) {
    use scene::{Shape, R};
    let theme = theme::Theme::load();
    let fonts = gfx::Fonts::load();
    let icons = Icons::new(&theme, 18);
    let scr = Screen::new(gfx::read_png(input), 1.0);
    let mut s = Scene::new();
    s.sel = Some(R { x0: 560.0, y0: 300.0, x1: 1320.0, y1: 740.0 });
    s.tool = Tool::Arrow;
    s.shapes = vec![
        Shape::Marker { pts: vec![(666.0, 390.0), (902.0, 390.0)], c: scene::PALETTE[2], w: 20.0 },
        Shape::Text { pos: (670.0, 372.0), text: "dunes from orbit".into(), c: scene::PALETTE[6], size: 26.0 },
        Shape::Arrow { a: (880.0, 430.0), b: (1135.0, 520.0), c: scene::PALETTE[0], w: 4.0 },
        Shape::Rect { a: (1150.0, 480.0), b: (1300.0, 610.0), c: scene::PALETTE[0], w: 4.0 },
        Shape::Pixel { a: (1010.0, 655.0), b: (1170.0, 715.0), block: 12.0 },
        Shape::Pen {
            pts: (0..50).map(|i| (640.0 + i as f32 * 4.0, 660.0 + (i as f32 * 0.4).sin() * 14.0)).collect(),
            c: scene::PALETTE[3],
            w: 4.0,
        },
    ];

    let mut frame = resvg::tiny_skia::Pixmap::new(scr.orig.width(), scr.orig.height()).unwrap();
    let t = std::time::Instant::now();
    s.render(0, &scr, &theme, &icons, &fonts, &mut frame);
    eprintln!("render: {:?}", t.elapsed());
    std::fs::write(format!("{out_dir}/frame.png"), gfx::encode_png(&frame)).unwrap();
    s.popover = true;
    s.hover = Some(4);
    s.tooltip = true;
    s.render(0, &scr, &theme, &icons, &fonts, &mut frame);
    std::fs::write(format!("{out_dir}/frame_popover.png"), gfx::encode_png(&frame)).unwrap();
    std::fs::write(format!("{out_dir}/export.png"), gfx::encode_png(&s.export(&scr, &fonts))).unwrap();
    println!("ok");
}
