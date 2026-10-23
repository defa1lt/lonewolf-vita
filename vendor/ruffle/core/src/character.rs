use crate::bitmap::bitmap_data::{BitmapData, Color, PixelBuffer};
use std::cell::{OnceCell, RefCell};
use std::sync::{Arc, Weak};

use crate::backend::audio::SoundHandle;
use crate::binary_data::BinaryData;
use crate::display_object::{
    Avm1Button, Avm2Button, BitmapClass, EditText, Graphic, MorphShape, MovieClip, Text, Video,
};
use crate::font::Font;
use gc_arena::barrier::unlock;
use gc_arena::lock::Lock;
use gc_arena::{Collect, Gc, Mutation};
use ruffle_render::backend::RenderBackend;
use ruffle_render::bitmap::{Bitmap as RenderBitmap, BitmapFormat, BitmapHandle, BitmapSize};
use ruffle_render::error::Error as RenderError;
use swf::DefineBitsLossless;

#[derive(Copy, Clone, Collect, Debug)]
#[collect(no_drop)]
pub enum Character<'gc> {
    EditText(EditText<'gc>),
    Graphic(Graphic<'gc>),
    MovieClip(MovieClip<'gc>),
    Bitmap(Gc<'gc, BitmapCharacter<'gc>>),
    Avm1Button(Avm1Button<'gc>),
    Avm2Button(Avm2Button<'gc>),
    Font(Font<'gc>),
    MorphShape(MorphShape<'gc>),
    Text(Text<'gc>),
    Sound(#[collect(require_static)] SoundHandle),
    Video(Video<'gc>),
    BinaryData(Gc<'gc, BinaryData>),
}

#[derive(Collect, Debug)]
#[collect(no_drop)]
pub struct BitmapCharacter<'gc> {
    #[collect(require_static)]
    compressed: CompressedBitmap,
    /// A lazily constructed GPU handle, used when performing fills with this bitmap
    #[collect(require_static)]
    handle: OnceCell<BitmapHandle>,
    #[collect(require_static)]
    shared_pixels: RefCell<Option<(Weak<PixelBuffer>, BitmapFormat)>>,
    /// The bitmap class set by `SymbolClass` - this is used when we instantaite
    /// a `Bitmap` displayobject.
    avm2_class: Lock<BitmapClass<'gc>>,
}

impl<'gc> BitmapCharacter<'gc> {
    pub fn new(compressed: CompressedBitmap) -> Self {
        Self {
            compressed,
            handle: OnceCell::default(),
            shared_pixels: RefCell::new(None),
            avm2_class: Lock::new(BitmapClass::NoSubclass),
        }
    }

    pub fn compressed(&self) -> &CompressedBitmap {
        &self.compressed
    }

    pub fn shared_pixels(
        &self,
        metrics: &gc_arena::metrics::Metrics,
    ) -> Result<(Arc<PixelBuffer>, BitmapFormat), RenderError> {
        if let Some((weak, format)) = self.shared_pixels.borrow().as_ref() {
            if let Some(pixels) = weak.upgrade() {
                return Ok((pixels, *format));
            }
        }
        let (pixels, format) = match &self.compressed {
            CompressedBitmap::Lossless(tag) => {
                ruffle_render::utils::validate_size(tag.width, tag.height)?;
                let mut pixels = vec![Color::default(); tag.width as usize * tag.height as usize];
                ruffle_render::utils::decode_define_bits_lossless_into(
                    tag,
                    bytemuck::cast_slice_mut(&mut pixels),
                )?;
                (pixels, BitmapFormat::Rgba)
            }
            _ => {
                let bitmap = self.compressed.decode()?;
                (
                    bitmap.as_colors().map(Color::from).collect(),
                    bitmap.format(),
                )
            }
        };
        let pixels = Arc::new(PixelBuffer::new(pixels, Some(metrics.clone())));
        *self.shared_pixels.borrow_mut() = Some((Arc::downgrade(&pixels), format));
        Ok((pixels, format))
    }

    pub fn bitmap_data(&self, mc: &Mutation<'gc>) -> Result<BitmapData<'gc>, RenderError> {
        let (pixels, format) = self.shared_pixels(mc.metrics())?;
        let size = self.compressed.size();
        Ok(BitmapData::new_with_shared_pixels(
            mc,
            u32::from(size.width),
            u32::from(size.height),
            format == BitmapFormat::Rgba,
            pixels,
        ))
    }

    pub fn avm2_class(&self) -> BitmapClass<'gc> {
        self.avm2_class.get()
    }

    pub fn set_avm2_class(this: Gc<'gc, Self>, bitmap_class: BitmapClass<'gc>, mc: &Mutation<'gc>) {
        unlock!(Gc::write(mc, this), Self, avm2_class).set(bitmap_class);
    }

    pub fn bitmap_handle(
        &self,
        backend: &mut dyn RenderBackend,
    ) -> Result<BitmapHandle, RenderError> {
        // FIXME - use `OnceCell::get_or_try_init` when stabilized.
        if let Some(handle) = self.handle.get() {
            return Ok(handle.clone());
        }
        let decoded = self.compressed.decode()?;
        let new_handle = backend.register_bitmap(decoded)?;
        // FIXME - do we ever want to release this handle, to avoid taking up GPU memory?
        self.handle.set(new_handle.clone()).unwrap();
        Ok(new_handle)
    }
}

/// Holds a bitmap from an SWF tag, plus the decoded width/height.
/// We avoid decompressing the image until it's actually needed - some pathological SWFS
/// like 'House' have thousands of highly-compressed (mostly empty) bitmaps, which can
/// take over 10GB of ram if we decompress them all during preloading.
#[derive(Clone, Debug)]
pub enum CompressedBitmap {
    Jpeg {
        data: Vec<u8>,
        alpha: Option<Vec<u8>>,
        width: u16,
        height: u16,
    },
    Lossless(DefineBitsLossless<'static>),
}

impl CompressedBitmap {
    pub fn size(&self) -> BitmapSize {
        match self {
            CompressedBitmap::Jpeg { width, height, .. } => BitmapSize {
                width: *width,
                height: *height,
            },
            CompressedBitmap::Lossless(define_bits_lossless) => BitmapSize {
                width: define_bits_lossless.width,
                height: define_bits_lossless.height,
            },
        }
    }
    pub fn decode(&self) -> Result<RenderBitmap<'static>, RenderError> {
        match self {
            CompressedBitmap::Jpeg {
                data,
                alpha,
                width: _,
                height: _,
            } => ruffle_render::utils::decode_define_bits_jpeg(data, alpha.as_deref()),
            CompressedBitmap::Lossless(define_bits_lossless) => {
                ruffle_render::utils::decode_define_bits_lossless(define_bits_lossless)
            }
        }
    }
}

#[cfg(test)]
mod pixel_cache_tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

    thread_local! {
        static ALLOCATIONS: std::cell::Cell<Option<(usize, usize)>> = const { std::cell::Cell::new(None) };
    }
    fn record_bytes(bytes: usize, allocated: bool) {
        let _ = ALLOCATIONS.try_with(|state| {
            if let Some((live, peak)) = state.get() {
                let live = if allocated {
                    live + bytes
                } else {
                    live.saturating_sub(bytes)
                };
                state.set(Some((live, peak.max(live))));
            }
        });
    }
    struct ObservedAllocator;
    static WATCH: AtomicPtr<u8> = AtomicPtr::new(std::ptr::null_mut());
    static FREED: AtomicBool = AtomicBool::new(false);
    #[global_allocator]
    static ALLOCATOR: ObservedAllocator = ObservedAllocator;
    unsafe impl GlobalAlloc for ObservedAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                record_bytes(layout.size(), true);
            }
            pointer
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            if WATCH.load(Ordering::SeqCst) == ptr {
                FREED.store(true, Ordering::SeqCst);
            }
            record_bytes(layout.size(), false);
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    fn symbol<'gc>(version: u8) -> BitmapCharacter<'gc> {
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder
            .write_all(&[128, 20, 30, 40, 255, 50, 60, 70])
            .unwrap();
        BitmapCharacter::new(CompressedBitmap::Lossless(DefineBitsLossless {
            version,
            id: 1,
            format: swf::BitmapFormat::Rgb32,
            width: 2,
            height: 1,
            data: std::borrow::Cow::Owned(encoder.finish().unwrap()),
        }))
    }

    #[test]
    fn symbol_pixels_share_and_release_allocation_while_library_survives() {
        gc_arena::Arena::<gc_arena::Rootable![()]>::new(|mc| {
            let symbol = symbol(2);
            let (pixels, format) = symbol.shared_pixels(mc.metrics()).unwrap();
            assert_eq!(format, BitmapFormat::Rgba);
            assert_eq!(
                pixels.as_slice(),
                &[Color::rgba(20, 30, 40, 128), Color::rgba(50, 60, 70, 255)]
            );
            let (second, _) = symbol.shared_pixels(mc.metrics()).unwrap();
            assert!(Arc::ptr_eq(&pixels, &second));
            drop(second);
            FREED.store(false, Ordering::SeqCst);
            WATCH.store(pixels.as_ptr() as *mut u8, Ordering::SeqCst);
            drop(pixels);
            let freed = FREED.load(Ordering::SeqCst);
            WATCH.store(std::ptr::null_mut(), Ordering::SeqCst);
            assert!(
                freed,
                "Library weak cache must release the actual pixel allocation"
            );
            assert!(symbol.shared_pixels.borrow().is_some());
            assert!(symbol
                .shared_pixels
                .borrow()
                .as_ref()
                .unwrap()
                .0
                .upgrade()
                .is_none());
            assert_eq!(
                symbol.shared_pixels(mc.metrics()).unwrap().0[0],
                Color::rgba(20, 30, 40, 128)
            );
        });
    }

    #[test]
    fn large_paletted_symbol_decodes_with_one_rgba_allocation() {
        gc_arena::Arena::<gc_arena::Rootable![()]>::new(|mc| {
            let (width, height) = (1927u16, 1306u16);
            let palette = [
                [0, 0, 0, 0],
                [20, 30, 40, 128],
                [90, 10, 20, 255],
                [8, 9, 10, 32],
                [10, 20, 30, 255],
            ];
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            for color in palette {
                encoder.write_all(&color).unwrap();
            }
            let mut row = vec![0xfe; (width as usize + 3) & !3];
            for (x, index) in row[..width as usize].iter_mut().enumerate() {
                *index = (x % 5) as u8;
            }
            for _ in 0..height {
                encoder.write_all(&row).unwrap();
            }
            let symbol = BitmapCharacter::new(CompressedBitmap::Lossless(DefineBitsLossless {
                version: 2,
                id: 1,
                format: swf::BitmapFormat::ColorMap8 { num_colors: 4 },
                width,
                height,
                data: std::borrow::Cow::Owned(encoder.finish().unwrap()),
            }));
            ALLOCATIONS.with(|state| state.set(Some((0, 0))));
            let (pixels, format) = symbol.shared_pixels(mc.metrics()).unwrap();
            let (_, peak) = ALLOCATIONS.with(|state| state.replace(None).unwrap());
            let rgba_bytes = width as usize * height as usize * 4;
            assert!(peak < rgba_bytes + 256 * 1024, "Decode peak {peak} must contain one RGBA buffer plus bounded inflate/row state, not a full indexed or second RGBA buffer");
            assert_eq!(format, BitmapFormat::Rgba);
            assert_eq!(mc.metrics().total_external_allocation(), rgba_bytes);
            for y in [0, 1, height as usize - 1] {
                for x in 0..width as usize {
                    let c = palette[x % 5];
                    assert_eq!(
                        pixels[y * width as usize + x],
                        Color::rgba(c[0], c[1], c[2], c[3])
                    );
                }
            }
            println!(
                "Large paletted decode: RGBA bytes={rgba_bytes}, actual allocator peak={peak}"
            );
            drop(pixels);
            assert_eq!(mc.metrics().total_external_allocation(), 0);
        });
    }

    #[test]
    fn symbol_bitmap_preserves_decoded_dimensions_and_transparency() {
        gc_arena::Arena::<gc_arena::Rootable![()]>::new(|mc| {
            for version in [1, 2] {
                let symbol = symbol(version);
                let data = symbol.bitmap_data(mc).unwrap();
                assert_eq!((data.width(), data.height()), (2, 1));
                assert_eq!(
                    data.transparency(),
                    symbol.compressed.decode().unwrap().format() == BitmapFormat::Rgba
                );
                let (pixels, _) = symbol.shared_pixels(mc.metrics()).unwrap();
                assert_eq!(pixels[0].alpha(), if version == 1 { 255 } else { 128 });
            }
            let opaque = BitmapCharacter::new(CompressedBitmap::Jpeg {
                data: include_bytes!("testdata/opaque-rgb.png").to_vec(),
                alpha: None,
                width: 2,
                height: 1,
            });
            let data = opaque.bitmap_data(mc).unwrap();
            assert!(!data.transparency());
            assert_eq!(
                opaque.shared_pixels(mc.metrics()).unwrap().0[0],
                Color::rgba(20, 30, 40, 255)
            );
        });
    }
}
