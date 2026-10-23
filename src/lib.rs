//! Noise Texture for FigCraft: seamless tiled noise, grain and procedural textures as image fills.
//!
//! The plugin draws a texture tile itself (see [`texture`]), encodes it as a PNG and hands it to
//! FigCraft, then adds it to the selected layers as a tiled image fill on top of their fills, or
//! makes a new textured layer when nothing is selected. Its panel (plugin API 2) has every setting
//! and the presets, and updates the selected layers' textures live; the `apply`, `preset` and
//! `remove` commands do the same from a form, a script, the CLI or MCP.
//!
//! A textured layer keeps the texture's image hash and settings in its layer data under [`KEY`], so
//! the plugin finds its own fill again (to replace, update or remove it, leaving the layer's other
//! fills alone) and the panel picks up the settings of the layer it opens on.

pub mod noise;
pub mod png;
pub mod presets;
pub mod texture;

use figcraft_plugin_sdk::serde_json::{self, Map};
use figcraft_plugin_sdk::{Event, Input, Result, Value, call, close_ui, entry, json, notify, selection, set_setting, setting, show_ui};

use presets::CUSTOM;
use texture::{BLENDS, ColorMode, Kind, Settings, Shape, TILES};

/// The layer data key of a textured layer: `{"image": hash, "settings": {...}}` as JSON text.
pub const KEY: &str = "io.github.atike1.noise-texture";

/// A new textured layer's side at least (pixels), so that the tiling shows.
const NEW_LAYER: u32 = 400;

/// Layer types without fills of their own.
const NO_FILLS: [&str; 4] = ["PAGE", "GROUP", "SLICE", "LINE"];

/// The texture as a PNG file.
pub fn texture_png(s: &Settings) -> Result<Vec<u8>> {
    let t = texture::render(s);
    png::encode(t.width, t.height, &t.pixels).ok_or_else(|| "The texture couldn't be encoded".to_string())
}

/// The tiled image paint of image `hash` with the settings' blend mode.
pub fn paint(s: &Settings, hash: &str) -> Value {
    json!({"type": "image", "image": hash, "scale_mode": "tile", "scale": 1.0, "blend": s.blend_id()})
}

/// The texture this plugin gave the layer (as `node.get` describes it): its image hash and
/// settings.
pub fn own_texture(node: &Value) -> Option<(String, Settings)> {
    let data: Value = serde_json::from_str(node.get("pluginData")?.get(KEY)?.as_str()?).ok()?;
    let hash = data.get("image")?.as_str()?.to_string();
    Some((hash, Settings::from_json(data.get("settings").unwrap_or(&Value::Null))))
}

/// The indices (bottom first) of the layer's fills that paint image `hash`.
pub fn fills_with(node: &Value, hash: &str) -> Vec<usize> {
    let Some(fills) = node.get("fills").and_then(Value::as_array) else { return Vec::new() };
    fills
        .iter()
        .enumerate()
        .filter(|(_, p)| p.get("type").and_then(Value::as_str) == Some("image") && p.get("image").and_then(Value::as_str) == Some(hash))
        .map(|(i, _)| i)
        .collect()
}

fn takes_fills(node: &Value) -> bool {
    node.get("type").and_then(Value::as_str).is_some_and(|t| !NO_FILLS.contains(&t))
}

/// `node.set` props that set (or, with `None`, clear) the layer's texture data.
fn data_props(data: Option<String>) -> Value {
    let mut entries = Map::new();
    entries.insert(KEY.to_string(), data.map_or(Value::Null, Value::String));
    json!({"pluginData": Value::Object(entries)})
}

/// Record on layer `id` that image `hash`, made with `s`, is its texture.
fn mark(id: u64, hash: &str, s: &Settings) -> Result<()> {
    let data = json!({"image": hash, "settings": s.to_json()}).to_string();
    call("node.set", &json!({"ids": [id], "props": data_props(Some(data))}))?;
    Ok(())
}

/// What [`texture_layers`] did.
#[derive(Debug, Default)]
struct Applied {
    /// The layers textured.
    ids: Vec<u64>,
    /// How many of them already had a texture from this plugin, now replaced.
    replaced: usize,
    /// How many targets can't have fills.
    skipped: usize,
    image: String,
}

/// Put the image into the file and return its hash: FigCraft takes images in as a layer, so place
/// one and delete it again (the image stays in the file once a fill uses it).
fn store_image(data: &str) -> Result<String> {
    let placed = call("image.place", &json!({"data": data, "mode": "tile", "name": "Noise texture"}))?;
    let hash = placed["image"].as_str().ok_or("FigCraft didn't say where it kept the texture")?.to_string();
    if let Some(id) = placed["id"].as_u64() {
        call("edit.delete", &json!({"ids": [id]}))?;
    }
    Ok(hash)
}

/// Give each of `targets` the texture: replace the fill this plugin gave it before, or add one on
/// top of its fills. The selection is set to `restore` afterwards.
fn texture_layers(s: &Settings, targets: &[u64], restore: &[u64]) -> Result<Applied> {
    let nodes: Vec<(u64, Value)> = targets.iter().map(|id| call("node.get", &json!({"id": id})).map(|n| (*id, n))).collect::<Result<_>>()?;
    let skipped = nodes.iter().filter(|(_, n)| !takes_fills(n)).count();
    if skipped == nodes.len() {
        return Err("The selected layers can't have fills (groups, slices, lines): select shapes, frames or text".into());
    }
    let image = store_image(&png::base64(&texture_png(s)?))?;
    let paint = paint(s, &image);
    let mut done = Applied { skipped, image: image.clone(), ..Applied::default() };
    for (id, node) in nodes.iter().filter(|(_, n)| takes_fills(n)) {
        let at = own_texture(node).and_then(|(old, _)| fills_with(node, &old).last().copied());
        match at {
            Some(index) => {
                call("fill.set", &json!({"ids": [id], "index": index, "paint": paint}))?;
                done.replaced += 1;
            }
            None => {
                call("fill.add", &json!({"ids": [id], "paint": paint}))?;
            }
        }
        mark(*id, &image, s)?;
        done.ids.push(*id);
    }
    call("select.set", &json!({"ids": restore}))?;
    Ok(done)
}

/// A new layer filled with the texture, centred in the view and selected.
fn new_layer(s: &Settings) -> Result<Value> {
    let side = f64::from(s.tile_px().max(NEW_LAYER));
    let view = call("view.get", &json!({}))?;
    let x = (view["cx"].as_f64().unwrap_or(0.0) - side / 2.0).round();
    let y = (view["cy"].as_f64().unwrap_or(0.0) - side / 2.0).round();
    let data = png::base64(&texture_png(s)?);
    let placed = call("image.place", &json!({"data": data, "mode": "tile", "name": "Noise texture", "x": x, "y": y, "width": side, "height": side}))?;
    let id = placed["id"].as_u64().ok_or("FigCraft didn't say which layer it made")?;
    let image = placed["image"].as_str().ok_or("FigCraft didn't say where it kept the texture")?.to_string();
    if s.blend_id() != "normal" {
        call("fill.set", &json!({"ids": [id], "index": 0, "blend": s.blend_id()}))?;
    }
    mark(id, &image, s)?;
    call("select.set", &json!({"ids": [id]}))?;
    Ok(json!({"layer": id, "image": image, "tile": s.tile_px()}))
}

/// Texture the selected layers (`selected`), or make a new textured layer when there are none,
/// and say what was done.
fn apply(s: &Settings, selected: &[u64]) -> Result<Value> {
    if selected.is_empty() {
        let out = new_layer(s)?;
        notify(&format!("Added a layer with a {} texture", s.kind.label().to_lowercase()));
        return Ok(out);
    }
    let done = texture_layers(s, selected, selected)?;
    let n = done.ids.len();
    let mut message = match (n, done.replaced) {
        (1, 1) => "Updated the texture".to_string(),
        (1, _) => "Added the texture".to_string(),
        (n, r) if r == n => format!("Updated the texture on {n} layers"),
        (n, _) => format!("Added the texture to {n} layers"),
    };
    if done.skipped > 0 {
        message.push_str(&format!(" ({} without fills skipped)", done.skipped));
    }
    notify(&message);
    Ok(json!({"layers": done.ids, "image": done.image, "replaced": done.replaced, "skipped": done.skipped, "tile": s.tile_px()}))
}

/// Re-texture the selected layers that have a texture from this plugin (live updates from the
/// panel): nothing to do when none of them has.
fn update_live(s: &Settings) -> Result<Value> {
    let selected = selection();
    let mut textured = Vec::new();
    for id in &selected {
        if own_texture(&call("node.get", &json!({"id": id}))?).is_some() {
            textured.push(*id);
        }
    }
    if textured.is_empty() {
        return Ok(Value::Null);
    }
    let done = texture_layers(s, &textured, &selected)?;
    Ok(json!({"layers": done.ids, "image": done.image}))
}

/// Take this plugin's texture off the layers `ids` (their other fills stay).
fn remove(ids: &[u64]) -> Result<Value> {
    let mut removed = Vec::new();
    for id in ids {
        let node = call("node.get", &json!({"id": id}))?;
        let Some((image, _)) = own_texture(&node) else { continue };
        for index in fills_with(&node, &image).into_iter().rev() {
            call("fill.remove", &json!({"ids": [id], "index": index}))?;
        }
        call("node.set", &json!({"ids": [id], "props": data_props(None)}))?;
        removed.push(*id);
    }
    if removed.is_empty() {
        return Err("None of the selected layers has a texture from Noise Texture".into());
    }
    notify(&if removed.len() == 1 { "Removed the texture".to_string() } else { format!("Removed the texture from {} layers", removed.len()) });
    Ok(json!({"layers": removed}))
}

/// The saved presets, by name.
fn saved_presets() -> Map<String, Value> {
    setting("presets").and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

/// The preset `name`: a saved one or a built-in one.
fn find_preset(name: &str) -> Option<Settings> {
    saved_presets().get(name).map(Settings::from_json).or_else(|| presets::find_built_in(name))
}

/// Remember the settings for the next time the panel opens (best effort: a full settings store
/// mustn't stop the texture).
fn remember(s: &Settings) {
    set_setting("last", &s.to_json()).ok();
}

/// The panel's tile-size option nearest to `tile`.
fn tile_option(tile: u32) -> String {
    TILES.iter().min_by_key(|t| t.abs_diff(tile)).copied().unwrap_or(256).to_string()
}

/// Show the panel with settings `s`, preset `preset` picked, live updates `live` and `name` in the
/// preset name field.
fn panel(s: &Settings, preset: &str, live: bool, name: &str) -> Result<Value> {
    let saved: Vec<String> = saved_presets().keys().cloned().collect();
    let mut options: Vec<String> = vec![CUSTOM.to_string()];
    options.extend(presets::built_in_names().into_iter().map(str::to_string));
    options.extend(saved);
    let preset = if options.iter().any(|o| o == preset) { preset } else { CUSTOM };
    let labels = |all: &[&'static str]| all.iter().map(|l| Value::from(*l)).collect::<Vec<_>>();
    let kinds: Vec<&str> = Kind::ALL.iter().map(|k| k.label()).collect();
    let modes: Vec<&str> = ColorMode::ALL.iter().map(|m| m.label()).collect();
    let shapes: Vec<&str> = Shape::ALL.iter().map(|x| x.label()).collect();
    let blends: Vec<&str> = BLENDS.iter().map(|b| b.0).collect();
    let tiles: Vec<String> = TILES.iter().map(u32::to_string).collect();
    show_ui(&json!({"title": "Noise Texture", "controls": [
        {"type": "select", "name": "preset", "label": "Preset", "options": options, "value": preset},
        {"type": "select", "name": "type", "label": "Texture", "options": labels(&kinds), "value": s.kind.label()},
        {"type": "note", "label": s.kind.about()},
        {"type": "number", "name": "size", "label": "Particle size (px)", "min": texture::SIZE.0, "max": texture::SIZE.1, "value": s.size},
        {"type": "number", "name": "scale", "label": "Feature scale (px)", "min": texture::SCALE.0, "max": texture::SCALE.1, "value": s.scale},
        {"type": "number", "name": "density", "label": "Amount (%)", "min": 0, "max": 100, "value": s.density},
        {"type": "number", "name": "detail", "label": "Detail", "min": texture::DETAIL.0, "max": texture::DETAIL.1, "value": s.detail},
        {"type": "select", "name": "shape", "label": "Particle shape", "options": labels(&shapes), "value": s.shape.label()},
        {"type": "select", "name": "colorMode", "label": "Colours", "options": labels(&modes), "value": s.color_mode.label()},
        {"type": "color", "name": "color", "label": "Particle colour", "value": texture::hex(s.color)},
        {"type": "color", "name": "color2", "label": "Second colour", "value": texture::hex(s.color2)},
        {"type": "number", "name": "opacity", "label": "Particle opacity (%)", "min": 0, "max": 100, "value": s.opacity},
        {"type": "color", "name": "background", "label": "Background", "value": texture::hex(s.background)},
        {"type": "number", "name": "backgroundOpacity", "label": "Background opacity (%)", "min": 0, "max": 100, "value": s.background_opacity},
        {"type": "select", "name": "blend", "label": "Blend mode", "options": labels(&blends), "value": s.blend_label()},
        {"type": "select", "name": "tile", "label": "Tile size (px)", "options": tiles, "value": tile_option(s.tile)},
        {"type": "number", "name": "seed", "label": "Seed", "min": texture::SEED.0, "max": texture::SEED.1, "value": s.seed},
        {"type": "boolean", "name": "live", "label": "Update selected textures live", "value": live},
        {"type": "note", "label": "Apply adds the texture on top of the selected layers' fills (or replaces the one it added before); with nothing selected it makes a new textured layer."},
        {"type": "button", "name": "apply", "label": "Apply"},
        {"type": "button", "name": "shuffle", "label": "Shuffle"},
        {"type": "button", "name": "remove", "label": "Remove"},
        {"type": "text", "name": "presetName", "label": "Preset name", "value": name},
        {"type": "button", "name": "savePreset", "label": "Save preset"},
        {"type": "button", "name": "deletePreset", "label": "Delete preset"},
        {"type": "button", "name": "close", "label": "Close"}
    ]}))?;
    Ok(Value::Null)
}

/// Open the panel: with the settings of a selected layer textured by this plugin, else the last
/// ones used.
fn open_panel(selected: &[u64]) -> Result<Value> {
    let from_layer = selected.iter().find_map(|id| call("node.get", &json!({"id": id})).ok().as_ref().and_then(own_texture).map(|(_, s)| s));
    let s = from_layer.or_else(|| setting("last").map(|v| Settings::from_json(&v))).unwrap_or_default();
    let live = setting("live").and_then(|v| v.as_bool()).unwrap_or(true);
    panel(&s, CUSTOM, live, "")
}

/// A button pressed or a field changed in the panel.
fn on_event(e: Event) -> Result<Value> {
    let values = &e.values;
    let s = Settings::from_json(values);
    let live = values["live"].as_bool().unwrap_or(true);
    let preset = values["preset"].as_str().unwrap_or(CUSTOM).to_string();
    let name = values["presetName"].as_str().unwrap_or_default().to_string();
    let refresh = |s: &Settings, preset: &str, name: &str| -> Result<Value> {
        remember(s);
        panel(s, preset, live, name)?;
        if live { update_live(s) } else { Ok(Value::Null) }
    };
    match e.control.as_str() {
        "apply" => {
            remember(&s);
            apply(&s, &selection())
        }
        "shuffle" => refresh(&s.shuffled(), &preset, &name),
        "remove" => remove(&selection()),
        "preset" => match find_preset(&preset) {
            Some(p) => refresh(&p, &preset, &name),
            None => Ok(Value::Null),
        },
        // The note describes the texture kind.
        "type" => refresh(&s, &preset, &name),
        "live" => {
            set_setting("live", &json!(live)).ok();
            if live { update_live(&s) } else { Ok(Value::Null) }
        }
        "savePreset" => {
            let name = presets::clean_name(&name)?;
            let mut all = saved_presets();
            if all.len() >= presets::MAX_SAVED && !all.contains_key(&name) {
                return Err(format!("There are {} saved presets already: delete one first", presets::MAX_SAVED));
            }
            all.insert(name.clone(), s.to_json());
            set_setting("presets", &Value::Object(all))?;
            panel(&s, &name, live, "")?;
            notify(&format!("Saved the preset “{name}”"));
            Ok(Value::Null)
        }
        "deletePreset" => {
            let typed = name.split_whitespace().collect::<Vec<_>>().join(" ");
            let target = if typed.is_empty() { preset } else { typed };
            let mut all = saved_presets();
            if all.remove(&target).is_none() {
                return Err(if target == CUSTOM || presets::find_built_in(&target).is_some() {
                    "Built-in presets can't be deleted: pick a saved preset or type its name".to_string()
                } else {
                    format!("There's no saved preset named “{target}”")
                });
            }
            set_setting("presets", &Value::Object(all))?;
            panel(&s, CUSTOM, live, "")?;
            notify(&format!("Deleted the preset “{target}”"));
            Ok(Value::Null)
        }
        "close" => {
            close_ui();
            Ok(Value::Null)
        }
        // Typing a preset's name changes nothing yet.
        "presetName" => Ok(Value::Null),
        // Any other field: the texture changed.
        _ => {
            remember(&s);
            if live { update_live(&s) } else { Ok(Value::Null) }
        }
    }
}

fn run(input: Input) -> Result<Value> {
    if let Some(e) = input.event {
        return on_event(e);
    }
    match input.command.as_str() {
        "apply" => apply(&Settings::from_json(&input.params), &input.selection),
        "preset" => {
            let name = input.str("preset").unwrap_or_default();
            let s = find_preset(name).ok_or_else(|| format!("There's no preset named “{name}”"))?;
            apply(&s, &input.selection)
        }
        "remove" => remove(&input.selection),
        _ => open_panel(&input.selection),
    }
}

entry!(run);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_its_own_fill_among_the_layers_fills() {
        let s = Settings { seed: 7, ..Settings::default() };
        let data = json!({"image": "abc", "settings": s.to_json()}).to_string();
        let node = json!({
            "type": "RECTANGLE",
            "fills": [
                {"type": "solid", "color": "#FF0000"},
                {"type": "image", "image": "abc", "scale_mode": "tile"},
                {"type": "image", "image": "other"},
                {"type": "image", "image": "abc"}
            ],
            "pluginData": {KEY: data, "someone.else": "x"}
        });
        let (image, got) = own_texture(&node).unwrap();
        assert_eq!((image.as_str(), got), ("abc", s));
        assert_eq!(fills_with(&node, "abc"), vec![1, 3]);
        assert!(fills_with(&json!({}), "abc").is_empty());
        assert!(own_texture(&json!({"pluginData": {KEY: "not json"}})).is_none());
        assert!(own_texture(&json!({"type": "FRAME"})).is_none());
        assert!(takes_fills(&node) && !takes_fills(&json!({"type": "GROUP"})) && !takes_fills(&json!({})));
    }

    #[test]
    fn layer_data_props() {
        assert_eq!(data_props(None), json!({"pluginData": {KEY: null}}));
        assert_eq!(data_props(Some("x".into())), json!({"pluginData": {KEY: "x"}}));
        let p = paint(&Settings { blend: 7, ..Settings::default() }, "h");
        assert_eq!((p["image"].as_str(), p["blend"].as_str(), p["scale_mode"].as_str()), (Some("h"), Some("overlay"), Some("tile")));
    }

    #[test]
    fn tile_options_snap_to_the_nearest() {
        assert_eq!(tile_option(100), "128");
        assert_eq!(tile_option(16), "64");
        assert_eq!(tile_option(512), "512");
    }

    #[test]
    fn outside_figcraft_host_calls_fail_cleanly() {
        // The SDK's host calls fail off WebAssembly: the plugin reports an error, never panics.
        assert!(run(Input { command: "apply".into(), selection: vec![1], ..Input::default() }).is_err());
        assert!(run(Input { command: "remove".into(), selection: vec![1], ..Input::default() }).is_err());
        assert!(run(Input { command: "preset".into(), params: json!({"preset": "Nope"}), ..Input::default() }).is_err());
        assert!(texture_png(&Settings::default()).is_ok_and(|p| p.starts_with(b"\x89PNG")));
    }
}
