use ruffle_core::events::PlayerEvent;
use ruffle_core::focus_tracker::{DisplayObject, TDisplayObject, TDisplayObjectContainer};
use ruffle_core::string::WStr;
use ruffle_core::swf::Point;
use ruffle_core::{Avm2Activation, Avm2Value, Player};
use std::collections::BTreeMap;

fn property<'gc>(
    value: Avm2Value<'gc>,
    name: &str,
    activation: &mut Avm2Activation<'_, 'gc>,
) -> Option<Avm2Value<'gc>> {
    if value.as_object().is_none() {
        return None;
    }
    let name = activation
        .context
        .strings
        .intern_wstr(WStr::from_units(name.as_bytes()));
    value.get_public_property(name, activation).ok()
}

fn active_screen<'gc>(
    activation: &mut Avm2Activation<'_, 'gc>,
) -> Option<(Avm2Value<'gc>, &'static str)> {
    let root = activation
        .context
        .stage
        .iter_render_list()
        .next()?
        .object2()?;
    let game = property(root.into(), "game", activation)?;
    for name in ["play_screen", "play_screen2", "play_screen3"] {
        let Some(screen) = property(game, name, activation) else {
            continue;
        };
        // A disposed mission can leave its empty screen container on stage.
        let hud = property(screen, "hud", activation);
        if visible(screen, activation).is_some()
            && hud.and_then(|hud| visible(hud, activation)).is_some()
        {
            return Some((screen, name));
        }
    }
    None
}

struct Hud<'gc> {
    screen: Avm2Value<'gc>,
    sniper: bool,
    aim: Option<DisplayObject<'gc>>,
    fire: Option<DisplayObject<'gc>>,
    zoom_in: Option<DisplayObject<'gc>>,
    zoom_out: Option<DisplayObject<'gc>>,
    reload: Option<DisplayObject<'gc>>,
}

fn visible<'gc>(
    value: Avm2Value<'gc>,
    activation: &Avm2Activation<'_, 'gc>,
) -> Option<DisplayObject<'gc>> {
    let target = value.as_object()?.as_display_object()?;
    if !target.is_on_stage(activation.context) {
        return None;
    }
    let mut object = Some(target);
    while let Some(ancestor) = object {
        if !ancestor.visible() {
            return None;
        }
        object = ancestor.parent();
    }
    Some(target)
}

impl<'gc> Hud<'gc> {
    fn read(activation: &mut Avm2Activation<'_, 'gc>) -> Option<Self> {
        let (screen, kind) = active_screen(activation)?;
        if property(screen, "gamePlay", activation)? != Avm2Value::Bool(true) {
            return None;
        }
        let hud = property(screen, "hud", activation)?;
        let loading = property(hud, "loadScreen", activation)?;
        let frame = property(loading, "currentFrame", activation)?
            .coerce_to_number(activation)
            .ok()?;
        let total = property(loading, "totalFrames", activation)?
            .coerce_to_number(activation)
            .ok()?;
        if frame < total {
            return None;
        }
        let pauser = property(hud, "pauser", activation)?;
        if property(pauser, "visible", activation)? == Avm2Value::Bool(true) {
            return None;
        }
        let menu = property(hud, "menuClip", activation)?;
        if property(menu, "currentFrame", activation)?
            .coerce_to_number(activation)
            .ok()?
            != 1.0
        {
            return None;
        }
        let aim = property(hud, "touchPad", activation).and_then(|v| visible(v, activation));
        let buttons = property(hud, "buttons", activation)?;
        let zoomer = property(buttons, "zoomer", activation)?;
        let fire = property(buttons, "fire", activation).and_then(|v| visible(v, activation));
        let zoom_in = property(zoomer, "zoomIn", activation).and_then(|v| visible(v, activation));
        let zoom_out = property(zoomer, "zoomOut", activation).and_then(|v| visible(v, activation));
        let reload = property(buttons, "reloader", activation).and_then(|v| visible(v, activation));
        Some(Self {
            screen,
            sniper: kind == "play_screen",
            aim,
            fire,
            zoom_in,
            zoom_out,
            reload,
        })
    }

    fn aiming(&self, activation: &mut Avm2Activation<'_, 'gc>) -> bool {
        property(self.screen, "action", activation)
            .and_then(|v| v.coerce_to_string(activation).ok())
            .is_some_and(|v| v.to_utf8_lossy() == "aiming")
    }
    fn ready(&self, activation: &mut Avm2Activation<'_, 'gc>) -> bool {
        !self.sniper || self.aiming(activation)
    }
}

fn origin(target: DisplayObject<'_>, activation: &Avm2Activation<'_, '_>) -> (f64, f64) {
    let p = activation.context.stage.view_matrix()
        * target.local_to_global(Point::from_pixels(0.0, 0.0));
    (p.x.to_pixels(), p.y.to_pixels())
}

fn contains<'gc>(target: DisplayObject<'gc>, mut picked: Option<DisplayObject<'gc>>) -> bool {
    while let Some(object) = picked {
        if DisplayObject::ptr_eq(target, object) {
            return true;
        }
        picked = object.parent();
    }
    false
}

#[derive(Clone, Copy)]
enum Navigation {
    Pause,
    Objective,
}

fn navigation_target<'gc>(
    action: Navigation,
    activation: &mut Avm2Activation<'_, 'gc>,
) -> Option<DisplayObject<'gc>> {
    let (screen, _) = active_screen(activation)?;
    let hud = property(screen, "hud", activation)?;
    let pauser = property(hud, "pauser", activation)?;
    let paused = property(pauser, "visible", activation)? == Avm2Value::Bool(true);
    let value = match action {
        Navigation::Pause if paused => property(pauser, "btn_unpause", activation)?,
        Navigation::Pause => {
            let ender = property(hud, "ender", activation)?;
            if property(ender, "visible", activation)? == Avm2Value::Bool(true) {
                return None;
            }
            let buttons = property(hud, "buttons", activation)?;
            property(buttons, "btn_pause", activation)?
        }
        Navigation::Objective => {
            if paused || property(screen, "gamePlay", activation)? != Avm2Value::Bool(true) {
                return None;
            }
            let menu = property(hud, "menuClip", activation)?;
            let frame = property(menu, "currentFrame", activation)?
                .coerce_to_number(activation)
                .ok()?;
            property(
                menu,
                if frame == 1.0 {
                    "touchOut"
                } else if frame == 15.0 {
                    "touchIn"
                } else {
                    return None;
                },
                activation,
            )?
        }
    };
    visible(value, activation)
}

// Coordinates are only exposed to host tests that exercise actual front-panel taps.
#[cfg(test)]
#[derive(Debug)]
pub struct GameplayControls {
    pub sniper: bool,
    pub aim: Option<(f64, f64)>,
    pub fire: Option<(f64, f64)>,
    pub zoom_in: Option<(f64, f64)>,
}
#[cfg(test)]
impl GameplayControls {
    pub fn read(player: &mut Player, _: &[(f64, f64)], _: Option<(f64, f64)>) -> Option<Self> {
        player.mutate_with_update_context(|context| {
            let mut activation = Avm2Activation::from_nothing(context);
            let hud = Hud::read(&mut activation)?;
            Some(Self {
                sniper: hud.sniper,
                aim: hud.aim.map(|v| origin(v, &activation)),
                fire: hud.fire.map(|v| origin(v, &activation)),
                zoom_in: hud.zoom_in.map(|v| origin(v, &activation)),
            })
        })
    }
}

#[repr(C)]
#[derive(Default)]
pub struct Input {
    pub buttons: u32,
    pub analog_x: f32,
    pub analog_y: f32,
    pub right_x: f32,
    pub right_y: f32,
    pub touch_count: u32,
    pub touches: [Touch; 8],
    pub gyro_x: f32,
    pub gyro_y: f32,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Touch {
    pub id: u32,
    pub x: f32,
    pub y: f32,
}

const _: () = assert!(std::mem::size_of::<Input>() == 128);

#[derive(Clone, Copy)]
enum FrontRole {
    Aim,
    Suppressed,
    Other,
}

#[derive(Default)]
pub struct InputRouter {
    contacts: BTreeMap<i32, (f64, f64)>,
    buttons: u32,
    blocked_buttons: u32,
    gameplay: bool,
    front: BTreeMap<i32, FrontRole>,
    primary: Option<i32>,
    aim_owner: Option<i32>,
    aim_origin: Option<(f64, f64, Option<i32>)>,
}

impl InputRouter {
    pub fn primary(&self) -> Option<i32> {
        self.primary
    }
    pub fn has_contacts(&self) -> bool {
        !self.contacts.is_empty()
    }

    fn cancel(&mut self, player: &mut Player, buttons: u32) {
        for (&id, &(x, y)) in &self.contacts {
            player.handle_event(PlayerEvent::TouchCancel { id, x, y });
        }
        self.contacts.clear();
        self.front
            .values_mut()
            .for_each(|role| *role = FrontRole::Suppressed);
        self.primary = None;
        self.aim_owner = None;
        self.aim_origin = None;
        // Closing a menu must not turn an already held trigger into a new press.
        self.blocked_buttons |= buttons & 0xc350;
    }

    pub fn update(
        &mut self,
        player: &mut Player,
        input: &Input,
        pointer: (f64, f64),
        dt: f64,
    ) -> bool {
        let raw: BTreeMap<i32, (f64, f64)> = input.touches[..(input.touch_count as usize).min(8)]
            .iter()
            .map(|t| (t.id as i32, (t.x as f64, t.y as f64)))
            .collect();
        let pressed = input.buttons & !self.buttons;
        self.buttons = input.buttons;
        self.blocked_buttons &= input.buttons;
        let new_front = raw.keys().any(|id| !self.front.contains_key(id));
        let navigation = if pressed & 9 != 0 || new_front {
            player.mutate_with_update_context(|context| {
                let mut activation = Avm2Activation::from_nothing(context);
                let action = if pressed & 8 != 0 {
                    Some(Navigation::Pause)
                } else if pressed & 1 != 0 {
                    Some(Navigation::Objective)
                } else {
                    raw.iter()
                        .filter(|(id, _)| !self.front.contains_key(id))
                        .find_map(|(_, &(x, y))| {
                            let mut picked = activation.context.pick_interactive_at(x, y);
                            while let Some(target) = picked {
                                let name = target.name().map(|n| n.to_utf8_lossy().into_owned());
                                match name.as_deref() {
                                    Some("btn_pause" | "btn_unpause") => {
                                        return Some(Navigation::Pause);
                                    }
                                    Some("touchOut" | "touchIn") => {
                                        return Some(Navigation::Objective);
                                    }
                                    _ => picked = target.parent(),
                                }
                            }
                            None
                        })
                }?;
                navigation_target(action, &mut activation).map(|_| action)
            })
        } else {
            None
        };
        if let Some(action) = navigation {
            // Original sniper Out cancels slow-motion only before pause freezes the movie.
            self.cancel(player, input.buttons);
            player.update(|context| {
                let mut activation = Avm2Activation::from_nothing(context);
                if let Some(target) = navigation_target(action, &mut activation) {
                    let (x, y) = origin(target, &activation);
                    activation.context.handle_touch_on(
                        target,
                        PlayerEvent::TouchBegin {
                            id: 1010,
                            x,
                            y,
                            primary: false,
                        },
                    );
                    activation
                        .context
                        .handle_touch_on(target, PlayerEvent::TouchEnd { id: 1010, x, y });
                }
            });
            for &id in raw.keys() {
                self.front.insert(id, FrontRole::Suppressed);
            }
        }
        let gameplay = player.mutate_with_update_context(|context| {
            let mut activation = Avm2Activation::from_nothing(context);
            Hud::read(&mut activation).is_some()
        });
        if self.gameplay != gameplay && !self.contacts.is_empty() {
            self.cancel(player, input.buttons);
        }
        self.gameplay = gameplay;
        let buttons = input.buttons & !self.blocked_buttons;
        self.front.retain(|id, _| raw.contains_key(id));
        if self.aim_owner.is_some_and(|id| !raw.contains_key(&id)) {
            self.aim_owner = None;
        }
        // Classify fingers once. A HUD finger crossing the aim pad never becomes a drag.
        if new_front {
            player.mutate_with_update_context(|context| {
                let mut activation = Avm2Activation::from_nothing(context);
                let hud = Hud::read(&mut activation);
                for (&id, &(x, y)) in &raw {
                    if self.front.contains_key(&id) {
                        continue;
                    }
                    let picked = activation.context.pick_interactive_at(x, y);
                    let on_pad = hud
                        .as_ref()
                        .and_then(|h| h.aim)
                        .is_some_and(|pad| contains(pad, picked));
                    let role = if on_pad {
                        if self.aim_owner.is_none() {
                            self.aim_owner = Some(id);
                            FrontRole::Aim
                        } else {
                            FrontRole::Suppressed
                        }
                    } else {
                        FrontRole::Other
                    };
                    self.front.insert(id, role);
                }
            });
        }
        let mut current = raw
            .iter()
            .filter(|(id, _)| matches!(self.front.get(id), Some(FrontRole::Other)))
            .map(|(&id, &point)| (id, point))
            .collect::<BTreeMap<_, _>>();
        let gyro = |rate: f32| {
            if buttons & 0x100 == 0 || !rate.is_finite() {
                return 0.0;
            }
            let rate = (rate as f64).clamp(-2.0, 2.0);
            rate.signum() * (rate.abs() - 0.015).max(0.0)
        };
        let axis = |left: f32, right: f32| {
            let v = if right.abs() > left.abs() {
                right
            } else {
                left
            };
            if v.abs() > 0.2 { v as f64 } else { 0.0 }
        };
        let viewport = player.viewport_dimensions();
        let aim = player.mutate_with_update_context(|context| {
            let mut activation = Avm2Activation::from_nothing(context);
            let hud = Hud::read(&mut activation)?;
            let pad = hud.aim?;
            if !hud.sniper && self.aim_owner.is_none() && buttons & 0x2000 != 0 {
                return None;
            }
            let point = self.aim_owner.and_then(|id| raw.get(&id).copied());
            let base = self
                .aim_origin
                .filter(|p| p.2 == self.aim_owner && self.contacts.contains_key(&1001))
                .map(|p| (p.0, p.1))
                .unwrap_or_else(|| point.unwrap_or_else(|| origin(pad, &activation)));
            let point = point.unwrap_or_else(|| {
                let x = axis(input.analog_x, input.right_x);
                let y = axis(input.analog_y, input.right_y);
                if hud.sniper {
                    // The original sniper timer turns a held drag into continuous panning.
                    // Its 100 ms timer integrates this velocity offset itself.
                    (
                        base.0 + x * 32.0 + gyro(input.gyro_x) * 20.0,
                        base.1 + y * 32.0 + gyro(input.gyro_y) * 20.0,
                    )
                } else {
                    // Pistol and rifle use absolute drag distance, so accumulate stick travel.
                    let previous = self
                        .contacts
                        .get(&1001)
                        .copied()
                        .filter(|_| self.aim_origin.is_some_and(|p| p.2 == self.aim_owner))
                        .unwrap_or(base);
                    let speed = |v: f64| {
                        let strength = ((v.abs() - 0.2) / 0.8).clamp(0.0, 1.0);
                        v.signum() * strength * strength * 320.0 * dt.clamp(0.0, 0.1)
                    };
                    (
                        (previous.0 + speed(x) + gyro(input.gyro_x) * 140.0 * dt.clamp(0.0, 0.1))
                            .clamp(
                                base.0 - viewport.width as f64 / 2.0,
                                base.0 + viewport.width as f64 / 2.0,
                            ),
                        (previous.1 + speed(y) + gyro(input.gyro_y) * 140.0 * dt.clamp(0.0, 0.1))
                            .clamp(
                                base.1 - viewport.height as f64 / 2.0,
                                base.1 + viewport.height as f64 / 2.0,
                            ),
                    )
                }
            });
            Some((base, point))
        });
        // End the old owner before starting the next, even when both are present this frame.
        let owner_changed = self.contacts.contains_key(&1001)
            && self.aim_origin.map(|p| p.2) != Some(self.aim_owner);
        if owner_changed {
            if let Some((x, y)) = self.contacts.remove(&1001) {
                player.handle_event(PlayerEvent::TouchEnd { id: 1001, x, y });
            }
            self.aim_origin = None;
        }
        if let Some((base, point)) = aim {
            current.insert(1001, point);
            self.aim_origin = Some((base.0, base.1, self.aim_owner));
        } else {
            self.aim_origin = None;
        }
        if gameplay {
            player.mutate_with_update_context(|context| {
                let mut activation = Avm2Activation::from_nothing(context);
                if let Some(hud) = Hud::read(&mut activation) {
                    for (id, mask, target) in [
                        (1002, 0x4200, hud.fire),
                        (1003, 0x10, hud.zoom_in),
                        (1004, 0x40, hud.zoom_out),
                        (1005, 0x8000, hud.reload),
                    ] {
                        if buttons & mask != 0 {
                            if let Some(point) = self
                                .contacts
                                .get(&id)
                                .copied()
                                .or_else(|| target.map(|target| origin(target, &activation)))
                            {
                                current.insert(id, point);
                            }
                        }
                    }
                }
            });
        } else if buttons & 0x4000 != 0 {
            current.insert(1000, pointer);
        }
        if self.primary.is_none_or(|id| !current.contains_key(&id)) {
            self.primary = current.keys().find(|id| **id < 1001).copied();
        }
        for (&id, &(x, y)) in &self.contacts {
            if !current.contains_key(&id) {
                player.handle_event(PlayerEvent::TouchEnd { id, x, y });
            }
        }
        let ordered = current
            .get_key_value(&1001)
            .into_iter()
            .chain(current.iter().filter(|(id, _)| **id != 1001));
        let mut deferred = Vec::new();
        for (&id, &(x, y)) in ordered {
            if !self.contacts.contains_key(&id) {
                let start = if id == 1001 {
                    self.aim_origin.map(|p| (p.0, p.1)).unwrap_or((x, y))
                } else {
                    (x, y)
                };
                let event = PlayerEvent::TouchBegin {
                    id,
                    x: start.0,
                    y: start.1,
                    primary: !gameplay && Some(id) == self.primary,
                };
                let accepted = if gameplay {
                    player.update(|context| {
                        let mut activation = Avm2Activation::from_nothing(context);
                        let hud = Hud::read(&mut activation);
                        if (1003..=1005).contains(&id)
                            && !hud.as_ref().is_some_and(|h| h.ready(&mut activation))
                        {
                            return false;
                        }
                        if id == 1002
                            && !hud.as_ref().is_some_and(|h| {
                                h.aiming(&mut activation)
                                    && (h.sniper || current.contains_key(&1001))
                            })
                        {
                            return false;
                        }
                        let target = match id {
                            1001 => hud.and_then(|h| h.aim),
                            1002 => hud.and_then(|h| h.fire),
                            1003 => hud.and_then(|h| h.zoom_in),
                            1004 => hud.and_then(|h| h.zoom_out),
                            1005 => hud.and_then(|h| h.reload),
                            _ => activation.context.pick_interactive_at(x, y),
                        };
                        target
                            .is_some_and(|target| activation.context.handle_touch_on(target, event))
                    })
                } else {
                    player.handle_event(event)
                };
                if !accepted {
                    deferred.push(id);
                    continue;
                }
                if start != (x, y) {
                    player.handle_event(PlayerEvent::TouchMove { id, x, y });
                }
            } else if self.contacts[&id] != (x, y) {
                player.handle_event(PlayerEvent::TouchMove { id, x, y });
            }
        }
        for id in deferred {
            current.remove(&id);
        }
        self.contacts = current;
        gameplay
    }
}
