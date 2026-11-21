//! 試験音 WAV を test_tones/ に書き出す。
//! cargo run --example gen_test_tones

use harmonic_analyzer::audio_file::write_wav_mono;
use harmonic_analyzer::test_tones::{SR, all};
use std::path::Path;

fn main() {
    let dir = Path::new("test_tones");
    std::fs::create_dir_all(dir).unwrap();
    for (name, samples) in all() {
        let p = dir.join(name);
        write_wav_mono(&p, &samples, SR).unwrap();
        println!("{}", p.display());
    }
}
