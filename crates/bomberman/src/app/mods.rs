use super::*;
use std::{path::PathBuf, sync::mpsc};

#[derive(Clone)]
struct Prepared {
    folder: Option<PathBuf>,
    scheme: usize,
    hosted_scheme: Option<usize>,
}

#[derive(Default)]
pub(super) struct State {
    picker: Option<crate::modding::FolderChoice>,
    job: Option<mpsc::Receiver<std::result::Result<Prepared, String>>>,
    ready: Option<Prepared>,
}

impl State {
    #[cfg(debug_assertions)]
    pub(super) fn probe(&self) -> serde_json::Value {
        serde_json::json!({"choosing": self.picker.is_some(), "preparing": self.job.is_some(), "can_apply": self.ready.is_some(),
            "selected": self.ready.as_ref().and_then(|ready| ready.folder.as_ref())})
    }
}

impl App {
    pub(super) fn open_mods(&mut self) {
        self.mod_ui = State::default();
        self.change_screen(Screen::Mods);
    }

    fn prepare_mod(&mut self, folder: Option<PathBuf>) {
        let base = self.cli.base_assets.clone();
        let cache = crate::settings::default_path().with_file_name("mod-cache");
        let scheme = self.media.manifest.schemes[self.settings.scheme].id.clone();
        let hosted = self.settings.hosting.as_ref().and_then(|prefs| {
            self.media
                .manifest
                .schemes
                .get(prefs.scheme)
                .map(|scheme| scheme.id.clone())
        });
        let (sender, receiver) = mpsc::channel();
        self.mod_ui.ready = None;
        self.mod_ui.job = Some(receiver);
        std::thread::spawn(move || {
            let prepare = || -> Result<Prepared> {
                let path = if let Some(directory) = &folder {
                    bomberman_assets::modded_cache(&base, directory, &cache)?
                } else {
                    base
                };
                let manifest = bomberman_assets::validate(&path)?;
                crate::media::animation_index(&manifest)?;
                let index = |name: &str| {
                    manifest
                        .schemes
                        .iter()
                        .position(|scheme| scheme.id == name)
                        .unwrap_or_else(|| {
                            manifest
                                .schemes
                                .iter()
                                .position(|scheme| scheme.id.ends_with("BASIC.SCH"))
                                .unwrap_or(0)
                        })
                };
                Ok(Prepared {
                    folder: folder.clone(),
                    scheme: index(&scheme),
                    hosted_scheme: hosted.as_deref().map(index),
                })
            };
            let _ = sender.send(prepare().map_err(|error| format!("{error:#}")));
        });
    }

    pub(super) fn update_mods(&mut self) {
        if let Some(result) = self.mod_ui.picker.as_ref().map(|picker| picker.try_recv()) {
            match result {
                Ok(choice) => {
                    self.mod_ui.picker = None;
                    match choice {
                        Ok(Some(folder)) => self.prepare_mod(Some(folder)),
                        Ok(None) => {}
                        Err(error) => self.notify(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.mod_ui.picker = None;
                    self.notify("Folder chooser stopped.");
                }
                Err(mpsc::TryRecvError::Empty) => return,
            }
        }
        if let Some(result) = self.mod_ui.job.as_ref().map(|job| job.try_recv()) {
            match result {
                Ok(Ok(prepared)) => {
                    self.mod_ui.job = None;
                    self.mod_ui.ready = Some(prepared);
                    self.selection = 2;
                }
                Ok(Err(error)) => {
                    self.mod_ui.job = None;
                    self.notify(error);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.mod_ui.job = None;
                    self.notify("Mod preparation stopped.");
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.navigation(4);
        let click = self.hover_rows(45., 225., 550., 32., 4);
        if self.back() || (self.selection == 3 && (self.confirm() || click)) {
            self.mod_ui = State::default();
            self.change_screen(Screen::Settings);
            self.selection = 18;
            return;
        }
        if !(self.confirm() || click) || self.mod_ui.job.is_some() {
            return;
        }
        match self.selection {
            0 => match crate::modding::choose_folder() {
                Ok(picker) => self.mod_ui.picker = Some(picker),
                Err(error) => self.notify(format!("{error:#}")),
            },
            1 => self.prepare_mod(None),
            2 => {
                let Some(prepared) = self.mod_ui.ready.take() else {
                    return;
                };
                let previous = self.settings.clone();
                self.settings.asset_mod.clone_from(&prepared.folder);
                self.settings.scheme = prepared.scheme;
                if let (Some(hosting), Some(index)) =
                    (&mut self.settings.hosting, prepared.hosted_scheme)
                {
                    hosting.scheme = index;
                }
                if let Err(error) = self.settings.save(&self.cli.settings) {
                    self.settings = previous;
                    self.mod_ui.ready = Some(prepared);
                    self.notify(format!("Could not save mod selection: {error:#}"));
                } else if let Err(error) = crate::modding::restart() {
                    let saved_snapshot = self.settings.saved_snapshot.clone();
                    self.settings = previous;
                    self.settings.saved_snapshot = saved_snapshot;
                    let _ = self.settings.save(&self.cli.settings);
                    self.mod_ui.ready = Some(prepared);
                    self.notify(format!("{error:#}"));
                } else {
                    self.quit = true;
                }
            }
            _ => {}
        }
    }

    pub(super) fn draw_mods(&self) {
        self.original_background(self.frontend.glue);
        documents::blue_panel(35., 65., 570., 330.);
        self.media.original_text("Asset mods", 55., 86., WHITE);
        let active = self.cli.mod_path.as_ref().map_or_else(
            || "Original assets".into(),
            |path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            },
        );
        self.media
            .original_text_fit(&format!("Current: {active}"), 55., 125., 530., SKYBLUE);
        self.media.original_text_fit(
            "The normal app remembers your choice.",
            55.,
            155.,
            530.,
            WHITE,
        );
        let status = if self.mod_ui.job.is_some() {
            "Preparing and validating assets...".into()
        } else if let Some(ready) = &self.mod_ui.ready {
            ready.folder.as_ref().map_or_else(
                || "Ready: Original assets".into(),
                |path| {
                    format!(
                        "Ready: {}",
                        path.file_name()
                            .unwrap_or(path.as_os_str())
                            .to_string_lossy()
                    )
                },
            )
        } else {
            "Choose a folder, then apply to restart.".into()
        };
        self.media
            .original_text_fit(&status, 55., 190., 530., WHITE);
        for (index, label) in [
            "Choose mod folder...",
            "Use original assets",
            "Apply and restart",
            "Back",
        ]
        .iter()
        .enumerate()
        {
            let y = 233. + index as f32 * 32.;
            self.media.original_text(
                label,
                65.,
                y,
                if index == 2 && self.mod_ui.ready.is_none() {
                    MUTED
                } else {
                    WHITE
                },
            );
            if self.selection == index {
                self.head_cursor(45., y + 16.);
            }
        }
        self.original_footer();
    }
}
