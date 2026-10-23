use crate::avm2::Error;
use crate::avm2::activation::Activation;
use crate::avm2::value::Value;

pub fn update_after_event<'gc>(
    activation: &mut Activation<'_, 'gc>,
    _this: Value<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    *activation.context.needs_render = true;
    Ok(Value::Undefined)
}

use crate::avm2::globals::slots::flash_events_touch_event as slots;

pub fn get_stage_x<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Value<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    super::mouse_event::local_to_stage_x(
        activation,
        this.as_object().unwrap(),
        slots::_LOCAL_X,
        slots::_LOCAL_Y,
    )
}
pub fn get_stage_y<'gc>(
    activation: &mut Activation<'_, 'gc>,
    this: Value<'gc>,
    _args: &[Value<'gc>],
) -> Result<Value<'gc>, Error<'gc>> {
    super::mouse_event::local_to_stage_y(
        activation,
        this.as_object().unwrap(),
        slots::_LOCAL_X,
        slots::_LOCAL_Y,
    )
}
