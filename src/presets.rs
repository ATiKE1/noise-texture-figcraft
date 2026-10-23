//! Built-in presets, and the ones the user saves (kept in the plugin's settings).

use figcraft_plugin_sdk::{Value, json};

use crate::texture::Settings;

/// The preset list's first entry: the fields as they are.
pub const CUSTOM: &str = "Custom";

/// Saved presets, at most (settings are capped at 64 KB per plugin).
pub const MAX_SAVED: usize = 40;

/// A preset name's length, at most.
pub const MAX_NAME: usize = 40;

/// The built-in presets: (name, the settings that differ from the defaults).
pub fn built_in() -> Vec<(&'static str, Value)> {
    vec![
        ("Fine grain", json!({"type": "Static", "density": 50, "color": "#000000", "opacity": 10})),
        ("Film grain", json!({"type": "Grain", "density": 100, "colorMode": "Two colours", "opacity": 35, "blend": "Overlay"})),
        ("Paper", json!({"type": "Grain", "density": 70, "color": "#5C4B37", "opacity": 18, "blend": "Multiply"})),
        ("TV static", json!({"type": "Static", "size": 2, "density": 100, "colorMode": "Two colours", "opacity": 100})),
        ("Colour static", json!({"type": "Static", "density": 70, "colorMode": "Random colours", "opacity": 45})),
        ("Speckles", json!({"type": "Static", "size": 4, "shape": "Circle", "density": 12, "opacity": 40})),
        (
            "Clouds",
            json!({"type": "Clouds", "scale": 128, "detail": 6, "density": 80, "color": "#FFFFFF", "opacity": 95,
                          "background": "#5B9BD5", "backgroundOpacity": 100}),
        ),
        (
            "Marble",
            json!({"type": "Marble", "scale": 64, "detail": 5, "density": 100, "colorMode": "Two colours",
                          "color": "#3B3A3A", "color2": "#F3F0EA", "opacity": 100}),
        ),
        (
            "Wood",
            json!({"type": "Wood", "scale": 24, "detail": 4, "density": 100, "colorMode": "Two colours",
                        "color": "#5E3A1F", "color2": "#B58351", "opacity": 100}),
        ),
        (
            "Stone",
            json!({"type": "Cells", "scale": 48, "detail": 5, "density": 100, "colorMode": "Two colours",
                         "color": "#45423F", "color2": "#ADA79E", "opacity": 100}),
        ),
        ("Ripples", json!({"type": "Waves", "scale": 16, "detail": 5, "density": 100, "color": "#1E5AA8", "opacity": 55})),
        ("Halftone", json!({"type": "Halftone", "scale": 8, "detail": 6, "density": 40, "opacity": 100})),
    ]
}

/// The built-in preset `name` (any case).
pub fn find_built_in(name: &str) -> Option<Settings> {
    built_in().into_iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).map(|(_, v)| Settings::from_json(&v))
}

/// The names of the built-in presets.
pub fn built_in_names() -> Vec<&'static str> {
    built_in().into_iter().map(|(n, _)| n).collect()
}

/// A preset name typed by the user, tidied (spaces collapsed), or why it can't be used: it's
/// empty, too long or a built-in preset's.
pub fn clean_name(name: &str) -> core::result::Result<String, String> {
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("Type a name for the preset first".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("A preset name can be {MAX_NAME} characters at most"));
    }
    if name.eq_ignore_ascii_case(CUSTOM) || find_built_in(&name).is_some() {
        return Err(format!("“{name}” is a built-in preset: pick another name"));
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::texture::{Kind, render};

    #[test]
    fn built_in_presets_are_valid_and_distinct() {
        let all = built_in();
        for (i, (name, v)) in all.iter().enumerate() {
            let s = Settings::from_json(v);
            // Every field a preset names parses as given (nothing falls back to the default).
            for (k, given) in v.as_object().into_iter().flatten() {
                let got = &s.to_json()[k.as_str()];
                let same = got == given || got.as_f64().zip(given.as_f64()).is_some_and(|(a, b)| a == b);
                assert!(same, "{name}.{k}: {given} became {got}");
            }
            assert!(all.iter().skip(i + 1).all(|(n, w)| n != name && w != v), "{name} is listed twice");
            let t = render(&Settings { tile: 64, ..s });
            assert!(t.pixels.iter().any(|p| p >> 24 > 0), "{name} draws nothing");
        }
        assert_eq!(find_built_in("marble").map(|s| s.kind), Some(Kind::Marble));
        assert!(find_built_in("Plasma").is_none());
    }

    #[test]
    fn preset_names_are_tidied_and_checked() {
        assert_eq!(clean_name("  my   grain "), Ok("my grain".into()));
        assert!(clean_name("   ").is_err());
        assert!(clean_name("film GRAIN").is_err());
        assert!(clean_name("custom").is_err());
        assert!(clean_name(&"x".repeat(MAX_NAME + 1)).is_err());
        assert!(clean_name(&"é".repeat(MAX_NAME)).is_ok());
    }
}
