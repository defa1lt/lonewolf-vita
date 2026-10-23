This renderer is copied from RuffleVita commit
08d719ac9b8938dda3bcb8ac1229769d7a418c05, directory
`emulator/ruffle_render_glow`. Its manifest declares MIT OR Apache-2.0 and its
source identifies its origin in Ruffle's WebGL renderer. See `LICENSE.md`.

Local changes: remove the four optional `rv_prof` scopes and their dependency;
pin glow to aa4238a60d17076b917eaa1ec662e5080df42b0a, matching the source project's
lockfile. No application frontend or profiler code is included.

The native loading screen uses `last_frame_had_content` to wait for movie artwork.
Clear, letterbox rectangles, stencil-only passes and zero-alpha transforms do
not mark a frame ready. `src/startup_tests.rs` exercises that decision with a
real macOS CGL context and the renderer's offscreen MSAA target; final window
presentation is omitted because the context has no drawable window.
