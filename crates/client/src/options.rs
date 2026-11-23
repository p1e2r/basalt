//! Options sub-pages (skin customization, music & sounds, video, controls, chat, accessibility,
//! online, language, resource packs, credits): data-driven option tables whose values live in
//! `Config::extra`, laid out like the vanilla 26.2 client (two columns of 150, rows 25 apart,
//! header/footer bars; measured from a recording, 2026-11-23).
use crate::menu::{Config, Kind, Widget};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Skin,
    Sounds,
    Video,
    Controls,
    Mouse,
    KeyBinds,
    Chat,
    Accessibility,
    Online,
    Language,
    Packs,
    Credits,
}

#[derive(Clone, Copy)]
pub enum OptKind {
    Toggle(bool),
    /// lang keys of the values, default index
    Cycle(&'static [&'static str], usize),
    /// min, max, step, default, display
    Slider(f32, f32, f32, f32, Fmt),
    /// opens another page / does something (`id` is the action)
    Button,
}

#[derive(Clone, Copy)]
pub enum Fmt {
    Percent,
    Int,
    Chunks,
    Px,
    Mult,
    Delay,
    Gamma,
}

#[derive(Clone, Copy)]
pub struct Opt {
    pub id: &'static str,
    pub key: &'static str,
    pub kind: OptKind,
    pub wide: bool,
}

const fn t(id: &'static str, key: &'static str, d: bool) -> Opt {
    Opt {
        id,
        key,
        kind: OptKind::Toggle(d),
        wide: false,
    }
}
const fn c(id: &'static str, key: &'static str, v: &'static [&'static str], d: usize) -> Opt {
    Opt {
        id,
        key,
        kind: OptKind::Cycle(v, d),
        wide: false,
    }
}
const fn sl(
    id: &'static str,
    key: &'static str,
    min: f32,
    max: f32,
    step: f32,
    d: f32,
    f: Fmt,
) -> Opt {
    Opt {
        id,
        key,
        kind: OptKind::Slider(min, max, step, d, f),
        wide: false,
    }
}
const fn b(id: &'static str, key: &'static str) -> Opt {
    Opt {
        id,
        key,
        kind: OptKind::Button,
        wide: false,
    }
}
const fn wide(mut o: Opt) -> Opt {
    o.wide = true;
    o
}

const HOLD: &[&str] = &["options.key.toggle", "options.key.hold"];

pub fn opts(p: Page) -> &'static [Opt] {
    match p {
        Page::Skin => {
            const O: &[Opt] = &[
                t("cape", "options.modelPart.cape", true),
                t("jacket", "options.modelPart.jacket", true),
                t("left_sleeve", "options.modelPart.left_sleeve", true),
                t("right_sleeve", "options.modelPart.right_sleeve", true),
                t("left_pants_leg", "options.modelPart.left_pants_leg", true),
                t("right_pants_leg", "options.modelPart.right_pants_leg", true),
                t("hat", "options.modelPart.hat", true),
                c(
                    "main_hand",
                    "options.mainHand",
                    &["options.mainHand.left", "options.mainHand.right"],
                    1,
                ),
            ];
            O
        }
        Page::Sounds => {
            const O: &[Opt] = &[
                wide(sl(
                    "vol.master",
                    "soundCategory.master",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                )),
                sl(
                    "vol.music",
                    "soundCategory.music",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.record",
                    "soundCategory.record",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.weather",
                    "soundCategory.weather",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.block",
                    "soundCategory.block",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.hostile",
                    "soundCategory.hostile",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.neutral",
                    "soundCategory.neutral",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.player",
                    "soundCategory.player",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.ambient",
                    "soundCategory.ambient",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.voice",
                    "soundCategory.voice",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "vol.ui",
                    "soundCategory.ui",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                wide(c(
                    "audio_device",
                    "options.audioDevice",
                    &["options.audioDevice.default"],
                    0,
                )),
                t("subtitles", "options.showSubtitles", false),
            ];
            O
        }
        Page::Video => {
            const O: &[Opt] = &[
                c(
                    "graphics",
                    "options.graphics",
                    &[
                        "options.graphics.fast",
                        "options.graphics.fancy",
                        "options.graphics.fabulous",
                    ],
                    1,
                ),
                sl(
                    "render_distance",
                    "options.renderDistance",
                    2.0,
                    32.0,
                    1.0,
                    10.0,
                    Fmt::Chunks,
                ),
                sl(
                    "simulation_distance",
                    "options.simulationDistance",
                    5.0,
                    32.0,
                    1.0,
                    12.0,
                    Fmt::Chunks,
                ),
                t("ao", "options.ao", true),
                sl(
                    "max_fps",
                    "options.framerateLimit",
                    10.0,
                    260.0,
                    10.0,
                    120.0,
                    Fmt::Int,
                ),
                t("vsync", "options.vsync", true),
                t("view_bobbing", "options.viewBobbing", true),
                b("gui_scale", "options.guiScale"),
                c(
                    "attack_indicator",
                    "options.attackIndicator",
                    &[
                        "options.off",
                        "options.attack.crosshair",
                        "options.attack.hotbar",
                    ],
                    1,
                ),
                sl("gamma", "options.gamma", 0.0, 1.0, 0.01, 0.5, Fmt::Gamma),
                c(
                    "clouds",
                    "options.renderClouds",
                    &["options.off", "options.clouds.fast", "options.clouds.fancy"],
                    2,
                ),
                b("fullscreen", "options.fullscreen"),
                c(
                    "particles",
                    "options.particles",
                    &[
                        "options.particles.all",
                        "options.particles.decreased",
                        "options.particles.minimal",
                    ],
                    0,
                ),
                sl(
                    "mipmap",
                    "options.mipmapLevels",
                    0.0,
                    4.0,
                    1.0,
                    4.0,
                    Fmt::Int,
                ),
                t("entity_shadows", "options.entityShadows", true),
                sl(
                    "distortion",
                    "options.screenEffectScale",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "entity_distance",
                    "options.entityDistanceScaling",
                    0.5,
                    5.0,
                    0.25,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "fov_effects",
                    "options.fovEffectScale",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "glint_speed",
                    "options.glintSpeed",
                    0.0,
                    1.0,
                    0.01,
                    0.5,
                    Fmt::Percent,
                ),
                sl(
                    "glint_strength",
                    "options.glintStrength",
                    0.0,
                    1.0,
                    0.01,
                    0.75,
                    Fmt::Percent,
                ),
                sl(
                    "menu_blur",
                    "options.accessibility.menu_background_blurriness",
                    0.0,
                    10.0,
                    1.0,
                    5.0,
                    Fmt::Int,
                ),
                sl(
                    "biome_blend",
                    "options.biomeBlendRadius",
                    0.0,
                    7.0,
                    1.0,
                    2.0,
                    Fmt::Int,
                ),
            ];
            O
        }
        Page::Controls => {
            const O: &[Opt] = &[
                b("mouse", "options.mouse_settings"),
                b("keybinds", "controls.keybinds"),
                c("toggle_sneak", "key.sneak", HOLD, 1),
                c("toggle_sprint", "key.sprint", HOLD, 1),
                c("toggle_attack", "key.attack", HOLD, 1),
                c("toggle_use", "key.use", HOLD, 1),
                t("auto_jump", "options.autoJump", false),
                sl(
                    "sprint_window",
                    "options.sprintWindow",
                    0.0,
                    10.0,
                    1.0,
                    7.0,
                    Fmt::Int,
                ),
                t("op_items", "options.operatorItemsTab", true),
            ];
            O
        }
        Page::Mouse => {
            const O: &[Opt] = &[
                sl(
                    "sensitivity",
                    "options.sensitivity",
                    0.0,
                    1.0,
                    0.005,
                    0.5,
                    Fmt::Percent,
                ),
                t("invert_mouse", "options.invertMouse", false),
                sl(
                    "scroll_sensitivity",
                    "options.mouseWheelSensitivity",
                    0.01,
                    10.0,
                    0.01,
                    1.0,
                    Fmt::Mult,
                ),
                t("discrete_scroll", "options.discrete_mouse_scroll", false),
                t("touchscreen", "options.touchscreen", false),
                t("raw_input", "options.rawMouseInput", true),
            ];
            O
        }
        Page::Chat => {
            const O: &[Opt] = &[
                c(
                    "chat_visibility",
                    "options.chat.visibility",
                    &[
                        "options.chat.visibility.full",
                        "options.chat.visibility.system",
                        "options.chat.visibility.hidden",
                    ],
                    0,
                ),
                t("chat_colors", "options.chat.color", true),
                t("chat_links", "options.chat.links", true),
                t("chat_links_prompt", "options.chat.links.prompt", true),
                sl(
                    "chat_opacity",
                    "options.chat.opacity",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "text_bg_opacity",
                    "options.accessibility.text_background_opacity",
                    0.0,
                    1.0,
                    0.01,
                    0.5,
                    Fmt::Percent,
                ),
                sl(
                    "chat_scale",
                    "options.chat.scale",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "chat_line_spacing",
                    "options.chat.line_spacing",
                    0.0,
                    1.0,
                    0.01,
                    0.0,
                    Fmt::Percent,
                ),
                sl(
                    "chat_delay",
                    "options.chat.delay",
                    0.0,
                    6.0,
                    0.1,
                    0.0,
                    Fmt::Delay,
                ),
                sl(
                    "chat_width",
                    "options.chat.width",
                    40.0,
                    320.0,
                    1.0,
                    320.0,
                    Fmt::Px,
                ),
                sl(
                    "chat_height_focused",
                    "options.chat.height.focused",
                    20.0,
                    180.0,
                    1.0,
                    180.0,
                    Fmt::Px,
                ),
                sl(
                    "chat_height_unfocused",
                    "options.chat.height.unfocused",
                    20.0,
                    180.0,
                    1.0,
                    90.0,
                    Fmt::Px,
                ),
                c("narrator", "options.narrator", &["options.narrator.off"], 0),
                t("command_suggestions", "options.autoSuggestCommands", true),
                t("hide_matched_names", "options.hideMatchedNames", true),
                t("reduced_debug", "options.reducedDebugInfo", false),
                t("secure_chat_only", "options.onlyShowSecureChat", false),
            ];
            O
        }
        Page::Accessibility => {
            const O: &[Opt] = &[
                c("narrator", "options.narrator", &["options.narrator.off"], 0),
                b("controls", "options.controls"),
                t("subtitles", "options.showSubtitles", false),
                t(
                    "high_contrast",
                    "options.accessibility.high_contrast",
                    false,
                ),
                sl(
                    "menu_blur",
                    "options.accessibility.menu_background_blurriness",
                    0.0,
                    10.0,
                    1.0,
                    5.0,
                    Fmt::Int,
                ),
                sl(
                    "text_bg_opacity",
                    "options.accessibility.text_background_opacity",
                    0.0,
                    1.0,
                    0.01,
                    0.5,
                    Fmt::Percent,
                ),
                c(
                    "text_bg",
                    "options.accessibility.text_background",
                    &[
                        "options.accessibility.text_background.chat",
                        "options.accessibility.text_background.everywhere",
                    ],
                    0,
                ),
                sl(
                    "chat_opacity",
                    "options.chat.opacity",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "chat_line_spacing",
                    "options.chat.line_spacing",
                    0.0,
                    1.0,
                    0.01,
                    0.0,
                    Fmt::Percent,
                ),
                sl(
                    "chat_delay",
                    "options.chat.delay",
                    0.0,
                    6.0,
                    0.1,
                    0.0,
                    Fmt::Delay,
                ),
                sl(
                    "notification_time",
                    "options.notifications.display_time",
                    0.5,
                    10.0,
                    0.1,
                    1.0,
                    Fmt::Mult,
                ),
                t("view_bobbing", "options.viewBobbing", true),
                sl(
                    "distortion",
                    "options.screenEffectScale",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "fov_effects",
                    "options.fovEffectScale",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "darkness_pulsing",
                    "options.darknessEffectScale",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "damage_tilt",
                    "options.damageTiltStrength",
                    0.0,
                    1.0,
                    0.01,
                    1.0,
                    Fmt::Percent,
                ),
                sl(
                    "glint_speed",
                    "options.glintSpeed",
                    0.0,
                    1.0,
                    0.01,
                    0.5,
                    Fmt::Percent,
                ),
                sl(
                    "glint_strength",
                    "options.glintStrength",
                    0.0,
                    1.0,
                    0.01,
                    0.75,
                    Fmt::Percent,
                ),
                t("hide_lightning", "options.hideLightningFlashes", false),
                t(
                    "mono_logo",
                    "options.darkMojangStudiosBackgroundColor",
                    false,
                ),
                t("hide_splash", "options.hideSplashTexts", false),
            ];
            O
        }
        Page::Online => {
            const O: &[Opt] = &[
                t("realms_notifications", "options.realmsNotifications", true),
                t("allow_server_listing", "options.allowServerListing", true),
            ];
            O
        }
        Page::KeyBinds | Page::Language | Page::Packs | Page::Credits => &[],
    }
}

pub fn title(p: Page) -> &'static str {
    match p {
        Page::Skin => "options.skinCustomisation.title",
        Page::Sounds => "options.sounds.title",
        Page::Video => "options.videoTitle",
        Page::Controls => "options.controls",
        Page::Mouse => "options.mouse_settings.title",
        Page::KeyBinds => "controls.keybinds.title",
        Page::Chat => "options.chat.title",
        Page::Accessibility => "options.accessibility.title",
        Page::Online => "options.online.title",
        Page::Language => "options.language.title",
        Page::Packs => "resourcePack.title",
        Page::Credits => "credits_and_attribution.screen.title",
    }
}

fn tr(k: &str) -> String {
    crate::lang::tr(k, &[])
}

/// Current value of an option (its default when never set).
pub fn value(cfg: &Config, o: &Opt) -> f32 {
    if let Some(v) = cfg.extra.get(o.id) {
        return *v;
    }
    match o.kind {
        OptKind::Toggle(d) => d as u8 as f32,
        OptKind::Cycle(_, d) => d as f32,
        OptKind::Slider(_, _, _, d, _) => d,
        OptKind::Button => 0.0,
    }
}

/// Button / slider caption for an option, like the vanilla client formats it.
pub fn caption(cfg: &Config, o: &Opt) -> String {
    let name = tr(o.key);
    let v = value(cfg, o);
    match o.kind {
        OptKind::Toggle(_) => crate::lang::tr(
            "options.generic_value",
            &[name, tr(if v > 0.5 { "options.on" } else { "options.off" })],
        ),
        OptKind::Cycle(vals, _) => crate::lang::tr(
            "options.generic_value",
            &[name, tr(vals[(v as usize).min(vals.len() - 1)])],
        ),
        OptKind::Slider(_, max, _, _, f) => match f {
            Fmt::Percent if o.id == "sensitivity" => crate::lang::tr(
                "options.percent_value",
                &[name, format!("{}", (v * 200.0).round())],
            ),
            Fmt::Percent => crate::lang::tr(
                "options.percent_value",
                &[name, format!("{}", (v * 100.0).round())],
            ),
            Fmt::Int if o.id == "max_fps" && v >= max => crate::lang::tr(
                "options.generic_value",
                &[name, tr("options.framerateLimit.max")],
            ),
            Fmt::Int => crate::lang::tr("options.generic_value", &[name, format!("{}", v.round())]),
            Fmt::Chunks => crate::lang::tr(
                "options.generic_value",
                &[
                    name,
                    crate::lang::tr("options.chunks", &[format!("{}", v.round())]),
                ],
            ),
            Fmt::Px => crate::lang::tr("options.pixel_value", &[name, format!("{}", v.round())]),
            Fmt::Mult => crate::lang::tr(
                "options.generic_value",
                &[
                    name,
                    crate::lang::tr("options.multiplier", &[format!("{:.1}", v)]),
                ],
            ),
            Fmt::Delay if v <= 0.0 => tr("options.chat.delay_none"),
            Fmt::Delay => crate::lang::tr("options.chat.delay", &[format!("{v:.1}")]),
            Fmt::Gamma if (v - 0.5).abs() < 0.005 => crate::lang::tr(
                "options.generic_value",
                &[name, tr("options.gamma.default")],
            ),
            Fmt::Gamma if v <= 0.0 => {
                crate::lang::tr("options.generic_value", &[name, tr("options.gamma.min")])
            }
            Fmt::Gamma if v >= 1.0 => {
                crate::lang::tr("options.generic_value", &[name, tr("options.gamma.max")])
            }
            Fmt::Gamma => crate::lang::tr(
                "options.percent_value",
                &[name, format!("{}", (v * 100.0).round())],
            ),
        },
        OptKind::Button => name,
    }
}

/// Slider fraction 0..1 of an option value.
pub fn fraction(o: &Opt, v: f32) -> f32 {
    match o.kind {
        OptKind::Slider(min, max, ..) => ((v - min) / (max - min)).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// Widgets of an option page in GUI units: rows of two 150-wide columns 25 apart below the
/// 33-unit header, `scroll` units scrolled; only rows inside the list area are returned.
pub fn layout(cfg: &Config, p: Page, cx: f32, gh: f32, scroll: f32) -> (Vec<Widget>, f32) {
    let mut v = Vec::new();
    let (top, bottom) = (33.0, gh - 33.0);
    let mut row = 0usize;
    let mut col = 0usize;
    let mut place = |w: f32, wide: bool| -> (f32, f32, f32) {
        if wide && col == 1 {
            row += 1;
            col = 0;
        }
        let y = top + 4.0 + row as f32 * 25.0 - scroll;
        let (x, ww) = if wide {
            (cx - 155.0, 310.0)
        } else {
            (if col == 0 { cx - 155.0 } else { cx + 5.0 }, w)
        };
        if wide {
            row += 1;
        } else {
            col += 1;
            if col == 2 {
                col = 0;
                row += 1;
            }
        }
        (x, y, ww)
    };
    for o in opts(p) {
        let (x, y, w) = place(150.0, o.wide);
        if y < top || y + 20.0 > bottom {
            continue;
        }
        let kind = match o.kind {
            OptKind::Slider(..) => Kind::Slider {
                label: caption(cfg, o),
                value: fraction(o, value(cfg, o)),
            },
            OptKind::Button => Kind::Button {
                label: match o.id {
                    "gui_scale" => format!(
                        "{}: {}",
                        tr(o.key),
                        if cfg.gui_scale == 0 {
                            tr("options.guiScale.auto")
                        } else {
                            cfg.gui_scale.to_string()
                        }
                    ),
                    "fullscreen" => crate::lang::tr(
                        "options.generic_value",
                        &[
                            tr(o.key),
                            tr(if cfg.fullscreen {
                                "options.on"
                            } else {
                                "options.off"
                            }),
                        ],
                    ),
                    _ => caption(cfg, o),
                },
                enabled: true,
            },
            _ => Kind::Button {
                label: caption(cfg, o),
                enabled: true,
            },
        };
        v.push(Widget {
            id: format!("opt:{}", o.id),
            x,
            y,
            w,
            h: 20.0,
            kind,
        });
    }
    let rows = if col == 1 { row + 1 } else { row };
    let content = 4.0 + rows as f32 * 25.0;
    (v, (content - (bottom - top)).max(0.0))
}
