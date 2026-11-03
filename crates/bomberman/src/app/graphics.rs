use super::*;
use crate::settings::GraphicsMode;
use bomberman_assets::hd::{self, Pack, Progress};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
};

enum Event {
    Progress(Progress),
    Ready(std::result::Result<Output, String>),
}
enum Output {
    Artwork(PathBuf, Pack),
    Export(PathBuf),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Load,
    Prepare,
    Import,
    Export,
}
enum Action {
    Load,
    Prepare,
    Import(PathBuf),
    Export(PathBuf),
}
impl Action {
    fn kind(&self) -> Kind {
        match self {
            Self::Load => Kind::Load,
            Self::Prepare => Kind::Prepare,
            Self::Import(_) => Kind::Import,
            Self::Export(_) => Kind::Export,
        }
    }
}
struct Job {
    receiver: mpsc::Receiver<Event>,
    cancel: Arc<AtomicBool>,
    save: bool,
    kind: Kind,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
#[derive(Default)]
pub(super) struct State {
    job: Option<Job>,
    progress: Option<Progress>,
    error: Option<String>,
    pending_save: bool,
    picker: Option<(Kind, crate::modding::FolderChoice)>,
    exported: Option<PathBuf>,
    notice: Option<String>,
    last_action: Option<Kind>,
}

impl State {
    #[cfg(debug_assertions)]
    pub(super) fn probe(&self) -> serde_json::Value {
        serde_json::json!({"preparing":self.job.is_some(),"choosing":self.picker.is_some(),"operation":self.last_action.map(|k|format!("{k:?}")),"exported":self.exported,"error":self.error,"completed":self.progress.as_ref().map(|p|p.completed),"total":self.progress.as_ref().map(|p|p.total),"message":self.progress.as_ref().map(|p|&p.message)})
    }
}

impl App {
    pub(super) fn open_graphics(&mut self) {
        self.change_screen(Screen::Graphics);
    }

    pub(super) fn start_hd_job(&mut self, generate: bool, save: bool) {
        self.start_hd_action(
            if generate {
                Action::Prepare
            } else {
                Action::Load
            },
            save,
        );
    }

    fn start_hd_action(&mut self, action: Action, save: bool) {
        if self.graphics_ui.job.is_some() {
            return;
        }
        let base = self.cli.assets.clone();
        let manifest = self.media.manifest.clone();
        let root = self.cli.settings.with_file_name("hd-cache");
        let (sender, receiver) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let kind = action.kind();
        self.graphics_ui.last_action = Some(kind);
        self.graphics_ui.error = None;
        self.graphics_ui.notice = None;
        self.graphics_ui.progress = Some(Progress {
            message: match kind {
                Kind::Prepare => "Preparing HD graphics",
                Kind::Load => "Checking saved HD graphics",
                Kind::Import => "Importing HD artwork",
                Kind::Export => "Exporting HD artwork",
            }
            .into(),
            completed: 0,
            total: 0,
        });
        self.graphics_ui.job = Some(Job {
            receiver,
            cancel,
            save,
            kind,
            worker: None,
        });
        let worker = std::thread::Builder::new()
            .name("hd-preparation".into())
            .spawn(move || {
                let result = (|| -> Result<Output> {
                    let progress = |progress| {
                        if sender.send(Event::Progress(progress)).is_err() {
                            worker_cancel.store(true, Ordering::Release);
                        }
                    };
                    let path = match action {
                        Action::Prepare => hd::prepare(&base, &root, &worker_cancel, progress)?,
                        Action::Load => hd::cache_path(&root, &manifest)?,
                        Action::Import(folder) => hd::import_pack(&base, &manifest, &root, &folder, &worker_cancel, progress)?,
                        Action::Export(folder) => return hd::export_pack(&base, &manifest, &root, &folder, &worker_cancel, progress).map(Output::Export),
                    };
                anyhow::ensure!(path.join("hd.json").is_file(),"HD artwork is not prepared for the selected game assets or mod. Choose Retry to prepare it.");
                let pack = Pack::load(&path, &manifest)?;
                    anyhow::ensure!(
                        !worker_cancel.load(Ordering::Acquire),
                        "HD preparation cancelled"
                    );
                    Ok(Output::Artwork(path, pack))
                })()
                .map_err(|e| format!("{e:#}"));
                let _ = sender.send(Event::Ready(result));
            });
        match worker {
            Ok(worker) => {
                if let Some(job) = self.graphics_ui.job.as_mut() {
                    job.worker = Some(worker);
                }
            }
            Err(error) => {
                self.graphics_ui.job = None;
                self.graphics_ui.progress = None;
                if matches!(kind, Kind::Import | Kind::Export) {
                    self.graphics_ui.error = Some(format!("Could not start HD transfer: {error}"));
                } else {
                    self.fallback_hd(format!("Could not start HD preparation: {error}"));
                }
            }
        }
    }

    pub(super) fn poll_graphics(&mut self) {
        // Drain progress without making the event/render loop wait for inference.
        loop {
            let Some(job) = self.graphics_ui.job.as_ref() else {
                break;
            };
            let result = job.receiver.try_recv();
            match result {
                Ok(Event::Progress(progress)) => self.graphics_ui.progress = Some(progress),
                Ok(Event::Ready(result)) => {
                    let save = job.save;
                    let kind = job.kind;
                    let cancelled = job.cancel.load(Ordering::Acquire);
                    self.graphics_ui.job = None;
                    self.graphics_ui.progress = None;
                    if cancelled {
                        self.graphics_ui.error = Some(
                            if self.media.hd_enabled() {
                                "HD operation cancelled. Existing HD graphics remain active."
                            } else {
                                "HD operation cancelled. Classic graphics remain available."
                            }
                            .into(),
                        );
                        break;
                    }
                    let result = match result {
                        Ok(Output::Export(path)) => {
                            self.graphics_ui.notice =
                                Some(format!("HD artwork exported to {}", path.display()));
                            self.graphics_ui.exported = Some(path);
                            break;
                        }
                        Ok(Output::Artwork(path, pack)) => Ok((path, pack)),
                        Err(error) if matches!(kind, Kind::Import | Kind::Export) => {
                            self.graphics_ui.error = Some(error);
                            break;
                        }
                        Err(error) => Err(error),
                    };
                    let result = result.map_err(anyhow::Error::msg).and_then(|(path, pack)| {
                        let (width, height) = self.hd_dimensions();
                        let target = crate::media::hd_target(width, height)?;
                        self.media.activate_hd(path, pack)?;
                        self.target = target;
                        Ok(())
                    });
                    match result {
                        Ok(()) => {
                            self.graphics_ui.pending_save = save;
                            self.graphics_ui.error = None;
                            if self
                                .message
                                .as_ref()
                                .is_some_and(|(text, _)| text.starts_with("HD unavailable;"))
                            {
                                self.message = None;
                            }
                        }
                        Err(error) => self.fallback_hd(format!("{error:#}")),
                    }
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.graphics_ui.job = None;
                    self.graphics_ui.progress = None;
                    self.graphics_ui.error = Some("HD operation stopped unexpectedly".into());
                    break;
                }
            }
        }
    }

    pub(super) fn fallback_hd(&mut self, reason: String) {
        self.media.disable_hd();
        self.target = self.classic_target.clone();
        self.graphics_ui.pending_save = false;
        self.graphics_ui.error = Some(reason.clone());
        self.graphics_ui.last_action = Some(Kind::Load);
        eprintln!("Atomic Bomberman: HD unavailable; using Classic. {reason}");
        self.notify(
            "HD unavailable; using Classic. Options > Graphics shows the reason and Retry.",
        );
    }

    fn hd_dimensions(&self) -> (u32, u32) {
        let scale = (screen_width() / 640.).min(screen_height() / 480.) * screen_dpi_scale();
        (
            (640. * scale).round().max(1.) as u32,
            (480. * scale).round().max(1.) as u32,
        )
    }

    pub(super) fn prepare_graphics_frame(&mut self) {
        self.media.begin_frame();
        if self.media.hd_enabled() {
            let (width, height) = self.hd_dimensions();
            if self.target.texture.width() as u32 != width
                || self.target.texture.height() as u32 != height
            {
                match crate::media::hd_target(width, height) {
                    Ok(target) => self.target = target,
                    Err(error) => self.fallback_hd(error.to_string()),
                }
            }
        }
    }

    pub(super) async fn finish_graphics_frame(&mut self) {
        if let Err(error) = self.media.load_hd_requests().await {
            self.fallback_hd(format!("{error:#}"));
            self.draw();
        } else if self.graphics_ui.pending_save && self.media.hd_bytes() > 0 {
            self.graphics_ui.pending_save = false;
            self.settings.graphics = GraphicsMode::Hd;
            self.save();
        }
    }

    pub(super) fn update_graphics(&mut self) {
        if let Some(result) = self
            .graphics_ui
            .picker
            .as_ref()
            .map(|(_, picker)| picker.try_recv())
        {
            match result {
                Ok(result) => {
                    let (kind, _) = self.graphics_ui.picker.take().unwrap();
                    match result {
                        Ok(Some(folder)) => self.start_hd_action(
                            if kind == Kind::Export {
                                Action::Export(folder)
                            } else {
                                Action::Import(folder)
                            },
                            kind == Kind::Import,
                        ),
                        Ok(None) => {}
                        Err(error) => self.graphics_ui.error = Some(error),
                    }
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.graphics_ui.picker = None;
                    self.graphics_ui.error = Some("Folder chooser stopped".into());
                }
            }
        }
        if let Some(job) = self.graphics_ui.job.as_ref() {
            if self.back() || self.confirm() || self.clicked {
                job.cancel.store(true, Ordering::Release);
                self.graphics_ui.progress = Some(Progress {
                    message: "Cancelling HD operation…".into(),
                    completed: 0,
                    total: 0,
                });
            }
            return;
        }
        self.navigation(6);
        let click = self.hover_rows(55., 125., 530., 34., 6);
        if self.back() || self.selection == 5 && (self.confirm() || click) {
            self.change_screen(Screen::Settings);
            self.selection = 17;
            return;
        }
        if self.delta() == 0 && !self.confirm() && !click {
            return;
        }
        match self.selection {
            0 if self.media.hd_enabled() || self.settings.graphics == GraphicsMode::Hd => {
                self.media.disable_hd();
                self.target = self.classic_target.clone();
                self.settings.graphics = GraphicsMode::Classic;
                self.graphics_ui.error = None;
                self.graphics_ui.pending_save = false;
                if self
                    .message
                    .as_ref()
                    .is_some_and(|(text, _)| text.starts_with("HD unavailable;"))
                {
                    self.message = None;
                }
                self.save();
            }
            0 | 2 => self.start_hd_job(true, true),
            1 => {
                self.settings.fullscreen = !self.settings.fullscreen;
                set_fullscreen(self.settings.fullscreen);
                self.save();
            }
            3 | 4 if self.confirm() || click => {
                match crate::modding::choose_hd_folder(self.selection == 4) {
                    Ok(picker) => {
                        self.graphics_ui.last_action = Some(if self.selection == 4 {
                            Kind::Export
                        } else {
                            Kind::Import
                        });
                        self.graphics_ui.picker = Some((
                            if self.selection == 4 {
                                Kind::Export
                            } else {
                                Kind::Import
                            },
                            picker,
                        ))
                    }
                    Err(error) => self.graphics_ui.error = Some(format!("{error:#}")),
                }
            }
            _ => {}
        }
    }

    pub(super) fn draw_graphics(&self) {
        self.original_background(self.frontend.glue);
        self.media.original_text("Graphics", 55., 55., WHITE);
        if let Some(progress) = &self.graphics_ui.progress {
            self.media.font_text(
                "FONT1.FON",
                match self.graphics_ui.job.as_ref().map(|job| job.kind) {
                    Some(Kind::Import) => "Importing HD artwork",
                    Some(Kind::Export) => "Exporting HD artwork",
                    _ => "Preparing HD graphics locally",
                },
                55.,
                130.,
                18.,
                GOLD,
            );
            self.media.font_text(
                "FONT1.FON",
                &truncate(&progress.message, 66),
                55.,
                185.,
                13.,
                WHITE,
            );
            if progress.total > 0 {
                let ratio = progress.completed as f32 / progress.total as f32;
                draw_rectangle(55., 225., 530., 14., DARKGRAY);
                draw_rectangle(55., 225., 530. * ratio, 14., GOLD);
                self.media.font_text(
                    "FONT1.FON",
                    &format!(
                        "{} / {} {}",
                        progress.completed,
                        progress.total,
                        if self
                            .graphics_ui
                            .job
                            .as_ref()
                            .is_some_and(|j| matches!(j.kind, Kind::Import | Kind::Export))
                        {
                            "files"
                        } else {
                            "frames"
                        }
                    ),
                    55.,
                    254.,
                    14.,
                    WHITE,
                );
            }
            self.media.font_text(
                "FONT1.FON",
                "This can take a while. Your original artwork stays intact.",
                55.,
                310.,
                13.,
                WHITE,
            );
            self.media
                .font_text("FONT1.FON", "Escape / Enter: cancel", 55., 365., 16., GOLD);
            return;
        }
        let rows = [
            format!(
                "Graphics: {}",
                if self.media.hd_enabled() {
                    "HD"
                } else {
                    "Classic"
                }
            ),
            format!(
                "Fullscreen: {}",
                if self.settings.fullscreen {
                    "Yes"
                } else {
                    "No"
                }
            ),
            if self.graphics_ui.error.is_some()
                && !matches!(
                    self.graphics_ui.last_action,
                    Some(Kind::Import | Kind::Export)
                )
            {
                "Retry HD preparation".into()
            } else {
                "Prepare / enable HD graphics".into()
            },
            "Use existing HD folder".into(),
            "Export HD artwork".into(),
            "Back".into(),
        ];
        for (i, row) in rows.iter().enumerate() {
            self.media
                .original_text(row, 75., 133. + i as f32 * 34., WHITE);
        }
        self.head_cursor(55., 149. + self.selection as f32 * 34.);
        if let Some(error) = &self.graphics_ui.error {
            self.media.font_text(
                "FONT1.FON",
                if self.media.hd_enabled() {
                    "HD operation stopped. Existing HD graphics are active."
                } else {
                    "Classic graphics are active. See the reason below."
                },
                55.,
                340.,
                13.,
                GOLD,
            );
            // Wrap diagnostics so a useful reason remains readable on the 4:3 UI.
            let text = truncate(error, 180);
            for (i, line) in text.as_bytes().chunks(70).enumerate() {
                self.media.font_text(
                    "FONT1.FON",
                    &String::from_utf8_lossy(line),
                    55.,
                    365. + i as f32 * 18.,
                    11.,
                    WHITE,
                );
            }
        } else if let Some(notice) = &self.graphics_ui.notice {
            let text = truncate(notice, 180);
            for (i, line) in text.as_bytes().chunks(70).enumerate() {
                self.media.font_text(
                    "FONT1.FON",
                    &String::from_utf8_lossy(line),
                    55.,
                    350. + i as f32 * 18.,
                    12.,
                    WHITE,
                );
            }
        } else {
            self.media.font_text(
                "FONT1.FON",
                "HD is optional. First use downloads tools and prepares artwork.",
                55.,
                350.,
                12.,
                WHITE,
            );
            self.media.font_text(
                "FONT1.FON",
                "A supported GPU and additional disk space are required.",
                55.,
                375.,
                12.,
                WHITE,
            );
        }
        self.original_footer();
    }
}
