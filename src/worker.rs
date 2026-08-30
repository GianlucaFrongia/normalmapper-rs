//! Background map generation.
//!
//! Generating from a 4K source takes long enough to drop frames, so all of it
//! happens on a worker thread. Requests carry a generation number; the UI keeps
//! the newest response and throws away anything it has already outrun.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use eframe::egui;
use image::RgbaImage;

use crate::normalmap::{self, MapKind, Settings};

pub struct Request {
    pub generation: u64,
    pub src: Arc<RgbaImage>,
    pub settings: Settings,
}

pub struct Response {
    pub generation: u64,
    pub maps: Vec<(MapKind, RgbaImage)>,
    pub millis: f32,
}

pub struct Worker {
    tx: Sender<Request>,
    rx: Receiver<Response>,
}

impl Worker {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (req_tx, req_rx) = channel::<Request>();
        let (res_tx, res_rx) = channel::<Response>();

        std::thread::Builder::new()
            .name("normalmapper-generate".to_owned())
            .spawn(move || {
                while let Ok(mut req) = req_rx.recv() {
                    // A burst of slider events queues up; only the last one matters.
                    while let Ok(newer) = req_rx.try_recv() {
                        req = newer;
                    }
                    let started = Instant::now();
                    let maps = normalmap::generate(&req.src, &req.settings, &MapKind::ALL);
                    let response = Response {
                        generation: req.generation,
                        maps,
                        millis: started.elapsed().as_secs_f32() * 1000.0,
                    };
                    if res_tx.send(response).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn generator thread");

        Self {
            tx: req_tx,
            rx: res_rx,
        }
    }

    pub fn request(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    /// The most recent finished result, if any.
    pub fn poll(&self) -> Option<Response> {
        self.rx.try_iter().last()
    }
}
