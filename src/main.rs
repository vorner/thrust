use std::cell::RefCell;
use std::time::{Duration, Instant};

use derive_more::Sub;
use quicksilver::QuicksilverError as QError;
use quicksilver::geom::{Circle, Vector};
use quicksilver::graphics::{Color, Graphics};
use quicksilver::lifecycle::{self, Event, EventStream, Settings, Window};
use specs::{Component, SystemData};
use specs::prelude::*;

use log::{debug, info, trace};

// TODO: Bugs/features to report
// * Why can't quicksilver Scalar be implemented for f64?
// * Panic → only logs, but keeps running.
// * Specs derive on typedef doesn't work. Should it?

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
#[storage(DenseVecStorage)]
struct Star {
    color: Color,
    size: f32,
}

#[derive(Copy, Clone, Component, Debug, Sub)]
#[storage(DenseVecStorage)]
struct Position(Vector);

// Note: while we might have several things that can't move (therefore don't have speed), the
// vector is small and the overhead for omitting empty ones is not worth it.
#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Speed(Vector);

#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Mass(f32);

#[derive(Debug)]
struct Gravity {
    /// Gravity constant tuned to match our unit-less masses and pixel-distances.
    force: f32,
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
        (&mut speeds, &masses, &positions)
            .par_join()
            .for_each(|(speed_1, mass_1, pos_1)| {
                let speed_inc: Vector = (&masses, &positions)
                    .join()
                    .map(|(mass_2, pos_2)| {
                        let dist_euclid = *pos_2 - *pos_1;
                        let dist_sq = dist_euclid.0.len2();
                        if dist_sq <= 0.0 {
                            return Vector::ZERO;
                        }
                        let force_size = self.force * mass_1.0 * mass_2.0 / dist_sq;
                        debug_assert!(force_size >= 0.0);
                        // TODO: Cap it somehow so it doesn't „shoot“ away
                        dist_euclid.0.normalize() * force_size
                    })
                    .fold(Vector::ZERO, |a, b| a + b);
                speed_1.0 += speed_inc
                    * self.force
                    * frame_duration.0.as_secs_f32()
                    * difficulty_mod.0;
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

struct Painter<'a> {
    gfx: &'a RefCell<Graphics>,
}

impl<'a> System<'a> for Painter<'_> {
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

async fn inner(window: Window, gfx: Graphics, mut ev: EventStream) -> Result<(), QError> {
    // XXX: Setup to its own function

    // :-( I don't like ref cells, but we need to thread the mut-borrow to both us for
    // synchronization, resizing etc, and the drawing systems.
    //
    // We do take turns in who borrow it, it's just each needs to be able to hold onto it in
    // between.
    let gfx = RefCell::new(gfx);
    let gfx = &gfx;
    let mut dispatcher = DispatcherBuilder::new()
        .with(
            UpdateDurations {
                last_frame: Instant::now()
            }, "update-durations", &[]
        )
        .with(Gravity { force: 1.0 }, "gravity", &["update-durations"])
        .with(Movement, "movement", &["update-durations", "gravity"])
        .with_thread_local(Painter { gfx })
        .build();
    let mut world = World::new();
    dispatcher.setup(&mut world);

    // This needs to be either loaded or generated somewhere. This is just for early
    // experiments/tests.
    world.insert(DifficultyTimeMod(100.0));
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

    loop {
        trace!("Checking for events");
        while let Some(e) = ev.next_event().await {
            debug!("Received event {:?}", e);
            match e {
                Event::Resized(_) => {
                    gfx.borrow_mut().fit_to_window(&window);
                    info!("Resize...");
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
