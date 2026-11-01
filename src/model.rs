use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is before 1970")
        .as_secs()
}

/// The app's color theme. `System` follows the macOS appearance.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

impl Appearance {
    pub const ALL: [Appearance; 3] = [Appearance::System, Appearance::Light, Appearance::Dark];

    pub fn label(self) -> &'static str {
        match self {
            Appearance::System => "System",
            Appearance::Light => "Light",
            Appearance::Dark => "Dark",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum ModelChoice {
    #[default]
    #[serde(rename = "system")]
    OnDevice,
    #[serde(rename = "pcc")]
    Cloud,
}

impl ModelChoice {
    pub fn fm_name(self) -> &'static str {
        match self {
            Self::OnDevice => "system",
            Self::Cloud => "pcc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::OnDevice => "On-device",
            Self::Cloud => "Apple Cloud",
        }
    }
}

/// Where a chat's title came from, so a generated summary never replaces a user's rename.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TitleSource {
    #[default]
    Prompt,
    Summary,
    Custom,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Complete,
    Streaming,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub text: String,
    pub status: MessageStatus,
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub draft: String,
    pub messages: Vec<Message>,
    // Keep Apple's opaque transcript intact, including its context metadata.
    pub transcript: Option<Value>,
    // A saved transcript belongs to one model, so a started chat keeps its model.
    #[serde(default)]
    pub model: ModelChoice,
    #[serde(default)]
    pub title_source: TitleSource,
}

impl Chat {
    pub fn new() -> Self {
        Self::with_model(ModelChoice::default())
    }

    pub fn with_model(model: ModelChoice) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: "New conversation".into(),
            updated_at: now(),
            draft: String::new(),
            messages: Vec::new(),
            transcript: None,
            model,
            title_source: TitleSource::Prompt,
        }
    }

    /// The first completed exchange, used to summarize a title.
    pub fn title_context(&self) -> Option<(&str, &str)> {
        if self.title_source != TitleSource::Prompt {
            return None;
        }
        let user = self.messages.iter().find(|m| m.role == Role::User)?;
        let reply = self
            .messages
            .iter()
            .find(|m| m.role == Role::Assistant && m.status == MessageStatus::Complete)?;
        Some((&user.text, &reply.text))
    }

    pub fn apply_summary_title(&mut self, title: String) -> bool {
        if self.title_source != TitleSource::Prompt {
            return false;
        }
        self.title = title;
        self.title_source = TitleSource::Summary;
        true
    }

    pub fn rename(&mut self, title: String) {
        self.title = title;
        self.title_source = TitleSource::Custom;
    }

    pub fn is_started(&self) -> bool {
        !self.messages.is_empty() || self.transcript.is_some()
    }

    pub fn begin(&mut self, prompt: String) {
        if self.messages.is_empty() && self.title == "New conversation" {
            let title = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
            self.title = title.chars().take(48).collect();
            if title.chars().count() > 48 {
                self.title.push_str("...");
            }
        }
        self.draft.clear();
        self.updated_at = now();
        self.messages.push(Message {
            role: Role::User,
            text: prompt,
            status: MessageStatus::Complete,
            detail: None,
        });
        self.messages.push(Message {
            role: Role::Assistant,
            text: String::new(),
            status: MessageStatus::Streaming,
            detail: None,
        });
    }

    pub fn retry_prompt(&self) -> Option<String> {
        let [.., user, assistant] = self.messages.as_slice() else {
            return None;
        };
        (user.role == Role::User
            && assistant.role == Role::Assistant
            && matches!(
                assistant.status,
                MessageStatus::Failed | MessageStatus::Interrupted
            ))
        .then(|| user.text.clone())
    }

    /// The prompt behind the latest reply when that reply repeats an earlier one word for
    /// word (ignoring case and spacing), a sign the on-device model is stuck in a loop.
    pub fn repeated_prompt(&self) -> Option<String> {
        let [earlier @ .., user, reply] = self.messages.as_slice() else {
            return None;
        };
        let normal = |text: &str| {
            text.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        };
        let latest = normal(&reply.text);
        (user.role == Role::User
            && reply.role == Role::Assistant
            && reply.status == MessageStatus::Complete
            && !latest.is_empty()
            && earlier.iter().any(|message| {
                message.role == Role::Assistant
                    && message.status == MessageStatus::Complete
                    && normal(&message.text) == latest
            }))
        .then(|| user.text.clone())
    }

    pub fn finish(&mut self, text: String, transcript: Value) {
        if let Some(message) = self.messages.last_mut() {
            message.text = text;
            message.status = MessageStatus::Complete;
            message.detail = None;
        }
        self.transcript = Some(transcript);
        self.updated_at = now();
    }

    pub fn interrupt(&mut self, status: MessageStatus, detail: String) {
        if let Some(message) = self.messages.last_mut() {
            message.status = status;
            message.detail = Some(detail);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    pub version: u32,
    pub selected: String,
    pub chats: Vec<Chat>,
    #[serde(default)]
    pub preferred_model: ModelChoice,
    #[serde(default)]
    pub appearance: Appearance,
}

impl Default for History {
    fn default() -> Self {
        let chat = Chat::new();
        Self {
            version: 1,
            selected: chat.id.clone(),
            chats: vec![chat],
            preferred_model: ModelChoice::default(),
            appearance: Appearance::default(),
        }
    }
}

impl History {
    pub fn selected(&self) -> &Chat {
        self.chats
            .iter()
            .find(|chat| chat.id == self.selected)
            .expect("selected chat must exist")
    }

    pub fn selected_mut(&mut self) -> &mut Chat {
        self.chats
            .iter_mut()
            .find(|chat| chat.id == self.selected)
            .expect("selected chat must exist")
    }

    pub fn new_chat(&mut self) {
        let model = self.preferred_model;
        if let Some(empty) = self
            .chats
            .iter_mut()
            .find(|chat| !chat.is_started() && chat.draft.is_empty())
        {
            empty.model = model;
            self.selected = empty.id.clone();
            return;
        }
        let chat = Chat::with_model(model);
        self.selected = chat.id.clone();
        self.chats.insert(0, chat);
    }

    /// Selects a model for the current chat. A started chat keeps its context with
    /// its original model, so the switch continues in a fresh conversation and the
    /// unsent draft moves along with it.
    pub fn switch_model(&mut self, model: ModelChoice) {
        self.preferred_model = model;
        let chat = self.selected_mut();
        if chat.model == model {
            return;
        }
        if !chat.is_started() {
            chat.model = model;
            return;
        }
        let draft = std::mem::take(&mut chat.draft);
        self.new_chat();
        let chat = self.selected_mut();
        chat.model = model;
        if chat.draft.is_empty() {
            chat.draft = draft;
        } else if !draft.is_empty() {
            chat.draft.push('\n');
            chat.draft.push_str(&draft);
        }
    }

    pub fn delete(&mut self, id: &str) {
        self.chats.retain(|chat| chat.id != id);
        if self.chats.is_empty() {
            self.chats.push(Chat::new());
        }
        if !self.chats.iter().any(|chat| chat.id == self.selected) {
            self.selected = self.chats[0].id.clone();
        }
    }

    pub fn recover(&mut self) {
        for chat in &mut self.chats {
            for message in &mut chat.messages {
                if message.status == MessageStatus::Streaming {
                    message.status = MessageStatus::Interrupted;
                    message.detail = Some(
                        "The app closed before this reply finished. You can retry this message."
                            .into(),
                    );
                }
            }
        }
        if self.chats.is_empty() {
            self.chats.push(Chat::new());
        }
        if !self.chats.iter().any(|chat| chat.id == self.selected) {
            self.selected = self.chats[0].id.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_unicode_safe_and_whitespace_is_collapsed() {
        let mut chat = Chat::new();
        chat.begin("  Hello\n world  ".into());
        assert_eq!(chat.title, "Hello world");
        let mut chat = Chat::new();
        chat.begin("🦦".repeat(60));
        assert_eq!(chat.title.chars().count(), 51);
    }

    #[test]
    fn failed_turn_keeps_last_successful_context() {
        let mut chat = Chat::new();
        let transcript = serde_json::json!({"transcript": {"test": "prior turn"}});
        chat.begin("First".into());
        chat.finish("Reply".into(), transcript.clone());
        chat.begin("Second".into());
        chat.interrupt(MessageStatus::Failed, "Model unavailable".into());
        assert_eq!(chat.retry_prompt().as_deref(), Some("Second"));
        assert_eq!(chat.transcript, Some(transcript));
    }

    #[test]
    fn recovery_and_deletion_keep_a_valid_selection() {
        let mut history = History::default();
        history.selected_mut().begin("Hello".into());
        history.recover();
        assert_eq!(
            history.selected().messages[1].status,
            MessageStatus::Interrupted
        );
        let id = history.selected.clone();
        history.delete(&id);
        assert!(history.selected().messages.is_empty());
        history.new_chat();
        assert_eq!(history.chats.len(), 1);
    }

    #[test]
    fn repeated_replies_are_detected_only_for_the_latest_turn() {
        let mut chat = Chat::new();
        chat.begin("how many params".into());
        chat.finish(
            "I have 154 billion parameters.".into(),
            serde_json::json!({}),
        );
        assert_eq!(chat.repeated_prompt(), None);
        chat.begin("name some colors".into());
        chat.finish("Red, blue.".into(), serde_json::json!({}));
        assert_eq!(chat.repeated_prompt(), None);
        chat.begin("why repeat".into());
        chat.finish(
            "  i have 154 billion\nparameters. ".into(),
            serde_json::json!({}),
        );
        assert_eq!(chat.repeated_prompt().as_deref(), Some("why repeat"));
        chat.begin("again".into());
        assert_eq!(chat.repeated_prompt(), None, "still streaming");
        chat.interrupt(MessageStatus::Failed, "x".into());
        assert_eq!(chat.repeated_prompt(), None);
    }

    #[test]
    fn summary_titles_replace_prompt_titles_but_never_user_renames() {
        let mut chat = Chat::new();
        chat.begin("tell me everything about otters please".into());
        assert!(chat.title_context().is_none(), "no completed reply yet");
        chat.finish("Otters are...".into(), serde_json::json!({}));
        assert_eq!(
            chat.title_context(),
            Some(("tell me everything about otters please", "Otters are..."))
        );
        assert!(chat.apply_summary_title("All About Otters".into()));
        assert_eq!(chat.title, "All About Otters");
        assert!(chat.title_context().is_none());

        let mut renamed = Chat::new();
        renamed.begin("Hello".into());
        renamed.rename("My name".into());
        renamed.finish("Hi".into(), serde_json::json!({}));
        assert!(renamed.title_context().is_none());
        assert!(!renamed.apply_summary_title("Greeting".into()));
        assert_eq!(renamed.title, "My name");
    }

    #[test]
    fn model_defaults_to_on_device_and_older_history_still_loads() {
        assert_eq!(Chat::new().model, ModelChoice::OnDevice);
        assert_eq!(ModelChoice::default().fm_name(), "system");
        assert_eq!(ModelChoice::Cloud.fm_name(), "pcc");
        let old = r#"{"version":1,"selected":"a","chats":[{"id":"a","title":"t","updated_at":0,"draft":"","messages":[],"transcript":null}]}"#;
        let history: History = serde_json::from_str(old).unwrap();
        assert_eq!(history.preferred_model, ModelChoice::OnDevice);
        assert_eq!(history.selected().model, ModelChoice::OnDevice);
        assert_eq!(history.selected().title_source, TitleSource::Prompt);
        assert_eq!(history.appearance, Appearance::System);
        let mut saved = serde_json::to_value(&history).unwrap();
        saved["appearance"] = "dark".into();
        let reopened: History = serde_json::from_value(saved).unwrap();
        assert_eq!(reopened.appearance, Appearance::Dark);
    }

    #[test]
    fn switching_models_keeps_started_context_with_its_original_model() {
        let mut history = History::default();
        history.switch_model(ModelChoice::Cloud);
        assert_eq!(history.chats.len(), 1);
        assert_eq!(history.selected().model, ModelChoice::Cloud);

        history.selected_mut().begin("Hello".into());
        history
            .selected_mut()
            .finish("Hi".into(), serde_json::json!({"transcript": {}}));
        let started = history.selected.clone();
        history.selected_mut().draft = "Next question".into();
        history.switch_model(ModelChoice::OnDevice);
        assert_ne!(history.selected, started);
        assert_eq!(history.selected().model, ModelChoice::OnDevice);
        assert_eq!(history.selected().draft, "Next question");
        let original = history
            .chats
            .iter()
            .find(|chat| chat.id == started)
            .unwrap();
        assert_eq!(original.model, ModelChoice::Cloud);
        assert!(original.draft.is_empty());
        assert_eq!(history.preferred_model, ModelChoice::OnDevice);
        history.new_chat();
        assert_eq!(history.selected().model, ModelChoice::OnDevice);
    }
}
