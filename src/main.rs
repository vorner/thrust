use std::cell::RefCell;
use std::time::{Duration, Instant};

use quicksilver::QuicksilverError as QError;
use quicksilver::geom::{Circle, Vector};
use quicksilver::graphics::{Color, Graphics};
use quicksilver::lifecycle::{self, Event, EventStream, Settings, Window};
use specs::Component;
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
    }
}

#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Star {
    color: Color,
    size: f32,
}

#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Position(Vector);

// Note: while we might have several things that can't move (therefore don't have speed), the
// vector is small and the overhead for omitting empty ones is not worth it.
#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Speed(Vector);

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
        .with(Movement, "movement", &["update-durations"])
        .with_thread_local(Painter { gfx })
        .build();
    let mut world = World::new();
    dispatcher.setup(&mut world);

    // This needs to be either loaded or generated somewhere. This is just for early
    // experiments/tests.
    world.insert(DifficultyTimeMod(1.0));
    world.create_entity()
        .with(Star { color: Color::BLUE, size: 2.0 })
        .with(Position(Vector::new(50.0, 50.0)))
        .with(Speed(Vector::new(5.5, 1.2)))
        .build();
    world.create_entity()
        .with(Star { color: Color::RED, size: 3.5 })
        .with(Position(Vector::new(400.0, 400.0)))
        .with(Speed(Vector::new(-0.5, -1.2)))
        .build();
    world.create_entity()
        .with(Star { color: Color::YELLOW, size: 3.5 })
        .with(Position(Vector::new(500.0, 500.0)))
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
