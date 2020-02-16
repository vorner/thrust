use quicksilver::QuicksilverError as QError;
use quicksilver::geom::Vector;
use quicksilver::graphics::{Color, Graphics};
use quicksilver::lifecycle::{self, Event, EventStream, Settings, Window};

use log::{debug, info, trace};

async fn inner(window: Window, mut g: Graphics, mut ev: EventStream) -> Result<(), QError> {
    let mut x = 0;
    loop {
        trace!("Checking for events");
        while let Some(e) = ev.next_event().await {
            debug!("Received event {:?}", e);
            match e {
                Event::Resized(_) => {
                    g.fit_to_window(&window);
                    info!("Resize...");
                }
                _ => (),
            }
        }

        trace!("Updating frame");

        x += 1;

        trace!("Drawing frame");

        g.clear(Color::BLACK);
        g.draw_point(Vector::new(x, 10), Color::BLUE);
        g.present(&window)?;
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
