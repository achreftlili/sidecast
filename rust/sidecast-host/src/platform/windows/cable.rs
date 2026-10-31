//! Recognizing a virtual audio cable from endpoint names. Pure, so it is
//! tested on every OS; `mic.rs` does the WASAPI work.
//!
//! A virtual cable such as VB-Audio's VB-CABLE has a playback endpoint
//! ("CABLE Input (VB-Audio Virtual Cable)") whose sound comes out of a
//! recording endpoint ("CABLE Output (VB-Audio Virtual Cable)"). Sidecast
//! plays the phone's microphone into the first, and apps record the second.

/// Whether a playback endpoint is the input side of a virtual cable.
pub fn is_cable_input(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("cable") && lower.contains("input") && !lower.contains("output")
}

/// The recording endpoint that carries what `input_name` plays: "Input" → "Output".
pub fn cable_output_name(input_name: &str) -> String {
    match input_name.to_ascii_lowercase().find("input") {
        Some(i) => format!("{}Output{}", &input_name[..i], &input_name[i + "input".len()..]),
        None => input_name.to_owned(),
    }
}

/// The best cable among playback endpoint names: VB-CABLE's own "CABLE Input" first.
pub fn pick_cable<'a>(playback: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let mut candidates: Vec<&str> = playback.into_iter().filter(|n| is_cable_input(n)).collect();
    candidates.sort_by_key(|n| !n.to_ascii_lowercase().starts_with("cable input"));
    candidates.first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_vb_cable_and_its_recording_side() {
        let playback = [
            "Speakers (Realtek(R) Audio)",
            "CABLE-A Input (VB-Audio Cable A)",
            "CABLE Input (VB-Audio Virtual Cable)",
            "VoiceMeeter Input (VB-Audio VoiceMeeter VAIO)",
        ];
        let cable = pick_cable(playback).unwrap();
        assert_eq!(cable, "CABLE Input (VB-Audio Virtual Cable)");
        assert_eq!(cable_output_name(cable), "CABLE Output (VB-Audio Virtual Cable)");
        assert_eq!(cable_output_name("CABLE-A Input (VB-Audio Cable A)"), "CABLE-A Output (VB-Audio Cable A)");
        assert!(pick_cable(["Speakers (Realtek(R) Audio)", "Headphones"]).is_none());
        assert!(!is_cable_input("CABLE Output (VB-Audio Virtual Cable)"));
    }
}
