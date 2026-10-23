use crate::controls::{Input, InputRouter};
use ruffle_core::backend::navigator::NullExecutor;
use ruffle_core::events::PlayerEvent;
use ruffle_core::{Player, PlayerBuilder, StageScaleMode};
use ruffle_render::quality::StageQuality;
use ruffle_render_glow::GlowRenderBackend;
use std::ffi::{CString, c_char, c_void};
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

type Error = Box<dyn std::error::Error>;

unsafe extern "C" {
    fn lonewolf_vita_log_init() -> i32;
    fn lonewolf_vita_log(data: *const c_char, length: u32) -> i32;
    fn lonewolf_vita_log_flush() -> i32;
    fn lonewolf_vita_log_memory(stage: *const c_char);
    fn lonewolf_vita_exit(status: i32) -> !;
    fn lonewolf_vita_init() -> i32;
    fn lonewolf_gl_proc(name: *const c_char) -> *mut c_void;
    fn lonewolf_vita_poll(input: *mut Input) -> i32;
    fn lonewolf_vita_present(x: f32, y: f32, show_cursor: i32);
    fn lonewolf_vita_finish();
    fn lonewolf_vita_loading(completed: u32);
    fn lonewolf_vita_loading_init(
        background: *const u8,
        bw: u32,
        bh: u32,
        logo: *const u8,
        lw: u32,
        lh: u32,
    ) -> i32;
    fn lonewolf_vita_loading_finish();
}

pub fn initialize_log() -> Result<(), Error> {
    let result = unsafe { lonewolf_vita_log_init() };
    if result < 0 {
        return Err(format!("Cannot open runtime.log: {result:#x}").into());
    }
    log("[boot] LONEWOLF Vita 00.02\n");
    Ok(())
}

pub fn log(message: &str) {
    let mut writer = NativeLog;
    let _ = std::io::Write::write_all(&mut writer, message.as_bytes());
}

pub fn log_memory(stage: &str) {
    let stage = CString::new(stage).expect("Memory stage has no NUL");
    unsafe { lonewolf_vita_log_memory(stage.as_ptr()) };
}

pub fn exit(status: i32) -> ! {
    unsafe { lonewolf_vita_exit(status) }
}

pub struct NativeLog;

impl std::io::Write for NativeLog {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let length = data.len().min(u32::MAX as usize);
        if unsafe { lonewolf_vita_log(data.as_ptr().cast(), length as u32) } < 0 {
            return Err(std::io::Error::other("Vita log write failed"));
        }
        Ok(length)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if unsafe { lonewolf_vita_log_flush() } < 0 {
            return Err(std::io::Error::other("Vita log sync failed"));
        }
        Ok(())
    }
}

pub fn loading(completed: u32) {
    unsafe { lonewolf_vita_loading(completed) };
}

pub struct Platform;

impl Platform {
    pub fn initialize() -> Result<Self, Error> {
        let code = unsafe { lonewolf_vita_init() };
        if code != 0 {
            return Err(format!("Vita platform initialization failed: {code}").into());
        }
        let platform = Self;
        let load = |path: &str, size: (u32, u32)| -> Result<_, Error> {
            let bitmap =
                ruffle_render::utils::decode_define_bits_jpeg(&std::fs::read(path)?, None)?
                    .to_rgba();
            if (bitmap.width(), bitmap.height()) != size
                || bitmap.data().len() != size.0 as usize * size.1 as usize * 4
            {
                return Err(format!("Invalid startup image: {path}").into());
            }
            Ok(bitmap)
        };
        let background = load("app0:sce_sys/livearea/contents/bg0.png", (840, 500))?;
        let logo = load("app0:sce_sys/livearea/contents/startup.png", (280, 158))?;
        let code = unsafe {
            lonewolf_vita_loading_init(
                background.data().as_ptr(),
                background.width(),
                background.height(),
                logo.data().as_ptr(),
                logo.width(),
                logo.height(),
            )
        };
        if code != 0 {
            return Err(format!("Startup graphics initialization failed: {code}").into());
        }
        loading(1);
        Ok(platform)
    }
}

impl Drop for Platform {
    fn drop(&mut self) {
        unsafe {
            lonewolf_vita_finish();
        }
    }
}

pub fn attach(builder: PlayerBuilder) -> Result<PlayerBuilder, Error> {
    let gl = unsafe {
        glow::Context::from_loader_function(|name| {
            let name = CString::new(name).expect("OpenGL names do not contain NUL");
            lonewolf_gl_proc(name.as_ptr()).cast_const()
        })
    };
    let renderer = GlowRenderBackend::new(Arc::new(gl), false, StageQuality::Medium)?;
    Ok(builder
        .with_renderer(renderer)
        .with_audio(crate::audio::VitaAudio::new()?)
        .with_viewport_dimensions(960, 544, 1.0)
        .with_scale_mode(StageScaleMode::ShowAll, true))
}

#[derive(Default)]
struct PhaseTiming {
    total: Duration,
    maximum: Duration,
    samples: u32,
}
impl PhaseTiming {
    fn record(&mut self, duration: Duration) {
        self.total += duration;
        self.maximum = self.maximum.max(duration);
        self.samples += 1;
    }
    fn mean_ms(&self) -> f64 {
        self.total.as_secs_f64() * 1000.0 / self.samples.max(1) as f64
    }
    fn max_ms(&self) -> f64 {
        self.maximum.as_secs_f64() * 1000.0
    }
}

pub fn run(
    shared: &Arc<Mutex<Player>>,
    executor: &mut NullExecutor,
    errors: &AtomicUsize,
) -> Result<(), Error> {
    let mut previous = Instant::now();
    let (mut x, mut y) = (480.0_f64, 272.0_f64);
    let mut input_router = InputRouter::default();
    let mut cursor_visible = true;
    let mut first_frame = true;
    let mut measured_at = Instant::now();
    let mut frames = 0_u32;
    let mut phases: [PhaseTiming; 4] = std::array::from_fn(|_| PhaseTiming::default());
    let mut player = shared.lock().map_err(|_| "Player mutex poisoned")?;
    player.handle_event(PlayerEvent::FocusGained);
    player.set_mouse_in_stage(true);
    player.handle_event(PlayerEvent::MouseMove { x, y });
    drop(player);
    loop {
        executor.run();
        let mut player = shared.lock().map_err(|_| "Player mutex poisoned")?;
        if errors.load(Ordering::Relaxed) != 0 {
            return Err(
                "AVM2 stopped after a runtime error; inspect ux0:data/lonewolf/runtime.log".into(),
            );
        }
        let now = Instant::now();
        let dt = now.duration_since(previous).as_secs_f64();
        previous = now;
        let previous_pointer = (x, y, cursor_visible);
        let mut input = Input::default();
        if unsafe { lonewolf_vita_poll(&mut input) } != 0 {
            return Err("Vita input polling failed".into());
        }
        if input.touch_count != 0 {
            let touch = input.touches[..(input.touch_count as usize).min(8)]
                .iter()
                .find(|touch| Some(touch.id as i32) == input_router.primary())
                .unwrap_or(&input.touches[0]);
            x = touch.x as f64;
            y = touch.y as f64;
            cursor_visible = false;
        } else {
            for (axis, coordinate) in [(input.analog_x, &mut x), (input.analog_y, &mut y)] {
                if axis.abs() > 0.2 {
                    *coordinate += axis as f64 * 500.0 * dt.min(0.1);
                    cursor_visible = true;
                }
            }
        }
        x = x.clamp(0.0, 959.0);
        y = y.clamp(0.0, 543.0);
        if input.touch_count == 0
            && !input_router.has_contacts()
            && (x, y) != (previous_pointer.0, previous_pointer.1)
        {
            player.handle_event(PlayerEvent::MouseMove { x, y });
        }
        let started = Instant::now();
        if input_router.update(&mut player, &input, (x, y), dt) {
            cursor_visible = false;
        }
        phases[0].record(started.elapsed());
        let started = Instant::now();
        player.tick(dt * 1000.0);
        phases[1].record(started.elapsed());
        if player.needs_render()
            || cursor_visible != previous_pointer.2
            || cursor_visible && (x, y) != (previous_pointer.0, previous_pointer.1)
        {
            let started = Instant::now();
            player.render();
            phases[2].record(started.elapsed());
            let started = Instant::now();
            let content = <dyn std::any::Any>::downcast_ref::<GlowRenderBackend>(player.renderer())
                .ok_or("Unexpected Vita renderer")?
                .last_frame_had_content();
            if first_frame && !content {
                loading(5);
                continue;
            }
            unsafe {
                lonewolf_vita_present(x as f32, y as f32, cursor_visible as i32);
            }
            if first_frame {
                unsafe {
                    lonewolf_vita_loading_finish();
                }
            }
            phases[3].record(started.elapsed());
            frames += 1;
            if first_frame {
                log("[boot] first frame presented\n");
                log_memory("first frame");
                first_frame = false;
            }
        } else {
            std::thread::sleep(player.time_til_next_frame().min(Duration::from_millis(8)));
        }
        let elapsed = measured_at.elapsed();
        if elapsed >= Duration::from_secs(5) {
            let gc_metrics =
                player.mutate_with_update_context(|context| context.gc().metrics().clone());
            let (gc_bytes, gc_objects) = (
                gc_metrics.total_gc_allocation(),
                gc_metrics.total_gc_count(),
            );
            log(&format!(
                "[perf] fps={:.1} movie_fps={:.1} input_ms={:.2} tick_ms={:.2} render_ms={:.2} present_ms={:.2} draws_per_frame={:.1} gc_bytes={} gc_objects={} external_bytes={}\n",
                frames as f64 / elapsed.as_secs_f64(),
                player.frame_rate(),
                phases[0].mean_ms(),
                phases[1].mean_ms(),
                phases[2].mean_ms(),
                phases[3].mean_ms(),
                ruffle_render_glow::take_draw_calls() as f64 / frames.max(1) as f64,
                gc_bytes,
                gc_objects,
                gc_metrics.total_external_allocation(),
            ));
            let work =
                <dyn std::any::Any>::downcast_mut::<GlowRenderBackend>(player.renderer_mut())
                    .ok_or("Unexpected Vita renderer")?
                    .take_work_stats();
            log(&format!(
                "[hitches] loops={} input_max_ms={:.2} tick_max_ms={:.2} render_max_ms={:.2} present_max_ms={:.2} shapes={} shape_ms={:.2} shape_max_ms={:.2} slowest_shape={} textures={} texture_ms={:.2} texture_max_ms={:.2} submit_ms={:.2} submit_max_ms={:.2}\n",
                phases[0].samples,
                phases[0].max_ms(),
                phases[1].max_ms(),
                phases[2].max_ms(),
                phases[3].max_ms(),
                work.shapes,
                work.shape_us as f64 / 1000.0,
                work.max_shape_us as f64 / 1000.0,
                work.slowest_shape,
                work.textures,
                work.texture_us as f64 / 1000.0,
                work.max_texture_us as f64 / 1000.0,
                work.submit_us as f64 / 1000.0,
                work.max_submit_us as f64 / 1000.0,
            ));
            log_memory("playback");
            measured_at = Instant::now();
            frames = 0;
            phases = std::array::from_fn(|_| PhaseTiming::default());
        }
    }
}
