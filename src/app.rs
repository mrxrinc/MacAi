use crate::engine::{self, Event, Generation};
use crate::model::{now, History, Message, MessageStatus, Role};
use crate::setup::{LicenseDocument, Readiness, SetupTask};
use crate::storage::Store;
use eframe::egui::{
    self, Align, Color32, FontId, Frame, Id, Key, Layout, Margin, RichText, Stroke, Vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use std::sync::mpsc::TryRecvError;
use std::time::{Duration, Instant};

const BACKGROUND: Color32 = Color32::from_rgb(247, 249, 251);
const SIDEBAR: Color32 = Color32::from_rgb(235, 241, 246);
const INK: Color32 = Color32::from_rgb(32, 46, 58);
const MUTED: Color32 = Color32::from_rgb(98, 115, 128);
const ACCENT: Color32 = Color32::from_rgb(93, 145, 181);
const ACCENT_TEXT: Color32 = Color32::from_rgb(49, 87, 113);
const ON_ACCENT: Color32 = Color32::from_rgb(16, 32, 44);
const LINE: Color32 = Color32::from_rgb(216, 226, 234);
const ERROR: Color32 = Color32::from_rgb(160, 62, 46);
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
    setup_task: Option<SetupTask>,
    license: Option<LicensePrompt>,
    markdown: CommonMarkCache,
    search: String,
    sidebar_open: bool,
    focus_composer: bool,
    rename: Option<(String, String)>,
    delete: Option<String>,
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
            setup_task: None,
            license: None,
            markdown: CommonMarkCache::default(),
            search: String::new(),
            sidebar_open: true,
            focus_composer: true,
            rename: None,
            delete: None,
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
            Ok(Readiness::Ready) => {
                self.model = ModelState::Ready;
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

    fn start_message(&mut self, ctx: &egui::Context, retry: bool) {
        if self.active.is_some()
            || self.store.is_none()
            || self.save_error.is_some()
            || self.rename.is_some()
            || self.delete.is_some()
            || self.close_confirmation
            || !matches!(self.model, ModelState::Ready)
        {
            return;
        }
        let chat = self.history.selected_mut();
        let original = chat.clone();
        let prompt = if retry {
            let Some(prompt) = chat.retry_prompt() else {
                return;
            };
            chat.messages.truncate(chat.messages.len() - 2);
            prompt
        } else {
            let prompt = chat.draft.trim().to_owned();
            if prompt.is_empty() {
                return;
            }
            prompt
        };
        let transcript = chat.transcript.clone();
        let chat_id = chat.id.clone();
        let draft = chat.draft.clone();
        chat.begin(prompt.clone());
        if retry {
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
                generation: Generation::start(
                    prompt,
                    transcript,
                    store.directory().to_owned(),
                    move || context.request_repaint(),
                ),
            });
        }
        self.focus_composer = true;
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
                        "The local model worker stopped unexpectedly.".into(),
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
                    Event::Finished { text, transcript } => chat.finish(text, transcript),
                    Event::Failed(error) => {
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
            .frame(Frame::new().fill(SIDEBAR).inner_margin(Margin::same(18)))
            .show(ctx, |ui| {
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    brand(ui, 30.0);
                    ui.add_space(3.0);
                    ui.label(RichText::new("MacAi").size(23.0).strong().color(INK));
                });
                ui.add_space(24.0);
                if ui
                    .add_sized(
                        [ui.available_width(), 39.0],
                        egui::Button::new(
                            RichText::new("+  New conversation")
                                .color(ON_ACCENT)
                                .font(FontId::new(13.0, egui::FontFamily::Name(BOLD_FONT.into()))),
                        )
                        .fill(ACCENT)
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
                        .color(MUTED),
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
                                    .color(MUTED)
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
                                    Color32::from_rgb(216, 231, 242)
                                } else {
                                    SIDEBAR
                                })
                                .corner_radius(8)
                                .inner_margin(Margin::symmetric(10, 10))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&title).size(13.0).color(INK),
                                        )
                                        .truncate(),
                                    );
                                    ui.add_space(4.0);
                                    ui.label(
                                        RichText::new(if busy {
                                            "Writing a reply".into()
                                        } else {
                                            relative_time(timestamp)
                                        })
                                        .size(10.0)
                                        .color(MUTED),
                                    );
                                })
                                .response;
                            let click = ui.interact(
                                response.rect,
                                Id::new(("chat", &id)),
                                egui::Sense::click(),
                            );
                            if click.clicked() {
                                self.history.selected = id;
                                self.dirty = true;
                                self.focus_composer = true;
                            }
                            ui.add_space(4.0);
                        }
                    });
                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new("No cloud. No account. Just your Mac.")
                            .size(10.0)
                            .color(MUTED),
                    );
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        status_dot(ui, ACCENT);
                        ui.label(RichText::new("Private by design").size(12.0).color(INK));
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
                    .fill(BACKGROUND)
                    .inner_margin(Margin::symmetric(25, 16)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new("Sidebar").small().frame(false))
                        .on_hover_text("Show or hide conversations")
                        .clicked()
                    {
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
                        let (text, color) = match self.model {
                            ModelState::Ready => ("On-device", ACCENT_TEXT),
                            ModelState::Checking => ("Checking model", MUTED),
                            ModelState::LicenseRequired => ("Setup needed", ACCENT_TEXT),
                            ModelState::Unavailable(_) => ("Model unavailable", ERROR),
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
                .fill(SIDEBAR)
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
                .fill(Color32::from_rgb(249, 237, 220))
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.label(RichText::new(error).size(12.0).color(ERROR));
                    if ui.small_button("Check again").clicked() {
                        self.check_model(ctx);
                    }
                });
            ui.add_space(8.0);
        }
        if let Some(error) = self.save_error.clone() {
            Frame::new()
                .fill(Color32::from_rgb(251, 231, 226))
                .corner_radius(8)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.label(RichText::new(error).size(12.0).color(ERROR));
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
                    .fill(BACKGROUND)
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
                                    .color(MUTED),
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
                    Frame::new()
                        .fill(Color32::WHITE)
                        .stroke(Stroke::new(1.0, LINE))
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
                                ui.label(
                                    RichText::new("APPLE FOUNDATION MODEL")
                                        .size(9.0)
                                        .color(MUTED)
                                        .strong(),
                                );
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
                                                RichText::new("Send").color(ON_ACCENT),
                                            )
                                            .fill(if ready {
                                                ACCENT
                                            } else {
                                                Color32::from_rgb(173, 194, 209)
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
                                .color(MUTED),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new("AI can make mistakes.")
                                    .size(10.0)
                                    .color(MUTED),
                            );
                        });
                    });
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
                    .fill(BACKGROUND)
                    .inner_margin(Margin::symmetric(28, 0)),
            )
            .show(ctx, |ui| {
                if self.history.selected().messages.is_empty() {
                    self.empty_state(ui);
                    return;
                }
                let mut retry = false;
                let mut copied = false;
                egui::ScrollArea::vertical()
                    .id_salt(("messages", &self.history.selected))
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        centered(ui, CONTENT_WIDTH, |ui| {
                            ui.add_space(22.0);
                            let chat = self.history.selected();
                            let count = chat.messages.len();
                            for (index, message) in chat.messages.iter().enumerate() {
                                ui.push_id((&chat.id, index), |ui| {
                                    let action = message_view(
                                        ui,
                                        message,
                                        &mut self.markdown,
                                        index + 1 == count && self.active.is_none(),
                                    );
                                    retry |= action.0;
                                    copied |= action.1;
                                });
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
                                .color(INK),
                        );
                        ui.add_space(11.0);
                        ui.label(
                            RichText::new("Your ideas. Your conversations. Your Mac.")
                                .size(14.0)
                                .color(MUTED),
                        );
                        ui.add_space(5.0);
                        ui.label(
                            RichText::new("A private assistant, with nothing sent to the cloud.")
                                .size(12.0)
                                .color(MUTED),
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
                                    .color(INK),
                            )
                            .fill(Color32::from_rgb(239, 244, 248))
                            .stroke(Stroke::new(1.0, LINE))
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
                        chat.title = title.trim().to_owned();
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
                        .button(RichText::new("Delete conversation").color(ERROR))
                        .clicked()
                    {
                        action = Some(true);
                    }
                });
            });
            if let Some(delete) = action {
                if delete {
                    self.history.delete(&id);
                    self.dirty = true;
                    self.save();
                    self.focus_composer = true;
                }
                self.delete = None;
            }
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

    fn prepare_close(&mut self) -> bool {
        self.setup_task = None;
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
            ui.label(RichText::new("Apple Foundation Models").color(ACCENT_TEXT).size(13.0));
            ui.add_space(8.0);
            ui.label(RichText::new("Before your first chat, review Apple's terms below. They are read directly from the fm command installed on this Mac.").size(12.0));
            Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, LINE))
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
            ui.label(RichText::new("This agreement applies to every user on this Mac. After you accept, macOS will ask for administrator approval. MacAi never sees or stores the administrator password.").size(12.0).color(MUTED));
            ui.add_enabled(
                !prompt.accepting && !read_only,
                egui::Checkbox::new(&mut prompt.agreed, "I have read and agree to these terms for this Mac."),
            );
            if let Some(error) = &prompt.error {
                egui::ScrollArea::vertical().id_salt("license-error").max_height(70.0).show(ui, |ui| {
                    ui.label(RichText::new(error).size(12.0).color(ERROR));
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
                            egui::Button::new(RichText::new("Accept and continue").color(ON_ACCENT))
                                .fill(ACCENT).corner_radius(8),
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
        if ctx.cumulative_pass_nr() >= 8 && !self.capture_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
            self.capture_requested = true;
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

impl eframe::App for MacAi {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(feature = "screenshot")]
        self.capture_frame(ctx);
        self.receive();
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
                    ui.label(RichText::new(error).color(ERROR));
                    ui.add_space(15.0);
                    ui.label("No existing conversations have been overwritten. Close any other MacAi window, or fix the file permissions, then reopen the app.");
                });
            });
            return;
        }
        if self.rename.is_none()
            && self.delete.is_none()
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
                            .fill(ACCENT)
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(15, 8))
                            .show(ui, |ui| {
                                ui.label(RichText::new(notice).color(ON_ACCENT).size(12.0));
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

    let mut visuals = egui::Visuals::light();
    visuals.override_text_color = Some(INK);
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = BACKGROUND;
    visuals.extreme_bg_color = Color32::WHITE;
    visuals.faint_bg_color = SIDEBAR;
    visuals.selection.bg_fill = Color32::from_rgb(199, 223, 241);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_TEXT);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(228, 237, 244);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(211, 228, 240);
    visuals.widgets.active.bg_fill = Color32::from_rgb(193, 217, 234);
    visuals.hyperlink_color = ACCENT_TEXT;
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
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
    cache: &mut CommonMarkCache,
    retry_allowed: bool,
) -> (bool, bool) {
    let mut retry = false;
    let mut copied = false;
    if message.role == Role::User {
        ui.with_layout(Layout::top_down(Align::RIGHT), |ui| {
            Frame::new()
                .fill(Color32::from_rgb(225, 237, 246))
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
                    .color(ACCENT_TEXT),
            );
            if message.status == MessageStatus::Streaming {
                ui.add(egui::Spinner::new().size(12.0).color(ACCENT));
                ui.label(
                    RichText::new("Thinking on your Mac")
                        .size(11.0)
                        .color(MUTED),
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
                    ERROR
                } else {
                    MUTED
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
                        egui::Button::new(RichText::new("Copy").size(10.0).color(MUTED))
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

        assert_eq!(ACCENT.to_array(), [0x5d, 0x91, 0xb5, 0xff]);
        for (text, background) in [(ON_ACCENT, ACCENT), (ACCENT_TEXT, BACKGROUND)] {
            let contrast = (luminance(background) + 0.05) / (luminance(text) + 0.05);
            assert!(contrast >= 4.5, "accent text contrast is {contrast:.2}:1");
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
