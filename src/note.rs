//! 音名と周波数の変換。音名は科学的音高表記（A4 = 440 Hz、C4 = 中央ド）。

const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

#[derive(Clone, Debug, PartialEq)]
pub struct Pitch {
    pub hz: f64,
    /// 音名で入力されたときの MIDI ノート番号
    pub midi: Option<i32>,
}

/// "A3", "C#4", "Bb2", "220", "220.5 Hz" などを解釈する。
pub fn parse_pitch(input: &str, a4_hz: f64) -> Result<Pitch, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("基音を入力してください（例: A3, C#4, 220）".into());
    }
    let lower = s.to_ascii_lowercase();
    let num = lower.trim_end_matches("hz").trim();
    if let Ok(hz) = num.parse::<f64>() {
        if hz.is_finite() && hz > 0.0 {
            return Ok(Pitch { hz, midi: None });
        }
        return Err("周波数は正の数にしてください".into());
    }

    let mut chars = s.chars().peekable();
    let letter = chars.next().unwrap().to_ascii_uppercase();
    let mut semitone: i32 = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(format!("「{s}」を音名または周波数として読めません（例: A3, C#4, 220）")),
    };
    while let Some(&c) = chars.peek() {
        match c {
            '#' | '♯' => semitone += 1,
            'b' | '♭' => semitone -= 1,
            _ => break,
        }
        chars.next();
    }
    let rest: String = chars.collect();
    let octave: i32 = rest
        .trim()
        .parse()
        .map_err(|_| format!("「{s}」のオクターブ番号が読めません（例: A3, C#4）"))?;
    let midi = 12 * (octave + 1) + semitone;
    Ok(Pitch { hz: midi_to_hz(midi as f64, a4_hz), midi: Some(midi) })
}

pub fn midi_to_hz(midi: f64, a4_hz: f64) -> f64 {
    a4_hz * 2f64.powf((midi - 69.0) / 12.0)
}

pub fn midi_name(midi: i32) -> String {
    format!("{}{}", NAMES[midi.rem_euclid(12) as usize], midi.div_euclid(12) - 1)
}

/// 最も近い音名と、そこからのずれ（cent）。
pub fn nearest_note(hz: f64, a4_hz: f64) -> (String, f64) {
    let m = 69.0 + 12.0 * (hz / a4_hz).log2();
    let r = m.round();
    (midi_name(r as i32), (m - r) * 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_numbers() {
        assert!((parse_pitch("A4", 440.0).unwrap().hz - 440.0).abs() < 1e-9);
        assert!((parse_pitch("a3", 440.0).unwrap().hz - 220.0).abs() < 1e-9);
        assert!((parse_pitch("C4", 440.0).unwrap().hz - 261.6256).abs() < 1e-3);
        assert_eq!(parse_pitch("C#4", 440.0).unwrap().midi, Some(61));
        assert_eq!(parse_pitch("Db4", 440.0).unwrap().midi, Some(61));
        assert!((parse_pitch("220 Hz", 440.0).unwrap().hz - 220.0).abs() < 1e-9);
        assert!(parse_pitch("H2", 440.0).is_err());
        assert!(parse_pitch("-5", 440.0).is_err());
        assert_eq!(nearest_note(220.0, 440.0).0, "A3");
    }
}
