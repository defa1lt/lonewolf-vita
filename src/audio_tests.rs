use ruffle_core::backend::audio::AudioMixer;
use ruffle_core::swf::{self, AudioCompression, SoundEvent, SoundInfo, Tag};

#[test]
fn game_mp3_sounds_decode_and_mix_at_vita_sample_rate() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("build/Lonewolf-air.swf");
    let bytes = std::fs::read(path).expect("Prepared verified local game is required");
    let data = swf::decompress_swf(&bytes[..]).expect("SWF decompresses");
    let movie = swf::parse_swf(&data).expect("SWF parses");
    let mut mixer = AudioMixer::new(2, 48_000);
    let proxy = mixer.proxy();
    let settings = SoundInfo {
        event: SoundEvent::Event,
        in_sample: None,
        out_sample: None,
        num_loops: 1,
        envelope: None,
    };
    let mut sounds = 0;
    let mut non_silent = 0;
    for tag in &movie.tags {
        let Tag::DefineSound(sound) = tag else {
            continue;
        };
        assert_eq!(sound.format.compression, AudioCompression::Mp3);
        let handle = mixer.register_sound(sound).expect("Embedded MP3 registers");
        assert!(mixer.get_sound_duration(handle).unwrap() > 0.0);
        let instance = mixer.start_sound(handle, &settings).expect("MP3 starts");
        let mut samples = [0_i16; 2048];
        let mut audible = false;
        for _ in 0..8 {
            proxy.mix(&mut samples);
            audible |= samples.iter().any(|&sample| sample != 0);
        }
        if let Some(position) = mixer.get_sound_position(instance) {
            assert!(
                position > 0.0,
                "Decoder position must advance with PCM output"
            );
        }
        non_silent += usize::from(audible);
        mixer.stop_all_sounds();
        proxy.mix(&mut samples);
        assert!(samples.iter().all(|&sample| sample == 0));
        assert!(mixer.get_sound_position(instance).is_none());
        sounds += 1;
    }
    assert_eq!(sounds, 175, "Exercise every sound in the verified game");
    assert!(non_silent > 0, "Real MP3 decoding must produce nonzero PCM");
    println!("Decoded {sounds} embedded MP3 sounds at 48000 Hz; {non_silent} non-silent previews");
}
