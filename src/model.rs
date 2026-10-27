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
}

impl Chat {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: "New conversation".into(),
            updated_at: now(),
            draft: String::new(),
            messages: Vec::new(),
            transcript: None,
        }
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
}

impl Default for History {
    fn default() -> Self {
        let chat = Chat::new();
        Self {
            version: 1,
            selected: chat.id.clone(),
            chats: vec![chat],
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
        if let Some(empty) = self
            .chats
            .iter()
            .find(|chat| chat.messages.is_empty() && chat.draft.is_empty())
        {
            self.selected = empty.id.clone();
            return;
        }
        let chat = Chat::new();
        self.selected = chat.id.clone();
        self.chats.insert(0, chat);
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
}
