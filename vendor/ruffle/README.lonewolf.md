Ruffle core is vendored from upstream commit
563854a8911eda672b1836fb374063162879035d under MIT OR Apache-2.0;
see LICENSE.md. No game assets are included.

Static DefineShape payloads remain packed in the existing SwfSlice until
rendering or exact shape hit testing needs the original SWF parser. Live Graphic
instances share decoded geometry and renderer handles through weak library
caches. Removing the last instance lets its geometry and mesh buffers be freed;
returning to the symbol recreates them. Drawing and symbol replacement invalidate
the instance caches. Parse failures are logged and remembered. The original
bounds, masks, curves and hit tests remain unchanged.
Instantiation and replacement acquire an existing weak-cached resource before
releasing the outgoing owner, avoiding a cache gap at timeline transitions.
The lifecycle regression also checks that unused resources are still released.

BitmapData stores uniform fills without a full CPU buffer and shares decoded
symbol pixels through Arc<PixelBuffer> until the first write. Mutations and GPU
readback detach before changing pixels; clones remain independent. PixelBuffer reports capacity to the owning arena metrics exactly once per
physical buffer. Clone/COW count new storage; disposal and GC drop remove it. The weak
symbol cache releases the Vec's pixel allocation when its last owner disappears.
GPU bitmap-fill textures retain the existing library cache. CPU lifetime tests
observe actual deallocation; a capture-feature test exercises real Metal draws,
GPU readback and cloned/shared bitmap independence without opening a window.

Cargo workspace fields retain their upstream values. Dependencies outside core
point to the same upstream revision; core, render, common, macros and build_playerglobal
remain local. The AS3 compiler and playerglobal sources are upstream files.

core/src/library.rs also logs the complete bitmap decoder error, rather than
only the generic format label, so device failures retain their cause.

The embedding API exports Avm2Activation/Avm2Value for read-only game/HUD queries.
NullNavigatorBackend supports a package root independent of the stable virtual
file URL, including Vita device paths, while sharing its existing file response.
The frontend retains and pumps its executor outside player locks. Loader and
URLLoader fetch failures already dispatch ActionScript error events; these two
recoverable paths now log warnings instead of errors. AVM/ABC failures keep their
original severity.

The input bridge adds GC-rooted contacts and TouchBegin/Move/End player events,
uses the existing interactive hit test at each contact position, bubbles real
TouchEvent objects, and synthesizes taps/over/out and primary mouse input. Native
TouchEvent.stageX/stageY reuse MouseEvent coordinate conversion. Focus loss releases
contacts without activating a tap or mouse click. UpdateContext exposes the same
viewport hit test so physical bindings can select unoccluded targets. Gesture,
proximity and touch-roll events are not implemented by this bridge.

PlayerBuilder exposes upstream gc-arena pacing before movie initialization.
Defaults remain unchanged; Lonewolf uses the same smaller sleep factor and
faster incremental work as Sierra 7 to limit collectible AVM garbage. The heap
remains 240 MiB; no allocator fallback or emergency collection is introduced.

The pinned render crate is local so core and GPU backends use one lossless decoder.
It expands paletted and 15-bit images one row at a time into the caller's final
RGBA storage; RGB32 swizzles in place. Core supplies its Vec<Color> allocation
through a safe Pod byte slice, preserving premultiplied bytes and row padding.
There is no Vec allocation-layout reinterpretation or allocation-failure GC.
