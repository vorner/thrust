use std::cell::RefCell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use derive_more::Sub;
use quicksilver::QuicksilverError as QError;
use quicksilver::geom::{Circle, Rectangle, Vector, Transform};
use quicksilver::graphics::{Color, Graphics};
use quicksilver::lifecycle::{self, Event, EventStream, Key, Settings, Window};
use specs::{Component, SystemData};
use shred::MultiDispatchController;
use specs::prelude::*;
use specs_hierarchy::{Hierarchy, HierarchySystem, Parent};

use log::{debug, info, trace};

type Keys = HashSet<Key>;

const COLOR_THRUSTER_OFF: Color = Color {
    r: 0.5,
    g: 0.5,
    b: 0.5,
    a: 0.5,
};

const COLOR_THRUSTER_ON: Color = Color {
    r: 1.0,
    g: 0.8,
    b: 0.1,
    a: 1.0,
};

#[derive(Copy, Clone, Component, Debug, Default)]
#[storage(NullStorage)]
struct Ship;

#[derive(Copy, Clone, Component, Debug)]
#[storage(HashMapStorage)]
struct Rotation(f32);

#[derive(Copy, Clone, Component, Debug)]
#[storage(HashMapStorage)]
struct RotationSpeed(f32);

#[derive(Copy, Clone, Debug)]
struct Thruster {
    ship: Entity,
    position: Vector,
    direction: f32,
    len: f32,
    // Add force and rotation force, the latter computed from the other info
    key: Key,
    push_direction: f32,
    push: f32,
    rotation: f32,
}

impl Component for Thruster {
    type Storage = FlaggedStorage<Self, HashMapStorage<Self>>;
}

impl Parent for Thruster {
    fn parent_entity(&self) -> Entity {
        self.ship
    }
}

#[derive(Copy, Clone, Debug)]
struct DifficultyTimeMod(f32);

#[derive(Copy, Clone, Default, Debug)]
struct FrameDuration(Duration);

#[derive(Debug)]
struct UpdateDurations {
    last_frame: Instant,
}

impl<'a> System<'a> for UpdateDurations {
    type SystemData = Write<'a, FrameDuration>;

    fn run(&mut self, mut fd: Self::SystemData) {
        let now = Instant::now();
        fd.0 = now - self.last_frame;
        self.last_frame = now;
    }
}

#[derive(Copy, Clone, Component, Debug)]
#[storage(VecStorage)]
struct Star {
    color: Color,
    size: f32,
}

#[derive(Copy, Clone, Component, Debug, Sub)]
#[storage(VecStorage)]
struct Position(Vector);

// Note: while we might have several things that can't move (therefore don't have speed), the
// vector is small and the overhead for omitting empty ones is not worth it.
#[derive(Copy, Clone, Component, Debug)]
#[storage(VecStorage)]
struct Speed(Vector);

#[derive(Copy, Clone, Component, Debug)]
#[storage(VecStorage)]
struct Mass(f32);

#[derive(Debug)]
struct Gravity {
    /// Gravity constant tuned to match our unit-less masses and pixel-distances.
    force: f32,
    /// Disable gravity when closer than this, to prevent shooting away.
    ///
    /// Measured in distance *squared*.
    closeness_limit: f32,
}

#[derive(SystemData)]
struct GravityParams<'a> {
    frame_duration: Read<'a, FrameDuration>,
    difficulty_mod: ReadExpect<'a, DifficultyTimeMod>,
    masses: ReadStorage<'a, Mass>,
    positions: ReadStorage<'a, Position>,
    speeds: WriteStorage<'a, Speed>,
}

impl<'a> System<'a> for Gravity {
    type SystemData = GravityParams<'a>;

    fn run(&mut self, params: GravityParams) {
        let GravityParams {
            frame_duration,
            difficulty_mod,
            masses,
            positions,
            mut speeds,
        } = params;
        let multiplier = self.force * frame_duration.0.as_secs_f32() * difficulty_mod.0;
        (&mut speeds, &masses, &positions)
            .par_join()
            .for_each(|(speed_1, mass_1, pos_1)| {
                let speed_inc: Vector = (&masses, &positions)
                    .join()
                    .map(|(mass_2, pos_2)| {
                        let dist_euclid = *pos_2 - *pos_1;
                        let dist_sq = dist_euclid.0.len2();
                        if dist_sq <= self.closeness_limit {
                            return Vector::ZERO;
                        }
                        let force_size = mass_1.0 * mass_2.0 / dist_sq;
                        debug_assert!(force_size >= 0.0);
                        // TODO: Cap it somehow so it doesn't „shoot“ away
                        dist_euclid.0.normalize() * force_size
                    })
                    .fold(Vector::ZERO, |a, b| a + b);
                speed_1.0 += speed_inc * multiplier;
            })
    }
}

struct Movement;

impl<'a> System<'a> for Movement {
    type SystemData = (
        Read<'a, FrameDuration>,
        ReadExpect<'a, DifficultyTimeMod>,
        ReadStorage<'a, Speed>,
        WriteStorage<'a, Position>,
    );

    fn run(&mut self, (frame_duration, difficulty, speeds, mut positions): Self::SystemData) {
        let dur = frame_duration.0.as_secs_f32() * difficulty.0;

        (&speeds, &mut positions)
            .par_join()
            .for_each(|(speed, position)| {
                position.0 += speed.0 * dur;
            });
    }
}

struct DrawStars<'a> {
    gfx: &'a RefCell<Graphics>,
}

impl<'a> System<'a> for DrawStars<'_> {
    type SystemData = (
        ReadStorage<'a, Star>,
        ReadStorage<'a, Position>,
    );

    fn run(&mut self, (stars, positions): Self::SystemData) {
        let mut gfx = self.gfx.borrow_mut();

        trace!("Drawing stars");
        // :-( Can't use par_join here, because of gfx not !Send
        for (star, pos) in (&stars, &positions).join() {
            gfx.fill_circle(&Circle::new(pos.0, star.size), star.color);
        }
    }
}

struct FireThrusters;

#[derive(SystemData)]
struct FireThrustersData<'a> {
    frame_duration: Read<'a, FrameDuration>,
    entities: Entities<'a>,
    ships: ReadStorage<'a, Ship>,
    thrusters: ReadStorage<'a, Thruster>,
    rotations: ReadStorage<'a, Rotation>,
    thruster_hierarchy: ReadExpect<'a, Hierarchy<Thruster>>,
    speeds: WriteStorage<'a, Speed>,
    rotation_speeds: WriteStorage<'a, RotationSpeed>,
    keys: Read<'a, Keys>,
}

impl<'a> System<'a> for FireThrusters {
    type SystemData = FireThrustersData<'a>;

    fn run(&mut self, mut d: Self::SystemData) {
        let parts = (&d.ships, &d.rotations, &mut d.speeds, &mut d.rotation_speeds, &d.entities);
        for (_, rotated, trans, rot, ent) in parts.join() {
            trace!("Fire thrusters of ship {:?} {:?}", trans, rot);
            for thruster in d.thruster_hierarchy.children(ent) {
                let thruster = d.thrusters
                    .get(*thruster)
                    .expect("Missing thruster reported as child");
                if d.keys.contains(&thruster.key) {
                    trace!("Thruster {:?} active", thruster.key);
                    let rotated = rotated.0 + thruster.push_direction;
                    let push = Vector::from_angle(rotated) * thruster.push;
                    // For unknown reasons, it seems to work in the opposite direction
                    trans.0 -= push * d.frame_duration.0.as_secs_f32();
                    rot.0 -= thruster.rotation * d.frame_duration.0.as_secs_f32();
                }
            }
        }
    }
}

struct DrawShips<'a> {
    gfx: &'a RefCell<Graphics>,
}

#[derive(SystemData)]
struct DrawShipData<'a> {
    entities: Entities<'a>,
    ships: ReadStorage<'a, Ship>,
    positions: ReadStorage<'a, Position>,
    rotations: ReadStorage<'a, Rotation>,
    thrusters: ReadStorage<'a, Thruster>,
    thruster_hierarchy: ReadExpect<'a, Hierarchy<Thruster>>,
    // We need to know which thrusters are active
    keys: Read<'a, Keys>,
}

impl<'a> System<'a> for DrawShips<'_> {
    type SystemData = DrawShipData<'a>;

    fn run(&mut self, d: Self::SystemData) {
        let mut gfx = self.gfx.borrow_mut();

        trace!("Drawing ships");

        for (_, pos, rotation, ent) in (&d.ships, &d.positions, &d.rotations, &d.entities).join() {
            trace!("Draw ship {:?} {:?}", pos, rotation);
            let transform = Transform::translate(pos.0) * Transform::rotate(rotation.0);
            gfx.set_transform(transform);
            gfx.stroke_path(&[Vector::new(-10.0, 0.0), Vector::new(10.0, 0.0)], Color::WHITE);
            for thruster in d.thruster_hierarchy.children(ent) {
                let thruster = d.thrusters
                    .get(*thruster)
                    .expect("Missing thruster reported as child");
                let t = transform
                    * Transform::translate(thruster.position)
                    * Transform::rotate(thruster.direction);
                gfx.set_transform(t);
                let color = if d.keys.contains(&thruster.key) {
                    COLOR_THRUSTER_ON
                } else {
                    COLOR_THRUSTER_OFF
                };
                gfx.stroke_path(&[Vector::ZERO, Vector::new(thruster.len, 0.0)], color);
            }
        }
        gfx.set_transform(Transform::default());
    }
}

struct Rotate;

impl<'a> System<'a> for Rotate {
    type SystemData = (
        Read<'a, FrameDuration>,
        ReadExpect<'a, DifficultyTimeMod>,
        ReadStorage<'a, RotationSpeed>,
        WriteStorage<'a, Rotation>,
    );

    fn run(&mut self, (frame_duration, difficulty, speeds, mut rotations): Self::SystemData) {
        let dur = frame_duration.0.as_secs_f32() * difficulty.0;

        (&speeds, &mut rotations)
            .par_join()
            .for_each(|(speed, rotation)| {
                // Seems like quicksilver works in degrees. Someone is sane at least.
                rotation.0 = (rotation.0 + speed.0 * dur).rem_euclid(360.0);
            });
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum GameState {
    Running,
    Paused,
}

struct PhysicsSystems;

impl<'a> MultiDispatchController<'a> for PhysicsSystems {
    type SystemData = ReadExpect<'a, GameState>;

    fn plan(&mut self, game_state: Self::SystemData) -> usize {
        (*game_state == GameState::Running) as usize
    }
}

async fn inner(window: Window, gfx: Graphics, mut ev: EventStream) -> Result<(), QError> {
    // XXX: Setup to its own function

    // :-( I don't like ref cells, but we need to thread the mut-borrow to both us for
    // synchronization, resizing etc, and the drawing systems.
    //
    // We do take turns in who borrow it, it's just each needs to be able to hold onto it in
    // between.
    let gfx = RefCell::new(gfx);
    let gfx = &gfx;
    let mut world = World::new();
    let physics = DispatcherBuilder::new()
        .with(Gravity { force: 1.0, closeness_limit: 100.0 }, "gravity", &[])
        .with(FireThrusters, "fire-thrusters", &[])
        .with(Movement, "movement", &["gravity", "fire-thrusters"])
        .with(Rotate, "rotate", &[]);

    let mut dispatcher = DispatcherBuilder::new()
        .with(HierarchySystem::<Thruster>::new(&mut world), "thruster-hierarchy", &[])
        .with(
            UpdateDurations {
                last_frame: Instant::now()
            }, "update-durations", &[]
        )
        .with_multi_batch(PhysicsSystems, physics, "physics", &["update-durations"])
        .with_thread_local(DrawStars { gfx })
        .with_thread_local(DrawShips { gfx })
        .build();
    dispatcher.setup(&mut world);

    // This needs to be either loaded or generated somewhere. This is just for early
    // experiments/tests.
    world.insert(DifficultyTimeMod(100.0));
    world.insert(Keys::new());
    world.insert(GameState::Running);
    world.create_entity()
        .with(Star { color: Color::BLUE, size: 2.0 })
        .with(Position(Vector::new(100.0, 250.0)))
        .with(Speed(Vector::new(3.5, 3.2)))
        .with(Mass(8.0))
        .build();
    world.create_entity()
        .with(Star { color: Color::RED, size: 3.5 })
        .with(Position(Vector::new(400.0, 400.0)))
        .with(Speed(Vector::new(-2, 1.2)))
        .with(Mass(10.0))
        .build();
    world.create_entity()
        .with(Star { color: Color::YELLOW, size: 3.5 })
        .with(Position(Vector::new(500.0, 500.0)))
        .with(Mass(50.0))
        .build();
    let ship = world.create_entity()
        .with(Ship)
        .with(Position(Vector::new(600.0, 650.0)))
        .with(Mass(50.0))
        .with(Speed(Vector::new(5.0, 0.0)))
        .with(Rotation(60.0))
        .with(RotationSpeed(1.0))
        .build();
    world.create_entity()
        .with(
            Thruster {
                position: Vector::new(10.0, 0.0),
                len: 10.0,
                direction: 20.0,
                ship,
                key: Key::Left,
                push: 3.0,
                push_direction: 20.0,
                rotation: 6.0,
            }
        )
        .build();
    world.create_entity()
        .with(
            Thruster {
                position: Vector::new(10.0, 0.0),
                len: 10.0,
                direction: -20.0,
                ship,
                key: Key::Right,
                push: 3.0,
                push_direction: -20.0,
                rotation: -6.0,
            }
        )
        .build();
    world.create_entity()
        .with(
            Thruster {
                position: Vector::new(-10.0, 0.0),
                len: 3.0,
                direction: 180.0,
                ship,
                key: Key::Up,
                push: 1.0,
                push_direction: 180.0,
                rotation: 0.0,
            }
        )
        .build();

    'mainloop: loop {
        trace!("Checking for events");
        while let Some(e) = ev.next_event().await {
            debug!("Received event {:?}", e);
            match e {
                Event::Resized(resize) => {
                    let mut gfx = gfx.borrow_mut();
                    let viewport = Rectangle::new((0, 0), resize.logical_size());
                    gfx.set_projection(Transform::orthographic(viewport));
                    gfx.fit_to_window(&window);
                    info!("Resize: {:?}", resize);
                }
                Event::KeyboardInput(event) => {
                    info!("Key press {:?}", event);
                    let keys = world.get_mut::<Keys>().expect("Keys are always present");
                    match event.key() {
                        Key::Space if !event.is_down() => {
                            let game_state = world
                                .get_mut::<GameState>()
                                .expect("The running condition is always present");
                            let state = *game_state;
                            let new = match state {
                                GameState::Running => GameState::Paused,
                                GameState::Paused => GameState::Running,
                            };
                            *game_state = new;
                        }
                        Key::Space => (),
                        Key::Escape if event.is_down() => {
                            info!("Terminating");
                            break 'mainloop;
                        }
                        key if event.is_down() => {
                            keys.insert(key);
                        }
                        key => {
                            keys.remove(&key);
                        }
                    }
                }
                _ => (),
            }
        }

        trace!("Running a frame");
        gfx.borrow_mut().clear(Color::BLACK);
        dispatcher.dispatch(&world);
        gfx.borrow_mut().present(&window)?;
        world.maintain();
    }

    Ok(())
}

fn main() {
    env_logger::init();
    lifecycle::run(
        Settings {
            fullscreen: false,
            resizable: true,
            vsync: true,
            title: "Thrust",
            ..Settings::default()
        },
        inner,
    );
}
