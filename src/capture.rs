use ruffle_core::backend::navigator::NullExecutor;
use ruffle_core::events::{MouseButton, PlayerEvent};
use ruffle_core::{Player, PlayerBuilder};
use ruffle_render_wgpu::{backend::WgpuRenderBackend, target::TextureTarget, wgpu};
use std::any::Any;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SIZE: (u32, u32) = (1135, 640);
type Error = Box<dyn std::error::Error>;

pub struct Options {
    output: PathBuf,
    seconds: f64,
    clicks: Vec<(f64, f64, f64)>,
}

impl Options {
    pub fn parse(
        args: impl Iterator<Item = OsString>,
        input: &Path,
    ) -> Result<Option<Self>, Error> {
        let mut args = args.peekable();
        if args.peek().is_none() {
            return Ok(None);
        }
        let mut output = None;
        let mut seconds: f64 = 6.0;
        let mut clicks = Vec::new();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or("Missing capture option value")?;
            match flag.to_str() {
                Some("--capture") if output.is_none() => output = Some(PathBuf::from(value)),
                Some("--seconds") => seconds = value.to_str().ok_or("Invalid seconds")?.parse()?,
                Some("--click") => {
                    let coordinates = value
                        .to_str()
                        .ok_or("Invalid click")?
                        .split(':')
                        .map(str::parse::<f64>)
                        .collect::<Result<Vec<_>, _>>()?;
                    if coordinates.len() != 3 {
                        return Err(
                            "Click format: second:x:y (coordinates in 1135 x 640 viewport)".into(),
                        );
                    }
                    clicks.push((coordinates[0], coordinates[1], coordinates[2]));
                }
                _ => {
                    return Err(
                        "Expected --capture PNG, --seconds 1..30, or --click second:x:y".into(),
                    );
                }
            }
        }
        if !seconds.is_finite() || !(1.0..=30.0).contains(&seconds) {
            return Err("Capture duration must be between 1 and 30 seconds".into());
        }
        let output = output.ok_or("Capture options require --capture <PNG>")?;
        if output.extension().and_then(|s| s.to_str()) != Some("png") {
            return Err("Capture output must have a .png extension".into());
        }
        if output
            .symlink_metadata()
            .is_ok_and(|entry| entry.file_type().is_symlink())
        {
            return Err("Capture output must not be a symbolic link".into());
        }
        if output.canonicalize().ok() == Some(input.canonicalize()?) {
            return Err("Capture output would overwrite the input SWF".into());
        }
        for &(time, x, y) in &clicks {
            if !time.is_finite()
                || !(0.0..seconds).contains(&time)
                || !x.is_finite()
                || !(0.0..SIZE.0 as f64).contains(&x)
                || !y.is_finite()
                || !(0.0..SIZE.1 as f64).contains(&y)
            {
                return Err("Click time or viewport coordinates are out of bounds".into());
            }
        }
        clicks.sort_by(|a, b| a.0.total_cmp(&b.0));
        Ok(Some(Self {
            output,
            seconds,
            clicks,
        }))
    }
}

pub fn attach(builder: PlayerBuilder) -> Result<PlayerBuilder, Error> {
    let renderer = WgpuRenderBackend::<TextureTarget>::for_offscreen(
        SIZE,
        wgpu::Backends::PRIMARY,
        wgpu::PowerPreference::HighPerformance,
    )?;
    Ok(builder
        .with_renderer(renderer)
        .with_viewport_dimensions(SIZE.0, SIZE.1, 1.0))
}

pub fn run(
    shared: &Arc<Mutex<Player>>,
    executor: &mut NullExecutor,
    options: &Options,
) -> Result<(), Error> {
    shared
        .lock()
        .map_err(|_| "Player mutex poisoned")?
        .set_mouse_in_stage(true);
    let started = Instant::now();
    let mut previous = started;
    let mut clicks = options.clicks.iter().peekable();
    while started.elapsed().as_secs_f64() < options.seconds {
        executor.run();
        let mut player = shared.lock().map_err(|_| "Player mutex poisoned")?;
        let now = Instant::now();
        let elapsed = now.duration_since(started).as_secs_f64();
        while let Some(&&(time, x, y)) = clicks.peek() {
            if time > elapsed {
                break;
            }
            player.handle_event(PlayerEvent::MouseMove { x, y });
            player.handle_event(PlayerEvent::MouseDown {
                x,
                y,
                button: MouseButton::Left,
                index: None,
            });
            player.handle_event(PlayerEvent::MouseUp {
                x,
                y,
                button: MouseButton::Left,
            });
            println!("[input] click at {elapsed:.3}s x={x} y={y}");
            clicks.next();
        }
        player.tick(now.duration_since(previous).as_secs_f64() * 1000.0);
        previous = now;
        player.render();
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut player = shared.lock().map_err(|_| "Player mutex poisoned")?;
    let renderer =
        <dyn Any>::downcast_mut::<WgpuRenderBackend<TextureTarget>>(player.renderer_mut())
            .ok_or("Unexpected capture renderer")?;
    let image = renderer
        .capture_frame()
        .ok_or("GPU did not return a frame")?;
    if let Some(parent) = options
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    image.save(&options.output)?;
    println!(
        "[capture] {} ({} x {}, {:.1}s)",
        options.output.display(),
        SIZE.0,
        SIZE.1,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

#[cfg(test)]
mod bitmap_gpu_tests {
    use super::*;
    use ruffle_core::bitmap::bitmap_data::{BitmapData, Color, IBitmapDrawable, PixelBuffer};
    use ruffle_core::bitmap::operations;
    use ruffle_core::swf::BlendMode;

    #[test]
    fn bitmap_draw_readback_and_clone_preserve_lazy_and_shared_pixels() {
        let renderer = WgpuRenderBackend::<TextureTarget>::for_offscreen(
            (16, 16),
            wgpu::Backends::METAL,
            wgpu::PowerPreference::HighPerformance,
        )
        .expect("A real Metal device is required for this test");
        let player = PlayerBuilder::new()
            .with_renderer(renderer)
            .with_viewport_dimensions(16, 16, 1.0)
            .build();
        player.lock().unwrap().update(|context| {
            let source = BitmapData::new(context.gc(), 8, 8, false, 0xffff0000);
            let shared = Arc::new(PixelBuffer::new(
                vec![Color::rgba(255, 255, 255, 255); 256],
                Some(context.gc().metrics().clone()),
            ));
            for target in [
                BitmapData::new(context.gc(), 16, 16, false, 0xffffffff),
                BitmapData::new_with_shared_pixels(context.gc(), 16, 16, false, shared.clone()),
            ] {
                let quality = context.stage.quality();
                // Multiply bypasses the identity CPU blit and requires a GPU draw.
                operations::draw(
                    context,
                    target,
                    IBitmapDrawable::BitmapData(source),
                    Default::default(),
                    false,
                    BlendMode::Multiply,
                    None,
                    quality,
                )
                .unwrap();
                let copy = target.clone_data(context.gc(), context.renderer);
                assert_eq!(
                    target.sync(context.renderer).borrow().get_pixel32_raw(0, 0),
                    Color::rgba(255, 0, 0, 255)
                );
                assert_eq!(
                    target
                        .sync(context.renderer)
                        .borrow()
                        .get_pixel32_raw(15, 15),
                    Color::rgba(255, 255, 255, 255)
                );
                assert_eq!(shared[0], Color::rgba(255, 255, 255, 255));
                operations::set_pixel32(context.gc(), context.renderer, copy, 0, 0, 0xff0000ff);
                assert_eq!(
                    target.sync(context.renderer).borrow().get_pixel32_raw(0, 0),
                    Color::rgba(255, 0, 0, 255)
                );
                let destination = BitmapData::new(context.gc(), 16, 16, false, 0xffffffff);
                operations::draw(
                    context,
                    destination,
                    IBitmapDrawable::BitmapData(copy),
                    Default::default(),
                    false,
                    BlendMode::Multiply,
                    None,
                    quality,
                )
                .unwrap();
                assert_eq!(
                    destination
                        .sync(context.renderer)
                        .borrow()
                        .get_pixel32_raw(0, 0),
                    Color::rgba(0, 0, 255, 255)
                );
            }
            let transparent = BitmapData::new(context.gc(), 16, 16, true, 0);
            let quality = context.stage.quality();
            operations::draw(
                context,
                transparent,
                IBitmapDrawable::BitmapData(source),
                Default::default(),
                false,
                BlendMode::Add,
                None,
                quality,
            )
            .unwrap();
            let pixels = transparent.sync(context.renderer).borrow();
            assert_eq!(pixels.get_pixel32_raw(0, 0), Color::rgba(255, 0, 0, 255));
            assert_eq!(pixels.get_pixel32_raw(15, 15), Color::default());
        });
    }
}
