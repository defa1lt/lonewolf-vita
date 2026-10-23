#![cfg(target_os = "macos")]

use glow::HasContext;
use ruffle_render::backend::{RenderBackend, ViewportDimensions};
use ruffle_render::bitmap::{Bitmap, BitmapFormat, PixelSnapping};
use ruffle_render::commands::{CommandHandler, CommandList};
use ruffle_render::matrix::Matrix;
use ruffle_render::quality::StageQuality;
use ruffle_render::transform::Transform;
use super::GlowRenderBackend;
use std::{ffi::CString, ptr, sync::Arc};
use swf::{Color, Fixed8};

#[link(name = "OpenGL", kind = "framework")]
unsafe extern "C" {
    fn CGLChoosePixelFormat(attributes: *const u32, format: *mut *mut (), count: *mut i32) -> i32;
    fn CGLCreateContext(format: *mut (), shared: *mut (), context: *mut *mut ()) -> i32;
    fn CGLSetCurrentContext(context: *mut ()) -> i32;
    fn CGLDestroyContext(context: *mut ()) -> i32;
    fn CGLDestroyPixelFormat(format: *mut ());
    fn dlsym(handle: *mut (), name: *const i8) -> *const ();
}

#[test]
fn startup_waits_for_artwork_instead_of_clear_letterbox_or_stencil() {
    unsafe {
        let attributes = [99, 0x3200, 73, 8, 32, 0];
        let (mut format, mut context) = (ptr::null_mut(), ptr::null_mut());
        let mut count = 0;
        assert_eq!(CGLChoosePixelFormat(attributes.as_ptr(), &mut format, &mut count), 0);
        assert_eq!(CGLCreateContext(format, ptr::null_mut(), &mut context), 0);
        assert_eq!(CGLSetCurrentContext(context), 0);
        let gl = Arc::new(glow::Context::from_loader_function(|name| {
            dlsym(-2isize as *mut (), CString::new(name).unwrap().as_ptr()).cast()
        }));
        let mut renderer = GlowRenderBackend::new(gl.clone(), false, StageQuality::High).unwrap();
        renderer.set_viewport_dimensions(ViewportDimensions { width: 32, height: 32, scale_factor: 1.0 });
        let bitmap = renderer.register_bitmap(Bitmap::new(1, 1, BitmapFormat::Rgba, vec![255, 0, 0, 255])).unwrap();
        let submit = |renderer: &mut GlowRenderBackend, commands: CommandList| {
            // The CGL context has no window. Draw into the real MSAA framebuffer;
            // omit only end_frame, which presents to a window drawable.
            renderer.begin_frame(Color::WHITE);
            commands.execute(renderer);
            renderer.flush_batch();
            assert_eq!(gl.get_error(), glow::NO_ERROR);
        };
        submit(&mut renderer, CommandList::new());
        assert!(!renderer.last_frame_had_content());
        let mut bars = CommandList::new();
        bars.draw_rect(Color::BLACK, Matrix::scale(32.0, 1.0));
        bars.draw_rect(Color::BLACK, Matrix::scale(1.0, 32.0));
        submit(&mut renderer, bars);
        assert!(!renderer.last_frame_had_content());
        let draw_bitmap = |commands: &mut CommandList, transform| {
            commands.render_bitmap(bitmap.clone(), transform, false, PixelSnapping::Never);
        };
        let mut stencil = CommandList::new();
        stencil.push_mask(); draw_bitmap(&mut stencil, Transform::default());
        stencil.activate_mask(); stencil.deactivate_mask();
        draw_bitmap(&mut stencil, Transform::default()); stencil.pop_mask();
        submit(&mut renderer, stencil);
        assert!(!renderer.last_frame_had_content());
        let mut transparent = CommandList::new();
        let mut hidden = Transform::default(); hidden.color_transform.a_multiply = Fixed8::ZERO;
        draw_bitmap(&mut transparent, hidden);
        submit(&mut renderer, transparent);
        assert!(!renderer.last_frame_had_content());
        let mut artwork = CommandList::new(); draw_bitmap(&mut artwork, Transform::default());
        submit(&mut renderer, artwork);
        assert!(renderer.last_frame_had_content());
        submit(&mut renderer, CommandList::new());
        assert!(!renderer.last_frame_had_content());
        drop(bitmap); drop(renderer); drop(gl);
        CGLSetCurrentContext(ptr::null_mut());
        CGLDestroyContext(context); CGLDestroyPixelFormat(format);
    }
}
