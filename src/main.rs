use std::cell::RefCell;

use quicksilver::QuicksilverError as QError;
use quicksilver::geom::{Circle, Vector};
use quicksilver::graphics::{Color, Graphics};
use quicksilver::lifecycle::{self, Event, EventStream, Settings, Window};
use specs::Component;
use specs::prelude::*;

use log::{debug, info, trace};

#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Star {
    color: Color,
    size: f32,
}

// TODO: We probably should use something like f64 for positions, because we need to update them in
// fine increments.
#[derive(Copy, Clone, Component, Debug)]
#[storage(DenseVecStorage)]
struct Position(Vector);

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
        .with_thread_local(Painter { gfx })
        .build();
    let mut world = World::new();
    dispatcher.setup(&mut world);

    // This needs to be either loaded or generated somewhere
    world.create_entity()
        .with(Star { color: Color::BLUE, size: 2.0 })
        .with(Position(Vector::new(50, 50)))
        .build();
    world.create_entity()
        .with(Star { color: Color::RED, size: 3.5 })
        .with(Position(Vector::new(400, 400)))
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
