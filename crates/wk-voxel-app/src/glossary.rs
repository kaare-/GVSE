//! F6 glossary / how-it-works overlay for wk-voxel-app.
//!
//! Isolation: macroquad only. Copy stays in this file so Tab and the
//! HUD can stay short.

use macroquad::prelude::*;

const PANEL_W: f32 = 680.0;
const PAD: f32 = 14.0;
const TITLE_H: f32 = 56.0;
const TAB_H: f32 = 22.0;
const LINE_H: f32 = 16.0;
const BODY: f32 = 15.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Keys,
    Water,
    Sky,
    Ground,
    Life,
    Hud,
}

impl Page {
    const ALL: [Page; 6] = [
        Page::Keys,
        Page::Water,
        Page::Sky,
        Page::Ground,
        Page::Life,
        Page::Hud,
    ];

    fn label(self) -> &'static str {
        match self {
            Page::Keys => "1 Keys",
            Page::Water => "2 Water",
            Page::Sky => "3 Sky",
            Page::Ground => "4 Ground",
            Page::Life => "5 Life",
            Page::Hud => "6 HUD",
        }
    }

    fn next(self) -> Self {
        match self {
            Page::Keys => Page::Water,
            Page::Water => Page::Sky,
            Page::Sky => Page::Ground,
            Page::Ground => Page::Life,
            Page::Life => Page::Hud,
            Page::Hud => Page::Keys,
        }
    }

    fn prev(self) -> Self {
        match self {
            Page::Keys => Page::Hud,
            Page::Water => Page::Keys,
            Page::Sky => Page::Water,
            Page::Ground => Page::Sky,
            Page::Life => Page::Ground,
            Page::Hud => Page::Life,
        }
    }

    fn from_digit(n: u32) -> Option<Self> {
        match n {
            1 => Some(Page::Keys),
            2 => Some(Page::Water),
            3 => Some(Page::Sky),
            4 => Some(Page::Ground),
            5 => Some(Page::Life),
            6 => Some(Page::Hud),
            _ => None,
        }
    }
}

/// F6 explanation layer (glossary + shortcuts + how the weather works).
pub struct Glossary {
    pub open: bool,
    page: Page,
    scroll: f32,
}

impl Default for Glossary {
    fn default() -> Self {
        Self {
            open: false,
            page: Page::Keys,
            scroll: 0.0,
        }
    }
}

impl Glossary {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.scroll = 0.0;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    fn layout() -> (f32, f32, f32, f32) {
        let sw = screen_width();
        let sh = screen_height();
        let w = PANEL_W.min(sw - 24.0).max(420.0);
        let h = (sh * 0.82).min(sh - 24.0).max(280.0);
        let x = ((sw - w) * 0.5).max(8.0);
        let y = ((sh - h) * 0.5).max(8.0);
        (x, y, w, h)
    }

    pub fn hits_panel(&self, mx: f32, my: f32) -> bool {
        if !self.open {
            return false;
        }
        let (x, y, w, h) = Self::layout();
        mx >= x && mx < x + w && my >= y && my < y + h
    }

    pub fn handle_input(&mut self) {
        if !self.open {
            return;
        }
        if is_key_pressed(KeyCode::RightBracket) {
            self.page = self.page.next();
            self.scroll = 0.0;
        }
        if is_key_pressed(KeyCode::LeftBracket) {
            self.page = self.page.prev();
            self.scroll = 0.0;
        }
        for (key, n) in [
            (KeyCode::Key1, 1),
            (KeyCode::Key2, 2),
            (KeyCode::Key3, 3),
            (KeyCode::Key4, 4),
            (KeyCode::Key5, 5),
            (KeyCode::Key6, 6),
        ] {
            if is_key_pressed(key) {
                if let Some(p) = Page::from_digit(n) {
                    self.page = p;
                    self.scroll = 0.0;
                }
            }
        }

        let (mx, my) = mouse_position();
        if self.hits_panel(mx, my) {
            let wheel = mouse_wheel().1;
            if wheel != 0.0 {
                self.scroll = (self.scroll - wheel * 36.0).max(0.0);
            }
        }

        if is_mouse_button_pressed(MouseButton::Left) && self.hits_panel(mx, my) {
            let (x, y, w, _) = Self::layout();
            let tab_y = y + 28.0;
            let n = Page::ALL.len() as f32;
            let tab_w = (w - PAD * 2.0) / n;
            for (i, page) in Page::ALL.iter().enumerate() {
                let tx = x + PAD + i as f32 * tab_w;
                if mx >= tx && mx < tx + tab_w && my >= tab_y && my < tab_y + TAB_H {
                    self.page = *page;
                    self.scroll = 0.0;
                    break;
                }
            }
        }
    }

    pub fn draw(&self) {
        if !self.open {
            return;
        }
        let (x, y, w, h) = Self::layout();
        draw_rectangle(x, y, w, h, Color::from_rgba(8, 10, 16, 242));
        draw_rectangle_lines(x, y, w, h, 1.5, Color::from_rgba(140, 180, 220, 255));

        draw_text(
            "GLOSSARY  (F6 close)",
            x + PAD,
            y + 20.0,
            18.0,
            Color::from_rgba(190, 220, 255, 255),
        );
        draw_text(
            "1–6 or [ ] pages  ·  wheel scroll  ·  Esc closes",
            x + PAD + 210.0,
            y + 20.0,
            14.0,
            GRAY,
        );

        let n = Page::ALL.len() as f32;
        let tab_w = (w - PAD * 2.0) / n;
        let tab_y = y + 28.0;
        for (i, page) in Page::ALL.iter().enumerate() {
            let tx = x + PAD + i as f32 * tab_w;
            let on = *page == self.page;
            if on {
                draw_rectangle(
                    tx,
                    tab_y,
                    tab_w - 4.0,
                    TAB_H,
                    Color::from_rgba(40, 70, 110, 230),
                );
            }
            draw_text(
                page.label(),
                tx + 6.0,
                tab_y + 16.0,
                14.0,
                if on {
                    WHITE
                } else {
                    Color::from_rgba(160, 170, 180, 255)
                },
            );
        }

        let lines = page_lines(self.page);
        let body_top = y + TITLE_H;
        let body_bot = y + h - PAD;
        let view_h = (body_bot - body_top).max(0.0);
        let content_h = lines.len() as f32 * LINE_H;
        let max_scroll = (content_h - view_h).max(0.0);
        let scroll = self.scroll.clamp(0.0, max_scroll);
        let first = (scroll / LINE_H).floor().max(0.0) as usize;
        let visible = (view_h / LINE_H).ceil() as usize + 1;

        for off in 0..visible {
            let i = first + off;
            if i >= lines.len() {
                break;
            }
            let ly = body_top + i as f32 * LINE_H - scroll;
            if ly + LINE_H < body_top || ly > body_bot {
                continue;
            }
            let (text, kind) = lines[i];
            let color = match kind {
                LineKind::Head => Color::from_rgba(170, 210, 255, 255),
                LineKind::Body => Color::from_rgba(220, 224, 230, 255),
                LineKind::Dim => Color::from_rgba(140, 150, 160, 255),
                LineKind::Blank => continue,
            };
            let size = if kind == LineKind::Head { 16.0 } else { BODY };
            draw_text(text, x + PAD, ly + 13.0, size, color);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Head,
    Body,
    Dim,
    Blank,
}

fn page_lines(page: Page) -> &'static [(&'static str, LineKind)] {
    match page {
        Page::Keys => KEYS,
        Page::Water => WATER,
        Page::Sky => SKY,
        Page::Ground => GROUND,
        Page::Life => LIFE,
        Page::Hud => HUD,
    }
}

const KEYS: &[(&str, LineKind)] = &[
    ("Shortcuts", LineKind::Head),
    ("Space    pause / resume physics", LineKind::Body),
    ("Arrows   pan camera (ring worlds wrap left/right)", LineKind::Body),
    ("Click    inspect the cell / creature under the cursor", LineKind::Body),
    ("R        regenerate world (new seed, same size)", LineKind::Body),
    ("Esc      close this / editors / Tab, then quit confirm", LineKind::Body),
    ("", LineKind::Blank),
    ("Weather toggles (also on Tab → Climate)", LineKind::Head),
    ("C        condensation drizzle — the real rain (default on)", LineKind::Body),
    ("E        evaporate standing water into humidity (default on)", LineKind::Body),
    ("W        climatic faucet — extra rain, off by default", LineKind::Body),
    ("K        karst — wet limestone + slow groundwater stone (default on)", LineKind::Body),
    ("I        ice / snow / slush phase pass", LineKind::Body),
    ("", LineKind::Blank),
    ("Overlays", LineKind::Head),
    ("N        soft clouds (picture of wet sky tiles)", LineKind::Body),
    ("H        humidity tile raster (default on; Tab: resample + min mass)", LineKind::Body),
    ("V        wind — coarse local-field arrows on the terrain (default off)", LineKind::Body),
    ("T        temperature heatmap", LineKind::Body),
    ("U        ground saturation heatmap (pores + free water)", LineKind::Body),
    ("M        mycelium strain colors", LineKind::Body),
    ("G        cycle geotech (shear → load → wet → off)", LineKind::Body),
    ("Tab→Climate  Landscape ↔ heatmap blend slider", LineKind::Body),
    ("O        step living creatures", LineKind::Body),
    ("", LineKind::Blank),
    ("Panels", LineKind::Head),
    ("Tab      live settings (World / Climate / Physics / Life)", LineKind::Body),
    ("F1       HUD chrome + inspector", LineKind::Body),
    ("F2       creature editor (spawn plants / fungi / atoms)", LineKind::Body),
    ("F3       terrain editor (paint / erase rock and water)", LineKind::Body),
    ("F4       creature list (living / dead roster)", LineKind::Body),
    ("F5 / F9  save / load  saves/*.gvsesim", LineKind::Body),
    ("F6       this glossary", LineKind::Body),
];

const WATER: &[(&str, LineKind)] = &[
    ("Where the water lives", LineKind::Head),
    ("The grid is a 2D ring of voxels. Each cell has a material and a", LineKind::Body),
    ("sat value (0–255). Air + sat is free water: haze, films, lakes.", LineKind::Body),
    ("Pores in sand / soil / organic hold water too, up to porosity.", LineKind::Body),
    ("", LineKind::Blank),
    ("Humidity is a second store — vapor on a coarse 4×4 tile grid.", LineKind::Body),
    ("It is real mass, not a decoration. Evap moves sat → humidity.", LineKind::Body),
    ("Condensation (C) moves humidity → sat on the ground.", LineKind::Body),
    ("", LineKind::Blank),
    ("Closed loop", LineKind::Head),
    ("A move is conservative when what lands equals what was taken.", LineKind::Body),
    ("Minting means new sat appears with no matching drain — floods", LineKind::Body),
    ("that were not in the ocean or the air. Keep W closed-loop.", LineKind::Body),
    ("C already pays from humidity. N clouds do not hold water.", LineKind::Body),
    ("Bone-dry ground takes a trickle so sheets can run past; uptake", LineKind::Body),
    ("climbs as the top cell wets. Underground peer flow is the same", LineKind::Body),
    ("idea — dry paths crawl, saturated pairs run at full permeability.", LineKind::Body),
    ("Pore water never freefalls through solids — only seepage moves it,", LineKind::Body),
    ("sideways and downward. Standing pond sides and beds soak banks at", LineKind::Body),
    ("full permeability; thin films still shed on bone-dry ground.", LineKind::Body),
    ("Leftover overflows the crest only when the source cell is already", LineKind::Body),
    ("at that height — ponds do not invent head to climb hillsides.", LineKind::Body),
    ("Surface hops stay soft so sheets spread as a gradient, not a", LineKind::Body),
    ("jagged dump of whole cells each pass.", LineKind::Body),
    ("Lake interiors still soak their beds; open surge faces do not", LineKind::Body),
    ("drink the column under them before the sheet moves.", LineKind::Body),
    ("Current washes dead stems and loose Organic downhill.", LineKind::Body),
    ("", LineKind::Blank),
    ("Words", LineKind::Head),
    ("sat        water units in one cell (255 = a full cell)", LineKind::Body),
    ("film       a full-ish wet Air cell sitting on rock or water", LineKind::Body),
    ("tile       4×4 cell block for humidity, wind, and temperature", LineKind::Body),
    ("hum        HUD: total vapor mass on the humidity field", LineKind::Body),
];

const SKY: &[(&str, LineKind)] = &[
    ("How weather runs", LineKind::Head),
    ("1. E  standing water evaporates into the humidity tile above it.", LineKind::Body),
    ("      Warm, windy, dry air pumps faster; ice lids block evap.", LineKind::Body),
    ("2.    Vapor advects with wind and rises (warm under cold lifts).", LineKind::Body),
    ("3. C  leftover vapor condenses: liquid drizzle, or thin frost", LineKind::Body),
    ("      when air and ground are both below freeze.", LineKind::Body),
    ("4. N  draws a few soft banks over the wettest sky tiles.", LineKind::Body),
    ("      Parcels are a picture. They do not rain and do not store.", LineKind::Body),
    ("", LineKind::Blank),
    ("W is optional", LineKind::Head),
    ("Climatic rain is an extra faucet from the sky ceiling. Default", LineKind::Body),
    ("off. When on, keep closed-loop so it drains humidity instead of", LineKind::Body),
    ("minting. Packed snow (cold air) is a W / phase path; C only", LineKind::Body),
    ("glazes frost, never snow towers.", LineKind::Body),
    ("", LineKind::Blank),
    ("Words", LineKind::Head),
    ("drizzle    C condensation (HUD tag). Default rain.", LineKind::Body),
    ("rain       W climatic faucet (HUD). off / on/closed / on/MINT", LineKind::Body),
    ("nimbus     how many N echo parcels are drawn (cap ~36)", LineKind::Body),
    ("echo       display mass of those parcels — not a water store", LineKind::Body),
    ("parcel     one soft cloud blob used for N, shade, and streaks", LineKind::Body),
    ("deck       retired — vapour rise is lapse-driven, not a height lock", LineKind::Body),
];

const GROUND: &[(&str, LineKind)] = &[
    ("Phase (I)", LineKind::Head),
    ("Freeze: only a full water cell becomes Ice (partial films would", LineKind::Body),
    ("mint on thaw). Thaw returns Air + full sat. Slush is snow on", LineKind::Body),
    ("water or rain on ice. Unsupported ice/snow falls as solids.", LineKind::Body),
    ("", LineKind::Blank),
    ("Karst (K)", LineKind::Head),
    ("Wet limestone neighbors can dissolve. Slow on purpose — Tab", LineKind::Body),
    ("period stretches the geology so it does not eat the frame.", LineKind::Body),
    ("", LineKind::Blank),
    ("Grain + failure", LineKind::Head),
    ("Sand / gravel / organic repose and can wash under flow. Organic", LineKind::Body),
    ("mats float, waterlog, and bind to roots / mycelium cream.", LineKind::Body),
    ("Roof collapse drops wide Air ceilings. Shear / compaction are", LineKind::Body),
    ("opt-in on Tab → Physics.", LineKind::Body),
    ("", LineKind::Blank),
    ("Editors", LineKind::Head),
    ("F3 paints the column you see. F2 stamps a creature genome.", LineKind::Body),
    ("Tab → World size + Regenerate rebuilds the ring (keeps seed", LineKind::Body),
    ("unless you press R).", LineKind::Body),
];

const LIFE: &[(&str, LineKind)] = &[
    ("Creatures", LineKind::Head),
    ("One plant, fungus, or atom counts as 1 toward the pop cap, not", LineKind::Body),
    ("each body pixel. F2 paints a body plan; F4 lists living / dead.", LineKind::Body),
    ("O steps them. Corpses linger, then become litter.", LineKind::Body),
    ("", LineKind::Blank),
    ("Plants", LineKind::Head),
    ("Roots drink cell sat. Leaves take sky light (clouds shade).", LineKind::Body),
    ("Genes on Tab → Life set default alloc / shade / digest.", LineKind::Body),
    ("", LineKind::Blank),
    ("Fungi", LineKind::Head),
    ("Mycelium is a cream field on Organic (M overlay). It composts", LineKind::Body),
    ("toward Soil, sticks litter, and can later fruit. Spore bank", LineKind::Body),
    ("hibernates genomes and may germinate on a wet seat.", LineKind::Body),
    ("", LineKind::Blank),
    ("Carbon HUD C=atm/dissolved", LineKind::Head),
    ("Not condensation. Two crude CO₂ buckets: air and water. Plants", LineKind::Body),
    ("and oxidation move them. Unrelated to the C drizzle key.", LineKind::Body),
];

const HUD: &[(&str, LineKind)] = &[
    ("Bottom status line", LineKind::Head),
    ("fps       frames per second (low often means a full sky or flood)", LineKind::Body),
    ("tick      physics steps since this world began", LineKind::Body),
    ("day/night climate clock (Tab sets day + night lengths)", LineKind::Body),
    ("T̄        mean temperature of the thermal tiles (°C)", LineKind::Body),
    ("rain      W faucet: off · on/closed · on/MINT", LineKind::Body),
    ("drizzle   C condensation: on / off", LineKind::Body),
    ("evap      E pump: on / off", LineKind::Body),
    ("phase     I ice/snow pass: on / off", LineKind::Body),
    ("nimbus    N parcel count", LineKind::Body),
    ("echo      N display mass (not inventory)", LineKind::Body),
    ("hum       humidity vapor mass (the sky water store)", LineKind::Body),
    ("C=        carbon atmosphere / dissolved (not the C key)", LineKind::Body),
    ("spores    hibernating genomes in the bank", LineKind::Body),
    ("wind      current horizontal wind (tiles / tick)", LineKind::Body),
    ("creatures living / cap   (p plants  f fungi  a atoms)", LineKind::Body),
    ("dead      lingering corpses", LineKind::Body),
    ("", LineKind::Blank),
    ("If lakes fill while rain=off, look at drizzle=on — that is C.", LineKind::Body),
    ("If hum explodes and FPS dies, C is dumping a mint or a leftover", LineKind::Body),
    ("soaked sky. Start a new world after a remint fix.", LineKind::Dim),
];
