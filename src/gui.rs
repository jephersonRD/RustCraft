use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape,
    Stroke, TextureHandle, TextureOptions, Vec2,
};

use crate::accounts_offline::OfflineAccount;
use crate::core::config::{Config, WindowConfig};
use crate::core::download;
use crate::core::error::Result as RcResult;
use crate::core::paths::AppPaths;
use crate::github_releases::{self, InstallEvent, InstallResult, Platform, Release};
use crate::instances::{self, Instance};
use crate::launch;
use crate::mojang::{self, Progress as MojangProgress};
use crate::prepare::{self, Progress as PrepareProgress};

const BG: Color32 = Color32::from_rgb(11, 10, 11);
const SIDEBAR: Color32 = Color32::from_rgb(8, 7, 8);
const CARD: Color32 = Color32::from_rgb(23, 21, 25);
const CARD_SOFT: Color32 = Color32::from_rgb(30, 27, 32);
const YELLOW: Color32 = Color32::from_rgb(247, 198, 20);
const TEXT: Color32 = Color32::from_rgb(242, 239, 233);
const MUTED: Color32 = Color32::from_rgb(158, 152, 148);
const GREEN: Color32 = Color32::from_rgb(61, 199, 91);
const RED: Color32 = Color32::from_rgb(240, 82, 82);

const SIDEBAR_WIDTH: f32 = 236.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Home,
    Library,
    Settings,
    About,
}

#[derive(Clone)]
enum TaskKind {
    Releases,
    Install(String),
    Prepare(String),
    Play(String),
}

#[derive(Clone)]
pub struct NewsItem {
    pub title: String,
    pub date: String,
    pub body: String,
}

pub const NEWS_URL: &str =
    "https://raw.githubusercontent.com/jephersonRD/RustCraft/main/Noticia.md";

struct Task {
    kind: TaskKind,
    rx: Receiver<Msg>,
}

enum Msg {
    Log(String),
    Progress(f32),
    Bytes { label: String, downloaded: u64, total: u64 },
    Remote(Vec<Release>),
    News(Vec<NewsItem>),
    Done(std::result::Result<String, String>),
}

pub struct RustCraftApp {
    paths: AppPaths,
    config: Config,
    page: Page,
    name_input: String,
    java_input: String,
    width_input: u32,
    height_input: u32,
    new_instance: String,
    selected_instance: Option<String>,
    account_dialog: bool,
    onboarding: bool,
    dialog_text: String,
    versions: Vec<github_releases::InstalledVersion>,
    instances: Vec<Instance>,
    remote: Vec<Release>,
    news: Vec<NewsItem>,
    latest_release: Option<Release>,
    bytes_note: Option<String>,
    logs: Vec<String>,
    progress: Option<f32>,
    status: String,
    task: Option<Task>,
    logo: Option<TextureHandle>,
    background: Option<TextureHandle>,
}

impl RustCraftApp {
    fn new(cc: &eframe::CreationContext<'_>, paths: AppPaths) -> Self {
        let _ = paths.ensure_layout();
        let config = Config::load(&paths.config_file()).unwrap_or_default();
        let name_input = config.player_name.clone().unwrap_or_default();
        let java_input = config
            .java_home
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        let width_input = config.window.width;
        let height_input = config.window.height;

        let logo = load_texture(
            &cc.egui_ctx,
            "logo",
            include_bytes!("../RustCraft.png"),
        );
        let background = load_texture(
            &cc.egui_ctx,
            "background",
            include_bytes!("../fondo.png"),
        );
        // No player is pre-selected: a fresh install must choose its own name.
        let onboarding = config
            .player_name
            .as_deref()
            .and_then(|name| OfflineAccount::new(name).ok())
            .is_none();

        let mut app = Self {
            paths,
            selected_instance: config.selected_instance.clone(),
            config,
            page: Page::Home,
            name_input,
            java_input,
            width_input,
            height_input,
            new_instance: String::new(),
            account_dialog: false,
            onboarding,
            dialog_text: String::new(),
            versions: Vec::new(),
            instances: Vec::new(),
            remote: Vec::new(),
            news: Vec::new(),
            latest_release: None,
            bytes_note: None,
            logs: Vec::new(),
            progress: None,
            status: String::new(),
            task: None,
            logo,
            background,
        };
        app.refresh();
        app.spawn_releases(&cc.egui_ctx);
        app
    }

    fn refresh(&mut self) {
        self.versions = github_releases::installed_versions(&self.paths).unwrap_or_default();
        self.instances = instances::all(&self.paths).unwrap_or_default();
        if self.selected_instance.is_none() {
            self.selected_instance = self.instances.first().map(|i| i.id.clone());
        }
        if self
            .selected_instance
            .as_deref()
            .is_some_and(|id| !self.instances.iter().any(|i| i.id == id))
        {
            self.selected_instance = self.instances.first().map(|i| i.id.clone());
        }
    }

    fn log(&mut self, line: String) {
        if self.logs.len() > 400 {
            self.logs.drain(..80);
        }
        self.logs.push(line);
    }

    fn busy(&self) -> bool {
        self.task.is_some()
    }

    fn poll_task(&mut self, ctx: &egui::Context) {
        let mut messages = Vec::new();
        if let Some(task) = &self.task {
            loop {
                match task.rx.try_recv() {
                    Ok(message) => messages.push(message),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => break,
                }
            }
        }
        for message in messages {
            match message {
                Msg::Log(line) => self.log(line),
                Msg::Progress(fraction) => self.progress = Some(fraction),
                Msg::Bytes {
                    label,
                    downloaded,
                    total,
                } => {
                    let mb = |value: u64| format!("{:.1} MB", value as f64 / 1_000_000.0);
                    self.bytes_note = Some(format!(
                        "{label}: {} de {} · faltan {}",
                        mb(downloaded),
                        mb(total),
                        mb(total.saturating_sub(downloaded))
                    ));
                }
                Msg::Remote(releases) => {
                    self.latest_release = releases.first().cloned();
                    self.remote = releases;
                }
                Msg::News(items) => {
                    self.news = items;
                }
                Msg::Done(result) => {
                    match result {
                        Ok(message) => {
                            self.status = message;
                            self.progress = None;
                        }
                        Err(error) => {
                            self.status = error.clone();
                            self.log(format!("error: {error}"));
                        }
                    }
                    self.bytes_note = None;
                    self.task = None;
                    self.refresh();
                }
            }
        }
        if self.task.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }

    fn spawn_releases(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let client = download::client();
            match client {
                Ok(client) => {
                    let releases =
                        github_releases::list_releases(&client, 30).unwrap_or_default();
                    let _ = tx.send(Msg::Remote(releases));
                    match download::fetch_text(&client, NEWS_URL) {
                        Ok(text) => {
                            let _ = tx.send(Msg::News(parse_news(&text)));
                        }
                        Err(error) => {
                            let _ = tx.send(Msg::Log(format!("noticias: {error}")));
                        }
                    }
                }
                Err(error) => {
                    let _ = tx.send(Msg::Log(format!("error: {error}")));
                    let _ = tx.send(Msg::Done(Err(format!("{error}"))));
                    ctx.request_repaint();
                    return;
                }
            }
            let _ = tx.send(Msg::Done(Ok("versiones y noticias actualizadas".into())));
            ctx.request_repaint();
        });
        self.task = Some(Task {
            kind: TaskKind::Releases,
            rx,
        });
        self.status = "consultando versiones y noticias".into();
    }

    fn spawn_install(&mut self, ctx: &egui::Context, tag: String) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let paths = self.paths.clone();
        let ctx = ctx.clone();
        let thread_tag = tag.clone();
        std::thread::spawn(move || {
            let result = (|| -> RcResult<InstallResult> {
                let client = download::client()?;
                let release = github_releases::fetch_release(&client, &thread_tag)?;
                let cancel = AtomicBool::new(false);
                let progress_tx = tx.clone();
                github_releases::install_release(
                    &client,
                    &paths,
                    &release,
                    Platform::current()?,
                    &cancel,
                    |event| match event {
                        InstallEvent::Message(message) => {
                            let _ = progress_tx.send(Msg::Log(message));
                        }
                        InstallEvent::Download { downloaded, total } => {
                            if let Some(total) = total.filter(|total| *total > 0) {
                                let _ = progress_tx.send(Msg::Progress(
                                    downloaded as f32 / total as f32,
                                ));
                                let _ = progress_tx.send(Msg::Bytes {
                                    label: format!("horizon {thread_tag}"),
                                    downloaded,
                                    total,
                                });
                            }
                        }
                    },
                )
            })();
            let _ = match result {
                Ok(result) => tx.send(Msg::Done(Ok(format!(
                    "Horizon {} listo en {}",
                    result.meta.tag,
                    result.path.display()
                )))),
                Err(error) => tx.send(Msg::Done(Err(format!("{error}")))),
            };
            ctx.request_repaint();
        });
        self.task = Some(Task {
            kind: TaskKind::Install(tag.clone()),
            rx,
        });
        self.progress = Some(0.0);
        self.status = format!("instalando Horizon {tag}");
        self.bytes_note = None;
        self.logs.clear();
    }

    fn spawn_prepare(&mut self, ctx: &egui::Context, tag: String) {
        if self.busy() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let paths = self.paths.clone();
        let ctx = ctx.clone();
        let thread_tag = tag.clone();
        let java_home = self.config.java_home.clone();
        std::thread::spawn(move || {
            let result = (|| -> RcResult<String> {
                let api = prepare::launch_api(&paths, &thread_tag)?;
                if !prepare::minecraft_ready(&paths, &api.minecraft) {
                    let client = download::client()?;
                    let cancel = AtomicBool::new(false);
                    let progress_tx = tx.clone();
                    let progress_ctx = ctx.clone();
                    mojang::install(&client, &paths, &api.minecraft, &cancel, |event| {
                        match event {
                            MojangProgress::Message(message) => {
                                let _ = progress_tx.send(Msg::Log(message));
                            }
                            MojangProgress::Download {
                                label,
                                downloaded,
                                total,
                            } => {
                                if let Some(total) = total.filter(|total| *total > 0) {
                                    let _ = progress_tx.send(Msg::Bytes {
                                        label: label.to_owned(),
                                        downloaded,
                                        total,
                                    });
                                }
                            }
                            MojangProgress::Files {
                                label,
                                done,
                                total,
                            } => {
                                if total > 0 {
                                    let _ = progress_tx.send(Msg::Log(format!(
                                        "{label}: {}%",
                                        done * 100 / total
                                    )));
                                }
                            }
                        }
                    })?;
                    progress_ctx.request_repaint();
                }

                let cancel = AtomicBool::new(false);
                let progress_tx = tx.clone();
                let progress_ctx = ctx.clone();
                let prepared = prepare::prepare(
                    &paths,
                    &thread_tag,
                    java_home.as_deref(),
                    &cancel,
                    |event| match event {
                        PrepareProgress::Message(message) => {
                            let _ = progress_tx.send(Msg::Log(message));
                        }
                        PrepareProgress::Log(line) => {
                            let _ = progress_tx.send(Msg::Log(line));
                        }
                        PrepareProgress::Fraction(fraction) => {
                            let _ = progress_tx.send(Msg::Progress(fraction));
                            progress_ctx.request_repaint();
                        }
                    },
                )?;
                Ok(format!("recursos listos en {}", prepared.output.display()))
            })();
            let _ = match result {
                Ok(message) => tx.send(Msg::Done(Ok(message))),
                Err(error) => tx.send(Msg::Done(Err(format!("{error}")))),
            };
            ctx.request_repaint();
        });
        self.task = Some(Task {
            kind: TaskKind::Prepare(tag.clone()),
            rx,
        });
        self.progress = Some(0.0);
        self.status = format!("preparando Horizon {tag}");
        self.bytes_note = None;
        self.logs.clear();
    }

    fn spawn_play(&mut self, ctx: &egui::Context, instance: &Instance) {
        if self.busy() {
            return;
        }
        let account = match OfflineAccount::new(self.name_input.trim()) {
            Ok(account) => account,
            Err(error) => {
                self.dialog_text = self.name_input.clone();
                self.onboarding = true;
                self.status = format!("nombre no válido: {error}");
                return;
            }
        };
        let (tx, rx) = mpsc::channel();
        let paths = self.paths.clone();
        let instance = instance.clone();
        let ctx = ctx.clone();
        let width = self.width_input.clamp(320, 7680);
        let height = self.height_input.clamp(240, 4320);
        let label = instance.name.clone();
        let instance_id = instance.id.clone();
        let instance_name = instance.name.clone();
        std::thread::spawn(move || {
            let cancel = AtomicBool::new(false);
            let log_tx = tx.clone();
            let result = launch::run(
                &paths,
                &instance,
                &account,
                width,
                height,
                &cancel,
                |event| {
                    let line = match event {
                        launch::Event::Stdout(line) | launch::Event::Stderr(line) => line,
                    };
                    let _ = log_tx.send(Msg::Log(line));
                },
            );
            let _ = match result {
                Ok(outcome) => {
                    let mut message = format!("{} cerró con código {}", label, outcome.code);
                    if let Some(report) = outcome.crash_report {
                        message.push_str(&format!(" · crash report: {}", report.display()));
                    }
                    tx.send(Msg::Done(Ok(message)))
                }
                Err(error) => tx.send(Msg::Done(Err(format!("{error}")))),
            };
            ctx.request_repaint();
        });
        self.task = Some(Task {
            kind: TaskKind::Play(instance_id),
            rx,
        });
        self.status = format!("abriendo {instance_name}");
        self.logs.clear();
        self.bytes_note = None;
        self.progress = None;
    }

    fn save_settings(&mut self) {
        let name = self.name_input.trim().to_owned();
        if !name.is_empty() {
            match OfflineAccount::new(&name) {
                Ok(account) => {
                    self.name_input = account.name.clone();
                    self.config.player_name = Some(account.name);
                }
                Err(error) => {
                    self.status = format!("nombre no válido: {error}");
                    return;
                }
            }
        } else {
            self.config.player_name = None;
        }

        self.config.java_home = if self.java_input.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(self.java_input.trim()))
        };
        self.config.window = WindowConfig {
            width: self.width_input.clamp(320, 7680),
            height: self.height_input.clamp(240, 4320),
        };
        self.config.selected_instance = self.selected_instance.clone();
        match self.config.save(&self.paths.config_file()) {
            Ok(()) => self.status = "configuración guardada".into(),
            Err(error) => self.status = format!("no se pudo guardar: {error}"),
        }
    }

    fn create_instance(&mut self) {
        let name = self.new_instance.trim().to_owned();
        if name.is_empty() {
            self.status = "escribe un nombre para la instancia".into();
            return;
        }
        let tag = self
            .config
            .selected_version
            .clone()
            .or_else(|| self.versions.first().map(|v| v.meta.tag.clone()))
            .unwrap_or_else(|| github_releases::DEFAULT_TAG.to_owned());
        match Instance::create(&self.paths, &name, &tag) {
            Ok(instance) => {
                self.new_instance.clear();
                self.selected_instance = Some(instance.id.clone());
                self.status = format!("instancia {} creada", instance.name);
                self.refresh();
            }
            Err(error) => self.status = format!("{error}"),
        }
    }

    fn selected_instance(&self) -> Option<&Instance> {
        let id = self.selected_instance.as_deref()?;
        self.instances.iter().find(|instance| instance.id == id)
    }
}

impl eframe::App for RustCraftApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_task(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG))
            .show(ctx, |ui| {
                let full = ui.max_rect();
                let sidebar_width = if full.width() < 940.0 {
                    196.0
                } else {
                    SIDEBAR_WIDTH
                };
                let side_rect = Rect::from_min_max(
                    full.min,
                    Pos2::new(full.min.x + sidebar_width, full.max.y),
                );
                let content_rect = Rect::from_min_max(
                    Pos2::new(side_rect.right() + 26.0, full.min.y + 18.0),
                    Pos2::new(full.max.x - 26.0, full.max.y - 18.0),
                );

                if self.page == Page::Home {
                    self.paint_background(ui, content_rect);
                }

                self.paint_sidebar(ui, side_rect);
                self.paint_window_controls(ui, full);

                // The window has no system decorations: drag it from empty top space.
                let drag_rect = Rect::from_min_max(
                    Pos2::new(full.min.x, full.min.y),
                    Pos2::new(full.max.x - 130.0, full.min.y + 34.0),
                );
                let drag = ui.interact(drag_rect, ui.id().with("drag"), Sense::drag());
                if drag.drag_started() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }

                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(content_rect),
                    |ui| self.paint_content(ui, content_rect),
                );

                if self.onboarding {
                    self.paint_account_dialog(ui, full, true);
                } else if self.account_dialog {
                    self.paint_account_dialog(ui, full, false);
                }
            });
    }
}

impl RustCraftApp {
    fn paint_background(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::same(18), Color32::from_rgb(16, 14, 18));
        if let Some(texture) = &self.background {
            let available = rect.size();
            let image_size = texture.size_vec2();
            let scale = (available.x / image_size.x).max(available.y / image_size.y);
            let size = image_size * scale;
            let center = rect.center();
            let image_rect = Rect::from_center_size(center, size);
            // A single dark wash keeps the text readable without hiding the art.
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
            // Left-edge vertical gradient band keeps the hero text readable
            // without a flat dark wash over the whole scene.
            let fade = 460.0_f32.min(rect.width() * 0.55);
            let mut mesh = egui::Mesh::default();
            let y0 = rect.min.y;
            let y1 = rect.max.y;
            let x0 = rect.min.x;
            let x1 = rect.min.x + fade;
            let dark = Color32::from_black_alpha(120);
            let clear = Color32::from_black_alpha(0);
            mesh.colored_vertex(Pos2::new(x0, y0), dark);
            mesh.colored_vertex(Pos2::new(x0, y1), dark);
            mesh.colored_vertex(Pos2::new(x1, y0), clear);
            mesh.colored_vertex(Pos2::new(x1, y1), clear);
            mesh.indices.extend_from_slice(&[0, 2, 1, 1, 2, 3]);
            painter.add(mesh);
        }
    }

    fn paint_sidebar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, CornerRadius::ZERO, SIDEBAR);
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(5.0, rect.height())),
            CornerRadius::ZERO,
            YELLOW,
        );

        if let Some(logo) = &self.logo {
            let size = 92.0;
            let logo_rect = Rect::from_center_size(
                Pos2::new(rect.center().x, rect.min.y + 100.0),
                Vec2::splat(size),
            );
            painter.image(
                logo.id(),
                logo_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        let title_font = FontId::proportional(27.0);
        let galley = painter.layout_no_wrap("RustCraft".to_owned(), title_font.clone(), TEXT);
        let title_x = rect.center().x - galley.size().x * 0.5;
        painter.text(
            Pos2::new(title_x, rect.min.y + 168.0),
            Align2::LEFT_CENTER,
            "Rust",
            title_font.clone(),
            TEXT,
        );
        painter.text(
            Pos2::new(title_x + galley.size().x, rect.min.y + 168.0),
            Align2::RIGHT_CENTER,
            "Craft",
            title_font,
            YELLOW,
        );

        let items = [
            (Page::Home, 0u8, "Inicio"),
            (Page::Library, 1, "Biblioteca"),
            (Page::Settings, 2, "Configuración"),
            (Page::About, 3, "Acerca de"),
        ];
        for (index, (page, icon, label)) in items.into_iter().enumerate() {
            let top = rect.min.y + 226.0 + index as f32 * 66.0;
            let item = Rect::from_min_max(
                Pos2::new(rect.min.x + 22.0, top),
                Pos2::new(rect.max.x - 18.0, top + 54.0),
            );
            let response = ui.interact(item, ui.id().with(("nav", index)), Sense::click());
            let active = self.page == page;
            let hovered = response.hovered();
            if active {
                ui.painter()
                    .rect_filled(item, CornerRadius::same(16), YELLOW);
            } else if hovered {
                ui.painter()
                    .rect_filled(item, CornerRadius::same(16), CARD_SOFT);
            }
            let color = if active { Color32::BLACK } else { TEXT };
            let icon_center = Pos2::new(item.min.x + 30.0, item.center().y);
            if icon == 2 {
                ui.painter().text(
                    icon_center,
                    Align2::CENTER_CENTER,
                    "⚙",
                    FontId::proportional(21.0),
                    color,
                );
            } else {
                paint_icon(ui.painter(), icon_center, icon, color, 1.0);
            }
            ui.painter().text(
                Pos2::new(item.min.x + 56.0, item.center().y),
                Align2::LEFT_CENTER,
                label,
                FontId::proportional(18.0),
                color,
            );
            if response.clicked() {
                self.page = page;
            }
        }

        let bottom = rect.max.y - 46.0;
        let color = if self.busy() { YELLOW } else { GREEN };
        ui.painter().circle_filled(
            Pos2::new(rect.min.x + 34.0, bottom),
            5.0,
            color,
        );
        ui.painter().text(
            Pos2::new(rect.min.x + 48.0, bottom),
            Align2::LEFT_CENTER,
            if self.busy() { "Trabajando…" } else { "En línea" },
            FontId::proportional(15.0),
            MUTED,
        );

        // A thin separator above the status line, like the mockup.
        ui.painter().line_segment(
            [
                Pos2::new(rect.min.x + 22.0, bottom - 28.0),
                Pos2::new(rect.max.x - 22.0, bottom - 28.0),
            ],
            Stroke::new(1.0_f32, Color32::from_rgb(38, 35, 40)),
        );
    }

    fn paint_window_controls(&mut self, ui: &mut egui::Ui, full: Rect) {
        let size = Vec2::splat(30.0);
        let start = Pos2::new(full.max.x - 122.0, full.min.y + 12.0);
        let labels = ["–", "□", "✕"];
        for (index, label) in labels.into_iter().enumerate() {
            let rect = Rect::from_min_size(
                Pos2::new(start.x + index as f32 * 38.0, start.y),
                size,
            );
            let response = ui.interact(rect, ui.id().with(("window", index)), Sense::click());
            let color = if response.hovered() {
                if index == 2 { RED } else { CARD_SOFT }
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius::same(8), color);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(16.0),
                TEXT,
            );
            if response.clicked() {
                match index {
                    0 => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                    1 => {
                        let maximized = ui
                            .ctx()
                            .input(|input| input.viewport().maximized)
                            .unwrap_or(false);
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                    }
                    _ => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                }
            }
        }
    }

    fn paint_content(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let top = Pos2::new(rect.min.x, rect.min.y + 30.0);
        let account = self.name_input.trim().to_owned();
        let (account_label, account_color) = if account.is_empty() {
            ("Sin cuenta".to_owned(), RED)
        } else {
            (account, TEXT)
        };
        let chip_width = 172.0;
        let chip = Rect::from_min_size(
            Pos2::new(rect.max.x - chip_width, top.y),
            Vec2::new(chip_width, 42.0),
        );
        ui.painter()
            .rect_filled(chip, CornerRadius::same(21), Color32::from_rgb(32, 29, 35));
        let chip_response = ui.interact(chip, ui.id().with("account-chip"), Sense::click());
        ui.painter().circle_filled(
            Pos2::new(chip.min.x + 22.0, chip.center().y),
            10.0,
            CARD_SOFT,
        );
        ui.painter().circle_filled(
            Pos2::new(chip.min.x + 22.0, chip.center().y - 3.0),
            4.0,
            TEXT,
        );
        ui.painter().text(
            Pos2::new(chip.min.x + 42.0, chip.center().y),
            Align2::LEFT_CENTER,
            account_label,
            FontId::proportional(15.0),
            account_color,
        );
        ui.painter().text(
            Pos2::new(chip.max.x - 18.0, chip.center().y),
            Align2::CENTER_CENTER,
            "▾",
            FontId::proportional(13.0),
            MUTED,
        );
        if chip_response.clicked() && !self.onboarding {
            self.dialog_text = self.name_input.clone();
            self.account_dialog = true;
        }

        match self.page {
            Page::Home => self.home(ui, rect, top.y + 16.0),
            Page::Library => self.library(ui, rect, top.y + 16.0),
            Page::Settings => self.settings(ui, rect, top.y + 16.0),
            Page::About => self.about(ui, rect, top.y + 16.0),
        }
    }

    fn home(&mut self, ui: &mut egui::Ui, rect: Rect, top: f32) {
        let left = rect.min.x + 10.0;
        let compact = rect.width() < 720.0;
        let title_size = if compact { 40.0 } else { 58.0 };
        let title_y = top + if compact { 46.0 } else { 74.0 };
        let title = ui.painter().text(
            Pos2::new(left, title_y),
            Align2::LEFT_CENTER,
            "Rust",
            FontId::proportional(title_size),
            TEXT,
        );
        ui.painter().text(
            Pos2::new(title.right() + 2.0, title_y),
            Align2::LEFT_CENTER,
            "Craft",
            FontId::proportional(title_size),
            YELLOW,
        );
        ui.painter().text(
            Pos2::new(left + 4.0, title_y + title_size * 0.85),
            Align2::LEFT_CENTER,
            "Juega. Crea. Explora.",
            FontId::proportional(if compact { 16.0 } else { 22.0 }),
            MUTED,
        );

        let button_height = if compact { 46.0 } else { 58.0 };
        let play_width = if compact { 170.0 } else { 252.0 };
        let play_rect = Rect::from_min_size(
            Pos2::new(left, title_y + title_size + 26.0),
            Vec2::new(play_width, button_height),
        );
        let has_account = OfflineAccount::new(self.name_input.trim()).is_ok();
        let enabled = !self.busy()
            && self.selected_instance().is_some()
            && has_account
            && !self.onboarding;
        let play_fill = if enabled { YELLOW } else { Color32::from_rgb(120, 100, 20) };
        let play = ui.put(
            play_rect,
            egui::Button::new(
                RichText::new("   ▶   Jugar")
                    .size(if compact { 18.0 } else { 22.0 })
                    .strong()
                    .color(Color32::BLACK),
            )
            .fill(play_fill)
            .corner_radius(16.0)
            .stroke(Stroke::new(2.0_f32, Color32::from_rgb(70, 58, 10))),
        );
        if play.clicked() && enabled {
            if let Some(instance) = self.selected_instance().cloned() {
                self.spawn_play(ui.ctx(), &instance);
            }
        }

        let folder_rect = if compact {
            Rect::from_min_size(
                Pos2::new(left, play_rect.max.y + 10.0),
                Vec2::new(play_width + 10.0, 36.0),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(play_rect.right() + 16.0, play_rect.min.y),
                Vec2::new(232.0, 58.0),
            )
        };
        let folder = ui.put(
            folder_rect,
            egui::Button::new(
                RichText::new("🗀  Examinar archivos")
                    .size(if compact { 14.0 } else { 17.0 })
                    .color(TEXT),
            )
            .fill(Color32::from_rgb(30, 27, 33))
            .corner_radius(if compact { 10.0 } else { 16.0 })
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(70, 65, 70))),
        );
        if folder.clicked() {
            open_path(&self.paths.root());
        }

        let news_top = if compact {
            (rect.min.y + 360.0).max(rect.max.y - 250.0)
        } else {
            rect.min.y + rect.height() * 0.60
        };
        let news_height = (rect.max.y - news_top - 10.0).max(150.0);
        let quick_width = 300.0;
        let news_right = if compact {
            rect.max.x
        } else {
            rect.max.x - quick_width - 18.0
        };
        let news_rect = Rect::from_min_max(
            Pos2::new(left, news_top),
            Pos2::new(news_right, news_top + news_height),
        );
        ui.painter()
            .rect_filled(news_rect, CornerRadius::same(18), CARD);
        ui.painter().text(
            Pos2::new(news_rect.min.x + 22.0, news_rect.min.y + 30.0),
            Align2::LEFT_CENTER,
            "Noticias",
            FontId::proportional(24.0),
            TEXT,
        );
        ui.painter().rect_filled(
            Rect::from_min_size(
                Pos2::new(news_rect.min.x + 4.0, news_rect.min.y + 18.0),
                Vec2::new(5.0, 26.0),
            ),
            CornerRadius::same(2),
            YELLOW,
        );

        let (headline, body, date) = if let Some(news) = self.news.first() {
            (
                news.title.clone(),
                news.body.clone(),
                news.date.clone(),
            )
        } else if let Some(release) = &self.latest_release {
            (
                format!("Horizon {}", release.tag_name),
                summarize(&release.body, 420),
                release.published_at.clone().unwrap_or_default(),
            )
        } else {
            (
                "Bienvenido a RustCraft".to_owned(),
                "Un lanzador offline para Horizon. Instala una versión, prepara los recursos y juega con tu nombre local. Sin cuentas de Microsoft.".to_owned(),
                String::new(),
            )
        };
        // Keep every text inside the card: clip to its inner rect.
        let text_rect = Rect::from_min_max(
            Pos2::new(news_rect.min.x + 22.0, news_rect.min.y + 64.0),
            Pos2::new(news_rect.max.x - 22.0, news_rect.max.y - 30.0),
        );
        ui.painter()
            .with_clip_rect(text_rect)
            .text(
                Pos2::new(text_rect.min.x, text_rect.min.y),
                Align2::LEFT_TOP,
                headline,
                FontId::proportional(19.0),
                TEXT,
            );
        ui.painter()
            .with_clip_rect(text_rect)
            .text(
                Pos2::new(text_rect.min.x, text_rect.min.y + 28.0),
                Align2::LEFT_TOP,
                body,
                FontId::proportional(15.0),
                MUTED,
            );
        if !self.versions.is_empty() {
            ui.painter().text(
                Pos2::new(text_rect.min.x, news_rect.max.y - 24.0),
                Align2::LEFT_CENTER,
                format!(
                    "{} versiones instaladas · {} instancias",
                    self.versions.len(),
                    self.instances.len()
                ),
                FontId::proportional(13.0),
                MUTED,
            );
        }
        if !date.is_empty() {
            ui.painter().text(
                Pos2::new(news_rect.max.x - 22.0, news_rect.max.y - 24.0),
                Align2::RIGHT_CENTER,
                date,
                FontId::proportional(12.0),
                MUTED,
            );
        }

        if !compact {
            let quick_rect = Rect::from_min_max(
                Pos2::new(news_rect.max.x + 18.0, news_top),
                Pos2::new(rect.max.x, news_top + news_height),
            );
            ui.painter()
                .rect_filled(quick_rect, CornerRadius::same(18), CARD);
            ui.painter().text(
                Pos2::new(quick_rect.min.x + 20.0, quick_rect.min.y + 30.0),
                Align2::LEFT_CENTER,
                "⚡",
                FontId::proportional(20.0),
                YELLOW,
            );
            ui.painter().text(
                Pos2::new(quick_rect.min.x + 44.0, quick_rect.min.y + 30.0),
                Align2::LEFT_CENTER,
                "Accesos rápidos",
                FontId::proportional(18.0),
                TEXT,
            );
            let entries = [
                (1u8, "Biblioteca", Page::Library),
                (2, "Configuración", Page::Settings),
                (3, "Ayuda", Page::About),
            ];
            for (index, (icon, label, page)) in entries.into_iter().enumerate() {
                let row = Rect::from_min_size(
                    Pos2::new(
                        quick_rect.min.x + 14.0,
                        quick_rect.min.y + 62.0 + index as f32 * 46.0,
                    ),
                    Vec2::new(quick_rect.width() - 28.0, 38.0),
                );
                let response =
                    ui.interact(row, ui.id().with(("quick", index)), Sense::click());
                if response.hovered() {
                    ui.painter()
                        .rect_filled(row, CornerRadius::same(10), CARD_SOFT);
                }
                let icon_center = Pos2::new(row.min.x + 16.0, row.center().y);
                if icon == 2 {
                    ui.painter().text(
                        icon_center,
                        Align2::CENTER_CENTER,
                        "⚙",
                        FontId::proportional(16.0),
                        YELLOW,
                    );
                } else {
                    paint_icon(ui.painter(), icon_center, icon, YELLOW, 0.8);
                }
                ui.painter().text(
                    Pos2::new(row.min.x + 36.0, row.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(15.0),
                    TEXT,
                );
                ui.painter().text(
                    Pos2::new(row.max.x - 16.0, row.center().y),
                    Align2::CENTER_CENTER,
                    "›",
                    FontId::proportional(20.0),
                    MUTED,
                );
                if response.clicked() {
                    self.page = page;
                }
            }
        }

        if self.busy() {
            let log_rect = Rect::from_min_max(
                Pos2::new(left, news_top),
                Pos2::new(rect.max.x, rect.max.y - 8.0),
            );
            self.paint_logs(ui, log_rect);
        }
    }

    fn library(&mut self, ui: &mut egui::Ui, rect: Rect, top: f32) {
        ui.painter().text(
            Pos2::new(rect.min.x + 10.0, top + 30.0),
            Align2::LEFT_CENTER,
            "Biblioteca",
            FontId::proportional(34.0),
            TEXT,
        );
        ui.painter().text(
            Pos2::new(rect.min.x + 12.0, top + 66.0),
            Align2::LEFT_CENTER,
            "Versiones de Horizon y recursos preparados.",
            FontId::proportional(15.0),
            MUTED,
        );

        let install_tag = match &self.task {
            Some(Task {
                kind: TaskKind::Install(tag),
                ..
            }) => Some(tag.clone()),
            _ => None,
        };
        if let Some(tag) = &install_tag {
            self.paint_install_progress(ui, rect, top, tag);
        }

        let column_top = top + 96.0 + if install_tag.is_some() { 96.0 } else { 0.0 };
        let compact = rect.width() < 720.0;
        let available_height =
            (rect.height() - (column_top - rect.min.y) - 190.0).max(260.0);
        let (installed_rect, remote_rect) = if compact {
            let height = (available_height - 14.0) / 2.0;
            (
                Rect::from_min_size(
                    Pos2::new(rect.min.x + 10.0, column_top),
                    Vec2::new(rect.width() - 20.0, height),
                ),
                Rect::from_min_size(
                    Pos2::new(rect.min.x + 10.0, column_top + height + 14.0),
                    Vec2::new(rect.width() - 20.0, height),
                ),
            )
        } else {
            let half = (rect.width() - 40.0) / 2.0;
            let installed = Rect::from_min_size(
                Pos2::new(rect.min.x + 10.0, column_top),
                Vec2::new(half, available_height),
            );
            let remote = Rect::from_min_size(
                Pos2::new(installed.max.x + 20.0, column_top),
                installed.size(),
            );
            (installed, remote)
        };

        ui.painter()
            .rect_filled(installed_rect, CornerRadius::same(16), CARD);
        ui.painter().text(
            Pos2::new(installed_rect.min.x + 20.0, installed_rect.min.y + 26.0),
            Align2::LEFT_CENTER,
            "Instaladas",
            FontId::proportional(20.0),
            TEXT,
        );

        let mut row_y = installed_rect.min.y + 54.0;
        if self.versions.is_empty() {
            ui.painter().text(
                Pos2::new(installed_rect.min.x + 20.0, row_y + 10.0),
                Align2::LEFT_TOP,
                "Todavía no hay ninguna versión.\nInstala nightly desde la lista de la derecha.",
                FontId::proportional(14.0),
                MUTED,
            );
        }
        let versions: Vec<_> = self
            .versions
            .iter()
            .map(|version| {
                (
                    version.meta.tag.clone(),
                    version.meta.asset.clone(),
                    version.meta.installed_at,
                )
            })
            .collect();
        for (tag, asset, _) in versions {
            if row_y + 82.0 > installed_rect.max.y {
                break;
            }
            let row = Rect::from_min_size(
                Pos2::new(installed_rect.min.x + 14.0, row_y),
                Vec2::new(installed_rect.width() - 28.0, 76.0),
            );
            ui.painter()
                .rect_filled(row, CornerRadius::same(12), CARD_SOFT);
            ui.painter().text(
                Pos2::new(row.min.x + 14.0, row.min.y + 22.0),
                Align2::LEFT_CENTER,
                &tag,
                FontId::proportional(17.0),
                TEXT,
            );
            ui.painter().text(
                Pos2::new(row.min.x + 14.0, row.min.y + 48.0),
                Align2::LEFT_CENTER,
                asset,
                FontId::proportional(12.0),
                MUTED,
            );

            let play_rect = Rect::from_min_size(
                Pos2::new(row.max.x - 156.0, row.min.y + 18.0),
                Vec2::new(70.0, 36.0),
            );
            let play = ui.put(
                play_rect,
                egui::Button::new(RichText::new("Jugar").size(14.0).color(Color32::BLACK))
                    .fill(YELLOW)
                    .corner_radius(10.0),
            );
            if play.clicked() {
                self.config.selected_version = Some(tag.clone());
                let _ = self.config.save(&self.paths.config_file());
                if let Some(instance) = self.selected_instance().cloned() {
                    self.spawn_play(ui.ctx(), &instance);
                }
            }

            let prep_rect = Rect::from_min_size(
                Pos2::new(row.max.x - 80.0, row.min.y + 18.0),
                Vec2::new(66.0, 36.0),
            );
            let prep = ui.put(
                prep_rect,
                egui::Button::new(RichText::new("Preparar").size(12.0).color(TEXT))
                    .fill(CARD)
                    .corner_radius(10.0)
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(70, 65, 70))),
            );
            if prep.clicked() {
                self.config.selected_version = Some(tag.clone());
                let _ = self.config.save(&self.paths.config_file());
                self.spawn_prepare(ui.ctx(), tag.clone());
            }

            row_y += 86.0;
        }

        ui.painter()
            .rect_filled(remote_rect, CornerRadius::same(16), CARD);
        ui.painter().text(
            Pos2::new(remote_rect.min.x + 20.0, remote_rect.min.y + 26.0),
            Align2::LEFT_CENTER,
            "Disponibles en GitHub",
            FontId::proportional(20.0),
            TEXT,
        );
        let refresh_rect = Rect::from_min_size(
            Pos2::new(remote_rect.max.x - 120.0, remote_rect.min.y + 12.0),
            Vec2::new(104.0, 30.0),
        );
        let refresh = ui.put(
            refresh_rect,
            egui::Button::new(RichText::new("Actualizar").size(13.0).color(YELLOW))
                .fill(CARD_SOFT)
                .corner_radius(9.0),
        );
        if refresh.clicked() {
            self.spawn_releases(ui.ctx());
        }

        let mut remote_y = remote_rect.min.y + 54.0;
        if self.remote.is_empty() {
            ui.painter().text(
                Pos2::new(remote_rect.min.x + 20.0, remote_y + 10.0),
                Align2::LEFT_TOP,
                "Pulsa Actualizar para consultar las versiones de horizon-rust/horizon.",
                FontId::proportional(14.0),
                MUTED,
            );
        }
        let releases: Vec<_> = self
            .remote
            .iter()
            .map(|release| {
                (
                    release.tag_name.clone(),
                    release.published_at.clone().unwrap_or_default(),
                    release.prerelease,
                )
            })
            .take(8)
            .collect();
        for (tag, date, prerelease) in releases {
            if remote_y + 66.0 > remote_rect.max.y {
                break;
            }
            let row = Rect::from_min_size(
                Pos2::new(remote_rect.min.x + 14.0, remote_y),
                Vec2::new(remote_rect.width() - 28.0, 60.0),
            );
            ui.painter()
                .rect_filled(row, CornerRadius::same(12), CARD_SOFT);
            ui.painter().text(
                Pos2::new(row.min.x + 14.0, row.min.y + 20.0),
                Align2::LEFT_CENTER,
                &tag,
                FontId::proportional(16.0),
                TEXT,
            );
            let note = if prerelease {
                format!("{date} · preliminar")
            } else {
                date.clone()
            };
            ui.painter().text(
                Pos2::new(row.min.x + 14.0, row.min.y + 42.0),
                Align2::LEFT_CENTER,
                note,
                FontId::proportional(11.0),
                MUTED,
            );
            let install_rect = Rect::from_min_size(
                Pos2::new(row.max.x - 92.0, row.min.y + 12.0),
                Vec2::new(80.0, 34.0),
            );
            let install = ui.put(
                install_rect,
                egui::Button::new(
                    RichText::new("Instalar").size(13.0).color(Color32::BLACK),
                )
                .fill(YELLOW)
                .corner_radius(10.0),
            );
            if install.clicked() {
                self.spawn_install(ui.ctx(), tag.clone());
            }
            remote_y += 68.0;
        }

        let log_rect = Rect::from_min_size(
            Pos2::new(rect.min.x + 10.0, rect.max.y - 170.0),
            Vec2::new(rect.width() - 20.0, 160.0),
        );
        self.paint_logs(ui, log_rect);
    }

    fn settings(&mut self, ui: &mut egui::Ui, rect: Rect, top: f32) {
        ui.painter().text(
            Pos2::new(rect.min.x + 10.0, top + 30.0),
            Align2::LEFT_CENTER,
            "Configuración",
            FontId::proportional(34.0),
            TEXT,
        );
        let compact = rect.width() < 720.0;
        let card = if compact {
            Rect::from_min_size(
                Pos2::new(rect.min.x + 10.0, top + 78.0),
                Vec2::new(rect.width() - 20.0, 400.0),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(rect.min.x + 10.0, top + 78.0),
                Vec2::new((rect.width() - 40.0) * 0.58, rect.height() - 300.0),
            )
        };
        ui.painter()
            .rect_filled(card, CornerRadius::same(16), CARD);

        let left = card.min.x + 24.0;
        let mut y = card.min.y + 28.0;
        ui.painter().text(
            Pos2::new(left, y),
            Align2::LEFT_CENTER,
            "Jugador offline",
            FontId::proportional(19.0),
            TEXT,
        );
        y += 34.0;
        ui.painter().text(
            Pos2::new(left, y),
            Align2::LEFT_CENTER,
            "Nombre (3-16 letras, números o _)",
            FontId::proportional(13.0),
            MUTED,
        );
        y += 24.0;
        let name_rect = Rect::from_min_size(Pos2::new(left, y), Vec2::new(300.0, 36.0));
        ui.put(
            name_rect,
            egui::TextEdit::singleline(&mut self.name_input)
                .desired_width(300.0)
                .font(FontId::proportional(16.0)),
        );
        y += 44.0;
        match OfflineAccount::new(self.name_input.trim()) {
            Ok(account) => {
                ui.painter().text(
                    Pos2::new(left, y),
                    Align2::LEFT_TOP,
                    format!("UUID offline: {}", account.uuid),
                    FontId::proportional(12.0),
                    MUTED,
                );
            }
            Err(error) => {
                ui.painter().text(
                    Pos2::new(left, y),
                    Align2::LEFT_TOP,
                    format!("{error}"),
                    FontId::proportional(12.0),
                    RED,
                );
            }
        }
        y += 44.0;
        ui.painter().text(
            Pos2::new(left, y),
            Align2::LEFT_CENTER,
            "Java 25 (carpeta del JRE)",
            FontId::proportional(13.0),
            MUTED,
        );
        y += 24.0;
        let java_rect = Rect::from_min_size(
            Pos2::new(left, y),
            Vec2::new(card.width() - 48.0, 36.0),
        );
        ui.put(
            java_rect,
            egui::TextEdit::singleline(&mut self.java_input)
                .desired_width(java_rect.width())
                .font(FontId::proportional(14.0)),
        );
        y += 46.0;
        ui.painter().text(
            Pos2::new(left, y),
            Align2::LEFT_CENTER,
            "Ventana del juego",
            FontId::proportional(13.0),
            MUTED,
        );
        y += 28.0;
        let width_rect = Rect::from_min_size(Pos2::new(left, y), Vec2::new(100.0, 30.0));
        let height_rect =
            Rect::from_min_size(Pos2::new(left + 120.0, y), Vec2::new(100.0, 30.0));
        ui.put(
            width_rect,
            egui::DragValue::new(&mut self.width_input)
                .range(320..=7680)
                .speed(2.0)
                .prefix("ancho "),
        );
        ui.put(
            height_rect,
            egui::DragValue::new(&mut self.height_input)
                .range(240..=4320)
                .speed(2.0)
                .prefix("alto "),
        );

        y += 54.0;
        let save_rect = Rect::from_min_size(Pos2::new(left, y), Vec2::new(150.0, 40.0));
        let save = ui.put(
            save_rect,
            egui::Button::new(
                RichText::new("Guardar").size(16.0).strong().color(Color32::BLACK),
            )
            .fill(YELLOW)
            .corner_radius(12.0),
        );
        if save.clicked() {
            self.save_settings();
        }

        let side = if compact {
            Rect::from_min_size(
                Pos2::new(rect.min.x + 10.0, card.max.y + 14.0),
                Vec2::new(
                    rect.width() - 20.0,
                    (rect.max.y - card.max.y - 30.0).max(150.0),
                ),
            )
        } else {
            Rect::from_min_size(
                Pos2::new(card.max.x + 18.0, card.min.y),
                Vec2::new(rect.max.x - 10.0 - (card.max.x + 18.0), card.height()),
            )
        };
        ui.painter()
            .rect_filled(side, CornerRadius::same(16), CARD);
        ui.painter().text(
            Pos2::new(side.min.x + 22.0, side.min.y + 30.0),
            Align2::LEFT_CENTER,
            "Instancias",
            FontId::proportional(19.0),
            TEXT,
        );
        let mut iy = side.min.y + 54.0;
        let instances: Vec<_> = self
            .instances
            .iter()
            .map(|instance| {
                (
                    instance.id.clone(),
                    instance.name.clone(),
                    instance.version_tag.clone(),
                )
            })
            .collect();
        for (id, name, tag) in instances {
            let row = Rect::from_min_size(
                Pos2::new(side.min.x + 14.0, iy),
                Vec2::new(side.width() - 28.0, 40.0),
            );
            let response = ui.interact(row, ui.id().with(("inst", &id)), Sense::click());
            if self.selected_instance.as_deref() == Some(id.as_str()) {
                ui.painter()
                    .rect_filled(row, CornerRadius::same(9), CARD_SOFT);
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(row, CornerRadius::same(9), CARD_SOFT);
            }
            ui.painter().text(
                Pos2::new(row.min.x + 12.0, row.min.y + 15.0),
                Align2::LEFT_CENTER,
                name,
                FontId::proportional(14.0),
                TEXT,
            );
            ui.painter().text(
                Pos2::new(row.min.x + 12.0, row.min.y + 31.0),
                Align2::LEFT_CENTER,
                tag,
                FontId::proportional(11.0),
                MUTED,
            );
            if response.clicked() {
                self.selected_instance = Some(id.clone());
            }
            iy += 44.0;
        }

        let input_rect = Rect::from_min_size(
            Pos2::new(side.min.x + 14.0, iy + 6.0),
            Vec2::new(side.width() - 108.0, 32.0),
        );
        ui.put(
            input_rect,
            egui::TextEdit::singleline(&mut self.new_instance)
                .hint_text("Nueva instancia")
                .desired_width(input_rect.width()),
        );
        let create_rect = Rect::from_min_size(
            Pos2::new(input_rect.max.x + 8.0, iy + 6.0),
            Vec2::new(78.0, 32.0),
        );
        let create = ui.put(
            create_rect,
            egui::Button::new(RichText::new("Crear").size(13.0).color(Color32::BLACK))
                .fill(YELLOW)
                .corner_radius(9.0),
        );
        if create.clicked() {
            self.create_instance();
        }

        ui.painter().text(
            Pos2::new(rect.min.x + 12.0, rect.max.y - 92.0),
            Align2::LEFT_TOP,
            format!("Datos: {}", self.paths.root().display()),
            FontId::proportional(12.0),
            MUTED,
        );
        ui.painter().text(
            Pos2::new(rect.min.x + 12.0, rect.max.y - 70.0),
            Align2::LEFT_TOP,
            format!("Config: {}", self.paths.config_file().display()),
            FontId::proportional(12.0),
            MUTED,
        );
    }

    fn about(&mut self, ui: &mut egui::Ui, rect: Rect, top: f32) {
        ui.painter().text(
            Pos2::new(rect.min.x + 10.0, top + 30.0),
            Align2::LEFT_CENTER,
            "Acerca de RustCraft",
            FontId::proportional(34.0),
            TEXT,
        );
        let card = Rect::from_min_size(
            Pos2::new(rect.min.x + 10.0, top + 86.0),
            Vec2::new((rect.width() - 20.0).min(820.0), 330.0),
        );
        ui.painter()
            .rect_filled(card, CornerRadius::same(16), CARD);
        let lines = [
            ("RustCraft", "Lanzador offline para Horizon, la reescritura de Minecraft Java Edition en Rust."),
            ("Cuenta", "Solo nombres offline. No hay Microsoft, OAuth ni Xbox Live."),
            ("UUID", "Se deriva del nombre (UUID v3 de OfflinePlayer:<nombre>), igual que el servidor."),
            ("Multijugador", "Compatible con un jugador, LAN y servidores con online-mode=false."),
            ("Créditos", "Horizon: horizon-rust/horizon. Archivos de Minecraft: Mojang."),
        ];
        let mut y = card.min.y + 30.0;
        for (title, body) in lines {
            ui.painter().text(
                Pos2::new(card.min.x + 24.0, y),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(15.0),
                YELLOW,
            );
            ui.painter().text(
                Pos2::new(card.min.x + 140.0, y),
                Align2::LEFT_CENTER,
                body,
                FontId::proportional(14.0),
                TEXT,
            );
            y += 48.0;
        }
        ui.painter().text(
            Pos2::new(card.min.x + 24.0, card.max.y - 24.0),
            Align2::LEFT_CENTER,
            format!("Versión {} · datos en {}", env!("CARGO_PKG_VERSION"), self.paths.root().display()),
            FontId::proportional(12.0),
            MUTED,
        );
    }

    fn paint_account_dialog(&mut self, ui: &mut egui::Ui, full: Rect, forced: bool) {
        ui.painter()
            .rect_filled(full, CornerRadius::ZERO, Color32::from_black_alpha(140));

        let size = Vec2::new(380.0, 300.0);
        let dialog = Rect::from_center_size(full.center(), size);
        ui.painter()
            .rect_filled(dialog, CornerRadius::same(18), CARD);
        ui.painter()
            .rect_stroke(
                dialog,
                CornerRadius::same(18),
                Stroke::new(1.0_f32, YELLOW),
                egui::StrokeKind::Outside,
            );

        ui.painter().text(
            Pos2::new(dialog.min.x + 24.0, dialog.min.y + 34.0),
            Align2::LEFT_CENTER,
            if forced {
                "Elige tu nombre"
            } else {
                "Gestionar cuenta"
            },
            FontId::proportional(22.0),
            TEXT,
        );
        ui.painter().text(
            Pos2::new(dialog.min.x + 24.0, dialog.min.y + 66.0),
            Align2::LEFT_TOP,
            if forced {
                "Antes de jugar necesitas un nombre offline.\n3-16 letras, números o guion bajo. Sin Microsoft."
            } else {
                "Elige el nombre offline con el que juegas.\n3-16 letras, números o guion bajo. Sin Microsoft."
            },
            FontId::proportional(13.0),
            MUTED,
        );

        let input_rect = Rect::from_min_size(
            Pos2::new(dialog.min.x + 24.0, dialog.min.y + 112.0),
            Vec2::new(size.x - 48.0, 36.0),
        );
        ui.put(
            input_rect,
            egui::TextEdit::singleline(&mut self.dialog_text)
                .hint_text("Nombre, por ejemplo Alex")
                .desired_width(input_rect.width())
                .font(FontId::proportional(16.0)),
        );

        let trimmed = self.dialog_text.trim();
        let valid = OfflineAccount::new(trimmed).is_ok();
        match OfflineAccount::new(trimmed) {
            Ok(account) if !trimmed.is_empty() => {
                ui.painter().text(
                    Pos2::new(dialog.min.x + 24.0, dialog.min.y + 160.0),
                    Align2::LEFT_TOP,
                    format!("UUID: {}", account.uuid),
                    FontId::proportional(12.0),
                    YELLOW,
                );
            }
            _ => {
                ui.painter().text(
                    Pos2::new(dialog.min.x + 24.0, dialog.min.y + 160.0),
                    Align2::LEFT_TOP,
                    if trimmed.is_empty() {
                        "Sin nombre: no podrás jugar todavía".to_owned()
                    } else {
                        "Nombre no válido".to_owned()
                    },
                    FontId::proportional(12.0),
                    RED,
                );
            }
        }

        let button_y = dialog.max.y - 62.0;
        let save_width = if forced { 160.0 } else { 140.0 };
        let save = ui.put(
            Rect::from_min_size(
                Pos2::new(
                    if forced {
                        dialog.center().x - save_width * 0.5
                    } else {
                        dialog.min.x + 24.0
                    },
                    button_y,
                ),
                Vec2::new(save_width, 42.0),
            ),
            egui::Button::new(
                RichText::new(if forced { "Empezar a jugar" } else { "Guardar" })
                    .size(15.0)
                    .strong()
                    .color(Color32::BLACK),
            )
            .fill(if valid {
                YELLOW
            } else {
                Color32::from_rgb(120, 100, 20)
            })
            .corner_radius(12.0),
        );
        if save.clicked() && valid {
            self.name_input = trimmed.to_owned();
            self.save_settings();
            self.account_dialog = false;
            self.onboarding = false;
        }

        if !forced {
            let remove = ui.put(
                Rect::from_min_size(
                    Pos2::new(dialog.min.x + 176.0, button_y),
                    Vec2::new(120.0, 42.0),
                ),
                egui::Button::new(RichText::new("Eliminar").size(15.0).color(RED))
                    .fill(CARD_SOFT)
                    .corner_radius(12.0)
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(90, 45, 45))),
            );
            if remove.clicked() {
                self.name_input.clear();
                self.config.player_name = None;
                let _ = self.config.save(&self.paths.config_file());
                self.status = "cuenta eliminada".into();
                self.dialog_text.clear();
                self.account_dialog = false;
                self.onboarding = true;
            }

            let close = ui.put(
                Rect::from_min_size(
                    Pos2::new(dialog.max.x - 92.0, button_y),
                    Vec2::new(68.0, 42.0),
                ),
                egui::Button::new(RichText::new("Cerrar").size(15.0).color(TEXT))
                    .fill(CARD_SOFT)
                    .corner_radius(12.0),
            );
            if close.clicked() {
                self.account_dialog = false;
            }
        }
    }

    fn paint_install_progress(&self, ui: &egui::Ui, rect: Rect, top: f32, tag: &str) {
        let card = Rect::from_min_size(
            Pos2::new(rect.min.x + 10.0, top + 88.0),
            Vec2::new(rect.width() - 20.0, 84.0),
        );
        ui.painter()
            .rect_filled(card, CornerRadius::same(14), CARD);
        ui.painter().rect_filled(
            Rect::from_min_size(card.min, Vec2::new(4.0, card.height())),
            CornerRadius::same(2),
            YELLOW,
        );

        let progress = self.progress.unwrap_or(0.0).clamp(0.0, 1.0);
        ui.painter().text(
            Pos2::new(card.min.x + 18.0, card.min.y + 20.0),
            Align2::LEFT_CENTER,
            format!("Instalando Horizon {tag}"),
            FontId::proportional(16.0),
            TEXT,
        );
        ui.painter().text(
            Pos2::new(card.max.x - 18.0, card.min.y + 20.0),
            Align2::RIGHT_CENTER,
            format!("{:.0} %", progress * 100.0),
            FontId::proportional(16.0),
            YELLOW,
        );

        let bar = Rect::from_min_size(
            Pos2::new(card.min.x + 18.0, card.min.y + 36.0),
            Vec2::new(card.width() - 36.0, 8.0),
        );
        ui.painter()
            .rect_filled(bar, CornerRadius::same(4), Color32::from_rgb(45, 41, 47));
        if progress > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_size(
                    bar.min,
                    Vec2::new(bar.width() * progress, bar.height()),
                ),
                CornerRadius::same(4),
                YELLOW,
            );
        }

        let stage = if progress >= 1.0 {
            "Instalando archivos…".to_owned()
        } else {
            self.bytes_note
                .clone()
                .unwrap_or_else(|| "Conectando con GitHub…".to_owned())
        };
        ui.painter().text(
            Pos2::new(card.min.x + 18.0, card.min.y + 62.0),
            Align2::LEFT_CENTER,
            truncate(&stage, 110),
            FontId::proportional(12.0),
            MUTED,
        );
        if progress < 1.0 {
            ui.painter().text(
                Pos2::new(card.max.x - 18.0, card.min.y + 62.0),
                Align2::RIGHT_CENTER,
                "Descargando…",
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }

    fn paint_logs(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, CornerRadius::same(12), CARD);
        let label = match &self.task {
            Some(Task { kind, .. }) => match kind {
                TaskKind::Releases => "Consultando versiones…".to_owned(),
                TaskKind::Install(tag) => format!("Instalando {tag}…"),
                TaskKind::Prepare(tag) => format!("Preparando {tag}…"),
                TaskKind::Play(name) => format!("Horizon abierto en {name}"),
            },
            None => self.status.clone(),
        };
        ui.painter().text(
            Pos2::new(rect.min.x + 14.0, rect.min.y + 16.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(13.0),
            if self.busy() { YELLOW } else { MUTED },
        );

        if let Some(progress) = self.progress {
            let bar = Rect::from_min_size(
                Pos2::new(rect.min.x + 14.0, rect.min.y + 28.0),
                Vec2::new(rect.width() - 28.0, 5.0),
            );
            ui.painter()
                .rect_filled(bar, CornerRadius::same(3), Color32::from_rgb(45, 41, 47));
            ui.painter().rect_filled(
                Rect::from_min_size(bar.min, Vec2::new(bar.width() * progress.clamp(0.0, 1.0), 5.0)),
                CornerRadius::same(3),
                YELLOW,
            );
        }

        if let Some(note) = &self.bytes_note {
            ui.painter().text(
                Pos2::new(rect.min.x + 14.0, rect.min.y + 42.0),
                Align2::LEFT_CENTER,
                truncate(note, 140),
                FontId::proportional(12.0),
                TEXT,
            );
        }

        let start = rect.min.y
            + if self.progress.is_some() { 40.0 } else { 26.0 }
            + if self.bytes_note.is_some() { 18.0 } else { 0.0 };
        let max_lines = ((rect.max.y - start - 6.0) / 14.0).max(1.0) as usize;
        let logs: Vec<String> = self
            .logs
            .iter()
            .rev()
            .take(max_lines)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        for (index, line) in logs.iter().enumerate() {
            ui.painter().text(
                Pos2::new(rect.min.x + 14.0, start + index as f32 * 14.0),
                Align2::LEFT_TOP,
                truncate(line, 160),
                FontId::monospace(10.5),
                MUTED,
            );
        }
    }
}

fn load_texture(ctx: &egui::Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(name, color, TextureOptions::LINEAR))
}

fn paint_icon(
    painter: &egui::Painter,
    center: Pos2,
    kind: u8,
    color: Color32,
    scale: f32,
) {
    match kind {
        0 => {
            let s = 8.0 * scale;
            painter.add(Shape::line(
                vec![
                    Pos2::new(center.x - s, center.y - s * 0.05),
                    Pos2::new(center.x, center.y - s * 0.9),
                    Pos2::new(center.x + s, center.y - s * 0.05),
                ],
                Stroke::new(1.8_f32, color),
            ));
            painter.line_segment(
                [
                    Pos2::new(center.x - s * 0.7, center.y - s * 0.05),
                    Pos2::new(center.x - s * 0.7, center.y + s * 0.8),
                ],
                Stroke::new(1.6_f32, color),
            );
            painter.line_segment(
                [
                    Pos2::new(center.x + s * 0.7, center.y - s * 0.05),
                    Pos2::new(center.x + s * 0.7, center.y + s * 0.8),
                ],
                Stroke::new(1.6_f32, color),
            );
            painter.line_segment(
                [
                    Pos2::new(center.x - s * 0.7, center.y + s * 0.8),
                    Pos2::new(center.x + s * 0.7, center.y + s * 0.8),
                ],
                Stroke::new(1.6_f32, color),
            );
        }
        1 => {
            let cell = 4.6 * scale;
            let gap = 2.2 * scale;
            for x in [-1.0_f32, 1.0] {
                for y in [-1.0_f32, 1.0] {
                    painter.rect_filled(
                        Rect::from_center_size(
                            Pos2::new(
                                center.x + x * (cell * 0.5 + gap * 0.5),
                                center.y + y * (cell * 0.5 + gap * 0.5),
                            ),
                            Vec2::splat(cell),
                        ),
                        CornerRadius::same(1),
                        color,
                    );
                }
            }
        }
        3 => {
            painter.circle_stroke(center, 8.0 * scale, Stroke::new(1.5_f32, color));
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "i",
                FontId::proportional(12.0 * scale),
                color,
            );
        }
        _ => {}
    }
}

fn summarize(text: &str, limit: usize) -> String {
    let cleaned = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>()
        .join(" ");
    truncate(&cleaned, limit)
}

/// Noticia.md entries: `## Title`, an optional `_date_` line, then body text
/// until the next heading.
fn parse_news(text: &str) -> Vec<NewsItem> {
    let mut items = Vec::new();
    let mut title: Option<String> = None;
    let mut date = String::new();
    let mut body: Vec<String> = Vec::new();

    let flush = |items: &mut Vec<NewsItem>,
                 title: &mut Option<String>,
                 date: &mut String,
                 body: &mut Vec<String>| {
        if let Some(title) = title.take() {
            items.push(NewsItem {
                title,
                date: std::mem::take(date),
                body: summarize(&body.join(" "), 400),
            });
        }
        body.clear();
    };

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("## ") {
            flush(&mut items, &mut title, &mut date, &mut body);
            title = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("_") {
            if let Some(rest) = rest.strip_suffix("_") {
                date = rest.trim().to_owned();
            }
        } else if title.is_some() && !line.is_empty() && !line.starts_with("---") {
            body.push(line.to_owned());
        }
    }
    flush(&mut items, &mut title, &mut date, &mut body);
    items
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut shortened: String = text.chars().take(limit.saturating_sub(1)).collect();
    shortened.push('…');
    shortened
}

fn open_path(path: &Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

pub fn run(paths: AppPaths) -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([1040.0, 680.0])
            .with_decorations(false)
            .with_title("RustCraft"),
        ..Default::default()
    };

    eframe::run_native(
        "RustCraft",
        options,
        Box::new(move |cc| Ok(Box::new(RustCraftApp::new(cc, paths)))),
    )
}
