use std::path::PathBuf;

use ferrocull_core::{Hook, IngestConfig, Profile};
use iced::Task;

use super::Ferrocull;
use crate::messages::{Message, profile};

pub(super) fn update(state: &mut Ferrocull, msg: profile::Message) -> Task<Message> {
    match msg {
        profile::Message::ProfileSelected(name) => state.handle_load_profile(name),
        profile::Message::SaveRequested => state.handle_save_profile(),
        profile::Message::DeleteRequested(name) => state.handle_delete_profile(&name),
        profile::Message::NameChanged(name) => state.profile_name_input = name,
        profile::Message::HookAddRequested => state.handle_add_hook(),
        profile::Message::HookRemoved(idx) => {
            state.hooks.remove(idx);
            state.persist_settings();
        }
        profile::Message::HookToggled(idx) => {
            state.hooks[idx].enabled = !state.hooks[idx].enabled;
            state.persist_settings();
        }
        profile::Message::HookCommandEdited(idx, cmd) => {
            state.hooks[idx].command = cmd;
            state.persist_settings();
        }
    }
    Task::none()
}

impl Ferrocull {
    fn handle_load_profile(&mut self, name: String) {
        if let Some(named) = self.profiles.iter().find(|p| p.name == name) {
            self.photos_dest = named.profile.ingest.photos_dest.display().to_string();
            self.videos_dest = named.profile.ingest.videos_dest.display().to_string();
            self.photo_pattern = named.profile.ingest.photo_pattern.clone();
            self.video_pattern = named.profile.ingest.video_pattern.clone();
            self.backup_destinations = named.profile.ingest.backup_destinations.clone();
            self.current_profile = Some(name);
            self.persist_settings();
        }
    }

    fn handle_save_profile(&mut self) {
        let name = self.profile_name_input.trim();
        if name.is_empty() {
            return;
        }
        let name = name.to_owned();
        let profile = Profile {
            ingest: IngestConfig {
                photos_dest: PathBuf::from(&self.photos_dest),
                videos_dest: PathBuf::from(&self.videos_dest),
                photo_pattern: self.photo_pattern.clone(),
                video_pattern: self.video_pattern.clone(),
                backup_destinations: self.backup_destinations.clone(),
            },
        };
        self.metadata.save_profile(&name, &profile);
        self.profiles = self.metadata.profiles();
        self.current_profile = Some(name);
        self.profile_name_input.clear();
        self.status_message = None;
    }

    fn handle_delete_profile(&mut self, name: &str) {
        self.metadata.delete_profile(name);
        self.profiles = self.metadata.profiles();
        if self.current_profile.as_deref() == Some(name) {
            self.current_profile = None;
        }
    }

    fn handle_add_hook(&mut self) {
        let idx = self.hooks.len() + 1;
        self.hooks.push(Hook {
            name: format!("Hook {idx}"),
            command: String::new(),
            enabled: true,
        });
        self.persist_settings();
    }
}
