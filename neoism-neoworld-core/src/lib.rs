#![no_std]
#![forbid(unsafe_code)]

//! Deterministic NeoWorld pet simulation shared by Neoism and embedded boxes.
//!
//! Hosts provide time, input, rendering, storage, and networking. Keeping those
//! concerns outside this crate makes Critter behavior reproducible on desktop
//! and constrained hardware while allowing Agent mode to submit only validated
//! high-level intentions later.

const EMOTION_MAX: u16 = 1_000;
const CURSE_THRESHOLD: u16 = 700;
const CURSE_COOLDOWN_SECONDS: f32 = 3.0;

pub const WORLD_WIDTH: f32 = 160.0;
pub const WORLD_HEIGHT: f32 = 120.0;
pub const WORLD_FLOOR: f32 = 108.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PetId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PetMode {
    Critter,
    Agent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CritterActivity {
    Idle,
    Wander,
    WalkTo,
    Play,
    Exercise,
    Rest,
    Tinker,
    Observe,
    Sulk,
}

/// How the current activity is being performed. Styles are picked with
/// weighted rolls shaped by temperament and the live emotion mix, so two pets
/// (or the same pet an hour later) act out the same activity differently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CritterStyle {
    Fidget,
    Stroll,
    March,
    Skip,
    Sneak,
    Moonwalk,
    Zoomies,
    Dance,
    Robot,
    AirGuitar,
    Juggle,
    Spin,
    Boxing,
    Pushups,
    Squats,
    JumpRope,
    Sleep,
    Hammer,
    Read,
    Gaze,
    WaterPlant,
    BirdWatch,
    Fume,
    KickGround,
}

/// Short one-shot interruptions layered over the current activity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    Sneeze,
    Shiver,
    Hiccup,
    Stare,
    Trip,
    Dizzy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomStyle {
    Workshop,
    Greenhouse,
    Arcade,
    Loft,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StationKind {
    Bed,
    Game,
    Plant,
    PunchBag,
    Desk,
    Radio,
    Window,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Station {
    pub kind: StationKind,
    pub x: f32,
}

impl Station {
    pub fn interaction_x(self) -> f32 {
        if self.kind == StationKind::Bed {
            self.x
        } else if self.x < WORLD_WIDTH * 0.5 {
            self.x + 15.0
        } else {
            self.x - 15.0
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoomPlan {
    pub style: RoomStyle,
    pub stations: [Station; 3],
    pub shelf_x: f32,
    pub wall_pattern: u8,
}

impl RoomPlan {
    pub fn from_id(id: PetId) -> Self {
        let seed = behavior_seed(id);
        let style = match seed & 3 {
            0 => RoomStyle::Workshop,
            1 => RoomStyle::Greenhouse,
            2 => RoomStyle::Arcade,
            _ => RoomStyle::Loft,
        };
        let options = match style {
            RoomStyle::Workshop => [StationKind::Desk, StationKind::Radio],
            RoomStyle::Greenhouse => [StationKind::Plant, StationKind::Window],
            RoomStyle::Arcade => [StationKind::Game, StationKind::Radio],
            RoomStyle::Loft => [StationKind::PunchBag, StationKind::Window],
        };
        let selected = options[((seed >> 2) & 1) as usize];
        let mut alternate = [
            StationKind::Game,
            StationKind::Plant,
            StationKind::PunchBag,
            StationKind::Desk,
        ][((seed >> 5) & 3) as usize];
        if alternate == selected {
            alternate = options[1 - ((seed >> 2) & 1) as usize];
        }
        let mut stations = [
            Station {
                kind: StationKind::Bed,
                x: 24.0,
            },
            Station {
                kind: selected,
                x: 80.0,
            },
            Station {
                kind: alternate,
                x: 136.0,
            },
        ];
        if seed & 0x100 != 0 {
            stations.swap(0, 2);
        }
        Self {
            style,
            stations,
            shelf_x: 54.0 + ((seed >> 10) % 52) as f32,
            wall_pattern: ((seed >> 16) & 3) as u8,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temperament {
    Gentle,
    Playful,
    Dramatic,
    Grumpy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PetAppearance {
    pub head: u8,
    pub accessory: u8,
    pub build: u8,
}

impl Temperament {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gentle => "gentle",
            Self::Playful => "playful",
            Self::Dramatic => "dramatic",
            Self::Grumpy => "grumpy",
        }
    }

    const fn poke_irritation(self) -> u16 {
        match self {
            Self::Gentle => 72,
            Self::Playful => 92,
            Self::Dramatic => 138,
            Self::Grumpy => 164,
        }
    }

    const fn walking_speed(self) -> f32 {
        match self {
            Self::Gentle => 35.0,
            Self::Playful => 56.0,
            Self::Dramatic => 46.0,
            Self::Grumpy => 40.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emotions {
    pub happiness: u16,
    pub irritation: u16,
    pub excitement: u16,
    pub affection: u16,
    pub tiredness: u16,
    pub loneliness: u16,
}

impl Default for Emotions {
    fn default() -> Self {
        Self {
            happiness: 650,
            irritation: 0,
            excitement: 300,
            affection: 500,
            tiredness: 150,
            loneliness: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SimulationEvents {
    pub poked: bool,
    pub cursed: bool,
    pub landed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PetState {
    pub id: PetId,
    pub mode: PetMode,
    pub position: Vec2,
    pub velocity: Vec2,
    pub emotions: Emotions,
    pub grabbed: bool,
    pub facing_right: bool,
    curse_cooldown: f32,
    emotion_seconds: f32,
    activity: CritterActivity,
    activity_seconds: f32,
    activity_duration: f32,
    flavor: u8,
    moment: Option<Moment>,
    moment_seconds: f32,
    moment_duration: f32,
    moment_cooldown: f32,
    behavior_rng: u32,
    walk_direction: f32,
    target_station: u8,
    sulk_lockout: f32,
    pointer: Vec2,
    pointer_velocity: Vec2,
}

impl PetState {
    pub fn new(id: PetId, position: Vec2) -> Self {
        let walk_direction = if id.0[0] & 1 == 0 { 1.0 } else { -1.0 };
        let behavior_rng = behavior_seed(id);
        Self {
            id,
            mode: PetMode::Critter,
            position,
            velocity: Vec2::default(),
            emotions: Emotions::default(),
            grabbed: false,
            facing_right: true,
            curse_cooldown: 0.0,
            emotion_seconds: 0.0,
            activity: CritterActivity::Idle,
            activity_seconds: 0.0,
            activity_duration: 0.8 + (behavior_rng & 255) as f32 / 255.0,
            flavor: 0,
            moment: None,
            moment_seconds: 0.0,
            moment_duration: 0.0,
            moment_cooldown: 4.0 + ((behavior_rng >> 8) & 255) as f32 / 64.0,
            behavior_rng,
            walk_direction,
            target_station: 0,
            sulk_lockout: 0.0,
            pointer: position,
            pointer_velocity: Vec2::default(),
        }
    }

    pub fn restored(
        id: PetId,
        mode: PetMode,
        position: Vec2,
        velocity: Vec2,
        emotions: Emotions,
        facing_right: bool,
    ) -> Self {
        let mut pet = Self::new(id, position);
        pet.mode = mode;
        pet.velocity = velocity;
        pet.emotions = emotions;
        pet.facing_right = facing_right;
        pet
    }

    pub fn red_cheeks(&self) -> bool {
        self.emotions.irritation >= 450
    }

    pub const fn temperament(&self) -> Temperament {
        match self.id.0[1] & 3 {
            0 => Temperament::Gentle,
            1 => Temperament::Playful,
            2 => Temperament::Dramatic,
            _ => Temperament::Grumpy,
        }
    }

    pub const fn appearance(&self) -> PetAppearance {
        PetAppearance {
            head: self.id.0[2] % 3,
            accessory: self.id.0[3] % 4,
            build: self.id.0[4] % 3,
        }
    }

    pub const fn activity(&self) -> CritterActivity {
        self.activity
    }

    pub const fn moment(&self) -> Option<Moment> {
        self.moment
    }

    pub fn moment_phase(&self) -> f32 {
        if self.moment_duration <= 0.0 {
            0.0
        } else {
            (self.moment_seconds / self.moment_duration).clamp(0.0, 1.0)
        }
    }

    /// Resolve (activity, flavor) into the concrete performance style.
    pub const fn style(&self) -> CritterStyle {
        match self.activity {
            CritterActivity::Idle => CritterStyle::Fidget,
            CritterActivity::WalkTo => CritterStyle::Stroll,
            CritterActivity::Wander => match self.flavor {
                1 => CritterStyle::March,
                2 => CritterStyle::Skip,
                3 => CritterStyle::Sneak,
                4 => CritterStyle::Moonwalk,
                5 => CritterStyle::Zoomies,
                _ => CritterStyle::Stroll,
            },
            CritterActivity::Play => match self.flavor {
                1 => CritterStyle::Robot,
                2 => CritterStyle::AirGuitar,
                3 => CritterStyle::Juggle,
                4 => CritterStyle::Spin,
                _ => CritterStyle::Dance,
            },
            CritterActivity::Exercise => match self.flavor {
                1 => CritterStyle::Pushups,
                2 => CritterStyle::Squats,
                3 => CritterStyle::JumpRope,
                _ => CritterStyle::Boxing,
            },
            CritterActivity::Rest => CritterStyle::Sleep,
            CritterActivity::Tinker => match self.flavor {
                1 => CritterStyle::Read,
                _ => CritterStyle::Hammer,
            },
            CritterActivity::Observe => match self.flavor {
                1 => CritterStyle::WaterPlant,
                2 => CritterStyle::BirdWatch,
                _ => CritterStyle::Gaze,
            },
            CritterActivity::Sulk => match self.flavor {
                1 => CritterStyle::KickGround,
                _ => CritterStyle::Fume,
            },
        }
    }

    pub const fn target_station(&self) -> u8 {
        self.target_station
    }

    pub fn activity_phase(&self) -> f32 {
        if self.activity_duration <= 0.0 {
            0.0
        } else {
            (self.activity_seconds / self.activity_duration).clamp(0.0, 1.0)
        }
    }

    pub fn pointer_down(&mut self, pointer: Vec2) -> SimulationEvents {
        self.pointer = pointer;
        self.pointer_velocity = Vec2::default();
        self.grabbed = true;
        SimulationEvents::default()
    }

    pub fn pointer_move(&mut self, pointer: Vec2, elapsed_seconds: f32) {
        let elapsed = elapsed_seconds.max(1.0 / 1_000.0);
        let sample = Vec2::new(
            (pointer.x - self.pointer.x) / elapsed,
            (pointer.y - self.pointer.y) / elapsed,
        );
        self.pointer_velocity.x = self.pointer_velocity.x * 0.55 + sample.x * 0.45;
        self.pointer_velocity.y = self.pointer_velocity.y * 0.55 + sample.y * 0.45;
        self.pointer = pointer;
        if sample.x.abs() > 0.5 {
            self.facing_right = sample.x > 0.0;
        }
    }

    pub fn pointer_up(&mut self) {
        if self.grabbed {
            self.velocity = self.pointer_velocity;
        }
        self.grabbed = false;
    }

    pub fn poke(&mut self) -> SimulationEvents {
        let previous = self.emotions.irritation;
        self.emotions.irritation = self
            .emotions
            .irritation
            .saturating_add(self.temperament().poke_irritation())
            .min(EMOTION_MAX);
        self.emotions.affection = self.emotions.affection.saturating_sub(8);
        let crossed_threshold =
            previous < CURSE_THRESHOLD && self.emotions.irritation >= CURSE_THRESHOLD;
        let cursed = crossed_threshold
            || (self.curse_cooldown <= 0.0
                && self.emotions.irritation >= CURSE_THRESHOLD);
        if cursed {
            self.curse_cooldown = CURSE_COOLDOWN_SECONDS;
        }
        if self.emotions.irritation >= CURSE_THRESHOLD && self.sulk_lockout <= 0.0 {
            self.activity = CritterActivity::Sulk;
            self.flavor = self.pick_sulk_flavor();
            self.activity_seconds = 0.0;
            self.activity_duration = 2.4 + (self.next_random() % 120) as f32 / 100.0;
            self.walk_direction = -self.walk_direction;
        }
        SimulationEvents {
            poked: true,
            cursed,
            landed: false,
        }
    }

    pub fn step(&mut self, elapsed_seconds: f32, room_size: Vec2) -> SimulationEvents {
        let dt = elapsed_seconds.clamp(0.0, 1.0 / 30.0);
        self.curse_cooldown = (self.curse_cooldown - dt).max(0.0);
        self.sulk_lockout = (self.sulk_lockout - dt).max(0.0);
        self.emotion_seconds += dt;
        self.activity_seconds += dt;
        if self.emotion_seconds >= 1.0 {
            let seconds = self.emotion_seconds as u16;
            self.emotion_seconds -= f32::from(seconds);
            let cool = if self.activity == CritterActivity::Sulk {
                48
            } else {
                18
            };
            self.emotions.irritation = self
                .emotions
                .irritation
                .saturating_sub(seconds.saturating_mul(cool));
            self.emotions.excitement = self
                .emotions
                .excitement
                .saturating_sub(seconds.saturating_mul(2));
            self.emotions.tiredness = self
                .emotions
                .tiredness
                .saturating_add(seconds.saturating_mul(2))
                .min(EMOTION_MAX);
            self.emotions.loneliness = self
                .emotions
                .loneliness
                .saturating_add(seconds)
                .min(EMOTION_MAX);
            self.emotions.happiness = self.emotions.happiness.saturating_sub(seconds);
        }
        let mut events = SimulationEvents::default();
        let floor = room_size.y.max(1.0);
        let grounded = self.position.y >= floor - 0.5 && self.velocity.y.abs() < 24.0;
        if self.moment.is_some() {
            self.moment_seconds += dt;
            if self.moment_seconds >= self.moment_duration {
                self.moment = None;
                self.moment_cooldown = 5.0 + (self.next_random() % 900) as f32 / 100.0;
            }
        } else if !self.grabbed
            && self.mode == PetMode::Critter
            && grounded
            && !matches!(self.activity, CritterActivity::Rest | CritterActivity::Sulk)
        {
            self.moment_cooldown -= dt;
            if self.moment_cooldown <= 0.0 {
                self.roll_moment();
            }
        }
        if !self.grabbed && self.mode == PetMode::Critter && grounded {
            if self.activity == CritterActivity::WalkTo {
                let station = self.room_plan().stations[self.target_station as usize];
                if (self.position.x - station.interaction_x()).abs() <= 2.5 {
                    self.position.x = station.interaction_x();
                    self.begin_station_activity(station);
                }
            } else if self.activity_seconds >= self.activity_duration {
                self.choose_next_activity();
            }
        }

        if self.grabbed {
            const SPRING: f32 = 90.0;
            const DAMPING: f32 = 13.0;
            self.velocity.x += ((self.pointer.x - self.position.x) * SPRING
                - self.velocity.x * DAMPING)
                * dt;
            self.velocity.y += ((self.pointer.y - self.position.y) * SPRING
                - self.velocity.y * DAMPING)
                * dt;
        } else {
            self.velocity.y += 780.0 * dt;
            if self.mode == PetMode::Critter && grounded && self.velocity.x.abs() < 220.0
            {
                let walking = matches!(
                    self.activity,
                    CritterActivity::Wander
                        | CritterActivity::WalkTo
                        | CritterActivity::Sulk
                );
                let angry_bonus = if self.emotions.irritation >= 600 {
                    28.0
                } else {
                    0.0
                };
                let desired = if self.moment.is_some() {
                    0.0 // stop mid-track for sneezes, trips, stares, ...
                } else if self.activity == CritterActivity::WalkTo {
                    let target = self.room_plan().stations[self.target_station as usize]
                        .interaction_x();
                    self.walk_direction =
                        if target >= self.position.x { 1.0 } else { -1.0 };
                    self.walk_direction * (self.temperament().walking_speed() + 8.0)
                } else if walking {
                    let style_speed = if self.activity == CritterActivity::Wander {
                        match self.flavor {
                            1 => 0.8,  // march
                            2 => 1.15, // skip
                            3 => 0.45, // sneak
                            4 => 0.55, // moonwalk
                            5 => 2.4,  // zoomies
                            _ => 1.0,
                        }
                    } else {
                        1.0
                    };
                    self.walk_direction
                        * (self.temperament().walking_speed() + angry_bonus)
                        * style_speed
                } else {
                    0.0
                };
                self.velocity.x += (desired - self.velocity.x) * (7.0 * dt).min(1.0);
                if walking && self.position.x <= 12.0 {
                    self.walk_direction = 1.0;
                } else if walking && self.position.x >= room_size.x - 12.0 {
                    self.walk_direction = -1.0;
                }
                if self.velocity.x.abs() > 0.5 {
                    self.facing_right = self.velocity.x > 0.0;
                }
            }
        }

        self.position.x += self.velocity.x * dt;
        self.position.y += self.velocity.y * dt;

        if self.position.y > floor {
            events.landed = self.velocity.y > 90.0;
            self.position.y = floor;
            self.velocity.y = if self.velocity.y > 90.0 {
                -self.velocity.y * 0.28
            } else {
                0.0
            };
            if events.landed {
                self.velocity.x *= 0.82;
                if self.mode == PetMode::Critter && !self.grabbed {
                    self.moment = Some(Moment::Dizzy);
                    self.moment_seconds = 0.0;
                    self.moment_duration = 1.5;
                }
            }
        }
        if self.position.x < 0.0 {
            self.position.x = 0.0;
            self.velocity.x = self.velocity.x.abs() * 0.35;
        } else if self.position.x > room_size.x {
            self.position.x = room_size.x;
            self.velocity.x = -self.velocity.x.abs() * 0.35;
        }

        events
    }

    /// Weighted roll over (flavor, weight) rows. Zero-weight rows are legal
    /// and simply unreachable, which lets tables stay static while emotions
    /// scale individual entries.
    fn weighted_flavor(&mut self, table: &[(u8, u16)]) -> u8 {
        let mut total: u32 = 0;
        for (_, weight) in table {
            total += u32::from(*weight);
        }
        if total == 0 {
            return 0;
        }
        let mut roll = self.next_random() % total;
        for (flavor, weight) in table {
            if roll < u32::from(*weight) {
                return *flavor;
            }
            roll -= u32::from(*weight);
        }
        0
    }

    fn pick_wander_flavor(&mut self) -> u8 {
        let excitement = self.emotions.excitement;
        let tiredness = self.emotions.tiredness;
        let temperament = self.temperament();
        let playful = (temperament == Temperament::Playful) as u16;
        let grumpy = (temperament == Temperament::Grumpy) as u16;
        let dramatic = (temperament == Temperament::Dramatic) as u16;
        self.weighted_flavor(&[
            (0, 42 + tiredness / 14),                // stroll
            (1, 8 + grumpy * 20 + dramatic * 6),     // march
            (2, 6 + excitement / 40 + playful * 12), // skip
            (3, 7 + grumpy * 5),                     // sneak
            (4, 3 + excitement / 90 + dramatic * 8), // moonwalk
            (
                5,
                if tiredness >= 650 {
                    0
                } else {
                    2 + excitement / 24 + playful * 12
                },
            ), // zoomies
        ])
    }

    fn pick_sulk_flavor(&mut self) -> u8 {
        let grumpy = (self.temperament() == Temperament::Grumpy) as u16;
        let irritation = self.emotions.irritation;
        self.weighted_flavor(&[
            (0, 55),                                 // fume
            (1, 20 + grumpy * 22 + irritation / 40), // kick the ground
        ])
    }

    fn roll_moment(&mut self) {
        let walking = matches!(
            self.activity,
            CritterActivity::Wander | CritterActivity::WalkTo
        );
        let dramatic = (self.temperament() == Temperament::Dramatic) as u16;
        let greenhouse = (self.room_plan().style == RoomStyle::Greenhouse) as u16;
        let choice = self.weighted_flavor(&[
            (0, 130),                                         // nothing this time
            (1, 10 + greenhouse * 10),                        // sneeze
            (2, 8),                                           // shiver
            (3, 9),                                           // hiccup
            (4, 6 + self.emotions.loneliness / 50),           // stare at the viewer
            (5, if walking { 8 + dramatic * 16 } else { 0 }), // trip
        ]);
        let (moment, duration) = match choice {
            1 => (Some(Moment::Sneeze), 1.0),
            2 => (Some(Moment::Shiver), 1.2),
            3 => (Some(Moment::Hiccup), 1.0),
            4 => (Some(Moment::Stare), 1.8),
            5 => (Some(Moment::Trip), 1.3),
            _ => (None, 0.0),
        };
        if let Some(moment) = moment {
            self.moment = Some(moment);
            self.moment_seconds = 0.0;
            self.moment_duration = duration;
            if moment == Moment::Trip {
                self.velocity.x *= 0.2;
            }
            if moment == Moment::Stare {
                self.emotions.loneliness = self.emotions.loneliness.saturating_sub(120);
            }
        } else {
            self.moment_cooldown = 2.5 + (self.next_random() % 500) as f32 / 100.0;
        }
    }

    fn choose_next_activity(&mut self) {
        if self.activity == CritterActivity::Sulk {
            self.sulk_lockout = 8.0;
            self.emotions.irritation = self.emotions.irritation.min(420);
        }
        let roll = self.next_random() % 100;
        self.activity =
            if self.emotions.irritation >= CURSE_THRESHOLD && self.sulk_lockout <= 0.0 {
                CritterActivity::Sulk
            } else if self.emotions.tiredness >= 720 {
                self.target_station = self.station_index(StationKind::Bed).unwrap_or(0);
                CritterActivity::WalkTo
            } else if roll < 18 {
                CritterActivity::Idle
            } else if roll < 35 {
                CritterActivity::Wander
            } else {
                self.target_station = if roll >= 72 {
                    self.preferred_station()
                } else {
                    (self.next_random() % 3) as u8
                };
                CritterActivity::WalkTo
            };
        self.flavor = match self.activity {
            CritterActivity::Wander => self.pick_wander_flavor(),
            CritterActivity::Sulk => self.pick_sulk_flavor(),
            _ => 0,
        };
        if self.activity == CritterActivity::Wander && self.flavor == 5 {
            self.emotions.excitement =
                self.emotions.excitement.saturating_add(70).min(EMOTION_MAX);
        }
        self.activity_seconds = 0.0;
        let variation = (self.next_random() % 180) as f32 / 100.0;
        self.activity_duration = match self.activity {
            CritterActivity::Idle => 0.8 + variation,
            CritterActivity::Wander => 1.4 + variation,
            CritterActivity::WalkTo => 8.0,
            CritterActivity::Play => 2.4 + variation,
            CritterActivity::Exercise => 2.0 + variation,
            CritterActivity::Rest => 3.5 + variation,
            CritterActivity::Tinker => 2.8 + variation,
            CritterActivity::Observe => 2.2 + variation,
            CritterActivity::Sulk => 1.8 + variation,
        };
        if self.next_random() & 1 == 0 {
            self.walk_direction = -self.walk_direction;
        }
        if self.temperament() == Temperament::Playful
            && matches!(self.activity, CritterActivity::Wander)
            && self.next_random() % 3 == 0
        {
            self.velocity.y = -155.0;
        }
    }

    pub fn room_plan(&self) -> RoomPlan {
        RoomPlan::from_id(self.id)
    }

    fn station_index(&self, kind: StationKind) -> Option<u8> {
        self.room_plan()
            .stations
            .iter()
            .position(|station| station.kind == kind)
            .map(|index| index as u8)
    }

    fn preferred_station(&self) -> u8 {
        let preferred = if self.emotions.loneliness >= 650 {
            [StationKind::Radio, StationKind::Game]
        } else {
            match self.temperament() {
                Temperament::Gentle => [StationKind::Plant, StationKind::Window],
                Temperament::Playful => [StationKind::Game, StationKind::PunchBag],
                Temperament::Dramatic => [StationKind::Radio, StationKind::Window],
                Temperament::Grumpy => [StationKind::Desk, StationKind::PunchBag],
            }
        };
        self.station_index(preferred[0])
            .or_else(|| self.station_index(preferred[1]))
            .unwrap_or(1)
    }

    fn begin_station_activity(&mut self, station: Station) {
        self.facing_right = station.x >= self.position.x;
        self.activity = match station.kind {
            StationKind::Bed => CritterActivity::Rest,
            StationKind::Game | StationKind::Radio => CritterActivity::Play,
            StationKind::PunchBag => CritterActivity::Exercise,
            StationKind::Desk => CritterActivity::Tinker,
            StationKind::Plant | StationKind::Window => CritterActivity::Observe,
        };
        let excitement = self.emotions.excitement;
        let tiredness = self.emotions.tiredness;
        let temperament = self.temperament();
        let playful = (temperament == Temperament::Playful) as u16;
        let gentle = (temperament == Temperament::Gentle) as u16;
        let dramatic = (temperament == Temperament::Dramatic) as u16;
        self.flavor = match self.activity {
            CritterActivity::Play => self.weighted_flavor(&[
                (0, 34),                                 // dance
                (1, 12 + dramatic * 14),                 // robot
                (2, 14 + excitement / 30),               // air guitar
                (3, 10 + gentle * 12),                   // juggle
                (4, 6 + excitement / 40 + playful * 10), // spin
            ]),
            CritterActivity::Exercise => self.weighted_flavor(&[
                (0, 38),                                  // boxing
                (1, 14),                                  // pushups
                (2, 12),                                  // squats
                (3, 10 + excitement / 40 + playful * 12), // jump rope
            ]),
            CritterActivity::Tinker => self.weighted_flavor(&[
                (0, 50),                                // hammer away
                (1, 24 + gentle * 16 + tiredness / 24), // read instead
            ]),
            CritterActivity::Observe => match station.kind {
                StationKind::Plant => self.weighted_flavor(&[
                    (0, 40),               // gaze
                    (1, 30 + gentle * 18), // water the plant
                ]),
                StationKind::Window => self.weighted_flavor(&[
                    (0, 40),                   // gaze
                    (2, 28 + excitement / 40), // bird watch
                ]),
                _ => 0,
            },
            _ => 0,
        };
        if self.activity == CritterActivity::Tinker && self.flavor == 1 {
            self.emotions.excitement = self.emotions.excitement.saturating_sub(40);
        }
        self.activity_seconds = 0.0;
        let variation = (self.next_random() % 160) as f32 / 100.0;
        self.activity_duration = match self.activity {
            CritterActivity::Rest => 4.5 + variation,
            CritterActivity::Exercise => 2.8 + variation,
            CritterActivity::Play => 3.2 + variation,
            CritterActivity::Tinker => 3.6 + variation,
            CritterActivity::Observe => 2.6 + variation,
            _ => 2.0 + variation,
        };
        match self.activity {
            CritterActivity::Rest => {
                self.emotions.tiredness = self.emotions.tiredness.saturating_sub(420);
                self.emotions.irritation = self.emotions.irritation.saturating_sub(80);
            }
            CritterActivity::Play => {
                self.emotions.happiness =
                    self.emotions.happiness.saturating_add(90).min(EMOTION_MAX);
                self.emotions.excitement = self
                    .emotions
                    .excitement
                    .saturating_add(140)
                    .min(EMOTION_MAX);
                self.emotions.loneliness = self.emotions.loneliness.saturating_sub(60);
            }
            CritterActivity::Exercise => {
                self.emotions.happiness =
                    self.emotions.happiness.saturating_add(45).min(EMOTION_MAX);
                self.emotions.tiredness =
                    self.emotions.tiredness.saturating_add(120).min(EMOTION_MAX);
            }
            CritterActivity::Tinker | CritterActivity::Observe => {
                self.emotions.happiness =
                    self.emotions.happiness.saturating_add(35).min(EMOTION_MAX);
                self.emotions.loneliness = self.emotions.loneliness.saturating_sub(30);
            }
            _ => {}
        }
    }

    fn next_random(&mut self) -> u32 {
        let mut value = self.behavior_rng;
        value ^= value << 13;
        value ^= value >> 17;
        value ^= value << 5;
        self.behavior_rng = value;
        value
    }
}

fn behavior_seed(id: PetId) -> u32 {
    let mut seed: u32 = 0x9e37_79b9;
    let mut index = 0;
    while index < id.0.len() {
        seed = seed.rotate_left(5) ^ u32::from(id.0[index]);
        seed = seed.wrapping_mul(0x85eb_ca6b);
        index += 1;
    }
    if seed == 0 {
        0xa341_316c
    } else {
        seed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pet() -> PetState {
        PetState::new(PetId([7; 16]), Vec2::new(100.0, 100.0))
    }

    #[test]
    fn repeated_pokes_cause_blushing_and_a_curse() {
        let mut pet = pet();
        let mut cursed = false;
        for _ in 0..8 {
            cursed |= pet.poke().cursed;
        }
        assert!(pet.red_cheeks());
        assert!(cursed);
    }

    #[test]
    fn dragging_then_releasing_preserves_throw_velocity() {
        let mut pet = pet();
        pet.pointer_down(Vec2::new(100.0, 100.0));
        pet.pointer_move(Vec2::new(160.0, 80.0), 0.05);
        pet.pointer_up();
        assert!(pet.velocity.x > 500.0);
        assert!(pet.velocity.y < 0.0);
    }

    #[test]
    fn falling_pet_lands_inside_room() {
        let mut pet = pet();
        pet.velocity.y = 500.0;
        let mut landed = false;
        for _ in 0..30 {
            landed |= pet.step(1.0 / 120.0, Vec2::new(240.0, 180.0)).landed;
        }
        assert!(landed);
        assert!(pet.position.y <= 180.0);
    }

    #[test]
    fn irritation_cools_down_over_time() {
        let mut pet = pet();
        for _ in 0..8 {
            pet.poke();
        }
        let irritated = pet.emotions.irritation;
        for _ in 0..240 {
            pet.step(1.0 / 120.0, Vec2::new(240.0, 180.0));
        }
        assert!(pet.emotions.irritation < irritated);
    }

    #[test]
    fn sulk_ends_instead_of_locking_forever() {
        let mut pet = pet();
        pet.emotions.irritation = EMOTION_MAX;
        pet.poke();
        assert_eq!(pet.activity(), CritterActivity::Sulk);
        let mut left_sulk = false;
        for _ in 0..2_400 {
            pet.step(1.0 / 60.0, Vec2::new(WORLD_WIDTH, WORLD_FLOOR));
            if pet.activity() != CritterActivity::Sulk {
                left_sulk = true;
                break;
            }
        }
        assert!(left_sulk);
        assert_ne!(pet.activity(), CritterActivity::Sulk);
    }

    #[test]
    fn critter_explores_without_pointer_input() {
        let mut pet = PetState::new(PetId([8; 16]), Vec2::new(100.0, WORLD_FLOOR));
        let start = pet.position.x;
        let mut wandered = false;
        let mut used_station = false;
        for _ in 0..7_200 {
            pet.step(1.0 / 120.0, Vec2::new(WORLD_WIDTH, WORLD_FLOOR));
            wandered |= pet.activity() == CritterActivity::Wander;
            used_station |= matches!(
                pet.activity(),
                CritterActivity::Play
                    | CritterActivity::Exercise
                    | CritterActivity::Rest
                    | CritterActivity::Tinker
                    | CritterActivity::Observe
            );
        }
        assert!(wandered);
        assert!(used_station);
        assert!((pet.position.x - start).abs() > 5.0);
    }

    fn style_slot(style: CritterStyle) -> usize {
        match style {
            CritterStyle::Fidget => 0,
            CritterStyle::Stroll => 1,
            CritterStyle::March => 2,
            CritterStyle::Skip => 3,
            CritterStyle::Sneak => 4,
            CritterStyle::Moonwalk => 5,
            CritterStyle::Zoomies => 6,
            CritterStyle::Dance => 7,
            CritterStyle::Robot => 8,
            CritterStyle::AirGuitar => 9,
            CritterStyle::Juggle => 10,
            CritterStyle::Spin => 11,
            CritterStyle::Boxing => 12,
            CritterStyle::Pushups => 13,
            CritterStyle::Squats => 14,
            CritterStyle::JumpRope => 15,
            CritterStyle::Sleep => 16,
            CritterStyle::Hammer => 17,
            CritterStyle::Read => 18,
            CritterStyle::Gaze => 19,
            CritterStyle::WaterPlant => 20,
            CritterStyle::BirdWatch => 21,
            CritterStyle::Fume => 22,
            CritterStyle::KickGround => 23,
        }
    }

    #[test]
    fn behavior_styles_vary_over_a_long_run() {
        let mut seen = [false; 24];
        for seed in 0u8..6 {
            let mut pet = PetState::new(
                PetId([seed.wrapping_mul(37).wrapping_add(3); 16]),
                Vec2::new(100.0, WORLD_FLOOR),
            );
            for _ in 0..36_000 {
                pet.step(1.0 / 120.0, Vec2::new(WORLD_WIDTH, WORLD_FLOOR));
                seen[style_slot(pet.style())] = true;
            }
        }
        let distinct = seen.iter().filter(|&&s| s).count();
        assert!(distinct >= 8, "only {distinct} distinct styles seen");
    }

    #[test]
    fn moments_fire_and_then_clear() {
        let mut pet = PetState::new(PetId([9; 16]), Vec2::new(100.0, WORLD_FLOOR));
        let mut fired = false;
        let mut cleared_after = false;
        for _ in 0..72_000 {
            pet.step(1.0 / 120.0, Vec2::new(WORLD_WIDTH, WORLD_FLOOR));
            if pet.moment().is_some() {
                fired = true;
            } else if fired {
                cleared_after = true;
                break;
            }
        }
        assert!(fired && cleared_after);
    }

    #[test]
    fn a_hard_landing_leaves_the_pet_dizzy() {
        let mut pet = PetState::new(PetId([4; 16]), Vec2::new(80.0, 20.0));
        pet.velocity.y = 400.0;
        let mut landed = false;
        for _ in 0..240 {
            landed |= pet
                .step(1.0 / 120.0, Vec2::new(WORLD_WIDTH, WORLD_FLOOR))
                .landed;
            if landed {
                break;
            }
        }
        assert!(landed);
        assert_eq!(pet.moment(), Some(Moment::Dizzy));
    }

    #[test]
    fn identity_generates_a_stable_room_with_a_bed() {
        let id = PetId([23; 16]);
        let first = RoomPlan::from_id(id);
        let second = RoomPlan::from_id(id);
        assert_eq!(first, second);
        assert!(first
            .stations
            .iter()
            .any(|station| station.kind == StationKind::Bed));
    }

    #[test]
    fn different_identities_generate_different_rooms() {
        let first = RoomPlan::from_id(PetId([1; 16]));
        let second = RoomPlan::from_id(PetId([2; 16]));
        assert_ne!(first, second);
    }

    #[test]
    fn identity_deterministically_selects_temperament() {
        let gentle = PetState::new(PetId([0; 16]), Vec2::default());
        let mut grumpy_id = [0; 16];
        grumpy_id[1] = 3;
        let grumpy = PetState::new(PetId(grumpy_id), Vec2::default());
        assert_eq!(gentle.temperament(), Temperament::Gentle);
        assert_eq!(grumpy.temperament(), Temperament::Grumpy);
    }
}
