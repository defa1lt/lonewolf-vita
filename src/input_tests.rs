use ruffle_core::backend::navigator::{NullExecutor, NullNavigatorBackend};
use ruffle_core::events::PlayerEvent;
use ruffle_core::focus_tracker::{TDisplayObject, TDisplayObjectContainer};
use ruffle_core::swf::{self, Tag};
use ruffle_core::tag_utils::SwfMovie;
use ruffle_core::{Avm2Activation as Activation, Avm2Value as Value};
use ruffle_core::{LoadBehavior, Player, PlayerBuilder, PlayerRuntime};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tracing_subscriber::prelude::*;

fn player(bytes: &[u8], assets: &Path) -> (Arc<Mutex<Player>>, NullExecutor) {
    player_with_storage(bytes, assets, None)
}

fn player_with_storage(
    bytes: &[u8],
    assets: &Path,
    saves: Option<&Path>,
) -> (Arc<Mutex<Player>>, NullExecutor) {
    let executor = NullExecutor::new();
    let movie = SwfMovie::from_data(bytes, "file:///Lonewolf.swf".into(), None).unwrap();
    let mut builder = PlayerBuilder::new()
        .with_player_runtime(PlayerRuntime::AIR)
        .with_load_behavior(LoadBehavior::Blocking)
        .with_navigator(NullNavigatorBackend::with_package_root(
            assets.to_owned(),
            &executor,
        ))
        .with_viewport_dimensions(640, 480, 1.0)
        .with_autoplay(true);
    if let Some(saves) = saves {
        builder = builder.with_storage(Box::new(crate::storage::FileStorage::new(saves).unwrap()));
    }
    let player = super::build_player(builder, movie);
    player.lock().unwrap().set_mouse_in_stage(true);
    (player, executor)
}

fn property(player: &Arc<Mutex<Player>>, names: &[&str]) -> String {
    player.lock().unwrap().update(|context| {
        let root = context
            .stage
            .iter_render_list()
            .next()
            .unwrap()
            .object2()
            .unwrap();
        let mut value = Value::from(root);
        let mut activation = Activation::from_nothing(context);
        for name in names {
            assert!(
                !matches!(value, Value::Null | Value::Undefined),
                "Missing parent of {name} in {names:?}"
            );
            let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), *name);
            value = value.get_public_property(name, &mut activation).unwrap();
        }
        value
            .coerce_to_string(&mut activation)
            .unwrap()
            .to_utf8_lossy()
            .into_owned()
    })
}

fn trait_number(player: &Arc<Mutex<Player>>, parents: &[&str], key: &str) -> f64 {
    let (key, member) = key
        .split_once('.')
        .map_or((key, None), |(key, member)| (key, Some(member)));
    player.lock().unwrap().update(|context| {
        let mut value = Value::from(
            context
                .stage
                .iter_render_list()
                .next()
                .unwrap()
                .object2()
                .unwrap(),
        );
        let mut a = Activation::from_nothing(context);
        for parent in parents {
            let name = ruffle_core::string::AvmString::new_utf8(a.gc(), *parent);
            value = value.get_public_property(name, &mut a).unwrap();
        }
        let table = value.vtable(&mut a);
        let (name, namespace, _) = table
            .resolved_traits()
            .iter()
            .find(|(n, _, _)| n.to_utf8_lossy() == key)
            .unwrap();
        let mut field = value.instance_class(&mut a).super_class_name().unwrap();
        field.set_single_namespace(namespace);
        field.set_local_name(name);
        let mut value = value.get_property(&field, &mut a).unwrap();
        if let Some(member) = member {
            let name = ruffle_core::string::AvmString::new_utf8(a.gc(), member);
            value = value.get_public_property(name, &mut a).unwrap();
        }
        value.coerce_to_number(&mut a).unwrap()
    })
}

fn globals<'gc>(activation: &mut Activation<'_, 'gc>) -> Value<'gc> {
    let domain = activation.context.avm2.stage_domain();
    let name = domain
        .get_defined_names()
        .into_iter()
        .find(|name| name.local_name().to_utf8_lossy() == "globalVar")
        .unwrap();
    domain.get_defined_value(activation, name).unwrap()
}

fn global_property(player: &Arc<Mutex<Player>>, key: &str) -> String {
    player.lock().unwrap().update(|context| {
        let mut activation = Activation::from_nothing(context);
        let class = globals(&mut activation);
        let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), key);
        class
            .get_public_property(name, &mut activation)
            .unwrap()
            .coerce_to_string(&mut activation)
            .unwrap()
            .to_utf8_lossy()
            .into_owned()
    })
}

#[test]
fn game_initializer_enables_premium_for_new_and_legacy_profiles() {
    use ruffle_core::Avm2FunctionArgs;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(root.join("build/Lonewolf-air.swf")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (first, mut executor) =
        player_with_storage(&bytes, &root.join("build"), Some(directory.path()));
    first.lock().unwrap().run_frame();
    executor.run();
    assert_eq!(
        global_property(&first, "premium"),
        "true",
        "Fresh profile branch must reach the patch"
    );
    assert_eq!(global_property(&first, "playAds"), "false");
    first.lock().unwrap().update(|context| {
        let mut activation = Activation::from_nothing(context);
        let class = globals(&mut activation);
        for (key, value) in [
            ("premium", Value::Bool(false)),
            ("playAds", Value::Bool(true)),
            ("money", Value::Number(1234.0)),
        ] {
            let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), key);
            class
                .set_public_property(name, value, &mut activation)
                .unwrap();
        }
        let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), "saveShared");
        let instance = class
            .construct(&mut activation, Avm2FunctionArgs::empty())
            .unwrap();
        instance
            .call_public_property(name, Avm2FunctionArgs::empty(), &mut activation)
            .unwrap();
    });
    drop(first);
    drop(executor);
    for absent_flags in [false, true] {
        let (player, mut executor) =
            player_with_storage(&bytes, &root.join("build"), Some(directory.path()));
        player.lock().unwrap().run_frame();
        executor.run();
        assert_eq!(global_property(&player, "premium"), "true");
        assert_eq!(global_property(&player, "playAds"), "false");
        assert_eq!(
            global_property(&player, "money"),
            "1234",
            "Existing progress must survive initialization"
        );
        if !absent_flags {
            player.lock().unwrap().update(|context| {
                let mut activation = Activation::from_nothing(context);
                let class = globals(&mut activation);
                let shared_name =
                    ruffle_core::string::AvmString::new_utf8(activation.gc(), "shared");
                let shared = class
                    .get_public_property(shared_name, &mut activation)
                    .unwrap();
                let data_name = ruffle_core::string::AvmString::new_utf8(activation.gc(), "data");
                let data = shared
                    .get_public_property(data_name, &mut activation)
                    .unwrap();
                for key in ["premium", "playAds"] {
                    let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), key);
                    data.set_public_property(name, Value::Undefined, &mut activation)
                        .unwrap();
                }
                let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), "flush");
                shared
                    .call_public_property(name, Avm2FunctionArgs::empty(), &mut activation)
                    .unwrap();
            });
        }
    }
}

#[test]
fn game_localization_loads_before_menu_is_created() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(root.join("build/Lonewolf-air.swf")).unwrap();
    let (player, mut executor) = player(&bytes, &root.join("build"));
    for frame in 0..300 {
        player.lock().unwrap().run_frame();
        executor.run(); // URLLoader completion must acquire its own player guard.
        if frame == 10 {
            assert_eq!(
                property(&player, &["game", "myXML", "splash", "0"]),
                "ПРОДОЛЖИТЬ"
            );
            assert_eq!(
                property(&player, &["game", "myXML", "splash", "1"]),
                "НОВАЯ ИГРА"
            );
        }
    }
    assert_eq!(
        property(&player, &["game", "splash_screen", "txt_newgame", "text"]),
        "НОВАЯ ИГРА"
    );
}

#[test]
fn transformed_touch_targets_bubble_track_two_contacts_and_preserve_mouse_clicks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let tmp = tempfile::tempdir().unwrap();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let mut definitions = Vec::new();
    fn find_definitions(path: &Path, output: &mut Vec<std::path::PathBuf>) {
        if !path.is_dir() {
            return;
        }
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .is_some_and(|name| name == "playerglobal.swf")
            {
                output.push(path);
            } else if path.is_dir() {
                find_definitions(&path, output);
            }
        }
    }
    for profile in ["release", "debug"] {
        find_definitions(&target.join(profile).join("build"), &mut definitions);
    }
    let sdk = definitions
        .into_iter()
        .max_by_key(|path| path.metadata().unwrap().modified().unwrap())
        .expect("Build playerglobal first");
    let sdk = std::fs::read(sdk).unwrap();
    let sdk = swf::decompress_swf(&sdk[..]).unwrap();
    let sdk = swf::parse_swf(&sdk).unwrap();
    let abc = sdk
        .tags
        .iter()
        .find_map(|tag| {
            if let Tag::DoAbc2(abc) = tag {
                Some(abc.data)
            } else {
                None
            }
        })
        .unwrap();
    let imported = tmp.path().join("playerglobal.abc");
    std::fs::write(&imported, abc).unwrap();
    let result = Command::new("java")
        .args(["-classpath"])
        .arg(root.join("vendor/ruffle/core/build_playerglobal/asc.jar"))
        .args([
            "macromedia.asc.embedding.ScriptCompiler",
            "-AS3",
            "-strict",
            "-optimize",
            "-import",
        ])
        .arg(imported)
        .arg("-outdir")
        .arg(tmp.path())
        .args(["-out", "TouchProbe"])
        .arg(root.join("src/testdata/TouchProbe.as"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let abc = std::fs::read(tmp.path().join("TouchProbe.abc"))
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&result.stderr)));
    let mut header = swf::Header::default_with_swf_version(43);
    header.stage_size = swf::Rectangle {
        x_min: swf::Twips::ZERO,
        y_min: swf::Twips::ZERO,
        x_max: swf::Twips::from_pixels(320.0),
        y_max: swf::Twips::from_pixels(240.0),
    };
    header.num_frames = 1;
    let tags = [
        Tag::FileAttributes(swf::FileAttributes::IS_ACTION_SCRIPT_3),
        Tag::DoAbc2(swf::DoAbc2 {
            flags: swf::DoAbc2Flag::LAZY_INITIALIZE,
            name: swf::SwfStr::from_utf8_str("fixture"),
            data: &abc,
        }),
        Tag::SymbolClass(vec![swf::SymbolClassLink {
            id: 0,
            class_name: swf::SwfStr::from_utf8_str("TouchProbe"),
        }]),
        Tag::ShowFrame,
        Tag::End,
    ];
    let mut bytes = Vec::new();
    swf::write_swf(&header, &tags, &mut bytes).unwrap();
    let (player, _) = player(&bytes, tmp.path());
    player.lock().unwrap().run_frame();
    player.lock().unwrap().set_mouse_in_stage(true);
    let send = |event| {
        assert!(player.lock().unwrap().handle_event(event));
    };
    send(PlayerEvent::TouchBegin {
        id: 7,
        x: 160.0,
        y: 160.0,
        primary: true,
    });
    for (name, expected) in [
        ("stagePointX", "80"),
        ("stagePointY", "80"),
        ("localPointX", "15"),
        ("localPointY", "25"),
        ("targetName", "zoomIn"),
        ("bubbleTarget", "zoomIn"),
        ("id", "7"),
        ("primary", "true"),
    ] {
        assert_eq!(property(&player, &[name]), expected, "{name}");
    }
    send(PlayerEvent::TouchBegin {
        id: 8,
        x: 180.0,
        y: 180.0,
        primary: false,
    });
    assert_eq!(property(&player, &["peak"]), "2");
    send(PlayerEvent::TouchMove {
        id: 8,
        x: 182.0,
        y: 180.0,
    });
    send(PlayerEvent::TouchEnd {
        id: 8,
        x: 182.0,
        y: 180.0,
    });
    assert_eq!(property(&player, &["clicks"]), "0");
    send(PlayerEvent::TouchEnd {
        id: 7,
        x: 160.0,
        y: 160.0,
    });
    assert_eq!(property(&player, &["clicks"]), "1");
    assert_eq!(property(&player, &["taps"]), "2");
    assert_eq!(property(&player, &["active"]), "0");
    send(PlayerEvent::TouchBegin {
        id: 7,
        x: 160.0,
        y: 160.0,
        primary: true,
    });
    send(PlayerEvent::TouchMove {
        id: 7,
        x: 600.0,
        y: 400.0,
    });
    send(PlayerEvent::TouchEnd {
        id: 7,
        x: 600.0,
        y: 400.0,
    });
    assert_eq!(property(&player, &["taps"]), "2");
    assert_eq!(property(&player, &["outs"]), "3");
    send(PlayerEvent::TouchBegin {
        id: 9,
        x: 160.0,
        y: 160.0,
        primary: true,
    });
    send(PlayerEvent::FocusLost);
    assert_eq!(property(&player, &["active"]), "0");
    assert_eq!(property(&player, &["taps"]), "2");
    assert_eq!(
        property(&player, &["clicks"]),
        "1",
        "Focus loss must cancel the mouse click too"
    );
    assert!(
        !player.lock().unwrap().handle_event(PlayerEvent::TouchMove {
            id: 9,
            x: 160.0,
            y: 160.0
        })
    );
    // A controller contact captures its explicit object without a viewport hit test.
    player.lock().unwrap().update(|context| {
        let target = context
            .stage
            .iter_render_list()
            .find_map(|root| find_object(root, "zoomIn", false))
            .unwrap();
        assert!(context.handle_touch_on(
            target,
            PlayerEvent::TouchBegin {
                id: 1002,
                x: 900.0,
                y: 900.0,
                primary: false,
            }
        ));
    });
    assert_eq!(property(&player, &["targetName"]), "zoomIn");
    assert_eq!(property(&player, &["primary"]), "false");
    send(PlayerEvent::TouchMove {
        id: 1002,
        x: 500.0,
        y: 400.0,
    });
    assert_eq!(property(&player, &["targetName"]), "zoomIn");
    send(PlayerEvent::FocusLost);
    assert_eq!(property(&player, &["active"]), "0");
    assert_eq!(property(&player, &["clicks"]), "1");
    assert_eq!(property(&player, &["taps"]), "2");
}

fn find_object<'gc>(
    object: ruffle_core::focus_tracker::DisplayObject<'gc>,
    key: &str,
    by_class: bool,
) -> Option<ruffle_core::focus_tracker::DisplayObject<'gc>> {
    let matches = if by_class {
        object
            .object2()
            .is_some_and(|o| format!("{o:?}").contains(&format!("name: {key},")))
    } else {
        object
            .name()
            .is_some_and(|name| name.to_utf8_lossy() == key)
    };
    if matches {
        return Some(object);
    }
    if let Some(container) = object.as_container() {
        for child in container.iter_render_list() {
            if let Some(found) = find_object(child, key, by_class) {
                return Some(found);
            }
        }
    }
    None
}

fn object_point(player: &Arc<Mutex<Player>>, name: &str) -> (f64, f64) {
    player.lock().unwrap().update(|context| {
        let object = context
            .stage
            .iter_render_list()
            .find_map(|root| find_object(root, name, false))
            .expect(name);
        let view = context.stage.view_matrix();
        let origin = view * object.local_to_global(swf::Point::from_pixels(0.0, 0.0));
        let bounds = object.world_bounds();
        let min = view * swf::Point::new(bounds.x_min, bounds.y_min);
        let max = view * swf::Point::new(bounds.x_max, bounds.y_max);
        let mut candidates = vec![(origin.x.to_pixels(), origin.y.to_pixels())];
        for fy in [0.5, 0.25, 0.75] {
            for fx in [0.5, 0.25, 0.75] {
                candidates.push((
                    min.x.to_pixels() + (max.x.to_pixels() - min.x.to_pixels()) * fx,
                    min.y.to_pixels() + (max.y.to_pixels() - min.y.to_pixels()) * fy,
                ));
            }
        }
        candidates
            .into_iter()
            .find(|&(x, y)| {
                let mut picked = context.pick_interactive_at(x, y);
                while let Some(target) = picked {
                    if ruffle_core::focus_tracker::DisplayObject::ptr_eq(target, object) {
                        return true;
                    }
                    picked = target.parent();
                }
                false
            })
            .unwrap_or_else(|| panic!("No exposed hit area for {name}"))
    })
}

fn background_position(player: &Arc<Mutex<Player>>) -> (f64, f64) {
    player.lock().unwrap().update(|context| {
        let bg = context
            .stage
            .iter_render_list()
            .find_map(|root| find_object(root, "bg_lot", true))
            .expect("Actual tutorial background");
        (bg.x().to_pixels(), bg.y().to_pixels())
    })
}

#[test]
fn original_tutorial_accepts_stick_aim_front_zoom_and_fire() {
    tutorial_controls(false);
}

#[test]
fn physical_buttons_drive_original_tutorial_zoom_and_fire() {
    tutorial_controls(true);
}

fn tutorial_controls(physical_fire: bool) {
    let errors = Arc::new(AtomicUsize::new(0));
    let _logging = tracing::subscriber::set_default(
        tracing_subscriber::registry().with(super::Errors(errors.clone())),
    );
    use crate::controls::{GameplayControls, Input, InputRouter, Touch};
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(root.join("build/Lonewolf-air.swf")).unwrap();
    let (player, mut executor) = player(&bytes, &root.join("build"));
    let step = |executor: &mut NullExecutor| {
        // Advance deterministic movie/timer time; the null audio backend reports position zero.
        {
            let mut player = player.lock().unwrap();
            player.run_frame();
            player.update_timers(1000.0 / 48.0);
        }
        executor.run();
    };
    for _ in 0..300 {
        step(&mut executor);
    }
    let (x, y) = object_point(&player, "skip");
    assert!(
        player
            .lock()
            .unwrap()
            .handle_event(PlayerEvent::TouchBegin {
                id: 1,
                x,
                y,
                primary: true
            })
    );
    player
        .lock()
        .unwrap()
        .handle_event(PlayerEvent::TouchEnd { id: 1, x, y });
    let mut ready = false;
    for _ in 0..500 {
        step(&mut executor);
        if GameplayControls::read(&mut player.lock().unwrap(), &[], None).is_some() {
            ready = true;
            break;
        }
    }
    if !ready {
        player.lock().unwrap().update(|context| {
            for root in context.stage.iter_render_list() {
                super::display_tree(root, 0);
            }
        });
        for names in [
            &["game", "play_screen"][..],
            &["game", "play_screen2"][..],
            &["game", "play_screen3"][..],
        ] {
            println!("State {names:?}: {}", property(&player, names));
        }
    }
    assert!(
        ready,
        "Tutorial must finish its loading screen using original game code"
    );
    assert_eq!(
        property(&player, &["game", "play_screen", "action"]),
        "aiming"
    );
    let mut router = InputRouter::default();
    assert!(!router.has_contacts());
    assert_eq!(router.primary(), None);
    let mut input = Input {
        analog_x: 1.0,
        ..Input::default()
    };
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 10);
    let after = background_position(&player);
    assert!(
        after.0 < before.0 - 1.0,
        "Entering with a deflected stick must immediately move aim: {before:?} -> {after:?}"
    );
    input.analog_x = 0.0;
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    // Use actual stick contacts to aim into the tutorial's original target region.
    let mut aligned = false;
    for _ in 0..600 {
        let (bx, by) = background_position(&player);
        if (-120.0..-90.0).contains(&bx) && (5.0..45.0).contains(&by) {
            aligned = true;
            break;
        }
        input.analog_x = if bx > -90.0 {
            0.5
        } else if bx < -120.0 {
            -0.5
        } else {
            0.0
        };
        input.analog_y = if by < 5.0 {
            -0.4
        } else if by > 45.0 {
            0.4
        } else {
            0.0
        };
        router.update(
            &mut player.lock().unwrap(),
            &input,
            (320.0, 240.0),
            1.0 / 48.0,
        );
        advance_input(&player, &mut executor, &mut router, &input, 1);
    }
    assert!(
        aligned,
        "Original aiming must pan bg_lot: {:?}",
        background_position(&player)
    );
    input.analog_x = 0.0;
    input.analog_y = 0.0;
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    advance_input(&player, &mut executor, &mut router, &input, 160);
    let hud = GameplayControls::read(&mut player.lock().unwrap(), &[], None).unwrap();
    let zoom = hud
        .zoom_in
        .expect("Original tutorial must reveal zoom after aiming");
    for _ in 0..2 {
        input.touch_count = 1;
        input.touches[0] = Touch {
            id: 2,
            x: zoom.0 as f32,
            y: zoom.1 as f32,
        };
        advance_input(&player, &mut executor, &mut router, &input, 2);
        input.touch_count = 0;
        advance_input(&player, &mut executor, &mut router, &input, 2);
    }
    assert_eq!(property(&player, &["game", "play_screen", "zoomNum"]), "2");
    input.touch_count = 0;
    input.buttons = 0x40;
    advance_input(&player, &mut executor, &mut router, &input, 24);
    input.buttons = 0;
    advance_input(&player, &mut executor, &mut router, &input, 2);
    assert_eq!(
        property(&player, &["game", "play_screen", "zoomNum"]),
        "1.5",
        "D-pad down zooms out"
    );
    input.buttons = 0x10;
    advance_input(&player, &mut executor, &mut router, &input, 2);
    input.buttons = 0;
    advance_input(&player, &mut executor, &mut router, &input, 2);
    assert_eq!(
        property(&player, &["game", "play_screen", "zoomNum"]),
        "2",
        "D-pad up zooms in without L"
    );
    let fire = GameplayControls::read(&mut player.lock().unwrap(), &[], None)
        .unwrap()
        .fire
        .expect("Tutorial must reveal fire");
    let ammo = property(&player, &["game", "play_screen", "mag"])
        .parse::<i32>()
        .unwrap();
    input.touch_count = 1;
    input.touches[0] = Touch {
        id: 3,
        x: fire.0 as f32,
        y: fire.1 as f32,
    };
    if physical_fire {
        input.touch_count = 0;
        input.buttons = 0x200;
    }
    input.analog_x = 0.5;
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 5);
    assert!(
        background_position(&player).0 < before.0 - 1.0,
        "Stick aim must continue while front fire is held"
    );
    input.analog_x = 0.0;
    input.buttons = 0;
    input.touch_count = 0;
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    advance_input(&player, &mut executor, &mut router, &input, 4);
    let remaining = property(&player, &["game", "play_screen", "mag"])
        .parse::<i32>()
        .unwrap();
    assert!(
        remaining < ammo,
        "Original fire must consume ammo: {ammo} -> {remaining}"
    );
    // A sniper bolt rejects zoom; a released pending press must never fire later.
    assert_eq!(
        property(&player, &["game", "play_screen", "action"]),
        "shot"
    );
    input.buttons = 0x40;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    assert_eq!(property(&player, &["game", "play_screen", "zoomNum"]), "2");
    input.buttons = 0;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    // A front-pad finger takes ownership of aim instead of competing with the stick.
    let aim = GameplayControls::read(&mut player.lock().unwrap(), &[], None)
        .unwrap()
        .aim
        .expect("Aim pad must be accessible");
    input.touch_count = 1;
    input.touches[0] = Touch {
        id: 4,
        x: aim.0 as f32,
        y: aim.1 as f32,
    };
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    input.touches[0].x += 8.0;
    router.update(
        &mut player.lock().unwrap(),
        &input,
        (320.0, 240.0),
        1.0 / 48.0,
    );
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 5);
    assert!(
        background_position(&player).0 < before.0,
        "Real front drag must move aim after handover"
    );
    // Another pad finger is suppressed for its entire lifetime, including release.
    input.touch_count = 2;
    input.touches[1] = Touch {
        id: 5,
        x: aim.0 as f32,
        y: aim.1 as f32,
    };
    advance_input(&player, &mut executor, &mut router, &input, 1);
    input.touch_count = 1;
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 5);
    assert!(
        background_position(&player).0 < before.0,
        "Second pad release must not cancel the owner"
    );
    // A HUD finger crossing onto the pad stays on its initial target.
    input.touch_count = 2;
    input.touches[1] = Touch {
        id: 6,
        x: fire.0 as f32,
        y: fire.1 as f32,
    };
    advance_input(&player, &mut executor, &mut router, &input, 1);
    input.touches[1].x = aim.0 as f32;
    input.touches[1].y = aim.1 as f32;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    input.touch_count = 1;
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 5);
    assert!(
        background_position(&player).0 < before.0,
        "HUD finger release on pad must not cancel the owner"
    );
    // Stick input cannot change a finger's baseline, then resumes after release.
    input.analog_x = -0.5;
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 5);
    assert!(
        background_position(&player).0 < before.0,
        "Finger owns aim until release"
    );
    input.touch_count = 0;
    let before = background_position(&player);
    advance_input(&player, &mut executor, &mut router, &input, 5);
    assert!(
        background_position(&player).0 > before.0,
        "Stick resumes after finger release"
    );
    input.analog_x = 0.0;
    for _ in 0..150 {
        if property(&player, &["game", "play_screen", "action"]) == "aiming" {
            break;
        }
        advance_input(&player, &mut executor, &mut router, &input, 1);
    }
    assert_eq!(
        property(&player, &["game", "play_screen", "zoomNum"]),
        "2",
        "Released pending zoom stays cancelled after bolt completes"
    );
    input.buttons = 0x40;
    advance_input(&player, &mut executor, &mut router, &input, 24);
    assert_eq!(
        property(&player, &["game", "play_screen", "zoomNum"]),
        "1.5",
        "One held D-pad press makes exactly one step after bolt completes"
    );
    input.buttons = 0;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    let pause_ammo = property(&player, &["game", "play_screen", "mag"]);
    input.buttons = 0x200;
    advance_input(&player, &mut executor, &mut router, &input, 30);
    assert!(
        player.lock().unwrap().frame_rate() < 48.0,
        "Holding sniper fire enters slow-motion"
    );
    if physical_fire {
        input.buttons |= 8;
    } else {
        let pause = object_point(&player, "btn_pause");
        input.touch_count = 1;
        input.touches[0] = Touch {
            id: 9,
            x: pause.0 as f32,
            y: pause.1 as f32,
        };
    }
    advance_input(&player, &mut executor, &mut router, &input, 3);
    assert_eq!(
        property(
            &player,
            &["game", "play_screen", "hud", "pauser", "visible"]
        ),
        "true"
    );
    assert_eq!(
        property(&player, &["game", "play_screen", "mag"]),
        pause_ammo,
        "Pausing held fire must cancel, not shoot"
    );
    input.touch_count = 0;
    input.buttons = 0x200;
    advance_input(&player, &mut executor, &mut router, &input, 3);
    input.buttons |= 8;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    input.buttons = 0x200;
    advance_input(&player, &mut executor, &mut router, &input, 30);
    assert_eq!(
        property(
            &player,
            &["game", "play_screen", "hud", "pauser", "visible"]
        ),
        "false"
    );
    assert_eq!(
        property(&player, &["game", "play_screen", "mag"]),
        pause_ammo
    );
    assert_eq!(
        player.lock().unwrap().frame_rate(),
        48.0,
        "Held trigger must stay cancelled after resume"
    );
    input.buttons = 0;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    input.buttons = 0x200;
    advance_input(&player, &mut executor, &mut router, &input, 30);
    input.buttons |= 1;
    advance_input(&player, &mut executor, &mut router, &input, 20);
    assert_eq!(
        property(
            &player,
            &["game", "play_screen", "hud", "menuClip", "currentFrame"]
        ),
        "15"
    );
    assert_eq!(
        property(&player, &["game", "play_screen", "mag"]),
        pause_ammo,
        "Objective menu cancels held fire"
    );
    input.buttons = 0x200;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    input.buttons |= 1;
    advance_input(&player, &mut executor, &mut router, &input, 20);
    assert_eq!(
        property(
            &player,
            &["game", "play_screen", "hud", "menuClip", "currentFrame"]
        ),
        "1"
    );
    assert_eq!(
        property(&player, &["game", "play_screen", "mag"]),
        pause_ammo
    );
    input.buttons = 0;
    advance_input(&player, &mut executor, &mut router, &input, 1);
    assert_eq!(
        errors.load(Ordering::Relaxed),
        0,
        "Original game flow must not log fatal AVM/runtime errors"
    );
    println!(
        "Original tutorial: localized menu, intro skip, stick aiming, front zoom, ammo {ammo}->{remaining}, front aim handover"
    );
}

#[test]
fn all_weapon_controls_use_original_range_scenes() {
    use crate::controls::{Input, InputRouter};
    use ruffle_core::Avm2FunctionArgs;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(root.join("build/Lonewolf-air.swf")).unwrap();
    for (range, screen) in [
        ("rangePistol", "play_screen2"),
        ("rangeM4", "play_screen3"),
        ("range1", "play_screen"),
    ] {
        let (player, mut executor) = player(&bytes, &root.join("build"));
        for _ in 0..300 {
            player.lock().unwrap().run_frame();
            executor.run();
        }
        let (x, y) = object_point(&player, "skip");
        player
            .lock()
            .unwrap()
            .handle_event(PlayerEvent::TouchBegin {
                id: 1,
                x,
                y,
                primary: true,
            });
        player
            .lock()
            .unwrap()
            .handle_event(PlayerEvent::TouchEnd { id: 1, x, y });
        for _ in 0..60 {
            player.lock().unwrap().run_frame();
            executor.run();
        }
        // Memory-only earned-progress fixture; actual menu handlers construct both scenes.
        player.lock().unwrap().update(|context| {
            let root = context
                .stage
                .iter_render_list()
                .next()
                .unwrap()
                .object2()
                .unwrap();
            let mut activation = Activation::from_nothing(context);
            let class = globals(&mut activation);
            for key in ["lock_tracks", "lock_suicide"] {
                let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), key);
                class
                    .set_public_property(name, Value::Bool(false), &mut activation)
                    .unwrap();
            }
            let name = ruffle_core::string::AvmString::new_utf8(activation.gc(), "game");
            let game = Value::from(root)
                .get_public_property(name, &mut activation)
                .unwrap();
            let name =
                ruffle_core::string::AvmString::new_utf8(activation.gc(), "select_the_level");
            game.call_public_property(name, Avm2FunctionArgs::empty(), &mut activation)
                .unwrap();
        });
        let mut router = InputRouter::default();
        let mut input = Input::default();
        advance_input(&player, &mut executor, &mut router, &input, 2);
        for key in ["back_out", "range", range] {
            let point = object_point(&player, key);
            input.buttons = 0x4000;
            router.update(&mut player.lock().unwrap(), &input, point, 1.0 / 48.0);
            input.buttons = 0;
            router.update(&mut player.lock().unwrap(), &input, point, 1.0 / 48.0);
            advance_input(&player, &mut executor, &mut router, &input, 2);
        }
        for _ in 0..400 {
            if crate::controls::GameplayControls::read(&mut player.lock().unwrap(), &[], None)
                .is_some_and(|hud| hud.aim.is_some() && hud.fire.is_some())
            {
                break;
            }
            advance_input(&player, &mut executor, &mut router, &input, 1);
        }
        advance_input(&player, &mut executor, &mut router, &input, 1);
        for _ in 0..150 {
            if property(&player, &["game", screen, "action"]) == "aiming" {
                break;
            }
            advance_input(&player, &mut executor, &mut router, &input, 1);
        }
        let hud = crate::controls::GameplayControls::read(&mut player.lock().unwrap(), &[], None)
            .unwrap();
        assert_eq!(
            hud.sniper,
            screen == "play_screen",
            "A retired screen must not capture {range} controls"
        );
        if screen != "play_screen" {
            assert_eq!(property(&player, &["game", screen, "touchActive"]), "true");
            let (x, y) = object_point(&player, "touchPad");
            input.touch_count = 1;
            input.touches[0] = crate::controls::Touch {
                id: 7,
                x: x as f32,
                y: y as f32,
            };
            advance_input(&player, &mut executor, &mut router, &input, 1);
            input.buttons = 0x2000;
            advance_input(&player, &mut executor, &mut router, &input, 1);
            assert_eq!(
                property(&player, &["game", screen, "touchActive"]),
                "true",
                "Front aim owns {range} even while Circle is held"
            );
            input.touch_count = 0;
            advance_input(&player, &mut executor, &mut router, &input, 12);
            assert_eq!(
                property(&player, &["game", screen, "touchActive"]),
                "false",
                "Circle lowers {range}"
            );
            let lowered = property(&player, &["game", screen, "gun", "main", "y"])
                .parse::<f64>()
                .unwrap();
            input.buttons = 0;
            advance_input(&player, &mut executor, &mut router, &input, 30);
            assert_eq!(property(&player, &["game", screen, "touchActive"]), "true");
            let raised = property(&player, &["game", screen, "gun", "main", "y"])
                .parse::<f64>()
                .unwrap();
            assert!(
                raised < lowered,
                "Releasing Circle raises {range}: {lowered}->{raised}"
            );
        }
        let sniper = screen == "play_screen";
        if !sniper {
            let gun = |axis| {
                property(&player, &["game", screen, "gun", axis])
                    .parse::<f64>()
                    .unwrap()
            };
            input.right_x = 1.0;
            input.right_y = 1.0;
            advance_input(&player, &mut executor, &mut router, &input, 180);
            let right = (gun("x"), gun("y"));
            input.right_x = -1.0;
            input.right_y = -1.0;
            advance_input(&player, &mut executor, &mut router, &input, 360);
            let left = (gun("x"), gun("y"));
            assert!(
                right.0 - left.0 > 800.0,
                "{range} must reach both aim limits: {left:?}..{right:?}"
            );
            assert!(
                right.1 - left.1 > 400.0,
                "{range} vertical aim must cover the scene: {left:?}..{right:?}"
            );
            input.right_x = 0.0;
            input.right_y = 0.0;
            advance_input(&player, &mut executor, &mut router, &input, 60);
            assert!(
                (gun("x") - left.0).abs() < 1.0,
                "Releasing the stick must keep {range} aim"
            );
            input.gyro_x = 0.8;
            advance_input(&player, &mut executor, &mut router, &input, 30);
            assert!((gun("x") - left.0).abs() < 1.0, "Gyro needs L in {range}");
            input.buttons = 0x100;
            advance_input(&player, &mut executor, &mut router, &input, 120);
            assert!(gun("x") > left.0 + 80.0, "L enables gyro in {range}");
            input.buttons = 0;
            advance_input(&player, &mut executor, &mut router, &input, 60);
            let settled = gun("x");
            let drag = trait_number(&player, &["game", screen], "touchDiffX");
            advance_input(&player, &mut executor, &mut router, &input, 60);
            assert_eq!(
                trait_number(&player, &["game", screen], "touchDiffX"),
                drag,
                "Releasing L holds the original drag distance"
            );
            // Original gunClip/gunAR animate hand sway even at constant drag.
            assert!(
                (gun("x") - settled).abs() < 12.0,
                "Releasing L holds {range} aim: {settled} -> {}",
                gun("x")
            );
            input.gyro_x = 0.0;
            println!("Original {range} aim limits: {left:?}..{right:?}; gyro hold/release passed");
        }
        if sniper {
            let before = trait_number(&player, &["game", screen], "bg.x");
            input.buttons = 0x100;
            input.gyro_x = 0.8;
            advance_input(&player, &mut executor, &mut router, &input, 60);
            assert!(
                (trait_number(&player, &["game", screen], "bg.x") - before).abs() > 2.0,
                "L enables sniper gyro panning"
            );
            input.buttons = 0;
            advance_input(&player, &mut executor, &mut router, &input, 30);
            let settled = trait_number(&player, &["game", screen], "bg.x");
            advance_input(&player, &mut executor, &mut router, &input, 60);
            assert!(
                (trait_number(&player, &["game", screen], "bg.x") - settled).abs() < 2.0,
                "Sniper gyro stops after L release"
            );
            input.gyro_x = 0.0;
        }
        let ammo = || {
            let text = if sniper {
                property(&player, &["game", screen, "mag"])
            } else {
                property(
                    &player,
                    &["game", screen, "hud", "magDisplay", "txt_mag", "text"],
                )
                .strip_prefix("x ")
                .unwrap()
                .to_owned()
            };
            text.parse::<i32>().unwrap()
        };
        let capacity = ammo();
        input.buttons = 0x200;
        router.update(
            &mut player.lock().unwrap(),
            &input,
            (320.0, 240.0),
            1.0 / 48.0,
        );
        assert_eq!(
            property(&player, &["game", screen, "action"]),
            if sniper { "aiming" } else { "shot" },
            "Original fire Begin in {range}"
        );
        advance_input(&player, &mut executor, &mut router, &input, 40);
        if sniper {
            assert_eq!(
                property(&player, &["game", screen, "mag"])
                    .parse::<i32>()
                    .unwrap(),
                capacity,
                "Sniper holds fire until release"
            );
        } else {
            assert_eq!(
                property(&player, &["game", screen, "action"]),
                "aiming",
                "Holding R does not restart fire in {range}"
            );
        }
        assert_eq!(
            ammo(),
            if sniper { capacity } else { capacity - 1 },
            "Original ammo UI after held fire in {range}"
        );
        input.buttons = 0;
        advance_input(&player, &mut executor, &mut router, &input, 100);
        assert_eq!(
            ammo(),
            capacity - 1,
            "One press/release consumes one round in {range}"
        );
        if sniper {
            assert_eq!(
                property(&player, &["game", screen, "mag"])
                    .parse::<i32>()
                    .unwrap(),
                capacity - 1,
                "Sniper release consumes one round"
            );
        }
        for _ in 0..400 {
            if property(&player, &["game", screen, "action"]) == "aiming" {
                break;
            }
            advance_input(&player, &mut executor, &mut router, &input, 1);
        }
        assert_eq!(
            property(&player, &["game", screen, "action"]),
            "aiming",
            "Original bolt/cooldown must finish in {range}"
        );
        input.buttons = 0x8000;
        router.update(
            &mut player.lock().unwrap(),
            &input,
            (320.0, 240.0),
            1.0 / 48.0,
        );
        assert_eq!(
            property(&player, &["game", screen, "action"]),
            if sniper { "loading" } else { "reload" },
            "Square reloads a partially spent magazine in {range}"
        );
        if sniper {
            assert_eq!(property(&player, &["game", screen, "mag"]), "0");
        }
        input.buttons = 0;
        advance_input(&player, &mut executor, &mut router, &input, 1);
        assert_eq!(ammo(), 0, "Reload empties the original ammo UI in {range}");
        // The starter sniper's original reload ends at timeline frame 525.
        advance_input(&player, &mut executor, &mut router, &input, 600);
        assert_eq!(
            property(&player, &["game", screen, "action"]),
            "aiming",
            "Original reload finishes in {range}"
        );
        if sniper {
            assert_eq!(
                property(&player, &["game", screen, "mag"])
                    .parse::<i32>()
                    .unwrap(),
                capacity
            );
        }
        assert_eq!(
            ammo(),
            capacity,
            "Original reload refills ammo UI in {range}"
        );
        input.buttons = 0x8000;
        router.update(
            &mut player.lock().unwrap(),
            &input,
            (320.0, 240.0),
            1.0 / 48.0,
        );
        assert_eq!(
            property(&player, &["game", screen, "action"]),
            "aiming",
            "Full magazine rejects reload in {range}"
        );
        input.buttons = 0;
        advance_input(&player, &mut executor, &mut router, &input, 1);
        input.buttons = 8;
        advance_input(&player, &mut executor, &mut router, &input, 3);
        assert_eq!(
            property(&player, &["game", screen, "hud", "pauser", "visible"]),
            "true"
        );
        input.buttons = 0;
        advance_input(&player, &mut executor, &mut router, &input, 1);
        input.buttons = 8;
        advance_input(&player, &mut executor, &mut router, &input, 1);
        assert_eq!(
            property(&player, &["game", screen, "hud", "pauser", "visible"]),
            "false"
        );
        input.buttons = 0;
        advance_input(&player, &mut executor, &mut router, &input, 1);
        println!(
            "Original {range}: aim, fire/hold/release, Square reload/full-mag guard, pause/resume"
        );
    }
}

fn advance_input(
    player: &Arc<Mutex<Player>>,
    executor: &mut NullExecutor,
    router: &mut crate::controls::InputRouter,
    input: &crate::controls::Input,
    frames: usize,
) {
    for _ in 0..frames {
        let mut player = player.lock().unwrap();
        router.update(&mut player, input, (320.0, 240.0), 1.0 / 48.0);
        player.run_frame();
        player.update_timers(1000.0 / 48.0);
        drop(player);
        executor.run();
    }
}
