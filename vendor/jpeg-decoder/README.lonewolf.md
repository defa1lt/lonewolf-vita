This is the published jpeg-decoder 0.3.2 crate from image-rs.
Upstream: https://github.com/image-rs/jpeg-decoder
Crates.io package checksum: 00810f1d8b74be64b13dbf3db89ac67740615d6c891f0e7b6179326533011a07
Licensed MIT OR Apache-2.0; see LICENSE-MIT and LICENSE-APACHE.

The only library change is in src/decoder.rs: on target_os="vita", select
Immediate instead of creating one thread per JPEG component. Those thread
stacks are allocated outside the reserved newlib heap, where VitaGL leaves only
4 MiB. Existing synchronous decoding performs the same image conversion on
the calling thread. Other targets retain upstream worker selection.

No game assets are included. Upstream tests and benchmark fixtures are unchanged.
