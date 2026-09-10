/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::borrow::ToOwned;
use std::{f32, thread};

use crossbeam_channel::{Sender, select, unbounded};
use euclid::default::{Rect, Size2D, Transform2D};
use log::warn;
use paint_api::CrossProcessPaintApi;
use pixels::Snapshot;
use profile_traits::mem::{
    ProcessReports, ProfilerChan, Report, ReportsChan, perform_memory_report,
};
use rustc_hash::FxHashMap;
use servo_base::generic_channel::GenericSender;
use servo_base::{Epoch, generic_channel};
use servo_canvas_traits::ConstellationCanvasMsg;
use servo_canvas_traits::canvas::*;
use webrender_api::ImageKey;

use crate::canvas_data::*;
// Bao (REQ-BRW-004 C13, user ruling 2026-09-09 vendor patch): global canvas
// noise consumed at the paint-thread readback choke point. Bao
// (BUN-EVOLUTION R53-A phase 2): the per-WebViewId registry resolves the
// OWNING page's config; the global remains the fallback bucket.
use crate::canvas_noise::{
    CanvasNoiseConfig, canvas_noise_for_webview, get_global_canvas_noise,
};
use servo_base::id::WebViewId;

pub struct CanvasPaintThread {
    canvases: FxHashMap<CanvasId, Canvas>,
    /// Bao (BUN-EVOLUTION R53-A phase 2): the owning webview of each
    /// canvas (stamped at creation by the script side), consulted at the
    /// `GetImageData` noise choke point to resolve the per-WebViewId
    /// canvas noise config. Identity-less canvases (None — not stored)
    /// keep the process-global fallback.
    canvas_webviews: FxHashMap<CanvasId, WebViewId>,
    next_canvas_id: CanvasId,
    paint_api: CrossProcessPaintApi,
}

impl CanvasPaintThread {
    fn new(paint_api: CrossProcessPaintApi) -> CanvasPaintThread {
        CanvasPaintThread {
            canvases: FxHashMap::default(),
            canvas_webviews: FxHashMap::default(),
            next_canvas_id: CanvasId(0),
            paint_api,
        }
    }

    /// Creates a new `CanvasPaintThread` and returns an `IpcSender` to
    /// communicate with it.
    pub fn start(
        paint_api: CrossProcessPaintApi,
        mem_profiler_chan: ProfilerChan,
    ) -> (Sender<ConstellationCanvasMsg>, GenericSender<CanvasMsg>) {
        let (ipc_sender, ipc_receiver) = generic_channel::channel::<CanvasMsg>().unwrap();
        let msg_receiver = ipc_receiver.route_preserving_errors();
        let (create_sender, create_receiver) = unbounded();
        let registration = mem_profiler_chan.prepare_memory_reporting(
            "canvas".into(),
            create_sender.clone(),
            ConstellationCanvasMsg::CollectMemoryReport,
        );
        thread::Builder::new()
            .name("Canvas".to_owned())
            .spawn(move || {
                let _registration = registration;
                let mut canvas_paint_thread = CanvasPaintThread::new(
                    paint_api);
                loop {
                    select! {
                        recv(msg_receiver) -> msg => {
                            match msg {
                                Ok(Ok((canvas_id, command))) => {
                                    canvas_paint_thread.process_command(command, canvas_id);
                                },
                                Ok(Err(e)) => {
                                    warn!("CanvasPaintThread message deserialization error: {e:?}");
                                }
                                Err(_disconnected) => {
                                    warn!("CanvasMsg receiver disconnected");
                                    break;
                                },
                            }
                        }
                        recv(create_receiver) -> msg => {
                            match msg {
                                Ok(ConstellationCanvasMsg::Create { sender: creator, size, webview_id }) => {
                                    if let Err(error) = creator.send(canvas_paint_thread.create_canvas(size, webview_id)) {
                                        warn!("Create canvas response failed ({error})");
                                    }
                                },
                                Ok(ConstellationCanvasMsg::CollectMemoryReport(sender)) => {
                                    canvas_paint_thread.collect_memory_reports(sender);
                                },
                                Ok(ConstellationCanvasMsg::Exit(exit_sender)) => {
                                    let _ = exit_sender.send(());
                                    break;
                                },
                                Err(e) => {
                                    warn!("Error on CanvasPaintThread receive ({})", e);
                                    break;
                                },
                            }
                        }
                    }
                }
            })
            .expect("Thread spawning failed");

        (create_sender, ipc_sender)
    }

    #[servo_tracing::instrument(skip_all)]
    pub fn create_canvas(
        &mut self,
        size: Size2D<u64>,
        webview_id: Option<WebViewId>,
    ) -> Option<CanvasId> {
        let canvas_id = self.next_canvas_id;
        self.next_canvas_id.0 += 1;

        let canvas = Canvas::new(size, self.paint_api.clone())?;
        self.canvases.insert(canvas_id, canvas);
        // Bao (BUN-EVOLUTION R53-A phase 2): remember which webview owns
        // this canvas so the GetImageData noise choke point resolves the
        // PER-WEBVIEW config (keyed hit authoritative, miss → process-global
        // fallback). Identity-less realms (None) keep pre-R53 semantics.
        if let Some(webview_id) = webview_id {
            self.canvas_webviews.insert(canvas_id, webview_id);
        }

        Some(canvas_id)
    }

    fn collect_memory_reports(&self, sender: ReportsChan) {
        perform_memory_report(|ops| {
            let reports = self
                .canvases
                .iter()
                .flat_map(|(canvas_id, canvas)| canvas.collect_memory_report(*canvas_id, ops))
                .collect();
            sender.send(ProcessReports::new(reports));
        });
    }

    #[servo_tracing::instrument(
        skip_all,
        fields(message = message.to_string())
    )]
    fn process_command(&mut self, message: CanvasCommand, canvas_id: CanvasId) {
        match message {
            CanvasCommand::Recreate(size) => self.canvas(canvas_id).recreate(size),
            CanvasCommand::Destroy => {
                self.canvases.remove(&canvas_id);
                // Bao (R53-A phase 2): drop the ownership record with the
                // canvas (the keyed noise config itself lives in the
                // per-WebViewId registry, cleared at page close).
                self.canvas_webviews.remove(&canvas_id);
            },
            CanvasCommand::SetImageKey(image_key) => {
                self.canvas(canvas_id).set_image_key(image_key);
            },
            CanvasCommand::FillText(
                text_bounds,
                text_runs,
                fill_or_stroke_style,
                shadow_options,
                composition_options,
                transform,
            ) => {
                self.canvas(canvas_id).fill_text(
                    text_bounds,
                    text_runs,
                    fill_or_stroke_style,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::StrokeText(
                text_bounds,
                text_runs,
                fill_or_stroke_style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ) => {
                self.canvas(canvas_id).stroke_text(
                    text_bounds,
                    text_runs,
                    fill_or_stroke_style,
                    line_options,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::FillRect(
                rect,
                style,
                shadow_options,
                composition_options,
                transform,
            ) => {
                self.canvas(canvas_id).fill_rect(
                    &rect,
                    style,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::StrokeRect(
                rect,
                style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ) => {
                self.canvas(canvas_id).stroke_rect(
                    &rect,
                    style,
                    line_options,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::ClearRect(ref rect, transform) => {
                self.canvas(canvas_id).clear_rect(rect, transform)
            },
            CanvasCommand::FillPath(
                style,
                path,
                fill_rule,
                shadow_options,
                composition_options,
                transform,
            ) => {
                self.canvas(canvas_id).fill_path(
                    &path,
                    fill_rule,
                    style,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::StrokePath(
                path,
                style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ) => {
                self.canvas(canvas_id).stroke_path(
                    &path,
                    style,
                    line_options,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::ClipPath(path, fill_rule, transform) => {
                self.canvas(canvas_id)
                    .clip_path(&path, fill_rule, transform);
            },
            CanvasCommand::DrawImage(
                snapshot,
                dest_rect,
                source_rect,
                smoothing_enabled,
                shadow_options,
                composition_options,
                transform,
            ) => self.canvas(canvas_id).draw_image(
                snapshot.to_owned(),
                dest_rect,
                source_rect,
                smoothing_enabled,
                shadow_options,
                composition_options,
                transform,
            ),
            CanvasCommand::DrawEmptyImage(
                image_size,
                dest_rect,
                source_rect,
                shadow_options,
                composition_options,
                transform,
            ) => self.canvas(canvas_id).draw_image(
                Snapshot::cleared(image_size),
                dest_rect,
                source_rect,
                false,
                shadow_options,
                composition_options,
                transform,
            ),
            CanvasCommand::DrawImageInOther(
                other_canvas_id,
                dest_rect,
                source_rect,
                smoothing,
                shadow_options,
                composition_options,
                transform,
            ) => {
                let snapshot = self
                    .canvas(canvas_id)
                    .read_pixels(Some(source_rect.to_u32()));
                self.canvas(other_canvas_id).draw_image(
                    snapshot,
                    dest_rect,
                    source_rect,
                    smoothing,
                    shadow_options,
                    composition_options,
                    transform,
                );
            },
            CanvasCommand::GetImageData(dest_rect, sender) => {
                let mut snapshot = self.canvas(canvas_id).read_pixels(dest_rect);
                // Bao (REQ-BRW-004 C13, user ruling 2026-09-09 vendor patch):
                // deterministic canvas noise at the paint-thread readback choke
                // point. Every pixel read that leaves the canvas — JS
                // `getImageData` (window and worker realms), `toDataURL`,
                // `toBlob`, `convertToBlob`, `transferToImageBitmap`,
                // `createImageBitmap` — funnels through this command, a surface
                // JS-realm hooks cannot fully cover. Coordinates are
                // region-local, matching the JS hook's
                // `addNoiseToImageData(imgData, sw)` semantics.
                //
                // Bao (BUN-EVOLUTION R53-A phase 2): the noise config
                // resolves per the canvas's OWNING webview — keyed registry
                // hit authoritative (an explicit disabled entry = a
                // stealth-free page reads back byte-exact, no inheritance
                // from a coexisting profile page); miss or identity-less
                // canvas → process-global fallback. Unset/disabled resolves
                // to `None`: byte-for-byte identical to upstream (W2 gate).
                let noise = match self.canvas_webviews.get(&canvas_id) {
                    Some(webview_id) => canvas_noise_for_webview(*webview_id),
                    None => get_global_canvas_noise(),
                };
                apply_canvas_noise(&mut snapshot, noise);
                if let Err(error) = sender.send(snapshot.to_shared()) {
                    warn!("GetImageData response failed ({error})");
                }
            },
            CanvasCommand::PutImageData(rect, snapshot) => {
                self.canvas(canvas_id)
                    .put_image_data(snapshot.to_owned(), rect);
            },
            CanvasCommand::UpdateImage(canvas_epoch) => {
                self.canvas(canvas_id).update_image_rendering(canvas_epoch);
            },
            CanvasCommand::PopClips(clips) => self.canvas(canvas_id).pop_clips(clips),
            CanvasCommand::ProcessBatchMessages(messages) => {
                for message in messages {
                    self.process_command(message, canvas_id);
                }
            },
        }
    }

    fn canvas(&mut self, canvas_id: CanvasId) -> &mut Canvas {
        self.canvases.get_mut(&canvas_id).expect("Bogus canvas id")
    }
}

#[cfg_attr(
    feature = "vello",
    expect(
        clippy::large_enum_variant,
        reason = "Current consensus is on keeping enum instead of boxing it. https://github.com/servo/servo/pull/46863"
    )
)]
enum Canvas {
    #[cfg(feature = "vello")]
    Vello(CanvasData<crate::vello_backend::VelloDrawTarget>),
    VelloCPU(CanvasData<crate::vello_cpu_backend::VelloCPUDrawTarget>),
}

impl Canvas {
    fn new(size: Size2D<u64>, paint_api: CrossProcessPaintApi) -> Option<Self> {
        match servo_config::pref!(dom_canvas_backend)
            .to_lowercase()
            .as_str()
        {
            #[cfg(feature = "vello")]
            "vello" => Some(Self::Vello(CanvasData::new(size, paint_api))),
            _ => Some(Self::VelloCPU(CanvasData::new(size, paint_api))),
        }
    }

    fn collect_memory_report(
        &self,
        canvas_id: CanvasId,
        ops: &mut malloc_size_of::MallocSizeOfOps,
    ) -> Vec<Report> {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.collect_memory_report(canvas_id, ops),
            Canvas::VelloCPU(canvas_data) => canvas_data.collect_memory_report(canvas_id, ops),
        }
    }

    fn set_image_key(&mut self, image_key: ImageKey) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.set_image_key(image_key),
            Canvas::VelloCPU(canvas_data) => canvas_data.set_image_key(image_key),
        }
    }

    fn pop_clips(&mut self, clips: usize) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.pop_clips(clips),
            Canvas::VelloCPU(canvas_data) => canvas_data.pop_clips(clips),
        }
    }

    fn stroke_text(
        &mut self,
        text_bounds: Rect<f64>,
        text_runs: Vec<TextRun>,
        fill_or_stroke_style: FillOrStrokeStyle,
        line_options: LineOptions,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.stroke_text(
                text_bounds,
                text_runs,
                fill_or_stroke_style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ),
            Canvas::VelloCPU(canvas_data) => canvas_data.stroke_text(
                text_bounds,
                text_runs,
                fill_or_stroke_style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ),
        }
    }

    fn fill_text(
        &mut self,
        text_bounds: Rect<f64>,
        text_runs: Vec<TextRun>,
        fill_or_stroke_style: FillOrStrokeStyle,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.fill_text(
                text_bounds,
                text_runs,
                fill_or_stroke_style,
                shadow_options,
                composition_options,
                transform,
            ),
            Canvas::VelloCPU(canvas_data) => canvas_data.fill_text(
                text_bounds,
                text_runs,
                fill_or_stroke_style,
                shadow_options,
                composition_options,
                transform,
            ),
        }
    }

    fn fill_rect(
        &mut self,
        rect: &Rect<f32>,
        style: FillOrStrokeStyle,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => {
                canvas_data.fill_rect(rect, style, shadow_options, composition_options, transform)
            },
            Canvas::VelloCPU(canvas_data) => {
                canvas_data.fill_rect(rect, style, shadow_options, composition_options, transform)
            },
        }
    }

    fn stroke_rect(
        &mut self,
        rect: &Rect<f32>,
        style: FillOrStrokeStyle,
        line_options: LineOptions,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.stroke_rect(
                rect,
                style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ),
            Canvas::VelloCPU(canvas_data) => canvas_data.stroke_rect(
                rect,
                style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ),
        }
    }

    fn fill_path(
        &mut self,
        path: &Path,
        fill_rule: FillRule,
        style: FillOrStrokeStyle,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.fill_path(
                path,
                fill_rule,
                style,
                shadow_options,
                composition_options,
                transform,
            ),
            Canvas::VelloCPU(canvas_data) => canvas_data.fill_path(
                path,
                fill_rule,
                style,
                shadow_options,
                composition_options,
                transform,
            ),
        }
    }

    fn stroke_path(
        &mut self,
        path: &Path,
        style: FillOrStrokeStyle,
        line_options: LineOptions,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.stroke_path(
                path,
                style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ),
            Canvas::VelloCPU(canvas_data) => canvas_data.stroke_path(
                path,
                style,
                line_options,
                shadow_options,
                composition_options,
                transform,
            ),
        }
    }

    fn clear_rect(&mut self, rect: &Rect<f32>, transform: Transform2D<f64>) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.clear_rect(rect, transform),
            Canvas::VelloCPU(canvas_data) => canvas_data.clear_rect(rect, transform),
        }
    }

    #[expect(clippy::too_many_arguments)]
    fn draw_image(
        &mut self,
        snapshot: Snapshot,
        dest_rect: Rect<f64>,
        source_rect: Rect<f64>,
        smoothing_enabled: bool,
        shadow_options: ShadowOptions,
        composition_options: CompositionOptions,
        transform: Transform2D<f64>,
    ) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.draw_image(
                snapshot,
                dest_rect,
                source_rect,
                smoothing_enabled,
                shadow_options,
                composition_options,
                transform,
            ),
            Canvas::VelloCPU(canvas_data) => canvas_data.draw_image(
                snapshot,
                dest_rect,
                source_rect,
                smoothing_enabled,
                shadow_options,
                composition_options,
                transform,
            ),
        }
    }

    fn read_pixels(&mut self, read_rect: Option<Rect<u32>>) -> Snapshot {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.read_pixels(read_rect),
            Canvas::VelloCPU(canvas_data) => canvas_data.read_pixels(read_rect),
        }
    }

    fn clip_path(&mut self, path: &Path, fill_rule: FillRule, transform: Transform2D<f64>) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.clip_path(path, fill_rule, transform),
            Canvas::VelloCPU(canvas_data) => canvas_data.clip_path(path, fill_rule, transform),
        }
    }

    fn put_image_data(&mut self, snapshot: Snapshot, rect: Rect<u32>) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.put_image_data(snapshot, rect),
            Canvas::VelloCPU(canvas_data) => canvas_data.put_image_data(snapshot, rect),
        }
    }

    fn update_image_rendering(&mut self, canvas_epoch: Option<Epoch>) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.update_image_rendering(canvas_epoch),
            Canvas::VelloCPU(canvas_data) => canvas_data.update_image_rendering(canvas_epoch),
        }
    }

    fn recreate(&mut self, size: Option<Size2D<u64>>) {
        match self {
            #[cfg(feature = "vello")]
            Canvas::Vello(canvas_data) => canvas_data.recreate(size),
            Canvas::VelloCPU(canvas_data) => canvas_data.recreate(size),
        }
    }
}

/// Bao (REQ-BRW-004 C13): apply the global deterministic canvas noise to a
/// readback [`Snapshot`] in place. `noise` is `get_global_canvas_noise()`;
/// `None` (seed never set or explicitly disabled) leaves the bytes untouched,
/// so the upstream behaviour is preserved bit-for-bit. Only the returned copy
/// is mutated — the canvas surface itself is never noised, matching the JS
/// hook which noises the returned `ImageData` only.
fn apply_canvas_noise(snapshot: &mut Snapshot, noise: Option<(u64, f64)>) {
    if let Some((seed, amplitude)) = noise {
        let size = snapshot.size();
        CanvasNoiseConfig::new(seed, amplitude).apply_to_pixels(
            snapshot.as_raw_bytes_mut(),
            size.width,
            size.height,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixels::{SnapshotAlphaMode, SnapshotPixelFormat};

    fn patterned_snapshot(width: u32, height: u32) -> Snapshot {
        let len = width as usize * height as usize * 4;
        Snapshot::from_vec(
            Size2D::new(width, height),
            SnapshotPixelFormat::RGBA,
            SnapshotAlphaMode::Transparent {
                premultiplied: true,
            },
            (0..len).map(|i| (i % 251) as u8).collect(),
        )
    }

    #[test]
    fn noise_none_leaves_bytes_untouched() {
        // Hard gate (completion ①): seed unset/disabled => zero diff.
        let mut snapshot = patterned_snapshot(4, 3);
        let before = snapshot.as_raw_bytes().to_vec();
        apply_canvas_noise(&mut snapshot, None);
        assert_eq!(snapshot.as_raw_bytes(), &before[..]);
    }

    #[test]
    fn noise_is_deterministic_for_same_seed() {
        // Hard gate (completion ③): two reads under the same seed are
        // byte-identical.
        let mut first = patterned_snapshot(8, 8);
        let mut second = patterned_snapshot(8, 8);
        apply_canvas_noise(&mut first, Some((42, 0.001)));
        apply_canvas_noise(&mut second, Some((42, 0.001)));
        assert_eq!(first.as_raw_bytes(), second.as_raw_bytes());
    }

    #[test]
    fn noise_different_seeds_diverge() {
        let mut seed_42 = patterned_snapshot(8, 8);
        let mut seed_43 = patterned_snapshot(8, 8);
        apply_canvas_noise(&mut seed_42, Some((42, 0.5)));
        apply_canvas_noise(&mut seed_43, Some((43, 0.5)));
        assert_ne!(seed_42.as_raw_bytes(), seed_43.as_raw_bytes());
    }

    #[test]
    fn noise_changes_readback_and_preserves_alpha() {
        let mut snapshot = patterned_snapshot(8, 8);
        let before = snapshot.as_raw_bytes().to_vec();
        apply_canvas_noise(&mut snapshot, Some((42, 0.5)));
        assert_ne!(snapshot.as_raw_bytes(), &before[..]);
        let after = snapshot.as_raw_bytes();
        for alpha_index in (3..after.len()).step_by(4) {
            assert_eq!(after[alpha_index], before[alpha_index]);
        }
    }

    #[test]
    fn noise_coordinates_are_region_local() {
        // Parity with the JS hook: `addNoiseToImageData(imgData, sw)` keys the
        // noise on coordinates local to the returned region, so a sub-rect
        // read must be byte-identical to the same region of a full read.
        let raw_full = patterned_snapshot(8, 8);
        let raw_crop = raw_full.get_rect(Rect::from_size(Size2D::new(4, 4)));
        // Precondition: the crop is the top-left 4x4 of the full snapshot
        // (`rgba8_get_rect` walks the source with the source's stride).
        let full_stride = 8usize * 4;
        let row_len = 4usize * 4;
        let raw_crop_bytes = raw_crop.as_raw_bytes();
        let raw_full_bytes = raw_full.as_raw_bytes();
        for row in 0..4usize {
            let src = row * full_stride;
            assert_eq!(
                &raw_crop_bytes[row * row_len..(row + 1) * row_len],
                &raw_full_bytes[src..src + row_len],
            );
        }

        let mut full = raw_full;
        let mut crop = raw_crop;
        apply_canvas_noise(&mut full, Some((42, 0.5)));
        apply_canvas_noise(&mut crop, Some((42, 0.5)));

        let noised_region_of_full = full.get_rect(Rect::from_size(Size2D::new(4, 4)));
        assert_eq!(crop.as_raw_bytes(), noised_region_of_full.as_raw_bytes());
    }

    #[test]
    fn noise_ignores_undersized_or_empty_data() {
        // `CanvasNoiseConfig::apply_to_pixels` no-ops on empty snapshots
        // (`read_pixels` returns `Snapshot::empty()` for off-canvas reads).
        let mut empty = Snapshot::empty();
        let before = empty.as_raw_bytes().to_vec();
        apply_canvas_noise(&mut empty, Some((42, 0.5)));
        assert_eq!(empty.as_raw_bytes(), &before[..]);
    }
}
