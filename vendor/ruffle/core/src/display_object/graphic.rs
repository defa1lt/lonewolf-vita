use crate::avm1::Object as Avm1Object;
use crate::avm2::{
    Activation as Avm2Activation, ClassObject as Avm2ClassObject, StageObject as Avm2StageObject,
};
use crate::context::{RenderContext, UpdateContext};
use crate::display_object::DisplayObjectBase;
use crate::drawing::Drawing;
use crate::library::MovieLibrarySource;
use crate::prelude::*;
use crate::tag_utils::{SwfMovie, SwfSlice};
use crate::utils::HasPrefixField;
use crate::vminterface::Instantiator;
use core::fmt;
use gc_arena::barrier::unlock;
use gc_arena::lock::Lock;
use gc_arena::{Collect, Gc, Mutation};
use ruffle_render::backend::{ShapeHandle, ShapeHandleImpl};
use ruffle_render::commands::CommandHandler;
use std::cell::{Cell, OnceCell, RefCell, RefMut};
use std::sync::{Arc, Weak};
use swf::extensions::ReadSwfExt;

#[derive(Clone, Collect, Copy)]
#[collect(no_drop)]
pub struct Graphic<'gc>(Gc<'gc, GraphicData<'gc>>);

impl fmt::Debug for Graphic<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Graphic")
            .field("ptr", &Gc::as_ptr(self.0))
            .finish()
    }
}

#[derive(Clone, Collect, HasPrefixField)]
#[collect(no_drop)]
#[repr(C, align(8))]
pub struct GraphicData<'gc> {
    base: DisplayObjectBase<'gc>,
    shared: Lock<Gc<'gc, GraphicShared>>,
    class: Lock<Option<Avm2ClassObject<'gc>>>,
    avm2_object: Lock<Option<Avm2StageObject<'gc>>>,
    /// This is lazily allocated on demand, to make `GraphicData` smaller in the common case.
    #[collect(require_static)]
    drawing: OnceCell<Box<RefCell<Drawing>>>,
    #[collect(require_static)]
    hit_shape: RefCell<Option<Arc<swf::Shape>>>,
    #[collect(require_static)]
    render_handle: RefCell<Option<ShapeHandle>>,
}

impl<'gc> Graphic<'gc> {
    /// Keep packed geometry shared until rendering or a shape hit-test needs it.
    pub fn from_swf_slice(
        context: &mut UpdateContext<'gc>,
        source: SwfSlice,
        version: u8,
    ) -> Result<Self, swf::error::Error> {
        let shared = GraphicShared::from_swf_slice(source, version)?;

        Ok(Graphic(Gc::new(
            context.gc(),
            GraphicData {
                base: Default::default(),
                shared: Lock::new(Gc::new(context.gc(), shared)),
                class: Lock::new(None),
                avm2_object: Lock::new(None),
                drawing: OnceCell::new(),
                hit_shape: RefCell::new(None),
                render_handle: RefCell::new(None),
            },
        )))
    }

    /// Construct an empty `Graphic`.
    pub fn empty(context: &mut UpdateContext<'gc>) -> Self {
        let shared = GraphicShared {
            id: 0,
            bounds: Default::default(),
            render_handle: RefCell::new(None),
            source: None,
            shape: RefCell::new(Weak::new()),
            parse_failed: Cell::new(false),
            movie: context.root_swf.clone(),
        };

        Graphic(Gc::new(
            context.gc(),
            GraphicData {
                base: Default::default(),
                shared: Lock::new(Gc::new(context.gc(), shared)),
                class: Lock::new(None),
                avm2_object: Lock::new(None),
                drawing: OnceCell::new(),
                hit_shape: RefCell::new(None),
                render_handle: RefCell::new(None),
            },
        ))
    }

    pub fn drawing_mut(&self) -> RefMut<'_, Drawing> {
        self.0.hit_shape.borrow_mut().take();
        self.0.render_handle.borrow_mut().take();
        self.0.drawing.get_or_init(Default::default).borrow_mut()
    }

    pub fn set_avm2_class(self, mc: &Mutation<'gc>, class: Avm2ClassObject<'gc>) {
        unlock!(Gc::write(mc, self.0), GraphicData, class).set(Some(class));
    }

    fn set_shared(self, mc: &Mutation<'gc>, shared: Gc<'gc, GraphicShared>) {
        // Acquire the incoming resources before releasing the outgoing instance.
        let (shape, handle) = if self.0.drawing.get().is_none() {
            (
                shared.shape.borrow().upgrade(),
                shared.cached_render_handle(),
            )
        } else {
            (None, None)
        };
        *self.0.hit_shape.borrow_mut() = shape;
        *self.0.render_handle.borrow_mut() = handle;
        unlock!(Gc::write(mc, self.0), GraphicData, shared).set(shared);
    }
}

impl<'gc> TDisplayObject<'gc> for Graphic<'gc> {
    fn base(self) -> Gc<'gc, DisplayObjectBase<'gc>> {
        HasPrefixField::as_prefix_gc(self.0)
    }

    fn instantiate(self, gc_context: &Mutation<'gc>) -> DisplayObject<'gc> {
        let instance = Self(Gc::new(gc_context, self.0.as_ref().clone()));
        instance.set_shared(gc_context, self.0.shared.get());
        instance.into()
    }

    fn id(self) -> CharacterId {
        self.0.shared.get().id
    }

    fn self_bounds(self) -> Rectangle<Twips> {
        if let Some(drawing) = self.0.drawing.get() {
            drawing.borrow().self_bounds()
        } else {
            self.0.shared.get().bounds
        }
    }

    fn construct_frame(self, context: &mut UpdateContext<'gc>) {
        if self.movie().is_action_script_3() && self.object2().is_none() {
            let class_object = self
                .0
                .class
                .get()
                .unwrap_or_else(|| context.avm2.classes().shape);

            let mut activation = Avm2Activation::from_nothing(context);

            match Avm2StageObject::for_display_object_childless(
                &mut activation,
                self.into(),
                class_object,
            ) {
                Ok(object) => self.set_object2(activation.context, object),
                Err(e) => {
                    tracing::error!("Got error when constructing AVM2 side of shape: {}", e)
                }
            }

            self.on_construction_complete(context);
        }
    }

    fn replace_with(self, context: &mut UpdateContext<'gc>, id: CharacterId) {
        // Static assets like Graphics can replace themselves via a PlaceObject tag with PlaceObjectAction::Replace.
        // This does not create a new instance, but instead swaps out the underlying static data to point to the new art.
        if let Some(new_graphic) = context
            .library
            .library_for_movie_mut(self.movie())
            .get_graphic(id)
        {
            self.set_shared(context.gc(), new_graphic.0.shared.get());
        } else {
            tracing::warn!("PlaceObject: expected Graphic at character ID {}", id);
        }
        self.invalidate_cached_bitmap();
    }

    fn render_self(self, context: &mut RenderContext) {
        if !context.is_offscreen && !self.world_bounds().intersects(&context.stage.view_bounds()) {
            // Off-screen; culled
            return;
        }

        if let Some(drawing) = self.0.drawing.get() {
            drawing.borrow().render(context);
        } else {
            let mut handle = self.0.render_handle.borrow_mut();
            if handle.is_none() {
                *handle = self.0.shared.get().render_handle(context);
            }
            if let Some(handle) = handle.as_ref() {
                context
                    .commands
                    .render_shape(handle.clone(), context.transform_stack.transform());
            }
        }
    }

    fn hit_test_shape(
        self,
        _context: &mut UpdateContext<'gc>,
        point: Point<Twips>,
        options: HitTestOptions,
    ) -> bool {
        // Transform point to local coordinates and test.
        if (!options.contains(HitTestOptions::SKIP_INVISIBLE) || self.visible())
            && self.world_bounds().contains(point)
        {
            let Some(local_matrix) = self.global_to_local_matrix() else {
                return false;
            };
            let point = local_matrix * point;
            if let Some(drawing) = self.0.drawing.get() {
                if drawing.borrow().hit_test(point, &local_matrix) {
                    return true;
                }
            } else {
                let mut shape = self.0.hit_shape.borrow_mut();
                if shape.is_none() {
                    *shape = self.0.shared.get().shape();
                }
                if let Some(shape) = shape.as_ref() {
                    return ruffle_render::shape_utils::shape_hit_test(shape, point, &local_matrix);
                }
            }
        }

        false
    }

    fn post_instantiation(
        self,
        context: &mut UpdateContext<'gc>,
        _init_object: Option<Avm1Object<'gc>>,
        _instantiated_by: Instantiator,
        _run_frame: bool,
    ) {
        if self.movie().is_action_script_3() {
            self.set_default_instance_name(context);
        }
    }

    fn movie(self) -> Arc<SwfMovie> {
        self.0.shared.get().movie.clone()
    }

    fn object1(self) -> Option<Avm1Object<'gc>> {
        None
    }

    fn object2(self) -> Option<Avm2StageObject<'gc>> {
        self.0.avm2_object.get()
    }

    fn set_object2(self, context: &mut UpdateContext<'gc>, to: Avm2StageObject<'gc>) {
        let mc = context.gc();
        unlock!(Gc::write(mc, self.0), GraphicData, avm2_object).set(Some(to));
    }

    fn as_drawing(&self) -> Option<RefMut<'_, Drawing>> {
        Some(self.drawing_mut())
    }
}

/// Data shared between all instances of a Graphic.
#[derive(Collect)]
#[collect(require_static)]
struct GraphicShared {
    id: CharacterId,
    source: Option<(SwfSlice, u8)>,
    shape: RefCell<Weak<swf::Shape>>,
    parse_failed: Cell<bool>,
    render_handle: RefCell<Option<Weak<dyn ShapeHandleImpl>>>,
    bounds: Rectangle<Twips>,
    movie: Arc<SwfMovie>,
}

impl GraphicShared {
    fn from_swf_slice(source: SwfSlice, version: u8) -> Result<Self, swf::error::Error> {
        let mut reader = source.read_from(0);
        let id = reader.read_u16()?;
        let bounds = reader.read_rectangle()?;
        Ok(Self {
            id,
            bounds,
            movie: source.movie.clone(),
            source: Some((source, version)),
            shape: RefCell::new(Weak::new()),
            parse_failed: Cell::new(false),
            render_handle: RefCell::new(None),
        })
    }

    fn shape(&self) -> Option<Arc<swf::Shape>> {
        if self.parse_failed.get() {
            return None;
        }
        if let Some(shape) = self.shape.borrow().upgrade() {
            return Some(shape);
        }
        let shape = if let Some((source, version)) = self.source.as_ref() {
            match source.read_from(0).read_define_shape(*version) {
                Ok(shape) => shape,
                Err(error) => {
                    tracing::error!("Error decoding shape {}: {}", self.id, error);
                    self.parse_failed.set(true);
                    return None;
                }
            }
        } else {
            swf::Shape {
                version: 32,
                id: 0,
                shape_bounds: Default::default(),
                edge_bounds: Default::default(),
                flags: swf::ShapeFlag::empty(),
                styles: swf::ShapeStyles {
                    fill_styles: Vec::new(),
                    line_styles: Vec::new(),
                },
                shape: Vec::new(),
            }
        };
        let shape = Arc::new(shape);
        *self.shape.borrow_mut() = Arc::downgrade(&shape);
        Some(shape)
    }

    fn cached_render_handle(&self) -> Option<ShapeHandle> {
        self.render_handle
            .borrow()
            .as_ref()
            .and_then(Weak::upgrade)
            .map(ShapeHandle)
    }

    fn render_handle(&self, context: &mut RenderContext<'_, '_>) -> Option<ShapeHandle> {
        if let Some(handle) = self.cached_render_handle() {
            return Some(handle);
        }
        let shape = self.shape()?;
        let library = context
            .library
            .library_for_movie(self.movie.clone())
            .unwrap();
        let handle = context
            .renderer
            .register_shape(shape.as_ref().into(), &MovieLibrarySource { library });
        *self.render_handle.borrow_mut() = Some(Arc::downgrade(&handle.0));
        Some(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(version: u8) -> (SwfSlice, swf::Shape) {
        let bounds = Rectangle {
            x_min: Twips::ZERO,
            y_min: Twips::ZERO,
            x_max: Twips::from_pixels(40.0),
            y_max: Twips::from_pixels(30.0),
        };
        let mut records = vec![swf::ShapeRecord::StyleChange(Box::new(
            swf::StyleChangeData {
                move_to: Some(Point::from_pixels(0.0, 0.0)),
                fill_style_0: Some(1),
                fill_style_1: None,
                line_style: None,
                new_styles: None,
            },
        ))];
        for (x, y) in [(40.0, 0.0), (0.0, 30.0), (-40.0, 0.0), (0.0, -30.0)] {
            records.push(swf::ShapeRecord::StraightEdge {
                delta: swf::PointDelta::from_pixels(x, y),
            });
        }
        let shape = swf::Shape {
            version,
            id: 7,
            shape_bounds: bounds,
            edge_bounds: bounds,
            flags: swf::ShapeFlag::HAS_NON_SCALING_STROKES,
            styles: swf::ShapeStyles {
                fill_styles: vec![swf::FillStyle::Color(Color {
                    r: 90,
                    g: 30,
                    b: 10,
                    a: 255,
                })],
                line_styles: vec![],
            },
            shape: records,
        };
        let mut bytes = Vec::new();
        swf::write_swf(
            &swf::Header::default_with_swf_version(10),
            &[swf::Tag::DefineShape(Box::new(shape)), swf::Tag::End],
            &mut bytes,
        )
        .unwrap();
        let movie =
            Arc::new(SwfMovie::from_data(&bytes, "file:///shape.swf".into(), None).unwrap());
        let source = SwfSlice::from(movie);
        let mut reader = source.read_from(0);
        let (_, length) = reader.read_tag_code_and_length().unwrap();
        let source = source.to_subslice(&reader.get_ref()[..length]);
        let original = source.read_from(0).read_define_shape(version).unwrap();
        (source, original)
    }

    #[test]
    fn packed_geometry_preserves_all_versions_and_hit_tests() {
        for version in 1..=4 {
            let (source, original) = fixture(version);
            let movie = source.movie.clone();
            let shared = GraphicShared::from_swf_slice(source, version).unwrap();
            assert_eq!(shared.id, original.id);
            assert_eq!(shared.bounds, original.shape_bounds);
            assert!(
                shared.shape.borrow().upgrade().is_none(),
                "Bounds must not expand geometry"
            );
            assert!(Arc::ptr_eq(
                &shared.source.as_ref().unwrap().0.movie,
                &movie
            ));
            let decoded = shared.shape().unwrap();
            assert_eq!(decoded.as_ref(), &original);
            assert!(
                Arc::ptr_eq(&decoded, &shared.shape().unwrap()),
                "Live instances share decoded geometry"
            );
            let matrix = Matrix::default();
            for (x, y, expected) in [(20.0, 15.0, true), (-5.0, 15.0, false), (45.0, 15.0, false)] {
                let hit = ruffle_render::shape_utils::shape_hit_test(
                    &decoded,
                    Point::from_pixels(x, y),
                    &matrix,
                );
                assert_eq!(hit, expected, "DefineShape{version} at ({x},{y})");
            }
            drop(decoded);
            assert!(
                shared.shape.borrow().upgrade().is_none(),
                "Unused geometry must not remain in the library"
            );
            assert_eq!(
                shared.shape().unwrap().as_ref(),
                &original,
                "Released geometry can be decoded again exactly"
            );
        }
    }

    #[test]
    fn timeline_instances_acquire_live_cached_resources_before_replacement() {
        let (source, original) = fixture(4);
        let player = crate::PlayerBuilder::new().build();
        player
            .lock()
            .unwrap()
            .mutate_with_update_context(|context| {
                let prototype = Graphic::from_swf_slice(context, source, 4).unwrap();
                let shared = prototype.0.shared.get();
                let shape = shared.shape().unwrap();
                let handle = context.renderer.register_shape(
                    (&original).into(),
                    &ruffle_render::backend::null::NullBitmapSource,
                );
                *shared.render_handle.borrow_mut() = Some(Arc::downgrade(&handle.0));
                let DisplayObject::Graphic(first) = prototype.instantiate(context.gc()) else {
                    panic!("Expected Graphic")
                };
                drop(shape);
                drop(handle);
                assert!(
                    shared.cached_render_handle().is_some(),
                    "New instance must own the existing mesh before the old owner can be collected"
                );
                assert!(first.0.hit_shape.borrow().is_some());
                let DisplayObject::Graphic(second) = prototype.instantiate(context.gc()) else {
                    panic!("Expected Graphic")
                };
                first.0.render_handle.borrow_mut().take();
                first.0.hit_shape.borrow_mut().take();
                second.set_shared(context.gc(), shared);
                assert!(
                    second.0.render_handle.borrow().is_some(),
                    "Replacing with the same symbol must not drop its last mesh owner"
                );
                assert!(second.0.hit_shape.borrow().is_some());
                second.drawing_mut();
                assert!(
                    shared.cached_render_handle().is_none(),
                    "Unused mesh must still be released"
                );
                assert!(shared.shape.borrow().upgrade().is_none());
            });
    }

    #[test]
    fn malformed_geometry_failure_is_cached_after_valid_bounds() {
        let (source, _) = fixture(4);
        let mut reader = source.read_from(0);
        reader.read_u16().unwrap();
        reader.read_rectangle().unwrap();
        let header_length = source.len() - reader.get_ref().len();
        let source = source.to_start_and_end(0, header_length);
        let shared = GraphicShared::from_swf_slice(source, 4).unwrap();
        assert!(shared.shape.borrow().upgrade().is_none());
        assert!(shared.shape().is_none());
        assert!(shared.parse_failed.get());
        assert!(shared.shape().is_none());
    }
}
