use ruffle_core::backend::log::LogBackend;
use ruffle_core::backend::navigator::{NullExecutor, NullNavigatorBackend};
#[cfg(not(target_os = "vita"))]
use ruffle_core::backend::storage::MemoryStorageBackend;
#[cfg(not(target_os = "vita"))]
use ruffle_core::focus_tracker::{DisplayObject, TDisplayObject, TDisplayObjectContainer};
use ruffle_core::tag_utils::SwfMovie;
use ruffle_core::{LoadBehavior, PlayerBuilder, PlayerRuntime};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tracing_subscriber::prelude::*;

#[cfg(all(feature = "capture", not(target_os = "vita")))]
mod capture;

#[cfg(target_os = "vita")]
mod vita;

#[cfg(target_os = "vita")]
mod audio;

#[cfg(test)]
mod audio_tests;

#[cfg(test)]
mod input_tests;

#[cfg(any(target_os = "vita", test))]
mod controls;

#[cfg(any(target_os = "vita", test))]
mod storage;

struct Errors(Arc<AtomicUsize>);

#[derive(Default)]
struct AbcLoadFailure(bool);

impl tracing::field::Visit for AbcLoadFailure {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 |= format!("{value:?}").starts_with("Error loading ABC file:");
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Errors {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut abc_failure = AbcLoadFailure::default();
        if *event.metadata().level() == tracing::Level::WARN
            && event.metadata().target() == "ruffle_core::display_object::movie_clip"
        {
            event.record(&mut abc_failure);
        }
        if *event.metadata().level() == tracing::Level::ERROR || abc_failure.0 {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(target_os = "vita")]
fn runtime_log_writer() -> vita::NativeLog {
    vita::NativeLog
}

#[cfg(not(target_os = "vita"))]
fn runtime_log_writer() -> std::io::Stderr {
    std::io::stderr()
}

fn runtime_log(message: &str) {
    #[cfg(target_os = "vita")]
    vita::log(&format!("{message}\n"));
    #[cfg(not(target_os = "vita"))]
    eprintln!("{message}");
}

struct BootstrapLog;

impl LogBackend for BootstrapLog {
    fn avm_trace(&self, message: &str) {
        runtime_log(&format!("[avm_trace] {message}"));
    }

    fn avm_warning(&self, message: &str) {
        runtime_log(&format!("[avm_warning] {message}"));
    }
}

#[cfg(not(target_os = "vita"))]
fn display_tree(node: DisplayObject<'_>, depth: usize) -> usize {
    let mut count = 0;
    if let Some(object) = node.object2() {
        println!(
            "[display_object] depth={depth} symbol={} {object:?}",
            node.id()
        );
        count += 1;
    }
    if depth < 6 {
        if let Some(container) = node.as_container() {
            for child in container.iter_render_list() {
                count += display_tree(child, depth + 1);
            }
        }
    }
    count
}

#[cfg(not(target_os = "vita"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    run()
}

#[cfg(target_os = "vita")]
fn main() {
    if vita::initialize_log().is_err() {
        vita::exit(1);
    }
    std::panic::set_hook(Box::new(|info| {
        vita::log(&format!("[fatal] {info}\n"));
        vita::exit(1);
    }));
    let result = std::thread::Builder::new()
        .name("Lonewolf AVM2".into())
        .stack_size(4 * 1024 * 1024)
        .spawn(|| run().map_err(|error| error.to_string()))
        .map_err(|error| error.to_string())
        .and_then(|thread| {
            thread
                .join()
                .map_err(|_| "Vita runtime thread panicked".into())
        })
        .and_then(|result| result);
    if let Err(error) = result {
        vita::log(&format!("[fatal] {error}\n"));
        vita::exit(1);
    }
    vita::exit(0);
}

fn build_player(
    builder: PlayerBuilder,
    movie: SwfMovie,
) -> Arc<std::sync::Mutex<ruffle_core::Player>> {
    // The packed SWF already occupies 49 MiB of Vita's 240 MiB heap.
    let player = builder
        .with_gc_pacing(ruffle_core::GcPacing {
            sleep_factor: 0.1,
            mark_factor: 0.05,
            trace_factor: 0.2,
            keep_factor: 0.025,
            drop_factor: 0.1,
            free_factor: 0.15,
            ..ruffle_core::GcPacing::DEFAULT
        })
        .build();
    player.lock().expect("New player mutex").update(|context| {
        let mut language = context.ui.language();
        language.language = "ru".parse().expect("Russian language code");
        context.system.language = language.into();
        context.set_root_movie(movie);
    });
    player
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "vita")]
    let _platform = vita::Platform::initialize()?;
    let errors = Arc::new(AtomicUsize::new(0));
    tracing_subscriber::registry()
        .with(Errors(errors.clone()))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(runtime_log_writer)
                .with_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "warn".into()),
                ),
        )
        .init();
    #[cfg(not(target_os = "vita"))]
    let mut args = std::env::args_os().skip(1);
    #[cfg(not(target_os = "vita"))]
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or("Usage: lonewolf-vita <verified local Lonewolf.swf>")?;
    #[cfg(target_os = "vita")]
    let path = PathBuf::from("app0:game.swf");
    #[cfg(all(feature = "capture", not(target_os = "vita")))]
    let capture = capture::Options::parse(args, &path)?;
    #[cfg(all(not(feature = "capture"), not(target_os = "vita")))]
    if args.next().is_some() {
        return Err("Extra arguments require the capture feature: cargo run --features capture -- <SWF> --capture <PNG> [--seconds 6] [--click second:x:y]".into());
    }
    // Host checkout paths must not change the game's SharedObject namespace.
    let bytes = std::fs::read(&path)?;
    #[cfg(target_os = "vita")]
    vita::loading(2);
    let movie = SwfMovie::from_data(&bytes, "file:///Lonewolf.swf".into(), None)?;
    drop(bytes);
    #[cfg(target_os = "vita")]
    vita::loading(3);
    runtime_log("[boot] SWF loaded; creating AIR player");
    #[cfg(target_os = "vita")]
    vita::log_memory("SWF loaded");
    let mut executor = NullExecutor::new();
    #[cfg(target_os = "vita")]
    let assets = PathBuf::from("app0:");
    #[cfg(not(target_os = "vita"))]
    let assets = path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let builder = PlayerBuilder::new()
        .with_navigator(NullNavigatorBackend::with_package_root(assets, &executor))
        .with_player_runtime(PlayerRuntime::AIR)
        .with_load_behavior(LoadBehavior::Blocking)
        .with_log(BootstrapLog)
        .with_max_execution_duration(Duration::from_secs(10))
        .with_autoplay(true);
    #[cfg(not(target_os = "vita"))]
    let builder = builder.with_storage(Box::new(MemoryStorageBackend::new()));
    #[cfg(target_os = "vita")]
    let builder = builder.with_storage(Box::new(storage::FileStorage::new(
        "ux0:data/lonewolf/saves",
    )?));
    #[cfg(target_os = "vita")]
    let builder = vita::attach(builder)?;
    #[cfg(target_os = "vita")]
    vita::loading(4);
    #[cfg(all(feature = "capture", not(target_os = "vita")))]
    let builder = if capture.is_some() {
        capture::attach(builder)?
    } else {
        builder
    };
    #[cfg(target_os = "vita")]
    vita::log_memory("renderer ready");
    let player = build_player(builder, movie);
    #[cfg(target_os = "vita")]
    vita::log_memory("player ready");
    #[cfg(target_os = "vita")]
    vita::loading(5);
    #[cfg(target_os = "vita")]
    return vita::run(&player, &mut executor, &errors);
    #[cfg(not(target_os = "vita"))]
    {
        for _ in 0..3 {
            player
                .lock()
                .map_err(|_| "Player mutex poisoned")?
                .run_frame();
            executor.run();
        }
        #[cfg(feature = "capture")]
        if let Some(options) = &capture {
            capture::run(&player, &mut executor, options)?;
        }
        let count: usize = player
            .lock()
            .map_err(|_| "Player mutex poisoned")?
            .update(|context| {
                let mut count = 0;
                for root in context.stage.iter_render_list() {
                    count += display_tree(root, 0);
                }
                count
            });
        let errors = errors.load(Ordering::Relaxed);
        if errors != 0 || count <= 1 {
            return Err(format!(
                "Bootstrap incomplete: {errors} runtime errors, {count} display objects"
            )
            .into());
        }
        println!(
            "AVM2 startup: {count} display objects, no logged errors. This is not gameplay or Vita validation."
        );
        Ok(())
    }
}
