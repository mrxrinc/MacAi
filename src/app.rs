use crate::engine::{self, Event, Generation, TitleTask};
use crate::model::{now, Appearance, History, Message, MessageStatus, ModelChoice, Role};
use crate::setup::{CloudStatus, LicenseDocument, Readiness, SetupTask};
use crate::storage::Store;
use eframe::egui::{
    self, Align, Color32, FontId, Frame, Id, Key, Layout, Margin, RichText, Stroke, Vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

/// Every color the app paints. Light keeps MacAi's original look; dark mirrors it with the
/// same calm blue accent on deep slate surfaces.
struct Palette {
    background: Color32,
    sidebar: Color32,
    surface: Color32,
    ink: Color32,
    muted: Color32,
    faint: Color32,
    accent: Color32,
    accent_text: Color32,
    // Filled controls use a deeper shade of the accent so white text stays readable.
    accent_fill: Color32,
    accent_fill_disabled: Color32,
    on_accent: Color32,
    line: Color32,
    error: Color32,
    row_selected: Color32,
    hover: Color32,
    user_bubble: Color32,
    card: Color32,
    info: Color32,
    warning: Color32,
    error_fill: Color32,
    selection: Color32,
    widget: Color32,
    widget_hover: Color32,
    widget_active: Color32,
    code: Color32,
}

const LIGHT: Palette = Palette {
    background: Color32::from_rgb(247, 249, 251),
    sidebar: Color32::from_rgb(235, 241, 246),
    surface: Color32::WHITE,
    ink: Color32::from_rgb(32, 46, 58),
    muted: Color32::from_rgb(98, 115, 128),
    faint: Color32::from_rgb(150, 163, 173),
    accent: Color32::from_rgb(93, 145, 181),
    accent_text: Color32::from_rgb(49, 87, 113),
    accent_fill: Color32::from_rgb(72, 122, 158),
    accent_fill_disabled: Color32::from_rgb(173, 194, 209),
    on_accent: Color32::WHITE,
    line: Color32::from_rgb(216, 226, 234),
    error: Color32::from_rgb(160, 62, 46),
    row_selected: Color32::from_rgb(216, 231, 242),
    hover: Color32::from_rgb(228, 237, 244),
    user_bubble: Color32::from_rgb(225, 237, 246),
    card: Color32::from_rgb(239, 244, 248),
    info: Color32::from_rgb(232, 240, 247),
    warning: Color32::from_rgb(249, 237, 220),
    error_fill: Color32::from_rgb(251, 231, 226),
    selection: Color32::from_rgb(199, 223, 241),
    widget: Color32::from_rgb(228, 237, 244),
    widget_hover: Color32::from_rgb(211, 228, 240),
    widget_active: Color32::from_rgb(193, 217, 234),
    code: Color32::from_rgb(234, 240, 245),
};

const DARK: Palette = Palette {
    background: Color32::from_rgb(23, 27, 32),
    sidebar: Color32::from_rgb(30, 36, 43),
    surface: Color32::from_rgb(34, 41, 49),
    ink: Color32::from_rgb(226, 233, 239),
    muted: Color32::from_rgb(150, 165, 178),
    faint: Color32::from_rgb(104, 118, 130),
    accent: Color32::from_rgb(112, 166, 204),
    accent_text: Color32::from_rgb(142, 192, 228),
    accent_fill: Color32::from_rgb(58, 108, 145),
    accent_fill_disabled: Color32::from_rgb(48, 62, 75),
    on_accent: Color32::WHITE,
    line: Color32::from_rgb(52, 63, 74),
    error: Color32::from_rgb(242, 139, 120),
    row_selected: Color32::from_rgb(41, 59, 76),
    hover: Color32::from_rgb(44, 54, 65),
    user_bubble: Color32::from_rgb(38, 57, 74),
    card: Color32::from_rgb(34, 41, 49),
    info: Color32::from_rgb(31, 45, 58),
    warning: Color32::from_rgb(62, 49, 30),
    error_fill: Color32::from_rgb(68, 36, 32),
    selection: Color32::from_rgb(52, 86, 114),
    widget: Color32::from_rgb(40, 49, 58),
    widget_hover: Color32::from_rgb(50, 65, 80),
    widget_active: Color32::from_rgb(60, 82, 102),
    code: Color32::from_rgb(31, 37, 44),
};

thread_local! {
    // Set at the start of every frame from egui's resolved theme.
    static DARK_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn pal() -> &'static Palette {
    if DARK_MODE.with(std::cell::Cell::get) {
        &DARK
    } else {
        &LIGHT
    }
}

const CONTENT_WIDTH: f32 = 720.0;
const BOLD_FONT: &str = "Ubuntu-Bold";

struct Active {
    chat_id: String,
    generation: Generation,
    stopping: bool,
}

enum ModelState {
    Checking,
    LicenseRequired,
    Ready,
    Unavailable(String),
}

struct LicensePrompt {
    document: LicenseDocument,
    agreed: bool,
    open: bool,
    accepting: bool,
    error: Option<String>,
}

impl LicensePrompt {
    fn can_accept(&self) -> bool {
        self.open && self.agreed && !self.accepting
    }
}

pub struct MacAi {
    store: Option<Store>,
    history: History,
    fatal: Option<String>,
    save_error: Option<String>,
    dirty: bool,
    last_save: Instant,
    active: Option<Active>,
    model: ModelState,
    // None until the availability check finishes.
    cloud: Option<CloudStatus>,
    // Read by the Apple Cloud banner, which is hidden while Apple keeps PCC Terminal-only.
    #[allow(dead_code)]
    cloud_notice: bool,
    titles: Vec<TitleTask>,
    // The `fm` executable; tests point this at a fake.
    fm: std::path::PathBuf,
    // Chats whose first exchange just finished and still need a summary title.
    title_queue: Vec<String>,
    setup_task: Option<SetupTask>,
    license: Option<LicensePrompt>,
    markdown: CommonMarkCache,
    search: String,
    sidebar_open: bool,
    focus_composer: bool,
    rename: Option<(String, String)>,
    delete: Option<String>,
    settings_open: bool,
    close_confirmation: bool,
    allow_close: bool,
    ime_active: bool,
    notice: Option<(String, Instant)>,
    #[cfg(feature = "screenshot")]
    capture_requested: bool,
}

impl MacAi {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        setup_style(&cc.egui_ctx);
        let opened = Store::default_directory().and_then(Store::open);
        let mut app = Self::from_store(opened);
        apply_appearance(&cc.egui_ctx, app.history.appearance);
        crate::menu::install(&cc.egui_ctx);
        #[cfg(feature = "screenshot")]
        if std::env::var_os("MACAI_PREVIEW_LICENSE").is_some() {
            app.finish_setup(crate::setup::preview_license());
            return app;
        }
        if app.fatal.is_none() {
            app.check_model(&cc.egui_ctx);
        }
        app
    }

    fn from_store(opened: Result<(Store, History), String>) -> Self {
        let (store, history, fatal) = match opened {
            Ok((store, history)) => (Some(store), history, None),
            Err(error) => (None, History::default(), Some(error)),
        };
        Self {
            store,
            history,
            fatal,
            save_error: None,
            dirty: false,
            last_save: Instant::now(),
            active: None,
            model: ModelState::Checking,
            cloud: None,
            cloud_notice: false,
            titles: Vec::new(),
            fm: engine::FM.into(),
            title_queue: Vec::new(),
            setup_task: None,
            license: None,
            markdown: CommonMarkCache::default(),
            search: String::new(),
            sidebar_open: true,
            focus_composer: true,
            rename: None,
            delete: None,
            settings_open: false,
            close_confirmation: false,
            allow_close: false,
            ime_active: false,
            notice: None,
            #[cfg(feature = "screenshot")]
            capture_requested: false,
        }
    }

    fn check_model(&mut self, ctx: &egui::Context) {
        self.setup_task = None;
        let context = ctx.clone();
        self.license = None;
        self.model = ModelState::Checking;
        self.setup_task = Some(SetupTask::check(move || {
            context.request_repaint();
        }));
    }

    fn accept_license(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.license else {
            return;
        };
        if !prompt.can_accept() || self.setup_task.is_some() {
            return;
        }
        #[cfg(feature = "screenshot")]
        if std::env::var_os("MACAI_PREVIEW_LICENSE").is_some() {
            prompt.error =
                Some("This setup preview is read-only and cannot accept a license.".into());
            return;
        }
        prompt.accepting = true;
        prompt.error = None;
        let document = prompt.document.clone();
        let context = ctx.clone();
        self.setup_task = Some(SetupTask::accept(document, move || {
            context.request_repaint()
        }));
    }

    fn finish_setup(&mut self, result: Result<Readiness, String>) {
        self.setup_task = None;
        match result {
            Ok(Readiness::Ready(cloud)) => {
                self.model = ModelState::Ready;
                if cloud == CloudStatus::Available {
                    self.cloud_notice = false;
                } else {
                    self.fall_back_to_on_device();
                }
                self.cloud = Some(cloud);
                self.license = None;
                self.focus_composer = true;
            }
            Ok(Readiness::LicenseRequired(document)) => {
                let changed = self.license.as_ref().is_some_and(|prompt| prompt.accepting);
                self.model = ModelState::LicenseRequired;
                self.license = Some(LicensePrompt {
                    document,
                    agreed: false,
                    open: true,
                    accepting: false,
                    error: changed.then(|| {
                        "Apple's terms changed. Please review this version before accepting.".into()
                    }),
                });
                self.focus_composer = false;
            }
            Ok(Readiness::ModelUnavailable(error)) => {
                self.license = None;
                self.model = ModelState::Unavailable(error);
            }
            Err(error) => {
                if let Some(prompt) = &mut self.license {
                    prompt.accepting = false;
                    prompt.agreed = false;
                    prompt.error = Some(error);
                    self.model = ModelState::LicenseRequired;
                } else {
                    self.model = ModelState::Unavailable(error);
                }
            }
        }
    }

    fn save(&mut self) -> bool {
        self.last_save = Instant::now();
        let Some(store) = &self.store else {
            return false;
        };
        match store.save(&self.history) {
            Ok(()) => {
                self.dirty = false;
                self.save_error = None;
                true
            }
            Err(error) => {
                eprintln!("MacAi: {error}");
                self.save_error = Some(error);
                self.dirty = true;
                false
            }
        }
    }

    fn new_chat(&mut self) {
        self.history.new_chat();
        self.search.clear();
        self.dirty = true;
        self.focus_composer = true;
        self.save();
    }

    fn cloud_unavailable_reason(&self) -> Option<&str> {
        match &self.cloud {
            Some(CloudStatus::Unavailable(reason)) => Some(reason),
            _ => None,
        }
    }

    fn selected_model_usable(&self) -> bool {
        self.history.selected().model == ModelChoice::OnDevice
            || self.cloud_unavailable_reason().is_none()
    }

    /// Apple Cloud's controls are hidden while it is unavailable, so nothing may stay
    /// selected on it: new and unstarted chats return to the on-device model.
    fn fall_back_to_on_device(&mut self) {
        let mut changed = self.history.preferred_model != ModelChoice::OnDevice;
        self.history.preferred_model = ModelChoice::OnDevice;
        for chat in &mut self.history.chats {
            if chat.model == ModelChoice::Cloud && !chat.is_started() {
                chat.model = ModelChoice::OnDevice;
                changed = true;
            }
        }
        if changed {
            self.dirty = true;
        }
    }

    fn set_appearance(&mut self, ctx: &egui::Context, appearance: Appearance) {
        self.history.appearance = appearance;
        apply_appearance(ctx, appearance);
        self.dirty = true;
        self.save();
    }

    fn choose_model(&mut self, model: ModelChoice) {
        if model == ModelChoice::Cloud && self.cloud_unavailable_reason().is_some() {
            self.cloud_notice = true;
            return;
        }
        self.cloud_notice = false;
        if self.history.selected().model == model && self.history.preferred_model == model {
            return;
        }
        self.history.switch_model(model);
        self.dirty = true;
        self.focus_composer = true;
        self.save();
    }

    fn start_message(&mut self, ctx: &egui::Context, retry: bool) {
        self.send(ctx, if retry { Turn::Retry } else { Turn::Draft });
    }

    /// Opens a new conversation and asks the latest (repeated) question there.
    fn ask_in_new_chat(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.history.selected().repeated_prompt() else {
            return;
        };
        if self.active.is_some() {
            return;
        }
        self.history.new_chat();
        self.history.selected_mut().draft = prompt;
        self.search.clear();
        self.send(ctx, Turn::Draft);
    }

    fn send(&mut self, ctx: &egui::Context, turn: Turn) {
        if self.active.is_some()
            || self.store.is_none()
            || self.save_error.is_some()
            || self.rename.is_some()
            || self.delete.is_some()
            || self.settings_open
            || self.close_confirmation
            || !matches!(self.model, ModelState::Ready)
        {
            return;
        }
        if !self.selected_model_usable() {
            // An older Apple Cloud chat can't continue, so carry on in a new on-device chat.
            self.choose_model(ModelChoice::OnDevice);
        }
        let chat = self.history.selected_mut();
        let original = chat.clone();
        let prompt = match turn {
            Turn::Draft => {
                let prompt = chat.draft.trim().to_owned();
                if prompt.is_empty() {
                    return;
                }
                prompt
            }
            Turn::Retry | Turn::Fresh => {
                let prompt = if turn == Turn::Retry {
                    chat.retry_prompt()
                } else {
                    chat.repeated_prompt()
                };
                let Some(prompt) = prompt else {
                    return;
                };
                chat.messages.truncate(chat.messages.len() - 2);
                prompt
            }
        };
        // A fresh answer starts the model's context over from this question; the earlier
        // messages stay visible but are no longer sent.
        let transcript = if turn == Turn::Fresh {
            None
        } else {
            chat.transcript.clone()
        };
        let model = chat.model;
        let chat_id = chat.id.clone();
        let draft = chat.draft.clone();
        chat.begin(prompt.clone());
        if turn != Turn::Draft {
            chat.draft = draft;
        }
        self.dirty = true;
        // Do not start inference until the user's message is safely on disk.
        if !self.save() {
            *self.history.selected_mut() = original;
            return;
        }
        if let Some(store) = &self.store {
            let context = ctx.clone();
            self.active = Some(Active {
                chat_id,
                stopping: false,
                generation: Generation::start_with_program(
                    self.fm.clone(),
                    model,
                    prompt,
                    transcript,
                    store.directory().to_owned(),
                    std::sync::Arc::new(move || context.request_repaint()),
                ),
            });
        }
        self.focus_composer = true;
    }

    fn start_titles(&mut self, ctx: &egui::Context) {
        for chat_id in std::mem::take(&mut self.title_queue) {
            if self.titles.iter().any(|task| task.chat_id == chat_id) {
                continue;
            }
            let Some((prompt, reply)) = self
                .history
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .and_then(|chat| chat.title_context())
            else {
                continue;
            };
            let context = ctx.clone();
            self.titles
                .push(TitleTask::start(chat_id, prompt, reply, move || {
                    context.request_repaint()
                }));
        }
    }

    fn receive_titles(&mut self) {
        let mut finished = Vec::new();
        self.titles.retain(|task| match task.events.try_recv() {
            Ok(result) => {
                finished.push((task.chat_id.clone(), result));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        for (chat_id, result) in finished {
            match result {
                Ok(title) => {
                    if let Some(chat) = self.history.chats.iter_mut().find(|c| c.id == chat_id) {
                        if chat.apply_summary_title(title) {
                            self.dirty = true;
                        }
                    }
                }
                // The first-prompt title stays; a later reply can try again.
                Err(error) => eprintln!("MacAi: could not summarize a chat title: {error}"),
            }
        }
    }

    fn receive(&mut self) {
        if let Some(result) = self.setup_task.as_ref().map(|task| task.events.try_recv()) {
            match result {
                Ok(result) => self.finish_setup(result),
                Err(TryRecvError::Disconnected) => {
                    self.finish_setup(Err(
                        "The setup check stopped unexpectedly. Please retry.".into()
                    ));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        self.receive_titles();
        let Some(active) = &self.active else {
            return;
        };
        let chat_id = active.chat_id.clone();
        let mut events = Vec::new();
        let mut done = false;
        loop {
            match active.generation.events.try_recv() {
                Ok(event) => {
                    done = !matches!(event, Event::Chunk(_));
                    events.push(event);
                    if done {
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    events.push(Event::Failed(
                        "The model worker stopped unexpectedly.".into(),
                    ));
                    done = true;
                    break;
                }
            }
        }
        if let Some(chat) = self
            .history
            .chats
            .iter_mut()
            .find(|chat| chat.id == chat_id)
        {
            for event in events {
                self.dirty = true;
                match event {
                    Event::Chunk(text) => {
                        if let Some(message) = chat.messages.last_mut() {
                            message.text.push_str(&text);
                        }
                    }
                    Event::Finished { text, transcript } => {
                        chat.finish(text, transcript);
                        if chat.title_context().is_some() {
                            self.title_queue.push(chat.id.clone());
                        }
                    }
                    Event::Failed(error) => {
                        if engine::is_cloud_context_error(&error) {
                            self.cloud = Some(CloudStatus::Unavailable(
                                "Private Cloud Compute is not available in this context. Please use the Terminal app.".into(),
                            ));
                        }
                        if engine::is_license_error(&error) {
                            self.model = ModelState::Unavailable(
                                "Apple's terms need attention. Choose Check again to review them in MacAi.".into(),
                            );
                        }
                        chat.interrupt(MessageStatus::Failed, error);
                    }
                    Event::Cancelled => chat.interrupt(
                        MessageStatus::Interrupted,
                        "Reply stopped. This unfinished turn is not included in the model's context.".into(),
                    ),
                }
            }
        }
        if done {
            self.active = None;
            self.save();
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        if !self.sidebar_open {
            return;
        }
        egui::SidePanel::left("history")
            .exact_width(244.0)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(pal().sidebar)
                    .inner_margin(Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    brand(ui, 30.0);
                    ui.add_space(3.0);
                    ui.label(RichText::new("MacAi").size(23.0).strong().color(pal().ink));
                });
                ui.add_space(24.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 39.0],
                        egui::Button::new(
                            RichText::new("+  New conversation")
                                .color(pal().on_accent)
                                .font(FontId::new(13.0, egui::FontFamily::Name(BOLD_FONT.into()))),
                        )
                        .fill(pal().accent_fill)
                        .corner_radius(9),
                    )
                    .on_hover_text("New conversation (Command+N)")
                    .clicked()
                {
                    self.new_chat();
                }
                ui.add_space(15.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Search conversations")
                        .desired_width(f32::INFINITY)
                        .margin(Vec2::new(10.0, 9.0)),
                );
                ui.add_space(21.0);
                ui.label(
                    RichText::new("YOUR CONVERSATIONS")
                        .size(10.0)
                        .strong()
                        .color(pal().muted),
                );
                ui.add_space(9.0);

                let filter = self.search.to_lowercase();
                let mut chats: Vec<_> = self
                    .history
                    .chats
                    .iter()
                    .filter(|chat| {
                        filter.is_empty()
                            || chat.title.to_lowercase().contains(&filter)
                            || chat
                                .messages
                                .iter()
                                .any(|message| message.text.to_lowercase().contains(&filter))
                    })
                    .map(|chat| (chat.id.clone(), chat.title.clone(), chat.updated_at))
                    .collect();
                chats.sort_by(|left, right| right.2.cmp(&left.2));

                let list_height = (ui.available_height() - 86.0).max(80.0);
                egui::ScrollArea::vertical()
                    .id_salt("chat-list")
                    .max_height(list_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if chats.is_empty() {
                            ui.label(
                                RichText::new("No conversations found.")
                                    .color(pal().muted)
                                    .size(12.0),
                            );
                        }
                        for (id, title, timestamp) in chats {
                            let selected = id == self.history.selected;
                            let busy = self
                                .active
                                .as_ref()
                                .is_some_and(|active| active.chat_id == id);
                            let response = Frame::new()
                                .fill(if selected {
                                    pal().row_selected
                                } else {
                                    pal().sidebar
                                })
                                .corner_radius(8)
                                .inner_margin(Margin::symmetric(10, 10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.scope(|ui| {
                                        ui.set_max_width(ui.available_width() - ROW_MENU_SPACE);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&title).size(13.0).color(pal().ink),
                                            )
                                            .truncate(),
                                        );
                                    });
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(if busy {
                                            "Writing a reply".into()
                                        } else {
                                            relative_time(timestamp)
                                        })
                                        .size(10.0)
                                        .color(pal().muted),
                                    );
                                })
                                .response;
                            let click = ui.interact(
                                response.rect,
                                Id::new(("chat", &id)),
                                egui::Sense::click(),
                            );
                            let menu_id = Id::new(("chat-menu", &id));
                            let menu_open = ui.memory(|memory| memory.is_popup_open(menu_id));
                            if selected || menu_open || ui.rect_contains_pointer(response.rect) {
                                match chat_row_menu(ui, response.rect, menu_id, busy) {
                                    Some(RowAction::Rename) => {
                                        self.rename = Some((id.clone(), title.clone()));
                                    }
                                    Some(RowAction::Delete) => self.delete = Some(id.clone()),
                                    None => {}
                                }
                            }
                            if click.clicked() {
                                self.history.selected = id;
                                self.dirty = true;
                                self.focus_composer = true;
                            }
                            ui.add_space(4.0);
                        }
                    });
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.add_space(3.0);
                    ui.label(RichText::new("mrxrinc").size(10.0).color(pal().muted));
                    ui.label(
                        RichText::new("Runs privately on your Mac.")
                            .size(10.0)
                            .color(pal().muted),
                    );
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        status_dot(ui, pal().accent);
                        ui.label(
                            RichText::new("No account. No API key.")
                                .size(12.0)
                                .color(pal().ink),
                        );
                    });
                    ui.add_space(12.0);
                    ui.separator();
                });
            });
    }

    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .frame(
                Frame::new()
                    .fill(pal().background)
                    .inner_margin(Margin::symmetric(25, 16)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if sidebar_toggle(ui, self.sidebar_open).clicked() {
                        self.sidebar_open = !self.sidebar_open;
                    }
                    ui.add_space(12.0);
                    let title = &self.history.selected().title;
                    ui.scope(|ui| {
                        ui.set_max_width((ui.available_width() - 150.0).max(40.0));
                        ui.add(
                            egui::Label::new(RichText::new(title).size(14.0).strong()).truncate(),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.menu_button("...", |ui| {
                            if ui.button("Rename conversation").clicked() {
                                let chat = self.history.selected();
                                self.rename = Some((chat.id.clone(), chat.title.clone()));
                                ui.close_menu();
                            }
                            let active_here = self
                                .active
                                .as_ref()
                                .is_some_and(|active| active.chat_id == self.history.selected);
                            if ui
                                .add_enabled(!active_here, egui::Button::new("Delete conversation"))
                                .clicked()
                            {
                                self.delete = Some(self.history.selected.clone());
                                ui.close_menu();
                            }
                        });
                        let selected_model = self.history.selected().model;
                        let usable = self.selected_model_usable();
                        let (text, color) = match self.model {
                            ModelState::Ready if usable => {
                                (selected_model.label(), pal().accent_text)
                            }
                            ModelState::Ready => ("Apple Cloud unavailable", pal().error),
                            ModelState::Checking => ("Checking model", pal().muted),
                            ModelState::LicenseRequired => ("Setup needed", pal().accent_text),
                            ModelState::Unavailable(_) => ("Model unavailable", pal().error),
                        };
                        ui.label(RichText::new(text).size(11.0).color(color));
                        status_dot(ui, color);
                    });
                });
            });
    }

    fn banners(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if matches!(self.model, ModelState::LicenseRequired) {
            Frame::new()
                .fill(pal().sidebar)
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.label(RichText::new("Review Apple's terms to enable local chat. Your saved conversations are still available.").size(12.0));
                    if ui.small_button("Review terms").clicked() {
                        if let Some(prompt) = &mut self.license {
                            prompt.open = true;
                            prompt.agreed = false;
                        }
                    }
                });
            ui.add_space(8.0);
        }
        if let ModelState::Unavailable(error) = &self.model {
            let error = error.clone();
            Frame::new()
                .fill(pal().warning)
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.label(RichText::new(error).size(12.0).color(pal().error));
                    if ui.small_button("Check again").clicked() {
                        self.check_model(ctx);
                    }
                });
            ui.add_space(8.0);
        }
        // Apple Cloud banner hidden along with its switch: macOS only allows Private Cloud
        // Compute from Terminal, not third-party apps. Uncomment to show it again once Apple
        // opens PCC to apps like MacAi.
        // let cloud_reason = self.cloud_unavailable_reason().map(str::to_owned);
        // let cloud_chat = self.history.selected().model == ModelChoice::Cloud;
        // if let Some(reason) = cloud_reason.filter(|_| {
        //     matches!(self.model, ModelState::Ready) && (self.cloud_notice || cloud_chat)
        // }) {
        //     Frame::new()
        //         .fill(pal().info)
        //         .stroke(Stroke::new(1.0_f32, pal().line))
        //         .corner_radius(10)
        //         .inner_margin(Margin::same(12))
        //         .show(ui, |ui| {
        //             ui.set_width(ui.available_width());
        //             ui.label(
        //                 RichText::new("Apple Cloud isn't available to MacAi right now")
        //                     .size(12.0)
        //                     .strong()
        //                     .color(pal().accent_text),
        //             );
        //             ui.label(
        //                 RichText::new(format!(
        //                     "macOS says: \"{reason}\" Apple currently allows Private Cloud Compute only from apps it trusts for it, such as Terminal. Your on-device model works everywhere."
        //                 ))
        //                 .size(11.0)
        //                 .color(pal().ink),
        //             );
        //             ui.add_space(4.0);
        //             ui.horizontal(|ui| {
        //                 if cloud_chat {
        //                     if ui.small_button("Continue on-device").clicked() {
        //                         self.choose_model(ModelChoice::OnDevice);
        //                     }
        //                 } else if ui.small_button("OK").clicked() {
        //                     self.cloud_notice = false;
        //                 }
        //                 if ui.small_button("Check again").clicked() {
        //                     self.check_model(ctx);
        //                 }
        //             });
        //         });
        //     ui.add_space(8.0);
        // }
        if let Some(error) = self.save_error.clone() {
            Frame::new()
                .fill(pal().error_fill)
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.label(RichText::new(error).size(12.0).color(pal().error));
                    if ui.small_button("Retry saving").clicked() {
                        self.save();
                    }
                });
            ui.add_space(8.0);
        }
    }

    fn composer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("composer")
            .frame(
                Frame::new()
                    .fill(pal().background)
                    .inner_margin(Margin::symmetric(28, 16)),
            )
            .show(ctx, |ui| {
                centered(ui, CONTENT_WIDTH, |ui| {
                    self.banners(ui, ctx);
                    let active_here = self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.chat_id == self.history.selected);
                    if self.active.is_some() && !active_here {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("A reply is being written in another conversation.")
                                    .size(11.0)
                                    .color(pal().muted),
                            );
                            if ui.small_button("View").clicked() {
                                if let Some(active) = &self.active {
                                    self.history.selected = active.chat_id.clone();
                                    self.dirty = true;
                                }
                            }
                        });
                        ui.add_space(7.0);
                    }
                    let mut send = false;
                    let mut switch = None;
                    Frame::new()
                        .fill(pal().surface)
                        .stroke(Stroke::new(1.0, pal().line))
                        .corner_radius(14)
                        .inner_margin(Margin::symmetric(16, 13))
                        .show(ui, |ui| {
                            let input_id = Id::new(("composer-input", &self.history.selected));
                            let draft = &mut self.history.selected_mut().draft;
                            let input = egui::ScrollArea::vertical()
                                .max_height(140.0)
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(draft)
                                            .id(input_id)
                                            .font(FontId::proportional(15.0))
                                            .hint_text("Ask anything, or think out loud...")
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(2)
                                            .frame(false)
                                            .return_key(Some(egui::KeyboardShortcut::new(
                                                egui::Modifiers::SHIFT,
                                                Key::Enter,
                                            ))),
                                    )
                                })
                                .inner;
                            if self.focus_composer {
                                input.request_focus();
                                self.focus_composer = false;
                            }
                            if input.changed() {
                                self.dirty = true;
                            }
                            let ready = self.active.is_none()
                                && matches!(self.model, ModelState::Ready)
                                && self.selected_model_usable()
                                && self.save_error.is_none()
                                && !self.history.selected().draft.trim().is_empty();
                            if input.has_focus()
                                && !self.ime_active
                                && ui.input_mut(|input| {
                                    input.consume_key(egui::Modifiers::NONE, Key::Enter)
                                })
                            {
                                send = ready;
                            }
                            ui.add_space(7.0);
                            ui.horizontal(|ui| {
                                if let Some(model) = model_switch(
                                    ui,
                                    self.history.selected().model,
                                    self.cloud.as_ref(),
                                ) {
                                    switch = Some(model);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if active_here {
                                        if let Some(active) = &mut self.active {
                                            if ui
                                                .add_enabled(
                                                    !active.stopping,
                                                    egui::Button::new(if active.stopping {
                                                        "Stopping..."
                                                    } else {
                                                        "Stop reply"
                                                    })
                                                    .corner_radius(8),
                                                )
                                                .clicked()
                                            {
                                                active.stopping = true;
                                                active.generation.cancel();
                                            }
                                        }
                                    } else if ui
                                        .add_enabled(
                                            ready,
                                            egui::Button::new(
                                                RichText::new("Send").color(pal().on_accent),
                                            )
                                            .fill(if ready {
                                                pal().accent_fill
                                            } else {
                                                pal().accent_fill_disabled
                                            })
                                            .min_size(Vec2::new(67.0, 30.0))
                                            .corner_radius(8),
                                        )
                                        .clicked()
                                    {
                                        send = true;
                                    }
                                });
                            });
                        });
                    ui.add_space(9.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Enter to send  /  Shift+Enter for a new line")
                                .size(10.0)
                                .color(pal().muted),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new("AI can make mistakes.")
                                    .size(10.0)
                                    .color(pal().muted),
                            );
                        });
                    });
                    if let Some(model) = switch {
                        self.choose_model(model);
                    }
                    if send {
                        self.start_message(ctx, false);
                    }
                });
            });
    }

    fn conversation(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(pal().background)
                    .inner_margin(Margin::symmetric(28, 0)),
            )
            .show(ctx, |ui| {
                if self.history.selected().messages.is_empty() {
                    self.empty_state(ui);
                    return;
                }
                let mut retry = false;
                let mut copied = false;
                let mut repeat_action = None;
                egui::ScrollArea::vertical()
                    .id_salt(("messages", &self.history.selected))
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        centered(ui, CONTENT_WIDTH, |ui| {
                            ui.add_space(22.0);
                            let chat = self.history.selected();
                            let model = chat.model;
                            let count = chat.messages.len();
                            for (index, message) in chat.messages.iter().enumerate() {
                                ui.push_id((&chat.id, index), |ui| {
                                    let action = message_view(
                                        ui,
                                        message,
                                        model,
                                        &mut self.markdown,
                                        index + 1 == count && self.active.is_none(),
                                    );
                                    retry |= action.0;
                                    copied |= action.1;
                                });
                                ui.add_space(25.0);
                            }
                            if self.active.is_none() && chat.repeated_prompt().is_some() {
                                repeat_action = repeat_notice(ui);
                                ui.add_space(25.0);
                            }
                            ui.add_space(8.0);
                        });
                    });
                if copied {
                    self.notice = Some(("Copied to clipboard".into(), Instant::now()));
                }
                if retry {
                    self.start_message(ctx, true);
                }
                match repeat_action {
                    Some(Turn::Fresh) => self.send(ctx, Turn::Fresh),
                    Some(_) => self.ask_in_new_chat(ctx),
                    None => {}
                }
            });
    }

    fn empty_state(&mut self, ui: &mut egui::Ui) {
        let top_space = (ui.available_height() * 0.20).max(20.0);
        egui::ScrollArea::vertical()
            .id_salt("welcome")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(top_space);
                centered(ui, 470.0, |ui| {
                    ui.vertical_centered(|ui| {
                        brand(ui, 56.0);
                        ui.add_space(22.0);
                        ui.label(
                            RichText::new("A little space to think.")
                                .size(30.0)
                                .color(pal().ink),
                        );
                        ui.add_space(11.0);
                        ui.label(
                            RichText::new("Your ideas. Your conversations. Your Mac.")
                                .size(14.0)
                                .color(pal().muted),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            RichText::new(match self.history.selected().model {
                                ModelChoice::OnDevice => "Running on your Mac's on-device model.",
                                ModelChoice::Cloud => {
                                    "Using Apple's Private Cloud Compute for bigger questions."
                                }
                            })
                            .size(12.0)
                            .color(pal().muted),
                        );
                        ui.add_space(30.0);
                        for (title, subtitle, prompt) in [
                            (
                                "Find the right words",
                                "A first draft, a fresh start",
                                "Help me write a short, thoughtful message about ",
                            ),
                            (
                                "Make something clearer",
                                "Untangle an idea or a piece of text",
                                "Explain this in simple terms: ",
                            ),
                            (
                                "Explore an idea",
                                "A second perspective, without the noise",
                                "Help me brainstorm ideas for ",
                            ),
                        ] {
                            let button = egui::Button::new(
                                RichText::new(format!("{title}   /   {subtitle}"))
                                    .size(12.0)
                                    .color(pal().ink),
                            )
                            .fill(pal().card)
                            .stroke(Stroke::new(1.0, pal().line))
                            .corner_radius(9);
                            if ui
                                .add_sized([ui.available_width().min(440.0), 43.0], button)
                                .clicked()
                            {
                                self.history.selected_mut().draft = prompt.into();
                                self.dirty = true;
                                self.focus_composer = true;
                            }
                            ui.add_space(8.0);
                        }
                    });
                });
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        self.license_dialog(ctx);
        if let Some((id, mut title)) = self.rename.clone() {
            let mut action = None;
            egui::Modal::new(Id::new("rename-dialog")).show(ctx, |ui| {
                ui.heading("Rename conversation");
                ui.add_space(12.0);
                ui.add(egui::TextEdit::singleline(&mut title).desired_width(300.0));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!title.trim().is_empty(), egui::Button::new("Save"))
                        .clicked()
                    {
                        action = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        action = Some(false);
                    }
                });
            });
            self.rename = Some((id.clone(), title.clone()));
            if let Some(save) = action {
                if save {
                    if let Some(chat) = self.history.chats.iter_mut().find(|chat| chat.id == id) {
                        chat.rename(title.trim().to_owned());
                        self.dirty = true;
                        self.save();
                    }
                }
                self.rename = None;
            }
        }
        if let Some(id) = self.delete.clone() {
            let mut action = None;
            egui::Modal::new(Id::new("delete-dialog")).show(ctx, |ui| {
                ui.heading("Delete this conversation?");
                ui.add_space(12.0);
                ui.label("The messages and model context will be removed from this Mac.");
                ui.label("This cannot be undone.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        action = Some(false);
                    }
                    if ui
                        .button(RichText::new("Delete conversation").color(pal().error))
                        .clicked()
                    {
                        action = Some(true);
                    }
                });
            });
            if let Some(delete) = action {
                if delete {
                    self.titles.retain(|task| task.chat_id != id);
                    self.history.delete(&id);
                    self.dirty = true;
                    self.save();
                    self.focus_composer = true;
                }
                self.delete = None;
            }
        }
        if self.settings_open {
            self.settings_dialog(ctx);
        }
        if self.close_confirmation {
            egui::Modal::new(Id::new("unsaved-dialog")).show(ctx, |ui| {
                ui.heading("Your latest changes are not saved");
                ui.add_space(12.0);
                ui.label("Keep MacAi open to retry saving, or quit without these changes.");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Keep open").clicked() {
                        self.close_confirmation = false;
                    }
                    if ui.button("Quit without saving").clicked() {
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
        }
    }

    fn handle_menu_requests(&mut self) {
        if crate::menu::take_settings_request() {
            self.settings_open = true;
        }
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let mut chosen = None;
        let mut done = false;
        let modal = egui::Modal::new(Id::new("settings-dialog"))
            .frame(
                Frame::popup(&ctx.style())
                    .fill(pal().surface)
                    .corner_radius(12)
                    .inner_margin(Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.set_width(3.0 * APPEARANCE_CARD.x + 2.0 * 12.0);
                ui.label(
                    RichText::new("Settings")
                        .size(18.0)
                        .strong()
                        .color(pal().ink),
                );
                ui.add_space(18.0);
                ui.label(
                    RichText::new("APPEARANCE")
                        .size(10.0)
                        .strong()
                        .color(pal().muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    for appearance in Appearance::ALL {
                        if appearance_card(ui, appearance, self.history.appearance == appearance)
                            .clicked()
                        {
                            chosen = Some(appearance);
                        }
                    }
                });
                ui.add_space(10.0);
                ui.label(
                    RichText::new(match self.history.appearance {
                        Appearance::System => "MacAi follows your Mac's light or dark appearance.",
                        Appearance::Light => "MacAi always uses the light appearance.",
                        Appearance::Dark => "MacAi always uses the dark appearance.",
                    })
                    .size(12.0)
                    .color(pal().muted),
                );
                ui.add_space(20.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Button::new(RichText::new("Done").color(pal().on_accent))
                                .fill(pal().accent_fill)
                                .corner_radius(7)
                                .min_size(Vec2::new(72.0, 30.0)),
                        )
                        .clicked()
                    {
                        done = true;
                    }
                });
            });
        if let Some(appearance) = chosen {
            if appearance != self.history.appearance {
                self.set_appearance(ctx, appearance);
            }
        }
        if done || modal.should_close() {
            self.settings_open = false;
            self.focus_composer = true;
        }
    }

    fn prepare_close(&mut self) -> bool {
        self.setup_task = None;
        self.titles.clear();
        if let Some(active) = self.active.take() {
            let id = active.chat_id.clone();
            drop(active);
            if let Some(chat) = self.history.chats.iter_mut().find(|chat| chat.id == id) {
                chat.interrupt(
                    MessageStatus::Interrupted,
                    "Reply stopped when MacAi closed. You can retry it.".into(),
                );
                self.dirty = true;
            }
        }
        !self.dirty || self.save()
    }

    fn license_dialog(&mut self, ctx: &egui::Context) {
        let Some(prompt) = &mut self.license else {
            return;
        };
        if !prompt.open {
            return;
        }
        let mut accept = false;
        let mut defer = false;
        let read_only =
            cfg!(feature = "screenshot") && std::env::var_os("MACAI_PREVIEW_LICENSE").is_some();
        egui::Modal::new(Id::new("license-dialog")).show(ctx, |ui| {
            ui.set_width((ctx.screen_rect().width() - 80.0).clamp(280.0, 600.0));
            ui.heading("Set up local AI");
            ui.label(RichText::new("Apple Foundation Models").color(pal().accent_text).size(13.0));
            ui.add_space(8.0);
            ui.label(RichText::new("Before your first chat, review Apple's terms below. They are read directly from the fm command installed on this Mac.").size(12.0));
            Frame::new()
                .fill(pal().surface)
                .stroke(Stroke::new(1.0, pal().line))
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("license-text")
                        .max_height((ctx.screen_rect().height() - 350.0).clamp(100.0, 300.0))
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&prompt.document.text).size(12.0)).wrap());
                        });
                });
            if ui.small_button("Copy terms").clicked() {
                ctx.copy_text(prompt.document.text.clone());
            }
            ui.label(RichText::new("This agreement applies to every user on this Mac. After you accept, macOS will ask for administrator approval. MacAi never sees or stores the administrator password.").size(12.0).color(pal().muted));
            ui.add_enabled(
                !prompt.accepting && !read_only,
                egui::Checkbox::new(&mut prompt.agreed, "I have read and agree to these terms for this Mac."),
            );
            if let Some(error) = &prompt.error {
                egui::ScrollArea::vertical().id_salt("license-error").max_height(70.0).show(ui, |ui| {
                    ui.label(RichText::new(error).size(12.0).color(pal().error));
                });
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if prompt.accepting {
                    ui.spinner();
                    ui.label(RichText::new("Waiting for administrator approval...").size(12.0));
                } else {
                    if ui.button("Not now").clicked() {
                        defer = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        accept = ui.add_enabled(
                            prompt.can_accept() && !read_only,
                            egui::Button::new(RichText::new("Accept and continue").color(pal().on_accent))
                                .fill(pal().accent_fill).corner_radius(8),
                        ).clicked();
                    });
                }
            });
        });
        if defer {
            prompt.open = false;
            prompt.agreed = false;
        }
        if accept {
            self.accept_license(ctx);
        }
    }

    #[cfg(feature = "screenshot")]
    fn capture_frame(&mut self, ctx: &egui::Context) {
        let Some(path) = std::env::var_os("MACAI_SCREENSHOT_TO") else {
            return;
        };
        for event in ctx.input(|input| input.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                let bytes: Vec<_> = image
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect();
                image::save_buffer(
                    &path,
                    &bytes,
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                )
                .expect("cannot save development screenshot");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }
        if ctx.cumulative_pass_nr() == 3 {
            if std::env::var_os("MACAI_SCREENSHOT_SETTINGS").is_some() {
                crate::menu::choose_settings_for_screenshot();
            }
            if std::env::var_os("MACAI_SCREENSHOT_ROW_MENU").is_some() {
                let id = Id::new(("chat-menu", &self.history.selected));
                ctx.memory_mut(|memory| memory.open_popup(id));
            }
        }
        if ctx.cumulative_pass_nr() >= 8 && !self.capture_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.capture_requested = true;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl eframe::App for MacAi {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        DARK_MODE.with(|dark| dark.set(ctx.theme() == egui::Theme::Dark));
        #[cfg(feature = "screenshot")]
        self.capture_frame(ctx);
        self.receive();
        self.start_titles(ctx);
        for event in ctx.input(|input| input.events.clone()) {
            match event {
                egui::Event::Ime(egui::ImeEvent::Preedit(_)) => self.ime_active = true,
                egui::Event::Ime(egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled) => {
                    self.ime_active = false
                }
                _ => {}
            }
        }
        if ctx.input(|input| input.viewport().close_requested())
            && !self.allow_close
            && !self.prepare_close()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_confirmation = true;
        }
        if let Some(error) = self.fatal.clone() {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.add_space(50.0);
                centered(ui, 650.0, |ui| {
                    ui.heading("MacAi could not open your chat history");
                    ui.add_space(15.0);
                    ui.label(RichText::new(error).color(pal().error));
                    ui.add_space(15.0);
                    ui.label("No existing conversations have been overwritten. Close any other MacAi window, or fix the file permissions, then reopen the app.");
                });
            });
            return;
        }
        self.handle_menu_requests();
        if self.rename.is_none()
            && self.delete.is_none()
            && !self.settings_open
            && !self.close_confirmation
            && !self.license.as_ref().is_some_and(|prompt| prompt.open)
            && ctx.input_mut(|input| input.consume_key(egui::Modifiers::COMMAND, Key::N))
        {
            self.new_chat();
        }
        self.sidebar(ctx);
        self.header(ctx);
        self.composer(ctx);
        self.conversation(ctx);
        self.dialogs(ctx);

        if let Some((notice, since)) = &self.notice {
            if since.elapsed() < Duration::from_secs(2) {
                egui::Area::new(Id::new("notice"))
                    .anchor(egui::Align2::CENTER_TOP, Vec2::new(90.0, 66.0))
                    .show(ctx, |ui| {
                        Frame::new()
                            .fill(pal().accent_fill)
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(15, 8))
                            .show(ui, |ui| {
                                ui.label(RichText::new(notice).color(pal().on_accent).size(12.0));
                            });
                    });
                ctx.request_repaint_after(Duration::from_secs(2).saturating_sub(since.elapsed()));
            }
        }
        // Generated Markdown is display-only: never fetch images or launch URLs.
        ctx.output_mut(|output| {
            let before = output.commands.len();
            output
                .commands
                .retain(|command| !matches!(command, egui::OutputCommand::OpenUrl(_)));
            if output.commands.len() != before {
                self.notice = Some((
                    "Offline mode: links stay inside the conversation.".into(),
                    Instant::now(),
                ));
            }
        });
        if self.dirty && self.save_error.is_none() {
            if self.last_save.elapsed() >= Duration::from_millis(650) {
                self.save();
            }
            ctx.request_repaint_after(Duration::from_millis(650));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if !self.allow_close {
            self.prepare_close();
        }
    }
}

fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        BOLD_FONT.into(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/Ubuntu-Bold.ttf")).into(),
    );
    fonts.families.insert(
        egui::FontFamily::Name(BOLD_FONT.into()),
        vec![BOLD_FONT.into()],
    );
    ctx.set_fonts(fonts);

    ctx.set_visuals_of(egui::Theme::Light, visuals(egui::Visuals::light(), &LIGHT));
    ctx.set_visuals_of(egui::Theme::Dark, visuals(egui::Visuals::dark(), &DARK));
    // Light stays the look when macOS doesn't report an appearance.
    ctx.options_mut(|options| options.fallback_theme = egui::Theme::Light);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = Vec2::new(8.0, 7.0);
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(15.0));
        style
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.0));
        style
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(11.0));
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(13.0));
        style.url_in_tooltip = true;
        style.interaction.selectable_labels = true;
    });
}

fn visuals(mut visuals: egui::Visuals, palette: &Palette) -> egui::Visuals {
    visuals.override_text_color = Some(palette.ink);
    visuals.panel_fill = palette.background;
    visuals.window_fill = palette.surface;
    visuals.window_stroke = Stroke::new(1.0_f32, palette.line);
    visuals.extreme_bg_color = palette.surface;
    visuals.faint_bg_color = palette.sidebar;
    visuals.code_bg_color = palette.code;
    visuals.selection.bg_fill = palette.selection;
    visuals.selection.stroke = Stroke::new(1.0_f32, palette.accent_text);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette.line);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, palette.ink);
    visuals.widgets.inactive.bg_fill = palette.widget;
    visuals.widgets.inactive.weak_bg_fill = palette.widget;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, palette.line);
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, palette.ink);
    visuals.widgets.hovered.bg_fill = palette.widget_hover;
    visuals.widgets.hovered.weak_bg_fill = palette.widget_hover;
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.5_f32, palette.ink);
    visuals.widgets.active.bg_fill = palette.widget_active;
    visuals.widgets.active.weak_bg_fill = palette.widget_active;
    visuals.widgets.active.fg_stroke = Stroke::new(1.5_f32, palette.ink);
    visuals.widgets.open.bg_fill = palette.widget_hover;
    visuals.widgets.open.weak_bg_fill = palette.widget_hover;
    visuals.hyperlink_color = palette.accent_text;
    visuals.warn_fg_color = palette.error;
    visuals.error_fg_color = palette.error;
    visuals
}

/// Applies the saved appearance to egui and to the window chrome.
fn apply_appearance(ctx: &egui::Context, appearance: Appearance) {
    let (preference, chrome) = match appearance {
        Appearance::System => (
            egui::ThemePreference::System,
            egui::SystemTheme::SystemDefault,
        ),
        Appearance::Light => (egui::ThemePreference::Light, egui::SystemTheme::Light),
        Appearance::Dark => (egui::ThemePreference::Dark, egui::SystemTheme::Dark),
    };
    ctx.set_theme(preference);
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(chrome));
}

fn centered(ui: &mut egui::Ui, max_width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width().min(max_width);
    let margin = ((ui.available_width() - width) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.set_width(width);
            add(ui);
        });
    });
}

fn brand(ui: &mut egui::Ui, size: f32) {
    let texture = brand_texture(ui.ctx());
    ui.add(egui::Image::new(&texture).fit_to_exact_size(Vec2::splat(size)));
}

pub fn icon_data() -> egui::IconData {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/MacAi.png"))
        .expect("the embedded MacAi app icon must be a valid PNG")
}

fn brand_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = Id::new("macai-app-icon");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let icon = icon_data();
    let image = egui::ColorImage::from_rgba_unmultiplied(
        [icon.width as usize, icon.height as usize],
        &icon.rgba,
    );
    let texture = ctx.load_texture("MacAi app icon", image, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(8.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.0, color);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    Draft,
    Retry,
    Fresh,
}

/// Shown under a reply that repeats an earlier one. Returns `Turn::Fresh` to answer again
/// without earlier context, or `Turn::Draft` to ask the question in a new conversation.
fn repeat_notice(ui: &mut egui::Ui) -> Option<Turn> {
    let mut action = None;
    Frame::new()
        .fill(pal().info)
        .stroke(Stroke::new(1.0_f32, pal().line))
        .corner_radius(10)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("This reply repeats an earlier answer")
                    .size(12.0)
                    .strong()
                    .color(pal().accent_text),
            );
            ui.label(
                RichText::new(
                    "The on-device model can get stuck repeating itself in longer chats. Try the question again without the earlier messages.",
                )
                .size(11.0)
                .color(pal().ink),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("Answer without earlier context").color(pal().on_accent),
                        )
                        .fill(pal().accent_fill)
                        .corner_radius(7),
                    )
                    .clicked()
                {
                    action = Some(Turn::Fresh);
                }
                if ui
                    .add(egui::Button::new("Start new chat with this question").corner_radius(7))
                    .clicked()
                {
                    action = Some(Turn::Draft);
                }
            });
        });
    action
}

/// A borderless chevron button: it points left to hide the sidebar and right to show it.
const ROW_MENU_SPACE: f32 = 26.0;
const APPEARANCE_CARD: Vec2 = Vec2::new(116.0, 92.0);

/// A selectable card with a small window preview for one appearance choice.
fn appearance_card(ui: &mut egui::Ui, appearance: Appearance, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(APPEARANCE_CARD, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::RadioButton,
            true,
            selected,
            appearance.label(),
        )
    });
    let painter = ui.painter();
    let border = if selected {
        Stroke::new(2.0_f32, pal().accent)
    } else if response.hovered() {
        Stroke::new(1.0_f32, pal().muted)
    } else {
        Stroke::new(1.0_f32, pal().line)
    };
    painter.rect(
        rect,
        10.0,
        if response.hovered() {
            pal().hover
        } else {
            pal().card
        },
        border,
        egui::StrokeKind::Inside,
    );
    let preview = egui::Rect::from_min_size(
        rect.min + Vec2::new(14.0, 12.0),
        Vec2::new(rect.width() - 28.0, 46.0),
    );
    let mini = |painter: &egui::Painter, area: egui::Rect, palette: &Palette| {
        painter.rect_filled(area, 0.0, palette.background);
        let side =
            egui::Rect::from_min_size(area.min, Vec2::new(area.width() * 0.3, area.height()));
        painter.rect_filled(side, 0.0, palette.sidebar);
        let bubble = egui::Rect::from_min_size(
            egui::pos2(area.right() - 30.0, area.top() + 8.0),
            Vec2::new(22.0, 8.0),
        );
        painter.rect_filled(bubble, 4.0, palette.user_bubble);
        let line = egui::Rect::from_min_size(
            egui::pos2(side.right() + 6.0, area.top() + 22.0),
            Vec2::new(area.right() - side.right() - 14.0, 4.0),
        );
        painter.rect_filled(line, 2.0, palette.faint);
        let pill = egui::Rect::from_min_size(
            egui::pos2(side.left() + 4.0, area.top() + 6.0),
            Vec2::new(side.width() - 8.0, 6.0),
        );
        painter.rect_filled(pill, 3.0, palette.accent_fill);
    };
    let clipped = painter.with_clip_rect(preview);
    match appearance {
        Appearance::Light => mini(&clipped, preview, &LIGHT),
        Appearance::Dark => mini(&clipped, preview, &DARK),
        Appearance::System => {
            mini(&clipped, preview, &LIGHT);
            let mut right = preview;
            right.min.x = preview.center().x;
            mini(&painter.with_clip_rect(right), preview, &DARK);
        }
    }
    painter.rect_stroke(
        preview,
        5.0,
        Stroke::new(1.0_f32, pal().line),
        egui::StrokeKind::Outside,
    );
    painter.text(
        egui::pos2(rect.center().x, rect.bottom() - 16.0),
        egui::Align2::CENTER_CENTER,
        appearance.label(),
        FontId::proportional(13.0),
        if selected {
            pal().accent_text
        } else {
            pal().ink
        },
    );
    response
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowAction {
    Rename,
    Delete,
}

/// The "..." button in a sidebar chat row, with its Rename / Delete popup.
fn chat_row_menu(ui: &mut egui::Ui, row: egui::Rect, menu_id: Id, busy: bool) -> Option<RowAction> {
    let rect = egui::Rect::from_center_size(
        egui::pos2(row.right() - 8.0 - 11.0, row.top() + 19.0),
        Vec2::splat(22.0),
    );
    let response = ui
        .interact(rect, menu_id.with("button"), egui::Sense::click())
        .on_hover_text("Conversation options");
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Conversation options")
    });
    let open = ui.memory(|memory| memory.is_popup_open(menu_id));
    if response.hovered() || open {
        ui.painter().rect_filled(rect, 6.0, pal().hover);
    }
    let color = if response.hovered() || open {
        pal().ink
    } else {
        pal().muted
    };
    for offset in [-5.0, 0.0, 5.0] {
        ui.painter()
            .circle_filled(rect.center() + Vec2::new(offset, 0.0), 1.6, color);
    }
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(menu_id));
    }
    let mut action = None;
    egui::popup::popup_below_widget(
        ui,
        menu_id,
        &response,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(140.0);
            let widgets = &mut ui.visuals_mut().widgets;
            widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            widgets.inactive.bg_stroke = Stroke::NONE;
            widgets.hovered.weak_bg_fill = pal().hover;
            widgets.hovered.bg_stroke = Stroke::NONE;
            widgets.active.bg_stroke = Stroke::NONE;
            ui.spacing_mut().button_padding = Vec2::new(10.0, 6.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                if ui.button("Rename").clicked() {
                    action = Some(RowAction::Rename);
                }
                if ui
                    .add_enabled(
                        !busy,
                        egui::Button::new(RichText::new("Delete chat").color(pal().error)),
                    )
                    .on_disabled_hover_text("Stop the reply before deleting this chat.")
                    .clicked()
                {
                    action = Some(RowAction::Delete);
                }
            });
        },
    );
    action
}

fn sidebar_toggle(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let label = if open {
        "Hide conversations"
    } else {
        "Show conversations"
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if response.hovered() || response.has_focus() {
            painter.rect_filled(rect, 7.0, pal().hover);
        }
        let color = if response.hovered() {
            pal().ink
        } else {
            pal().muted
        };
        let center = rect.center();
        let (dx, dy) = (3.5_f32, 6.5_f32);
        let (tip, tail) = if open {
            (center.x - dx, center.x + dx)
        } else {
            (center.x + dx, center.x - dx)
        };
        painter.line(
            vec![
                egui::pos2(tail, center.y - dy),
                egui::pos2(tip, center.y),
                egui::pos2(tail, center.y + dy),
            ],
            Stroke::new(2.0_f32, color),
        );
    }
    response
        .on_hover_text(label)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A compact segmented control. Returns the model the user clicked, if any.
fn model_switch(
    ui: &mut egui::Ui,
    selected: ModelChoice,
    cloud: Option<&CloudStatus>,
) -> Option<ModelChoice> {
    let mut clicked = None;
    Frame::new()
        .fill(pal().sidebar)
        .stroke(Stroke::new(1.0_f32, pal().line))
        .corner_radius(10)
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.spacing_mut().button_padding = Vec2::new(10.0, 4.0);
            // Apple Cloud (PCC) is hidden: macOS only allows it from Terminal, not third-party
            // apps. All Cloud logic remains; re-add `ModelChoice::Cloud` here to show it again.
            for model in [ModelChoice::OnDevice /* , ModelChoice::Cloud */] {
                let active = model == selected;
                let unavailable = model == ModelChoice::Cloud
                    && matches!(cloud, Some(CloudStatus::Unavailable(_)));
                let color = if active {
                    pal().on_accent
                } else if unavailable {
                    pal().faint
                } else {
                    pal().muted
                };
                let response = ui
                    .add(
                        egui::Button::new(
                            RichText::new(model.label())
                                .size(11.0)
                                .color(color)
                                .font(FontId::new(
                                    11.0,
                                    if active {
                                        egui::FontFamily::Name(BOLD_FONT.into())
                                    } else {
                                        egui::FontFamily::Proportional
                                    },
                                )),
                        )
                        .fill(if active {
                            pal().accent_fill
                        } else {
                            Color32::TRANSPARENT
                        })
                        .stroke(Stroke::NONE)
                        .corner_radius(8),
                    )
                    .on_hover_text(match (model, cloud) {
                        (ModelChoice::OnDevice, _) => {
                            "Apple's on-device model. Private and works offline.".to_owned()
                        }
                        (ModelChoice::Cloud, Some(CloudStatus::Unavailable(reason))) => {
                            format!("Apple Cloud is unavailable: {reason}")
                        }
                        (ModelChoice::Cloud, _) => {
                            "Apple's larger Private Cloud Compute model. Needs internet.".to_owned()
                        }
                    });
                if response.clicked() && !active {
                    clicked = Some(model);
                }
            }
        });
    clicked
}

fn relative_time(timestamp: u64) -> String {
    match now().saturating_sub(timestamp) / 86400 {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        days => format!("{days} days ago"),
    }
}

fn message_view(
    ui: &mut egui::Ui,
    message: &Message,
    model: ModelChoice,
    cache: &mut CommonMarkCache,
    retry_allowed: bool,
) -> (bool, bool) {
    let mut retry = false;
    let mut copied = false;
    if message.role == Role::User {
        ui.with_layout(Layout::top_down(Align::RIGHT), |ui| {
            Frame::new()
                .fill(pal().user_bubble)
                .corner_radius(13)
                .inner_margin(Margin::symmetric(17, 13))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width().min(CONTENT_WIDTH * 0.82));
                    ui.label(&message.text);
                });
        });
    } else {
        ui.horizontal(|ui| {
            brand(ui, 22.0);
            ui.add_space(2.0);
            ui.label(
                RichText::new("MacAi")
                    .size(12.0)
                    .strong()
                    .color(pal().accent_text),
            );
            if message.status == MessageStatus::Streaming {
                ui.add(egui::Spinner::new().size(12.0).color(pal().accent));
                ui.label(
                    RichText::new(match model {
                        ModelChoice::OnDevice => "Thinking on your Mac",
                        ModelChoice::Cloud => "Thinking in Apple's Private Cloud",
                    })
                    .size(11.0)
                    .color(pal().muted),
                );
            }
        });
        ui.add_space(9.0);
        if !message.text.is_empty() {
            CommonMarkViewer::new().show(ui, cache, &message.text);
        }
        if let Some(detail) = &message.detail {
            ui.add_space(8.0);
            ui.label(RichText::new(detail).size(12.0).color(
                if message.status == MessageStatus::Failed {
                    pal().error
                } else {
                    pal().muted
                },
            ));
        }
    }
    if message.status != MessageStatus::Streaming {
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            if !message.text.is_empty()
                && ui
                    .add(
                        egui::Button::new(RichText::new("Copy").size(10.0).color(pal().muted))
                            .frame(false),
                    )
                    .clicked()
            {
                ui.ctx().copy_text(message.text.clone());
                copied = true;
            }
            if retry_allowed
                && matches!(
                    message.status,
                    MessageStatus::Failed | MessageStatus::Interrupted
                )
            {
                retry = ui.small_button("Retry reply").clicked();
            }
        });
    }
    (retry, copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_run_app(directory: &std::path::Path) -> MacAi {
        let mut app = MacAi::from_store(Store::open(directory.to_owned()));
        app.finish_setup(Ok(Readiness::LicenseRequired(
            LicenseDocument::new("Apple test terms\n\nRead these terms before accepting.".into())
                .unwrap(),
        )));
        app
    }

    #[test]
    fn setup_never_accepts_or_sends_without_explicit_consent() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = first_run_app(directory.path());
        let ctx = egui::Context::default();
        app.history.selected_mut().draft = "Do not send before consent".into();
        app.accept_license(&ctx);
        assert!(app.setup_task.is_none());
        app.start_message(&ctx, false);
        assert!(app.active.is_none());
        assert!(app.history.selected().messages.is_empty());
        assert_eq!(app.history.selected().draft, "Do not send before consent");
        let prompt = app.license.as_mut().unwrap();
        assert!(!prompt.can_accept());
        prompt.agreed = true;
        assert!(prompt.can_accept());
        prompt.open = false;
        assert!(!prompt.can_accept());
        app.accept_license(&ctx);
        assert!(app.setup_task.is_none());
    }

    #[test]
    fn approval_errors_allow_retry_and_changed_terms_reset_consent() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = first_run_app(directory.path());
        app.license.as_mut().unwrap().accepting = true;
        app.license.as_mut().unwrap().agreed = true;
        app.finish_setup(Err("Administrator approval was cancelled.".into()));
        let prompt = app.license.as_ref().unwrap();
        assert!(!prompt.agreed && !prompt.accepting && prompt.open);
        assert!(prompt.error.as_ref().unwrap().contains("cancelled"));
        assert!(matches!(app.model, ModelState::LicenseRequired));
        app.license.as_mut().unwrap().accepting = true;
        app.finish_setup(Ok(Readiness::LicenseRequired(
            LicenseDocument::new("Changed terms".into()).unwrap(),
        )));
        assert!(!app.license.as_ref().unwrap().agreed);
        assert!(app
            .license
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .contains("changed"));
        app.finish_setup(Ok(Readiness::ModelUnavailable(
            "Enable Apple Intelligence".into(),
        )));
        assert!(app.license.is_none());
        assert!(matches!(app.model, ModelState::Unavailable(_)));
    }

    #[test]
    fn license_controls_fit_small_windows_with_long_terms() {
        for size in [[1080.0, 780.0], [760.0, 560.0]] {
            let directory = tempfile::tempdir().unwrap();
            let mut app = first_run_app(directory.path());
            app.license.as_mut().unwrap().document =
                LicenseDocument::new("A long license clause for scrolling.\n".repeat(200)).unwrap();
            let ctx = egui::Context::default();
            setup_style(&ctx);
            let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::from(size));
            for frame in 0..5 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(bounds),
                        ..Default::default()
                    },
                    |ctx| app.license_dialog(ctx),
                );
                if frame == 4 {
                    for label in ["Set up local AI", "Not now", "Accept and continue"] {
                        let (clip, text) = output
                            .shapes
                            .iter()
                            .find_map(|clipped| {
                                text_bounds(&clipped.shape, label)
                                    .map(|rect| (clipped.clip_rect, rect))
                            })
                            .unwrap_or_else(|| panic!("Missing control: {label}"));
                        assert!(
                            bounds.expand(1.0).contains_rect(text),
                            "{label} outside {size:?}: {text:?}"
                        );
                        assert!(clip.expand(1.0).contains_rect(text), "{label} clipped");
                    }
                }
            }
        }
    }

    #[test]
    fn new_conversation_button_uses_the_bold_font() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        let ctx = egui::Context::default();
        setup_style(&ctx);
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1080.0, 780.0),
                )),
                ..Default::default()
            },
            |ctx| app.sidebar(ctx),
        );
        assert!(output.shapes.iter().any(|clipped| {
            matches!(&clipped.shape, egui::Shape::Text(shape)
                if shape.galley.job.text == "+  New conversation"
                    && shape.galley.job.sections.iter().all(|section|
                        section.format.font_id.family == egui::FontFamily::Name(BOLD_FONT.into())))
        }));
    }

    #[test]
    fn in_app_brand_uses_the_embedded_app_icon_and_reuses_its_texture() {
        let ctx = egui::Context::default();
        let first = brand_texture(&ctx);
        let second = brand_texture(&ctx);
        assert_eq!(first.size(), [512, 512]);
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn blue_palette_uses_the_requested_base_and_readable_accent_text() {
        fn luminance(color: Color32) -> f32 {
            [color.r(), color.g(), color.b()]
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(channel, weight)| {
                    let channel = f32::from(channel) / 255.0;
                    let linear = if channel <= 0.04045 {
                        channel / 12.92
                    } else {
                        ((channel + 0.055) / 1.055).powf(2.4)
                    };
                    linear * weight
                })
                .sum()
        }

        assert_eq!(LIGHT.accent.to_array(), [0x5d, 0x91, 0xb5, 0xff]);
        for (theme, palette) in [("light", &LIGHT), ("dark", &DARK)] {
            // Filled blue controls use light text, not dark text on blue.
            assert!(luminance(palette.on_accent) > luminance(palette.accent_fill));
            for (name, text, background) in [
                ("button", palette.on_accent, palette.accent_fill),
                ("accent", palette.accent_text, palette.background),
                ("accent on sidebar", palette.accent_text, palette.sidebar),
                ("accent on notice", palette.accent_text, palette.info),
                ("body", palette.ink, palette.background),
                ("body on surface", palette.ink, palette.surface),
                ("body in bubble", palette.ink, palette.user_bubble),
                ("body on selected row", palette.ink, palette.row_selected),
                ("body in code", palette.ink, palette.code),
                ("body on warning", palette.ink, palette.warning),
                ("muted", palette.muted, palette.background),
                ("error", palette.error, palette.background),
                ("error on its notice", palette.error, palette.error_fill),
            ] {
                let (text, background) = (luminance(text), luminance(background));
                let contrast = (text.max(background) + 0.05) / (text.min(background) + 0.05);
                assert!(
                    contrast >= 4.5,
                    "{theme} {name} contrast is {contrast:.2}:1"
                );
            }
        }
        {
            // The original light palette's sidebar dates sit at 4.3:1; dark must do better.
            let (text, background) = (luminance(DARK.muted), luminance(DARK.sidebar));
            let contrast = (text.max(background) + 0.05) / (text.min(background) + 0.05);
            assert!(
                contrast >= 4.5,
                "dark muted on sidebar contrast is {contrast:.2}:1"
            );
        }
    }

    fn text_bounds(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(shape) if shape.galley.job.text == text => {
                Some(shape.galley.rect.translate(shape.pos.to_vec2()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_bounds(shape, text)),
            _ => None,
        }
    }

    #[test]
    fn composer_and_controls_stay_visible_at_supported_window_sizes() {
        for size in [[1080.0, 780.0], [760.0, 560.0]] {
            let directory = tempfile::tempdir().unwrap();
            let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
            app.history.selected_mut().title = "A deliberately very long conversation title that must not push the menu or model status off the edge of the window".into();
            let ctx = egui::Context::default();
            setup_style(&ctx);
            let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, Vec2::from(size));
            for frame in 0..5 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(bounds),
                        ..Default::default()
                    },
                    |ctx| {
                        app.sidebar(ctx);
                        app.header(ctx);
                        app.composer(ctx);
                        app.conversation(ctx);
                    },
                );
                if frame == 4 {
                    for label in [
                        "Send",
                        "Enter to send  /  Shift+Enter for a new line",
                        "A little space to think.",
                        "...",
                        "Checking model",
                        "On-device",
                    ] {
                        let (clip, text) = output
                            .shapes
                            .iter()
                            .find_map(|clipped| {
                                text_bounds(&clipped.shape, label)
                                    .map(|rect| (clipped.clip_rect, rect))
                            })
                            .unwrap_or_else(|| panic!("Missing control: {label}"));
                        assert!(
                            bounds.expand(1.0).contains_rect(text),
                            "{label} outside window at {size:?}: {text:?}"
                        );
                        assert!(
                            clip.expand(1.0).contains_rect(text),
                            "{label} clipped at {size:?}: {text:?}, clip {clip:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unavailable_apple_cloud_is_explained_and_never_selected_or_sent() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        let reason =
            "Private Cloud Compute is not available in this context. Please use the Terminal app.";
        // Apple Cloud was remembered from an earlier run, on a fresh chat too.
        app.history.preferred_model = ModelChoice::Cloud;
        app.history.selected_mut().model = ModelChoice::Cloud;
        app.finish_setup(Ok(Readiness::Ready(CloudStatus::Unavailable(
            reason.into(),
        ))));
        assert_eq!(app.history.preferred_model, ModelChoice::OnDevice);
        assert_eq!(app.history.selected().model, ModelChoice::OnDevice);
        app.choose_model(ModelChoice::Cloud);
        assert_eq!(app.history.selected().model, ModelChoice::OnDevice);

        // A saved Apple Cloud chat stays readable; its header explains why it can't continue.
        app.history.selected_mut().model = ModelChoice::Cloud;
        app.history.selected_mut().begin("Earlier".into());
        app.history
            .selected_mut()
            .finish("Reply".into(), serde_json::json!({}));
        app.history.selected_mut().draft = "Hello".into();
        let ctx = egui::Context::default();
        setup_style(&ctx);
        let mut output = None;
        for _ in 0..5 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1080.0, 780.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    app.header(ctx);
                    app.composer(ctx);
                },
            ));
        }
        let output = output.unwrap();
        let shows = |label: &str| {
            output
                .shapes
                .iter()
                .any(|clipped| text_bounds(&clipped.shape, label).is_some())
        };
        assert!(shows("Apple Cloud unavailable"));
        // The switch and banner are hidden until Apple opens PCC to third-party apps.
        assert!(!shows("Apple Cloud"));
        assert!(!shows("Apple Cloud isn't available to MacAi right now"));
        // Sending moves the draft to a new on-device chat (what start_message does first).
        let old = app.history.selected().id.clone();
        app.choose_model(ModelChoice::OnDevice);
        assert_ne!(app.history.selected().id, old);
        assert_eq!(app.history.selected().model, ModelChoice::OnDevice);
        assert_eq!(app.history.selected().draft, "Hello");
    }

    fn wait_for_reply(app: &mut MacAi) {
        let start = Instant::now();
        while app.active.is_some() {
            assert!(start.elapsed() < Duration::from_secs(15), "reply timed out");
            app.receive();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn repeated_replies_offer_a_fresh_answer_or_a_new_chat() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let fm = directory.path().join("fm");
        std::fs::write(
            &fm,
            r#"#!/bin/sh
resumed=no
for arg in "$@"; do
  [ "$arg" = "--resume" ] && resumed=yes
  [ "$previous" = "--save-transcript" ] && output="$arg"
  previous="$arg"
done
prompt=$(/bin/cat)
echo "$resumed" >> "$0.log"
printf '{"modelName":"system","transcript":{"transcript":{"entries":[]}}}' > "$output"
printf 'Fresh answer to %s\n' "$prompt"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fm, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = directory.path().join("store");
        let mut app = MacAi::from_store(Store::open(store));
        app.model = ModelState::Ready;
        app.fm = fm.clone();
        let chat = app.history.selected_mut();
        for (prompt, reply) in [
            ("how many params", "I have 154 billion parameters."),
            ("name some colors", "I have 154 billion parameters."),
        ] {
            chat.begin(prompt.into());
            chat.finish(reply.into(), serde_json::json!({"old": true}));
        }
        let ctx = egui::Context::default();
        setup_style(&ctx);
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        Vec2::new(1080.0, 780.0),
                    )),
                    ..Default::default()
                },
                |ctx| app.conversation(ctx),
            ));
        }
        let output = output.unwrap();
        for label in [
            "This reply repeats an earlier answer",
            "Answer without earlier context",
            "Start new chat with this question",
        ] {
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|clipped| text_bounds(&clipped.shape, label).is_some()),
                "missing {label}"
            );
        }

        // Answer again without --resume, replacing the repeated reply in place.
        let id = app.history.selected().id.clone();
        app.send(&ctx, Turn::Fresh);
        wait_for_reply(&mut app);
        let chat = app.history.selected();
        assert_eq!(chat.id, id);
        assert_eq!(chat.messages.len(), 4);
        assert_eq!(chat.messages[3].text, "Fresh answer to name some colors");
        assert_eq!(chat.repeated_prompt(), None);
        assert_eq!(
            chat.transcript.as_ref().unwrap()["old"],
            serde_json::Value::Null
        );

        // Or ask the same question in a brand-new conversation.
        let chat = app.history.selected_mut();
        chat.begin("again".into());
        chat.finish(
            "I have 154 billion parameters.".into(),
            serde_json::json!({}),
        );
        app.ask_in_new_chat(&ctx);
        wait_for_reply(&mut app);
        let chat = app.history.selected();
        assert_ne!(chat.id, id);
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[1].text, "Fresh answer to again");
        let log = std::fs::read_to_string(fm.with_extension("log")).unwrap();
        assert_eq!(log, "no\nno\n");
    }

    #[test]
    fn appearance_choice_switches_theme_and_is_saved() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        let ctx = egui::Context::default();
        setup_style(&ctx);
        assert_eq!(app.history.appearance, Appearance::System);
        assert_eq!(
            ctx.theme(),
            egui::Theme::Light,
            "light when macOS reports nothing"
        );
        app.set_appearance(&ctx, Appearance::Dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        assert_eq!(ctx.style().visuals.panel_fill, DARK.background);
        drop(app);
        let (_, history) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(history.appearance, Appearance::Dark);
        apply_appearance(&ctx, Appearance::Light);
        assert_eq!(ctx.style().visuals.panel_fill, LIGHT.background);
    }

    fn ui_frame(
        app: &mut MacAi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1080.0, 780.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                app.sidebar(ctx);
                app.header(ctx);
                app.composer(ctx);
                app.conversation(ctx);
                app.dialogs(ctx);
            },
        );
        ctx.request_repaint();
        output
    }

    fn click(app: &mut MacAi, ctx: &egui::Context, pos: egui::Pos2) -> egui::FullOutput {
        let press = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        ui_frame(app, ctx, vec![egui::Event::PointerMoved(pos), press(true)]);
        ui_frame(app, ctx, vec![press(false)]);
        ui_frame(app, ctx, vec![])
    }

    fn find_text(output: &egui::FullOutput, text: &str) -> Option<egui::Rect> {
        output
            .shapes
            .iter()
            .find_map(|clipped| text_bounds(&clipped.shape, text))
    }

    fn row_menu_dots(output: &egui::FullOutput) -> Vec<egui::Pos2> {
        fn collect(shape: &egui::Shape, found: &mut Vec<egui::Pos2>) {
            match shape {
                egui::Shape::Circle(circle) if circle.radius == 1.6 => found.push(circle.center),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect(shape, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut found);
        }
        found
    }

    #[test]
    fn sidebar_row_menu_renames_and_deletes_chats_with_confirmation() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        app.history.selected_mut().title = "Kept chat".into();
        app.history.selected_mut().updated_at = 10;
        let kept = app.history.selected.clone();
        app.history.chats.push(crate::model::Chat::new());
        app.history.selected = app.history.chats[1].id.clone();
        app.history.selected_mut().title = "Doomed chat".into();
        app.history.selected_mut().updated_at = 20;
        let doomed = app.history.selected.clone();
        let ctx = egui::Context::default();
        setup_style(&ctx);

        let output = ui_frame(&mut app, &ctx, vec![]);
        let dots = row_menu_dots(&output);
        assert_eq!(
            dots.len(),
            3,
            "only the selected, unhovered row shows its button"
        );
        let button = dots[1];

        let output = click(&mut app, &ctx, button);
        let rename = find_text(&output, "Rename").expect("menu offers Rename");
        assert!(find_text(&output, "Delete chat").is_some());
        click(&mut app, &ctx, rename.center());
        assert_eq!(app.rename, Some((doomed.clone(), "Doomed chat".into())));
        app.rename = None;

        // Let the closed rename modal leave the previous frame's hit-test layers.
        ui_frame(&mut app, &ctx, vec![]);
        let output = click(&mut app, &ctx, button);
        let delete = find_text(&output, "Delete chat").unwrap();
        let output = click(&mut app, &ctx, delete.center());
        assert_eq!(
            app.delete,
            Some(doomed.clone()),
            "deletion waits for confirmation"
        );
        assert_eq!(app.history.chats.len(), 2);
        let confirm = find_text(&output, "Delete conversation").expect("confirmation dialog");
        click(&mut app, &ctx, confirm.center());
        assert!(app.delete.is_none());
        assert_eq!(app.history.selected, kept);
        assert!(app.history.chats.iter().all(|chat| chat.id != doomed));
        drop(app);
        let (_, history) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(history.chats.len(), 1, "deletion is saved");
        assert_eq!(history.selected, kept);
    }

    #[test]
    fn settings_menu_item_opens_appearance_settings() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        let ctx = egui::Context::default();
        setup_style(&ctx);

        let output = ui_frame(&mut app, &ctx, vec![]);
        let menu = find_text(&output, "...").unwrap();
        let output = click(&mut app, &ctx, menu.center());
        assert!(find_text(&output, "Rename conversation").is_some());
        assert!(
            find_text(&output, "Appearance").is_none(),
            "Appearance moved to Settings"
        );
        ui_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );

        crate::menu::request_settings();
        app.handle_menu_requests();
        assert!(app.settings_open);
        assert!(
            !crate::menu::take_settings_request(),
            "each request opens Settings once"
        );
        // A newly shown modal is laid out invisibly on its first frame.
        ui_frame(&mut app, &ctx, vec![]);
        let output = ui_frame(&mut app, &ctx, vec![]);
        for label in ["Settings", "APPEARANCE", "System", "Light", "Dark", "Done"] {
            assert!(find_text(&output, label).is_some(), "Missing {label}");
        }
        let dark = find_text(&output, "Dark").unwrap();
        click(&mut app, &ctx, dark.center());
        assert_eq!(app.history.appearance, Appearance::Dark);
        assert_eq!(ctx.theme(), egui::Theme::Dark);
        let output = ui_frame(&mut app, &ctx, vec![]);
        let done = find_text(&output, "Done").unwrap();
        click(&mut app, &ctx, done.center());
        assert!(!app.settings_open);
        drop(app);
        let (_, history) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(history.appearance, Appearance::Dark);
    }

    #[test]
    fn sidebar_chevron_flips_and_is_labelled() {
        let ctx = egui::Context::default();
        for open in [true, false] {
            let mut clicked = false;
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    clicked = sidebar_toggle(ui, open).clicked();
                });
            });
            let expected = if open {
                "Hide conversations"
            } else {
                "Show conversations"
            };
            let tip_points_left = output.shapes.iter().any(|clipped| {
                matches!(&clipped.shape, egui::Shape::Path(path)
                    if path.points.len() == 3 && path.points[1].x < path.points[0].x)
            });
            assert_eq!(tip_points_left, open, "{expected}");
            assert!(!clicked);
        }
    }

    #[test]
    fn summary_titles_apply_once_and_respect_renames() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        let chat = app.history.selected_mut();
        chat.begin("can you help me plan a trip to rome".into());
        chat.finish("Sure!".into(), serde_json::json!({}));
        let id = chat.id.clone();
        let (sender, events) = std::sync::mpsc::channel();
        app.titles.push(TitleTask::finished(id.clone(), events));
        sender.send(Ok("Planning a Rome Trip".into())).unwrap();
        app.receive_titles();
        assert_eq!(app.history.selected().title, "Planning a Rome Trip");
        assert!(app.titles.is_empty());

        app.history.selected_mut().rename("Mine".into());
        let (sender, events) = std::sync::mpsc::channel();
        app.titles.push(TitleTask::finished(id, events));
        sender.send(Ok("Something Else".into())).unwrap();
        app.receive_titles();
        assert_eq!(app.history.selected().title, "Mine");
    }

    #[test]
    fn available_apple_cloud_can_be_chosen_and_is_remembered() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        app.finish_setup(Ok(Readiness::Ready(CloudStatus::Available)));
        assert_eq!(app.history.selected().model, ModelChoice::OnDevice);
        app.choose_model(ModelChoice::Cloud);
        assert_eq!(app.history.selected().model, ModelChoice::Cloud);
        drop(app);
        let (_, history) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(history.preferred_model, ModelChoice::Cloud);
        assert_eq!(history.selected().model, ModelChoice::Cloud);
    }

    fn composer_frame(
        app: &mut MacAi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1080.0, 780.0),
                )),
                events,
                modifiers,
                ..Default::default()
            },
            |ctx| app.composer(ctx),
        );
    }

    #[test]
    fn long_messages_wrap_inside_the_conversation_column() {
        for size in [[1080.0, 780.0], [760.0, 560.0]] {
            let directory = tempfile::tempdir().unwrap();
            let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
            let prompt =
                "A longer message that should wrap without escaping its bubble. ".repeat(8);
            app.history.selected_mut().begin(prompt.clone());
            app.history.selected_mut().finish(
                "## A helpful reply\n\nA **clear** explanation with `inline code`.\n\n```rust\nfn main() {\n    println!(\"Hello\");\n}\n```".into(),
                serde_json::json!({"modelName":"system","transcript":{}}),
            );
            let ctx = egui::Context::default();
            setup_style(&ctx);
            for frame in 0..5 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            Vec2::from(size),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        app.sidebar(ctx);
                        app.header(ctx);
                        app.composer(ctx);
                        app.conversation(ctx);
                    },
                );
                if frame == 4 {
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|clipped| text_bounds(&clipped.shape, &prompt))
                        .expect("user message must be rendered");
                    assert!(
                        text.min.x >= 244.0 && text.max.x <= size[0],
                        "message escapes its column: {text:?} at {size:?}"
                    );
                }
            }
        }
    }

    fn enter(modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn shift_enter_adds_a_line_without_sending_or_losing_the_draft() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        let ctx = egui::Context::default();
        setup_style(&ctx);
        composer_frame(&mut app, &ctx, vec![], egui::Modifiers::NONE);
        composer_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("A draft".into())],
            egui::Modifiers::NONE,
        );
        assert_eq!(app.history.selected().draft, "A draft");
        composer_frame(
            &mut app,
            &ctx,
            vec![enter(egui::Modifiers::SHIFT)],
            egui::Modifiers::SHIFT,
        );
        assert_eq!(app.history.selected().draft, "A draft\n");
        assert!(app.active.is_none());
        assert!(app.save());
        drop(app);
        let (_, history) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(history.selected().draft, "A draft\n");
    }

    #[test]
    #[ignore = "requires the real Apple Foundation Model on macOS 27"]
    fn live_enter_sends_a_reply_and_persists_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut app = MacAi::from_store(Store::open(directory.path().to_owned()));
        app.model = ModelState::Ready;
        let ctx = egui::Context::default();
        setup_style(&ctx);
        composer_frame(&mut app, &ctx, vec![], egui::Modifiers::NONE);
        composer_frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text(
                "Reply with only: Hello from MacAi.".into(),
            )],
            egui::Modifiers::NONE,
        );
        composer_frame(
            &mut app,
            &ctx,
            vec![enter(egui::Modifiers::NONE)],
            egui::Modifiers::NONE,
        );
        assert!(app.active.is_some(), "Enter must start generation");
        assert!(app.history.selected().draft.is_empty());
        let start = Instant::now();
        while app.active.is_some() {
            assert!(
                start.elapsed() < Duration::from_secs(120),
                "generation timed out"
            );
            app.receive();
            std::thread::sleep(Duration::from_millis(25));
        }
        let reply = &app.history.selected().messages[1];
        assert_eq!(reply.status, MessageStatus::Complete, "{:?}", reply.detail);
        assert!(reply.text.contains("Hello from MacAi"), "{}", reply.text);
        drop(app);
        let (_, history) = Store::open(directory.path().to_owned()).unwrap();
        assert_eq!(history.selected().messages.len(), 2);
        assert!(history.selected().transcript.is_some());
    }
}
