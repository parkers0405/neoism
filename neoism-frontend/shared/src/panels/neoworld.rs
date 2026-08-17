use neoism_neoworld_core::{
    CritterActivity, PetId, PetState, RoomStyle, SimulationEvents, StationKind, Vec2,
    WORLD_FLOOR, WORLD_HEIGHT, WORLD_WIDTH,
};
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

use crate::primitives::IdeTheme;

const DEPTH: f32 = 0.0;
const ORDER_ROOM: u8 = 17;
const ORDER_PET: u8 = 19;
const ORDER_BUBBLE: u8 = 21;

#[derive(Debug)]
pub struct NeoWorldPane {
    pet: PetState,
    name: String,
    last_frame: Instant,
    last_pointer: Instant,
    last_snapshot: Instant,
    press_started: Option<Instant>,
    press_origin: Vec2,
    pet_hit_rect: [f32; 4],
    room_rect: [f32; 4],
    display_scale: f32,
    bubble_seconds: f32,
    impact_seconds: f32,
    animation_seconds: f32,
    snapshot_due: bool,
}

impl NeoWorldPane {
    pub fn new(pet: PetState, name: impl Into<String>) -> Self {
        let now = Instant::now();
        Self {
            pet,
            name: name.into(),
            last_frame: now,
            last_pointer: now,
            last_snapshot: now,
            press_started: None,
            press_origin: Vec2::default(),
            pet_hit_rect: [0.0; 4],
            room_rect: [0.0; 4],
            display_scale: 1.0,
            bubble_seconds: 0.0,
            impact_seconds: 0.0,
            animation_seconds: 0.0,
            snapshot_due: false,
        }
    }

    pub fn preview() -> Self {
        Self::new(
            PetState::new(PetId([0; 16]), Vec2::new(120.0, 150.0)),
            "Pip",
        )
    }

    pub fn pet(&self) -> &PetState {
        &self.pet
    }

    pub fn pet_mut(&mut self) -> &mut PetState {
        &mut self.pet
    }

    pub fn take_periodic_snapshot(&mut self) -> Option<PetState> {
        if !self.snapshot_due && self.last_snapshot.elapsed().as_secs_f32() < 60.0 {
            return None;
        }
        self.last_snapshot = Instant::now();
        self.snapshot_due = false;
        Some(self.pet)
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) -> bool {
        if !contains(self.pet_hit_rect, x, y) {
            return false;
        }
        let local = self.local_point(x, y);
        self.pet.pointer_down(local);
        self.press_started = Some(Instant::now());
        self.press_origin = local;
        self.last_pointer = Instant::now();
        true
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        if !self.pet.grabbed {
            return false;
        }
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_pointer)
            .as_secs_f32();
        self.pet.pointer_move(self.local_point(x, y), elapsed);
        self.last_pointer = now;
        true
    }

    pub fn pointer_up(&mut self, x: f32, y: f32) -> bool {
        if !self.pet.grabbed {
            return false;
        }
        let released = self.local_point(x, y);
        let quick = self
            .press_started
            .take()
            .is_some_and(|start| start.elapsed().as_secs_f32() <= 0.22);
        let moved = (released.x - self.press_origin.x).abs()
            + (released.y - self.press_origin.y).abs();
        self.pet.pointer_up();
        if quick && moved <= 10.0 {
            let events = self.pet.poke();
            self.apply_events(events);
        }
        true
    }

    #[rustfmt::skip]
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        rect: [f32; 4],
        theme: &IdeTheme,
        chrome_scale: f32,
    ) {
        let [x, y, w, h] = rect;
        if w <= 40.0 || h <= 80.0 {
            return;
        }
        let now = Instant::now();
        let elapsed = now
            .saturating_duration_since(self.last_frame)
            .as_secs_f32()
            .min(0.1);
        self.last_frame = now;
        self.bubble_seconds = (self.bubble_seconds - elapsed).max(0.0);
        self.impact_seconds = (self.impact_seconds - elapsed).max(0.0);
        self.animation_seconds += elapsed;
        if self.animation_seconds >= 1_000.0 {
            self.animation_seconds %= 1_000.0;
        }
        let events = self
            .pet
            .step(elapsed, Vec2::new(WORLD_WIDTH, WORLD_FLOOR));
        self.apply_events(events);

        let scale = chrome_scale.clamp(0.75, 2.0);
        let available_scale = (w / (WORLD_WIDTH + 16.0)).min(h / (WORLD_HEIGHT + 16.0));
        let display_scale = if available_scale >= 1.0 {
            available_scale.floor()
        } else {
            available_scale
        };
        let display_w = WORLD_WIDTH * display_scale;
        let display_h = WORLD_HEIGHT * display_scale;
        let display_x = x + (w - display_w) * 0.5;
        let display_y = y + (h - display_h) * 0.5;
        self.room_rect = [display_x, display_y, display_w, display_h];
        self.display_scale = display_scale;
        let clip = Some(self.room_rect);

        sugarloaf.rect(None, x, y, w, h, theme.f32(theme.bg), DEPTH, ORDER_ROOM);
        sugarloaf.rounded_rect(
            None,
            display_x - 8.0 * display_scale,
            display_y - 8.0 * display_scale,
            display_w + 16.0 * display_scale,
            display_h + 16.0 * display_scale,
            theme.f32_alpha(theme.border, 0.72),
            DEPTH,
            7.0 * display_scale,
            ORDER_ROOM,
        );

        let ink = theme.f32(theme.fg);
        let mid = theme.f32_alpha(theme.fg, 0.55);
        let faint = theme.f32_alpha(theme.fg, 0.18);
        let screen = theme.f32(theme.bg);
        sugarloaf.rect(
            None,
            display_x,
            display_y,
            display_w,
            display_h,
            screen,
            DEPTH,
            ORDER_ROOM + 1,
        );

        draw_room_pattern(
            sugarloaf,
            self.room_rect,
            display_scale,
            self.pet.room_plan().wall_pattern,
            faint,
        );
        lcd_rect(sugarloaf, self.room_rect, display_scale, [0.0, WORLD_FLOOR, WORLD_WIDTH, 1.0], ink, ORDER_ROOM + 3);
        lcd_rect(sugarloaf, self.room_rect, display_scale, [self.pet.room_plan().shelf_x, 28.0, 22.0, 1.0], mid, ORDER_ROOM + 3);
        lcd_rect(sugarloaf, self.room_rect, display_scale, [self.pet.room_plan().shelf_x + 16.0, 24.0, 4.0, 4.0], mid, ORDER_ROOM + 3);

        for station in self.pet.room_plan().stations {
            draw_station(sugarloaf, self.room_rect, display_scale, station.kind, station.x, ink, mid);
        }

        let px = display_x + self.pet.position.x * display_scale;
        let py = display_y + self.pet.position.y * display_scale;
        self.pet_hit_rect = [
            px - 8.0 * display_scale,
            py - 28.0 * display_scale,
            16.0 * display_scale,
            30.0 * display_scale,
        ];
        draw_stick_pet(
            sugarloaf,
            self.room_rect,
            display_scale,
            &self.pet,
            self.animation_seconds,
            self.impact_seconds,
            ink,
            screen,
            theme.f32(theme.red),
        );

        if self.bubble_seconds > 0.0 {
            let bubble = [px + 8.0 * display_scale, py - 31.0 * display_scale, 35.0 * display_scale, 13.0 * display_scale];
            sugarloaf.rect(None, bubble[0], bubble[1], bubble[2], bubble[3], ink, DEPTH, ORDER_BUBBLE);
            let bubble_text = DrawOpts {
                font_size: 8.0 * display_scale.min(2.0) * scale,
                color: theme.u8(theme.bg),
                bold: true,
                clip_rect: clip,
                ..DrawOpts::default()
            };
            sugarloaf.text_mut().draw(
                bubble[0] + 3.0 * display_scale,
                bubble[1] + 2.0 * display_scale,
                "@#$%!",
                &bubble_text,
            );
        }

        let label = DrawOpts {
            font_size: 9.0 * scale,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(rect),
            ..DrawOpts::default()
        };
        let style = match self.pet.room_plan().style {
            RoomStyle::Workshop => "WORKSHOP",
            RoomStyle::Greenhouse => "GREENHOUSE",
            RoomStyle::Arcade => "ARCADE",
            RoomStyle::Loft => "LOFT",
        };
        sugarloaf.text_mut().draw(
            display_x,
            display_y - 6.0 * display_scale,
            &format!("{} // {}", self.name.to_uppercase(), style),
            &label,
        );
    }

    fn local_point(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(
            ((x - self.room_rect[0]) / self.display_scale).clamp(0.0, WORLD_WIDTH),
            ((y - self.room_rect[1]) / self.display_scale).clamp(0.0, WORLD_HEIGHT),
        )
    }

    fn apply_events(&mut self, events: SimulationEvents) {
        if events.cursed {
            self.bubble_seconds = 2.4;
            self.snapshot_due = true;
        }
        if events.landed {
            self.impact_seconds = 0.18;
            self.snapshot_due = true;
        }
    }
}

fn contains([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px <= x + w && py >= y && py <= y + h
}

fn lcd_rect(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    [x, y, w, h]: [f32; 4],
    color: [f32; 4],
    order: u8,
) {
    sugarloaf.rect(
        None,
        display[0] + x * scale,
        display[1] + y * scale,
        w * scale,
        h * scale,
        color,
        DEPTH,
        order,
    );
}

#[rustfmt::skip]
fn draw_room_pattern(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    pattern: u8,
    color: [f32; 4],
) {
    match pattern {
        0 => {
            for x in (8..156).step_by(16) {
                lcd_rect(sugarloaf, display, scale, [x as f32, 8.0, 1.0, 74.0], color, ORDER_ROOM + 2);
            }
        }
        1 => {
            for y in (12..88).step_by(14) {
                lcd_rect(sugarloaf, display, scale, [5.0, y as f32, 150.0, 1.0], color, ORDER_ROOM + 2);
            }
        }
        2 => {
            for y in (12..88).step_by(16) {
                for x in ((y / 2)..156).step_by(24) {
                    lcd_rect(sugarloaf, display, scale, [x as f32, y as f32, 5.0, 2.0], color, ORDER_ROOM + 2);
                }
            }
        }
        _ => {
            lcd_rect(sugarloaf, display, scale, [6.0, 8.0, 48.0, 1.0], color, ORDER_ROOM + 2);
            lcd_rect(sugarloaf, display, scale, [106.0, 8.0, 48.0, 1.0], color, ORDER_ROOM + 2);
            lcd_rect(sugarloaf, display, scale, [6.0, 80.0, 48.0, 1.0], color, ORDER_ROOM + 2);
            lcd_rect(sugarloaf, display, scale, [106.0, 80.0, 48.0, 1.0], color, ORDER_ROOM + 2);
        }
    }
}

#[rustfmt::skip]
fn draw_station(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    kind: StationKind,
    x: f32,
    ink: [f32; 4],
    mid: [f32; 4],
) {
    let order = ORDER_ROOM + 4;
    match kind {
        StationKind::Bed => {
            lcd_rect(sugarloaf, display, scale, [x - 16.0, 100.0, 32.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 16.0, 97.0, 1.0, 4.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 15.0, 97.0, 1.0, 4.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 16.0, 97.0, 8.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 15.0, 104.0, 1.0, 4.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 14.0, 104.0, 1.0, 4.0], ink, order);
        }
        StationKind::Game => {
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 82.0, 16.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 82.0, 1.0, 26.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 7.0, 82.0, 1.0, 26.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 107.0, 16.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 5.0, 85.0, 10.0, 8.0], mid, order);
            lcd_rect(sugarloaf, display, scale, [x - 4.0, 86.0, 8.0, 6.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 3.0, 98.0, 2.0, 2.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 2.0, 99.0, 1.0, 1.0], ink, order);
        }
        StationKind::Plant => {
            lcd_rect(sugarloaf, display, scale, [x - 5.0, 102.0, 10.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 4.0, 102.0, 1.0, 6.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 3.0, 102.0, 1.0, 6.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x, 78.0, 1.0, 24.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 82.0, 8.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 1.0, 88.0, 8.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 5.0, 76.0, 6.0, 1.0], ink, order);
        }
        StationKind::PunchBag => {
            lcd_rect(sugarloaf, display, scale, [x, 62.0, 1.0, 14.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 4.0, 76.0, 9.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 4.0, 76.0, 1.0, 26.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 4.0, 76.0, 1.0, 26.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 4.0, 101.0, 9.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 7.0, 107.0, 15.0, 1.0], ink, order);
        }
        StationKind::Desk => {
            lcd_rect(sugarloaf, display, scale, [x - 16.0, 93.0, 32.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 15.0, 93.0, 1.0, 15.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 14.0, 93.0, 1.0, 15.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 6.0, 82.0, 12.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 6.0, 82.0, 1.0, 8.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 5.0, 82.0, 1.0, 8.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 5.0, 83.0, 10.0, 6.0], mid, order);
        }
        StationKind::Radio => {
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 96.0, 16.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 96.0, 1.0, 12.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 7.0, 96.0, 1.0, 12.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 8.0, 107.0, 16.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 5.0, 99.0, 5.0, 5.0], mid, order);
            lcd_rect(sugarloaf, display, scale, [x + 3.0, 100.0, 2.0, 2.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 4.0, 92.0, 9.0, 1.0], ink, order);
        }
        StationKind::Window => {
            lcd_rect(sugarloaf, display, scale, [x - 12.0, 70.0, 24.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 12.0, 70.0, 1.0, 22.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x + 11.0, 70.0, 1.0, 22.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 12.0, 91.0, 24.0, 1.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 1.0, 70.0, 1.0, 22.0], ink, order);
            lcd_rect(sugarloaf, display, scale, [x - 12.0, 80.0, 24.0, 1.0], ink, order);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StickPose {
    Idle,
    Wave,
    WalkA,
    WalkB,
    Work,
    PlayA,
    PlayB,
    PunchA,
    PunchB,
    Sleep,
    Sulk,
}

#[rustfmt::skip]
fn draw_stick_pet(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    pet: &PetState,
    animation_seconds: f32,
    impact_seconds: f32,
    ink: [f32; 4],
    screen: [f32; 4],
    anger: [f32; 4],
) {
    let x = pet.position.x.round();
    let mut y = pet.position.y.round();
    if impact_seconds > 0.0 {
        y += 2.0;
    }
    let facing = if pet.facing_right { 1.0 } else { -1.0 };
    let tick = (animation_seconds * 8.0) as i32;
    let pose = match pet.activity() {
        CritterActivity::Rest => StickPose::Sleep,
        CritterActivity::Play => if tick & 1 == 0 { StickPose::PlayA } else { StickPose::PlayB },
        CritterActivity::Exercise => if tick & 1 == 0 { StickPose::PunchA } else { StickPose::PunchB },
        CritterActivity::Tinker | CritterActivity::Observe => StickPose::Work,
        CritterActivity::Idle => if (tick / 6) & 1 == 0 { StickPose::Idle } else { StickPose::Wave },
        CritterActivity::Sulk => StickPose::Sulk,
        CritterActivity::Wander | CritterActivity::WalkTo => {
            if tick & 1 == 0 { StickPose::WalkA } else { StickPose::WalkB }
        }
    };
    draw_stick_pose(sugarloaf, display, scale, x, y, facing, pose, ink, screen);
    if pet.red_cheeks() {
        let head_y = if pose == StickPose::Sleep { y - 11.0 } else { y - 26.0 };
        lcd_rect(sugarloaf, display, scale, [x - 6.0, head_y - 4.0, 1.0, 3.0], anger, ORDER_PET);
        lcd_rect(sugarloaf, display, scale, [x, head_y - 5.0, 1.0, 4.0], anger, ORDER_PET);
        lcd_rect(sugarloaf, display, scale, [x + 5.0, head_y - 4.0, 1.0, 3.0], anger, ORDER_PET);
    }
}

#[rustfmt::skip]
fn draw_stick_pose(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    x: f32,
    y: f32,
    facing: f32,
    pose: StickPose,
    ink: [f32; 4],
    screen: [f32; 4],
) {
    let sx = |dx: f32| x + dx * facing;

    if pose == StickPose::Sleep {
        hollow_head(sugarloaf, display, scale, x - 10.0, y - 11.0, ink, screen);
        stick_line(sugarloaf, display, scale, [sx(-3.0), y - 8.0], [sx(10.0), y - 7.0], ink);
        stick_line(sugarloaf, display, scale, [sx(-1.0), y - 7.0], [sx(-4.0), y - 4.0], ink);
        stick_line(sugarloaf, display, scale, [sx(8.0), y - 7.0], [sx(12.0), y - 5.0], ink);
        return;
    }

    let head_y = if pose == StickPose::Sulk { -22.0 } else { -26.0 };
    hollow_head(sugarloaf, display, scale, x, y + head_y, ink, screen);
    stick_line(sugarloaf, display, scale, [sx(0.0), y + head_y + 5.0], [sx(0.0), y - 12.0], ink);
    let limbs: &[[f32; 4]] = match pose {
        StickPose::Idle | StickPose::Sulk => &[
            [0.0, -18.0, -7.0, -12.0],
            [0.0, -18.0, 7.0, -12.0],
            [0.0, -12.0, -5.0, 0.0],
            [0.0, -12.0, 5.0, 0.0],
        ],
        StickPose::Wave => &[
            [0.0, -18.0, -7.0, -12.0],
            [0.0, -18.0, 8.0, -24.0],
            [0.0, -12.0, -5.0, 0.0],
            [0.0, -12.0, 5.0, 0.0],
        ],
        StickPose::WalkA => &[
            [0.0, -18.0, -6.0, -10.0],
            [0.0, -18.0, 7.0, -14.0],
            [0.0, -12.0, -7.0, 0.0],
            [0.0, -12.0, 6.0, -4.0],
            [6.0, -4.0, 8.0, 0.0],
        ],
        StickPose::WalkB => &[
            [0.0, -18.0, -7.0, -14.0],
            [0.0, -18.0, 6.0, -10.0],
            [0.0, -12.0, -6.0, -4.0],
            [-6.0, -4.0, -8.0, 0.0],
            [0.0, -12.0, 7.0, 0.0],
        ],
        StickPose::Work => &[
            [0.0, -18.0, -6.0, -13.0],
            [0.0, -18.0, 8.0, -16.0],
            [8.0, -16.0, 10.0, -13.0],
            [0.0, -12.0, -4.0, 0.0],
            [0.0, -12.0, 5.0, 0.0],
        ],
        StickPose::PlayA => &[
            [0.0, -18.0, -8.0, -22.0],
            [0.0, -18.0, 8.0, -22.0],
            [0.0, -12.0, -4.0, 0.0],
            [0.0, -12.0, 6.0, 0.0],
        ],
        StickPose::PlayB => &[
            [0.0, -18.0, -7.0, -12.0],
            [0.0, -18.0, 9.0, -16.0],
            [0.0, -12.0, -6.0, 0.0],
            [0.0, -12.0, 4.0, 0.0],
        ],
        StickPose::PunchA => &[
            [0.0, -18.0, -6.0, -13.0],
            [0.0, -18.0, 10.0, -17.0],
            [0.0, -12.0, -5.0, 0.0],
            [0.0, -12.0, 4.0, 0.0],
        ],
        StickPose::PunchB => &[
            [0.0, -18.0, -5.0, -14.0],
            [0.0, -18.0, 7.0, -12.0],
            [0.0, -12.0, -4.0, 0.0],
            [0.0, -12.0, 6.0, 0.0],
        ],
        StickPose::Sleep => &[],
    };
    for &[ax, ay, bx, by] in limbs {
        stick_line(sugarloaf, display, scale, [sx(ax), y + ay], [sx(bx), y + by], ink);
    }
}

fn hollow_head(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    x: f32,
    y: f32,
    ink: [f32; 4],
    screen: [f32; 4],
) {
    lcd_rect(
        sugarloaf,
        display,
        scale,
        [x - 4.0, y - 5.0, 8.0, 10.0],
        ink,
        ORDER_PET,
    );
    lcd_rect(
        sugarloaf,
        display,
        scale,
        [x - 3.0, y - 4.0, 6.0, 8.0],
        screen,
        ORDER_PET + 1,
    );
}

fn stick_line(
    sugarloaf: &mut Sugarloaf,
    display: [f32; 4],
    scale: f32,
    start: [f32; 2],
    end: [f32; 2],
    color: [f32; 4],
) {
    let dx = end[0] - start[0];
    let dy = end[1] - start[1];
    let steps = dx.abs().max(dy.abs()).max(1.0) as i32;
    for step in 0..=steps {
        let t = step as f32 / steps as f32;
        lcd_rect(
            sugarloaf,
            display,
            scale,
            [
                (start[0] + dx * t).round(),
                (start[1] + dy * t).round(),
                1.0,
                1.0,
            ],
            color,
            ORDER_PET,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pet_hit_test_only_starts_a_grab_on_the_pet() {
        let mut pane = NeoWorldPane::preview();
        pane.pet_hit_rect = [90.0, 60.0, 60.0, 90.0];
        assert!(!pane.pointer_down(20.0, 20.0));
        assert!(pane.pointer_down(110.0, 100.0));
        assert!(pane.pet.grabbed);
    }

    #[test]
    fn landing_requests_a_durable_snapshot_without_constant_writes() {
        let mut pane = NeoWorldPane::preview();
        assert!(pane.take_periodic_snapshot().is_none());
        pane.apply_events(SimulationEvents {
            landed: true,
            ..SimulationEvents::default()
        });
        assert!(pane.take_periodic_snapshot().is_some());
        assert!(pane.take_periodic_snapshot().is_none());
    }

    #[test]
    fn pointer_coordinates_map_into_the_fixed_lcd_world() {
        let mut pane = NeoWorldPane::preview();
        pane.room_rect = [40.0, 20.0, WORLD_WIDTH * 3.0, WORLD_HEIGHT * 3.0];
        pane.display_scale = 3.0;
        assert_eq!(pane.local_point(280.0, 200.0), Vec2::new(80.0, 60.0));
    }
}
